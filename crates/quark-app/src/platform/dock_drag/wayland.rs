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

/// What a drag's signals add up to, apart from the protocol.
#[derive(Debug, Default)]
struct Machine {
    can_follow: bool,
    following: bool,
    /// The window the drag is over and the pointer's last point in it.
    entered: Option<(Surface, (f32, f32))>,
    /// The drag left our windows and has not come back.
    away: bool,
    /// The user released the drag (`dnd_drop_performed`).
    performed: bool,
    /// The compositor ended the drag.
    over: bool,
}

impl Machine {
    fn new(can_follow: bool) -> Self {
        Self {
            can_follow,
            ..Self::default()
        }
    }

    /// Whether a window may be attached now.
    fn may_follow(&self) -> Result<(), FollowError> {
        if self.over {
            Err(FollowError::Ended)
        } else if !self.can_follow {
            Err(FollowError::Unsupported)
        } else if self.following {
            Err(FollowError::Following)
        } else {
            Ok(())
        }
    }

    fn signal(&mut self, signal: DockSignal) -> Option<Step> {
        if self.over {
            return None;
        }
        let point = |x: f64, y: f64| (x as f32, y as f32);
        match signal {
            DockSignal::Enter { surface, x, y } => {
                let point = point(x, y);
                self.entered = Some((surface, point));
                self.away = false;
                Some(Step::Moved(Spot::Over { surface, point }))
            }
            DockSignal::Motion { x, y } => {
                let (surface, last) = self.entered.as_mut()?;
                *last = point(x, y);
                Some(Step::Moved(Spot::Over {
                    surface: *surface,
                    point: *last,
                }))
            }
            DockSignal::Leave => {
                self.entered = None;
                self.away = true;
                Some(Step::Moved(Spot::Unknown))
            }
            DockSignal::Drop => {
                self.over = true;
                let spot = self
                    .entered
                    .take()
                    .map_or(Spot::Unknown, |(surface, point)| Spot::Over {
                        surface,
                        point,
                    });
                Some(Step::Released(spot))
            }
            DockSignal::DropPerformed => {
                self.performed = true;
                None
            }
            DockSignal::Taken => {
                self.over = true;
                Some(Step::Released(Spot::Outside))
            }
            DockSignal::Cancelled | DockSignal::Finished => {
                self.over = true;
                // A following window can only be put back on a cancel the
                // compositor says is one.
                let released = self.performed || (self.away && !self.following);
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
        self.machine.can_follow
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
        self.machine.following = true;
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
        if !self.machine.over {
            self.machine.over = true;
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

    // A drag must end exactly once, the way the compositor's signals say:
    // dropped on a window of ours at its last point, released outside when
    // dropped elsewhere (including Hyprland's way, with no
    // dnd_drop_performed), and cancelled otherwise.
    #[test]
    fn compositor_signals_end_the_drag_once_as_released_or_cancelled() {
        use DockSignal::*;
        let enter = |surface| Enter {
            surface,
            x: 10.0,
            y: 20.0,
        };
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
            let mut machine = Machine::new(true);
            machine.following = following;
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
            machine.following = following;
            machine.over = over;
            assert_eq!(machine.may_follow(), expected, "{name}");
        }
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
