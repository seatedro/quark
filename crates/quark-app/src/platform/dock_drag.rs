//! The native side of docking drags: following a dragged tab across the
//! app's windows, finding the window under it, and moving a torn-off
//! window with the pointer.
//!
//! A dock drag starts in one window's router as pointer capture. Once it
//! must leave that window, the app hands the capture off into a
//! [`quark_ui::element::DragSession`] and starts a [`DockDrag`] beside it.
//! From then on the app feeds the [`DockDrag`] every window's input
//! ([`DockDrag::handle_input`]) and wakes ([`DockDrag::poll`]), and passes
//! each [`DockDragEvent`] it returns on to the session: a
//! [`DockDragEvent::Moved`] location to `DragSession::update`, a
//! [`DockDragEvent::Released`] one to `DragSession::finish`, and a
//! [`DockDragEvent::Cancelled`] to `DragSession::cancel`. Locations name
//! windows by [`drag_window_id`].
//!
//! # Transports
//!
//! | Platform | Locating the pointer | Live tear-off |
//! |---|---|---|
//! | Windows | The source window keeps the mouse (winit's `SetCapture`), so its motion is put on the desktop; `WindowFromPoint`, then the z-order below a followed window, finds what is on top | `align_window` on every motion |
//! | macOS | AppKit sends the pressed view `mouseDragged` outside it; `+[NSWindow windowNumberAtPoint:belowWindowWithWindowNumber:]` finds what is on top | `align_window` on every motion |
//! | X11 | The press's implicit (XI2) grab keeps motion on the source window; the root window's stacking order, on a connection of our own, finds what is on top | `align_window` on every motion |
//! | Wayland | The compositor's drag and drop (`wl_data_device`) names the window under the pointer and the point in it, on the queue `drag_out` already runs | `xdg_toplevel_drag_v1` where the compositor has it, else none |
//!
//! On the desktop platforms a window of another app over one of ours
//! hides it: the pointer over it is [`DragLocation::Outside`], never a drop
//! into the window underneath. The stacking query sits behind
//! [`WindowStack`], which tests replace with `ScriptedStack` (feature
//! `test-support`).
//!
//! # Live tear-off
//!
//! Once the dock model has detached the payload into a new floating window
//! (opened with `active: false`), call [`DockDrag::follow`] with it and the
//! point of it to hold under the pointer, from its
//! [`crate::AppEvent::WindowOpened`]. The window then moves with the pointer
//! and is never itself the drop location. At the end:
//!
//! - [`DockDragEvent::Released`] over a drop target: re-dock
//!   (`drop_live_detach`), closing the floating window.
//! - Released anywhere else: the window stays where it is
//!   (`end_live_detach`).
//! - [`DockDragEvent::Cancelled`], including Escape: put the payload back
//!   (`cancel_live_detach`).
//!
//! On Wayland without `xdg_toplevel_drag_v1` ([`DockDrag::can_live_detach`]
//! is false) nothing can follow the pointer; a drag that ends away from
//! the app's windows arrives as [`DockDragEvent::Released`] at
//! [`DragLocation::Outside`], to open the payload in a new window there.

use quark_ui::element::{DragLocation, DragWindowId};
use winit::event::{ElementState, MouseButton};
use winit::keyboard::NamedKey;

use crate::input::{InputEvent, KeyKind};
use crate::platform::drag_out::{DragImage, DragOutError};
use crate::runner::{DesktopPoint, EventContext, WindowHandle};

mod stack;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "linux")]
mod wayland;
#[cfg(windows)]
mod win32;
#[cfg(target_os = "linux")]
mod x11;

#[cfg(any(test, feature = "test-support"))]
pub use stack::ScriptedStack;
pub use stack::{StackHit, WindowStack};

#[cfg(test)]
mod tests;

/// How drag locations name `window`: unique among every window the runner
/// ever opened.
pub fn drag_window_id(window: WindowHandle) -> DragWindowId {
    DragWindowId(window.scope_id())
}

/// The open window [`drag_window_id`] gave `id`, if it is still open.
pub fn window_of(cx: &EventContext, id: DragWindowId) -> Option<WindowHandle> {
    cx.windows()
        .into_iter()
        .find(|&window| drag_window_id(window) == id)
}

/// What starts a [`DockDrag`].
#[derive(Debug, Clone)]
pub struct DockDragStart {
    source: WindowHandle,
    pointer: (f32, f32),
    kind: String,
    seat: Option<String>,
    image: Option<DragImage>,
}

impl DockDragStart {
    /// A drag from `source`, whose primary button is held, with the pointer
    /// at `pointer` in its logical coordinates. `kind` names what is
    /// dragged (such as `panel` or `group`), in ASCII letters, digits, `-`,
    /// and `.`; on Wayland it is part of the drag's MIME type.
    pub fn new(source: WindowHandle, pointer: (f32, f32), kind: impl Into<String>) -> Self {
        Self {
            source,
            pointer,
            kind: kind.into(),
            seat: None,
            image: None,
        }
    }

    /// Drag with the Wayland seat of this name; see
    /// [`crate::platform::drag_out::DragOutOptions::seat`]. Ignored
    /// elsewhere.
    pub fn seat(mut self, name: impl Into<String>) -> Self {
        self.seat = Some(name.into());
        self
    }

    /// What the compositor shows under the pointer on Wayland until a
    /// window follows it. Ignored elsewhere, where the source window paints
    /// the drag's preview.
    pub fn image(mut self, image: DragImage) -> Self {
        self.image = Some(image);
        self
    }
}

/// Which native mechanism carries a [`DockDrag`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    /// Pointer motion of the source window, put on the desktop and located
    /// by the window stack: Windows, macOS, X11.
    Desktop,
    /// The compositor's drag and drop. `live`: it can move a window with
    /// the drag (`xdg_toplevel_drag_v1`).
    Wayland { live: bool },
}

/// What a [`DockDrag`] reports.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DockDragEvent {
    /// The pointer is now at this location.
    Moved(DragLocation),
    /// The button was released at this location. The drag is over.
    Released(DragLocation),
    /// The drag ended without a drop. The drag is over.
    Cancelled(CancelReason),
}

/// Why a [`DockDrag`] was cancelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelReason {
    /// Escape was pressed.
    Escape,
    /// The button is up but the release never came: another client or the
    /// system took the pointer.
    CaptureLost,
    /// The source window, or the window following the pointer, closed.
    WindowClosed,
    /// The compositor ended the drag without a drop (Escape on Wayland).
    Platform,
}

/// Why [`DockDrag::follow`] refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FollowError {
    /// Windows cannot follow the pointer here: no window positions, or a
    /// Wayland compositor without `xdg_toplevel_drag_v1`.
    Unsupported,
    /// The window is not open (yet), or has no native surface.
    NotOpen,
    /// Another window follows the pointer already.
    Following,
    /// The drag is over.
    Ended,
}

impl std::fmt::Display for FollowError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Unsupported => "windows cannot follow the pointer here",
            Self::NotOpen => "the window is not open",
            Self::Following => "another window follows the pointer already",
            Self::Ended => "the drag is over",
        })
    }
}

impl std::error::Error for FollowError {}

/// A window moved with the pointer, and the point of it held under the
/// pointer, in its logical coordinates.
#[derive(Debug, Clone, Copy)]
struct Follow {
    window: WindowHandle,
    hotspot: (f32, f32),
}

enum Backend {
    Desktop {
        stack: Box<dyn WindowStack>,
        /// The pointer's last position on the desktop.
        pointer: Option<DesktopPoint>,
    },
    #[cfg(target_os = "linux")]
    Wayland(wayland::WaylandDrag),
}

/// One docking drag across the app's windows; see the [module docs](self).
/// Over once it reports [`DockDragEvent::Released`] or
/// [`DockDragEvent::Cancelled`]; dropping it before then cancels the native
/// drag.
pub struct DockDrag {
    source: WindowHandle,
    location: DragLocation,
    follow: Option<Follow>,
    over: bool,
    backend: Backend,
}

impl std::fmt::Debug for DockDrag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DockDrag")
            .field("source", &self.source)
            .field("transport", &self.transport())
            .field("location", &self.location)
            .field("following", &self.following())
            .field("over", &self.over)
            .finish_non_exhaustive()
    }
}

impl DockDrag {
    /// Start a drag the platform's way, from a pointer event of `start`'s
    /// source window while its primary button is held.
    pub fn start(cx: &mut EventContext, start: DockDragStart) -> Result<Self, DragOutError> {
        validate_kind(&start.kind)?;
        let window = cx
            .window_by_handle(start.source)
            .ok_or(DragOutError::NoWindow)?;
        #[cfg(target_os = "linux")]
        if !cx.capabilities().desktop_pointer {
            let drag = wayland::WaylandDrag::start(window, &start, cx.waker().clone())?;
            return Ok(Self {
                source: start.source,
                location: DragLocation::Unknown,
                follow: None,
                over: false,
                backend: Backend::Wayland(drag),
            });
        }
        let stack = stack::native(window)?;
        Self::start_with_stack(cx, start, stack)
    }

    /// Start a drag located by `stack` instead of the platform's window
    /// stack: for tests, and windows the platform's stack cannot see.
    /// Needs desktop pointer coordinates
    /// ([`crate::PlatformCapabilities::desktop_pointer`]).
    pub fn start_with_stack(
        cx: &mut EventContext,
        start: DockDragStart,
        stack: Box<dyn WindowStack>,
    ) -> Result<Self, DragOutError> {
        validate_kind(&start.kind)?;
        if !cx.capabilities().desktop_pointer {
            return Err(DragOutError::Unsupported);
        }
        let pointer = cx
            .to_desktop(start.source, start.pointer)
            .ok_or(DragOutError::NoWindow)?;
        let mut drag = Self {
            source: start.source,
            location: DragLocation::Unknown,
            follow: None,
            over: false,
            backend: Backend::Desktop {
                stack,
                pointer: Some(pointer),
            },
        };
        drag.locate(cx);
        Ok(drag)
    }

    pub fn transport(&self) -> Transport {
        match &self.backend {
            Backend::Desktop { .. } => Transport::Desktop,
            #[cfg(target_os = "linux")]
            Backend::Wayland(drag) => Transport::Wayland {
                live: drag.can_follow(),
            },
        }
    }

    /// Whether a torn-off window can follow the pointer
    /// ([`Self::follow`]). Where it cannot, a release outside the app's
    /// windows still ends at [`DragLocation::Outside`].
    pub fn can_live_detach(&self, cx: &EventContext) -> bool {
        match &self.backend {
            Backend::Desktop { .. } => cx.capabilities().window_positions,
            #[cfg(target_os = "linux")]
            Backend::Wayland(drag) => drag.can_follow(),
        }
    }

    pub fn source(&self) -> WindowHandle {
        self.source
    }

    /// Where the pointer was last located.
    pub fn location(&self) -> DragLocation {
        self.location
    }

    /// The window following the pointer.
    pub fn following(&self) -> Option<WindowHandle> {
        self.follow.map(|follow| follow.window)
    }

    /// Whether the drag has ended.
    pub fn is_over(&self) -> bool {
        self.over
    }

    /// Move `window` with the pointer from now on, holding its logical
    /// point `hotspot` under it, and leave it out of the drop location.
    /// On the desktop it moves at once to the pointer's last position.
    /// Call once the window is open ([`crate::AppEvent::WindowOpened`]),
    /// before its first frame: on Wayland a window must be attached to the
    /// drag before it is shown.
    pub fn follow(
        &mut self,
        cx: &mut EventContext,
        window: WindowHandle,
        hotspot: (f32, f32),
    ) -> Result<(), FollowError> {
        if self.over {
            return Err(FollowError::Ended);
        }
        if self.follow.is_some() {
            return Err(FollowError::Following);
        }
        match &mut self.backend {
            Backend::Desktop { pointer, .. } => {
                if !cx.capabilities().window_positions {
                    return Err(FollowError::Unsupported);
                }
                if cx.placement(window).is_none() {
                    return Err(FollowError::NotOpen);
                }
                if let Some(at) = *pointer {
                    cx.align_window(window, hotspot, at);
                }
            }
            #[cfg(target_os = "linux")]
            Backend::Wayland(drag) => drag.follow(cx, window, hotspot)?,
        }
        self.follow = Some(Follow { window, hotspot });
        if matches!(self.backend, Backend::Desktop { .. }) {
            self.locate(cx);
        }
        Ok(())
    }

    /// Feed an input event of the context's window. Returns what it did to
    /// the drag: motion moves it (and the window following it), a primary
    /// release drops it, Escape cancels it, and losing focus with the
    /// button up cancels it as the pointer taken away. On Wayland the
    /// compositor holds the pointer, so input does nothing; see
    /// [`Self::poll`].
    pub fn handle_input(
        &mut self,
        cx: &mut EventContext,
        event: &InputEvent,
    ) -> Option<DockDragEvent> {
        if self.over {
            return None;
        }
        // Only Wayland drags are not desktop ones.
        #[cfg_attr(not(target_os = "linux"), allow(irrefutable_let_patterns))]
        let Backend::Desktop { stack, pointer } = &mut self.backend else {
            return None;
        };
        let window = cx.window_handle()?;
        let event = match event {
            InputEvent::PointerMoved { x, y } => {
                // X11 reports motion over the window following the pointer
                // against that window, whose position is in flux while the
                // server catches up with the moves asked of it: ask the
                // window system where the pointer is instead, or skip it.
                let at = if self.follow.is_some_and(|f| f.window == window) {
                    stack.pointer()?
                } else {
                    cx.to_desktop(window, (*x, *y))?
                };
                *pointer = Some(at);
                if let Some(follow) = self.follow {
                    cx.align_window(follow.window, follow.hotspot, at);
                }
                DockDragEvent::Moved(self.locate(cx))
            }
            InputEvent::PointerButton {
                button: MouseButton::Left,
                state: ElementState::Released,
            } => DockDragEvent::Released(self.locate(cx)),
            InputEvent::KeyPress(chord) if chord.logical == KeyKind::Named(NamedKey::Escape) => {
                DockDragEvent::Cancelled(CancelReason::Escape)
            }
            InputEvent::Focused(false) if stack.primary_held() == Some(false) => {
                DockDragEvent::Cancelled(CancelReason::CaptureLost)
            }
            _ => return None,
        };
        Some(self.report(event))
    }

    /// The transport's own news since the last call, for
    /// [`crate::App::wake`]: on Wayland the compositor's drag events, which
    /// a thread receives and wakes the app for. Empty elsewhere.
    pub fn poll(&mut self, cx: &mut EventContext) -> Vec<DockDragEvent> {
        let _ = &cx;
        #[cfg(target_os = "linux")]
        if let Backend::Wayland(drag) = &mut self.backend
            && !self.over
        {
            let events = drag.drain(cx);
            return events.into_iter().map(|event| self.report(event)).collect();
        }
        Vec::new()
    }

    /// Tell the drag `window` closed. The source window or the window
    /// following the pointer closing cancels it.
    pub fn window_closed(&mut self, window: WindowHandle) -> Option<DockDragEvent> {
        if self.over || (window != self.source && self.following() != Some(window)) {
            return None;
        }
        Some(self.report(DockDragEvent::Cancelled(CancelReason::WindowClosed)))
    }

    /// Note `event` as the drag's latest news, ending the drag on a release
    /// or cancel.
    fn report(&mut self, event: DockDragEvent) -> DockDragEvent {
        match event {
            DockDragEvent::Moved(location) => self.location = location,
            DockDragEvent::Released(location) => {
                self.location = location;
                self.end();
            }
            DockDragEvent::Cancelled(_) => self.end(),
        }
        event
    }

    fn end(&mut self) {
        self.over = true;
        #[cfg(target_os = "linux")]
        if let Backend::Wayland(drag) = &mut self.backend {
            drag.end();
        }
    }

    /// Locate the desktop pointer: the window on top under it, leaving out
    /// the window following it.
    fn locate(&mut self, cx: &EventContext) -> DragLocation {
        let skip = self.following();
        let Backend::Desktop {
            stack,
            pointer: Some(at),
        } = &mut self.backend
        else {
            return self.location;
        };
        self.location = match stack.hit(cx, *at, skip) {
            StackHit::Owned(window) => match cx.from_desktop(window, *at) {
                Some(point) => DragLocation::Window {
                    window: drag_window_id(window),
                    point,
                },
                None => DragLocation::Unknown,
            },
            StackHit::Other => DragLocation::Outside,
            StackHit::Unknown => DragLocation::Unknown,
        };
        self.location
    }
}

impl Drop for DockDrag {
    fn drop(&mut self) {
        if !self.over {
            self.end();
        }
    }
}

/// `kind` goes into a MIME type, so it must be a plain token.
fn validate_kind(kind: &str) -> Result<(), DragOutError> {
    let plain = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '.';
    if kind.is_empty() || !kind.chars().all(plain) {
        return Err(DragOutError::Platform(format!(
            "dock drag kind {kind:?} is not a plain token"
        )));
    }
    Ok(())
}
