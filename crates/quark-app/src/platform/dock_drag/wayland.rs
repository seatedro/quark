//! Dock drags on Wayland, where windows have no desktop positions: the
//! compositor's drag and drop, run on the queue and thread
//! [`crate::platform::drag_out`] keeps for dragging files out, says which
//! of our windows the drag is over and where.
//!
//! Each drag offers a MIME type made for it alone, from the process and a
//! per-drag sequence number: its session token. The thread accepts an
//! offer, and forwards its events, only if it carries the running drag's
//! token and is over one of our windows, so neither another app's drag
//! nor one of ours that already ended can be dropped as this one.
//!
//! The compositor reports a release away from our windows as the source
//! cancelled after `dnd_drop_performed`, and Escape as cancelled without
//! it; either is a release outside ([`DragLocation::Outside`]) or a cancel.
//! Hyprland sends no `dnd_drop_performed`: it cancels a release over no
//! window without it, and ends one over another app's window by that app
//! asking for the data and nothing else. Without a window attached through
//! `xdg_toplevel_drag_v1`, which Hyprland lacks, a drag that ends with the
//! pointer away from our windows is therefore taken as a release there,
//! so the payload opens in a window of its own; Escape away from them does
//! the same.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{OnceLock, mpsc};

use quark_ui::element::DragLocation;
use winit::window::Window;

use super::{CancelReason, DockDragEvent, DockDragStart, FollowError, drag_window_id};
use crate::platform::drag_out::{self, DockRequest, DockSignal, DockStarted, DragOutError};
use crate::runner::{EventContext, Waker, WindowHandle};

/// A `wl_surface`, by proxy address.
type Surface = usize;

/// One dock drag's name among every drag of every process: its MIME type.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SessionToken {
    sequence: u64,
    mime: String,
}

impl SessionToken {
    fn next(kind: &str) -> Self {
        static SEQUENCE: AtomicU64 = AtomicU64::new(1);
        static PROCESS: OnceLock<u64> = OnceLock::new();
        // Random per process, so a process reusing a pid differs too.
        let process = *PROCESS.get_or_init(|| {
            use std::hash::{BuildHasher, Hasher};
            std::collections::hash_map::RandomState::new()
                .build_hasher()
                .finish()
        });
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        Self::new(kind, std::process::id(), process, sequence)
    }

    fn new(kind: &str, pid: u32, process: u64, sequence: u64) -> Self {
        Self {
            sequence,
            mime: format!(
                "application/x-quark-dock-{kind};session={pid}-{process:016x}-{sequence}"
            ),
        }
    }
}

/// Where a [`Step`] puts the pointer.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Spot {
    Over { surface: Surface, point: (f32, f32) },
    Outside,
    Unknown,
}

/// A [`DockDragEvent`] with windows still named by their surfaces.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Step {
    Moved(Spot),
    Released(Spot),
    Cancelled(CancelReason),
}

/// Whether a window follows the pointer through `xdg_toplevel_drag_v1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Follow {
    /// The compositor cannot move a window with the drag.
    Unsupported,
    /// A window may be attached.
    Available,
    /// A window is attached and follows the pointer.
    Following,
}

/// Where the compositor last said the drag is.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Whereabouts {
    /// Over none of our windows yet.
    Unentered,
    /// Over this window, at the pointer's last point in it.
    Over { surface: Surface, point: (f32, f32) },
    /// The drag left our windows and has not come back.
    Away,
}

/// How far the drag has come.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Dragging,
    /// The user released the drag (`dnd_drop_performed`) and the
    /// compositor has yet to end it.
    Performed,
    /// The compositor or the app ended the drag.
    Ended,
}

/// What a drag's signals add up to, apart from the protocol.
#[derive(Debug)]
struct Machine {
    follow: Follow,
    whereabouts: Whereabouts,
    phase: Phase,
}

impl Machine {
    fn new(can_follow: bool) -> Self {
        Self {
            follow: if can_follow {
                Follow::Available
            } else {
                Follow::Unsupported
            },
            whereabouts: Whereabouts::Unentered,
            phase: Phase::Dragging,
        }
    }

    /// Whether a window may be attached now.
    fn may_follow(&self) -> Result<(), FollowError> {
        match (self.phase, self.follow) {
            (Phase::Ended, _) => Err(FollowError::Ended),
            (_, Follow::Unsupported) => Err(FollowError::Unsupported),
            (_, Follow::Following) => Err(FollowError::Following),
            (_, Follow::Available) => Ok(()),
        }
    }

    /// A window was attached, after [`Self::may_follow`] allowed it.
    fn attached(&mut self) {
        self.follow = Follow::Following;
    }

    /// The app gave the drag up. True when the compositor still runs it
    /// and must be told.
    fn end(&mut self) -> bool {
        let running = self.phase != Phase::Ended;
        self.phase = Phase::Ended;
        running
    }

    fn signal(&mut self, signal: DockSignal) -> Option<Step> {
        if self.phase == Phase::Ended {
            return None;
        }
        let point = |x: f64, y: f64| (x as f32, y as f32);
        match signal {
            DockSignal::Enter { surface, x, y } => {
                let point = point(x, y);
                self.whereabouts = Whereabouts::Over { surface, point };
                Some(Step::Moved(Spot::Over { surface, point }))
            }
            DockSignal::Motion { x, y } => {
                let Whereabouts::Over {
                    surface,
                    point: last,
                } = &mut self.whereabouts
                else {
                    return None;
                };
                *last = point(x, y);
                Some(Step::Moved(Spot::Over {
                    surface: *surface,
                    point: *last,
                }))
            }
            DockSignal::Leave => {
                self.whereabouts = Whereabouts::Away;
                Some(Step::Moved(Spot::Unknown))
            }
            DockSignal::Drop => {
                self.phase = Phase::Ended;
                let spot = match self.whereabouts {
                    Whereabouts::Over { surface, point } => Spot::Over { surface, point },
                    Whereabouts::Unentered | Whereabouts::Away => Spot::Unknown,
                };
                Some(Step::Released(spot))
            }
            DockSignal::DropPerformed => {
                self.phase = Phase::Performed;
                None
            }
            DockSignal::Taken => {
                self.phase = Phase::Ended;
                Some(Step::Released(Spot::Outside))
            }
            DockSignal::Cancelled | DockSignal::Finished => {
                // A following window can only be put back on a cancel the
                // compositor says is one.
                let released = self.phase == Phase::Performed
                    || (self.whereabouts == Whereabouts::Away && self.follow != Follow::Following);
                self.phase = Phase::Ended;
                Some(if released {
                    Step::Released(Spot::Outside)
                } else {
                    Step::Cancelled(CancelReason::Platform)
                })
            }
        }
    }
}

/// A dock drag the compositor runs.
pub(super) struct WaylandDrag {
    sequence: u64,
    started: DockStarted,
    signals: mpsc::Receiver<DockSignal>,
    machine: Machine,
}

impl WaylandDrag {
    pub(super) fn start(
        window: &Window,
        start: &DockDragStart,
        waker: Waker,
    ) -> Result<Self, DragOutError> {
        let token = SessionToken::next(&start.kind);
        let (sender, signals) = mpsc::channel();
        let request = DockRequest {
            token: token.sequence,
            mime: token.mime,
            signals: sender,
            waker,
        };
        let started =
            drag_out::start_dock(window, request, start.seat.clone(), start.image.as_ref())?;
        Ok(Self {
            sequence: token.sequence,
            machine: Machine::new(started.can_follow()),
            started,
            signals,
        })
    }

    pub(super) fn can_follow(&self) -> bool {
        self.machine.follow != Follow::Unsupported
    }

    pub(super) fn follow(
        &mut self,
        cx: &EventContext,
        window: WindowHandle,
        (x, y): (f32, f32),
    ) -> Result<(), FollowError> {
        self.machine.may_follow()?;
        let surface = cx.wayland_surface(window).ok_or(FollowError::NotOpen)?;
        // SAFETY: the window is open for the rest of this callback, and its
        // proxies with it.
        let attached = unsafe {
            self.started
                .attach(surface.xdg_toplevel, (x.round() as i32, y.round() as i32))
        };
        if let Err(error) = attached {
            tracing::warn!("dock drag: could not attach the window: {error}");
            return Err(FollowError::NotOpen);
        }
        self.machine.attached();
        Ok(())
    }

    /// The events the compositor's signals since the last call add up to.
    pub(super) fn drain(&mut self, cx: &EventContext) -> Vec<DockDragEvent> {
        let mut events = Vec::new();
        while let Ok(signal) = self.signals.try_recv() {
            if let Some(step) = self.machine.signal(signal) {
                events.push(event(cx, step));
            }
        }
        events
    }

    /// The app is done with the drag: give it up unless the compositor
    /// already ended it.
    pub(super) fn end(&mut self) {
        if self.machine.end() {
            drag_out::end_dock(self.sequence);
        }
    }
}

/// `step` with its surface resolved to one of the app's windows.
fn event(cx: &EventContext, step: Step) -> DockDragEvent {
    let location = |spot| match spot {
        Spot::Over { surface, point } => {
            window_of_surface(cx, surface).map_or(DragLocation::Unknown, |window| {
                DragLocation::Window {
                    window: drag_window_id(window),
                    point,
                }
            })
        }
        Spot::Outside => DragLocation::Outside,
        Spot::Unknown => DragLocation::Unknown,
    };
    match step {
        Step::Moved(spot) => DockDragEvent::Moved(location(spot)),
        Step::Released(spot) => DockDragEvent::Released(location(spot)),
        Step::Cancelled(reason) => DockDragEvent::Cancelled(reason),
    }
}

fn window_of_surface(cx: &EventContext, surface: Surface) -> Option<WindowHandle> {
    cx.windows().into_iter().find(|&window| {
        cx.wayland_surface(window)
            .is_some_and(|native| native.surface.as_ptr() as Surface == surface)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAIN: Surface = 0xa000;
    const TOOLS: Surface = 0xb000;

    fn enter(surface: Surface) -> DockSignal {
        DockSignal::Enter {
            surface,
            x: 10.0,
            y: 20.0,
        }
    }

    /// A drag a compositor that can move windows runs, with one following
    /// it if `following`.
    fn machine(following: bool) -> Machine {
        let mut machine = Machine::new(true);
        if following {
            machine.attached();
        }
        machine
    }

    // A drag must end exactly once, the way the compositor's signals say:
    // dropped on a window of ours at its last point, released outside when
    // dropped elsewhere (including Hyprland's way, with no
    // dnd_drop_performed), and cancelled otherwise.
    #[test]
    fn compositor_signals_end_the_drag_once_as_released_or_cancelled() {
        use DockSignal::*;
        let cases: [(&str, bool, &[DockSignal], &str); 11] = [
            (
                "dropped on a window",
                false,
                &[enter(MAIN), Motion { x: 30.0, y: 5.0 }, Drop, Finished],
                "Released(Over { surface: 40960, point: (30.0, 5.0) })",
            ),
            (
                "moved on to another window before the drop",
                false,
                &[
                    enter(MAIN),
                    Leave,
                    enter(TOOLS),
                    Drop,
                    DropPerformed,
                    Finished,
                ],
                "Released(Over { surface: 45056, point: (10.0, 20.0) })",
            ),
            (
                "released elsewhere with a window following",
                true,
                &[enter(MAIN), Leave, DropPerformed, Cancelled],
                "Released(Outside)",
            ),
            (
                "released elsewhere with nothing following",
                false,
                &[enter(MAIN), Leave, DropPerformed, Cancelled],
                "Released(Outside)",
            ),
            (
                "Hyprland: released over no window",
                false,
                &[enter(MAIN), Leave, Cancelled],
                "Released(Outside)",
            ),
            (
                "Hyprland: released over another app",
                false,
                &[enter(MAIN), Leave, Taken, Cancelled],
                "Released(Outside)",
            ),
            (
                "cancelled back over a window of ours",
                false,
                &[enter(MAIN), Leave, enter(MAIN), Cancelled],
                "Cancelled(Platform)",
            ),
            (
                "cancelled away with a window following",
                true,
                &[enter(MAIN), Leave, Cancelled],
                "Cancelled(Platform)",
            ),
            (
                "cancelled by the compositor (Escape) with a window following",
                true,
                &[enter(MAIN), Cancelled],
                "Cancelled(Platform)",
            ),
            (
                "motion before any enter is not a location",
                false,
                &[Motion { x: 1.0, y: 1.0 }, Cancelled],
                "Cancelled(Platform)",
            ),
            (
                "nothing after the end counts",
                false,
                &[enter(MAIN), Drop, Cancelled, enter(TOOLS)],
                "Released(Over { surface: 40960, point: (10.0, 20.0) })",
            ),
        ];
        for (name, following, signals, ending) in cases {
            let mut machine = machine(following);
            let ends: Vec<String> = signals
                .iter()
                .filter_map(|&signal| machine.signal(signal))
                .filter(|step| !matches!(step, Step::Moved(_)))
                .map(|step| format!("{step:?}"))
                .collect();
            assert_eq!(ends, [ending], "{name}");
        }
    }

    // Without xdg_toplevel_drag_v1 no window can follow the pointer, and
    // one window at most follows it where it can.
    #[test]
    fn a_window_follows_only_where_the_compositor_can_move_it() {
        let cases = [
            (
                "no toplevel drag",
                false,
                false,
                false,
                Err(FollowError::Unsupported),
            ),
            ("toplevel drag", true, false, false, Ok(())),
            (
                "already following",
                true,
                true,
                false,
                Err(FollowError::Following),
            ),
            ("drag over", true, false, true, Err(FollowError::Ended)),
        ];
        for (name, can_follow, following, over, expected) in cases {
            let mut machine = Machine::new(can_follow);
            if following {
                machine.attached();
            }
            if over {
                machine.end();
            }
            assert_eq!(machine.may_follow(), expected, "{name}");
        }
    }

    // Moves name the window the compositor last entered and the pointer's
    // point in it; motion while over none of our windows is no location.
    #[test]
    fn moves_follow_the_window_the_compositor_last_entered() {
        use DockSignal::*;
        let mut machine = machine(false);
        let signals = [
            Motion { x: 1.0, y: 1.0 },
            enter(MAIN),
            Motion { x: 30.0, y: 5.0 },
            Leave,
            Motion { x: 2.0, y: 2.0 },
            enter(TOOLS),
        ];
        let moves: Vec<String> = signals
            .into_iter()
            .filter_map(|signal| machine.signal(signal))
            .map(|step| format!("{step:?}"))
            .collect();
        assert_eq!(
            moves,
            [
                "Moved(Over { surface: 40960, point: (10.0, 20.0) })",
                "Moved(Over { surface: 40960, point: (30.0, 5.0) })",
                "Moved(Unknown)",
                "Moved(Over { surface: 45056, point: (10.0, 20.0) })",
            ]
        );
    }

    // The app's end reaches the compositor once, and only while it still
    // runs the drag.
    #[test]
    fn the_app_gives_up_only_a_drag_the_compositor_still_runs() {
        use DockSignal::*;
        let cases: [(&str, &[DockSignal], [bool; 2]); 4] = [
            ("over a window", &[enter(MAIN)], [true, false]),
            (
                "released, not yet ended",
                &[enter(MAIN), Leave, DropPerformed],
                [true, false],
            ),
            ("dropped", &[enter(MAIN), Drop], [false, false]),
            ("cancelled", &[enter(MAIN), Cancelled], [false, false]),
        ];
        for (name, signals, ends) in cases {
            let mut machine = machine(false);
            for &signal in signals {
                machine.signal(signal);
            }
            assert_eq!([machine.end(), machine.end()], ends, "{name}");
        }
    }

    // Signals still queued when the app gives the drag up neither move nor
    // end it again.
    #[test]
    fn nothing_after_the_app_gives_the_drag_up_counts() {
        use DockSignal::*;
        let mut machine = machine(true);
        machine.signal(enter(MAIN));
        machine.end();
        let signals = [
            enter(TOOLS),
            Motion { x: 1.0, y: 1.0 },
            Leave,
            DropPerformed,
            Drop,
            Taken,
            Cancelled,
            Finished,
        ];
        let steps: Vec<Step> = signals
            .into_iter()
            .filter_map(|signal| machine.signal(signal))
            .collect();
        assert_eq!(steps, []);
    }

    // The thread accepts an offer only if it carries the running drag's
    // MIME type, so the token must differ between drags of a process and
    // between processes, or a stale or foreign drag could be dropped as
    // the current one.
    #[test]
    fn session_tokens_differ_between_drags_and_processes() {
        let current = SessionToken::new("panel", 40, 0xfeed, 7);
        let others = [
            ("an earlier drag", SessionToken::new("panel", 40, 0xfeed, 6)),
            ("another process", SessionToken::new("panel", 41, 0xfeed, 7)),
            ("a reused pid", SessionToken::new("panel", 40, 0xbeef, 7)),
            ("another kind", SessionToken::new("group", 40, 0xfeed, 7)),
        ];
        for (name, other) in others {
            assert_ne!(other.mime, current.mime, "{name}");
            assert!(!current.mime.starts_with(&other.mime), "{name} is a prefix");
        }
        let fresh = [SessionToken::next("panel"), SessionToken::next("panel")];
        assert_ne!(fresh[0].mime, fresh[1].mime);
    }
}
