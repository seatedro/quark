//! What the app's callbacks see: [`ViewContext`] while a window's frame is
//! built, and [`UiContext`] everywhere else.

use std::any::Any;

use quark::Rect;
use quark::reactive::SignalStore;
use quark_ui::FocusId;
use quark_ui::animation::AnimationTable;
use quark_ui::element::{
    DropTargets, ElementHandle, ElementHandles, LayoutSnapshot, TooltipRegion, WindowSignals,
};
use quark_ui::key_context::KeyBindings;
use quark_ui::theme::Theme;

use super::adapter::Shared;
use super::window::{WindowStates, WindowUiState};
use super::{HoveredTooltip, UiSender, drag};
use crate::{EventContext, FrameContext, WindowHandle};

/// What [`UiApp::view`](crate::UiApp::view) can see while building a frame of one window.
pub struct ViewContext<'a, 'f> {
    pub frame: &'a mut FrameContext<'f>,
    pub theme: &'a Theme,
    pub(super) signals: &'a SignalStore,
    pub(super) window_signals: WindowSignals,
    pub(super) focus: Option<FocusId>,
    pub(super) focus_visible: bool,
    pub(super) animations: &'a mut AnimationTable,
    pub(super) geometry: &'a LayoutSnapshot,
    pub(super) handles: &'a mut ElementHandles,
    pub(super) tooltips: &'a [TooltipRegion],
    pub(super) hovered_tooltip: Option<&'a HoveredTooltip>,
}

impl ViewContext<'_, '_> {
    /// The window this frame is for.
    pub fn window_handle(&self) -> WindowHandle {
        self.frame.window_handle()
    }

    /// The app's signals, shared by every window. Read them inside a
    /// [`reactive`](quark_ui::element::reactive) boundary's build, through
    /// its context, so the boundary depends on them.
    pub fn signals(&self) -> &SignalStore {
        self.signals
    }

    /// The signals this window publishes its focus and theme through,
    /// current for this frame.
    pub fn window_signals(&self) -> WindowSignals {
        self.window_signals
    }

    /// Where the elements of the window's last completed frame landed: the
    /// frame before the one this view builds. Empty before the first frame.
    /// Reading it lays nothing out.
    pub fn geometry(&self) -> &LayoutSnapshot {
        self.geometry
    }

    /// A new handle to name an element of this window with
    /// (`Div::element_handle`) and find it in [`Self::geometry`]. Release
    /// it once the element is gone.
    pub fn new_element_handle(&mut self) -> ElementHandle {
        self.handles.allocate()
    }

    pub fn release_element_handle(&mut self, handle: ElementHandle) {
        self.handles.release(handle);
    }

    /// The tooltip (`Div::tooltip`) of the innermost element under
    /// `(x, y)` in the window's last completed frame, with that element's
    /// bounds. The adapter collects tooltips but draws none: the app
    /// decides when and where to show them.
    pub fn tooltip_at(&self, x: f32, y: f32) -> Option<(&str, Rect)> {
        // Paint order: a child's region comes after its parent's.
        self.tooltips
            .iter()
            .rev()
            .find(|t| t.bounds.contains(x, y))
            .map(|t| (&*t.text, t.bounds))
    }

    /// The tooltip under the pointer. The window redraws when it changes,
    /// so an app shows it after a delay by requesting a frame at
    /// `since + delay`.
    pub fn hovered_tooltip(&self) -> Option<&HoveredTooltip> {
        self.hovered_tooltip
    }

    /// The window's animation table, ticked to this frame's clock. Rows
    /// still moving after paint schedule the window's next frame.
    pub fn animations(&mut self) -> &mut AnimationTable {
        self.animations
    }

    /// The window's focus.
    pub fn focus(&self) -> Option<FocusId> {
        self.focus
    }

    pub fn is_focused(&self, target: FocusId) -> bool {
        self.focus == Some(target)
    }

    /// Whether `target` is focused and its focus shows, as CSS
    /// `:focus-visible`: it came from the keyboard or assistive tech, not
    /// a click. Paint custom focus indicators on this; text fields keep
    /// their caret on [`Self::is_focused`].
    pub fn is_focus_visible(&self, target: FocusId) -> bool {
        self.is_focused(target) && self.focus_visible
    }
}

/// An [`EventContext`] plus the adapter's focus and message sender. Methods
/// without a window in their name act on the context's window.
pub struct UiContext<'a, 'w> {
    pub window: &'a mut EventContext<'w>,
    pub(super) windows: WindowStates<'a>,
    pub(super) shared: &'a mut Shared,
    /// The adapter's `UiSender<U::Message>`.
    pub(super) sender: &'a dyn Any,
}

impl UiContext<'_, '_> {
    /// The window this callback is for. `None` only for app events that
    /// arrive while no window is open.
    pub fn window_handle(&self) -> Option<WindowHandle> {
        self.windows.handle
    }

    /// The app's signals, shared by every window. A write redraws every
    /// window once the callback returns; boundaries that did not read the
    /// signal replay.
    pub fn signals(&self) -> &SignalStore {
        &self.shared.signals
    }

    /// Where the elements of the window's last completed frame landed, the
    /// frame input is routed through. Empty before the first frame. Reading
    /// it lays nothing out, so it does not see changes the app made since.
    pub fn geometry(&self) -> &LayoutSnapshot {
        &self.windows.current.router.frame().geometry
    }

    /// [`Self::geometry`] of `window`, if it has painted or is open.
    pub fn geometry_in(&self, window: WindowHandle) -> Option<&LayoutSnapshot> {
        let state = self.windows.get(window)?;
        Some(&state.router.frame().geometry)
    }

    /// Where `window`'s last completed frame takes drops.
    pub fn drop_targets_in(&self, window: WindowHandle) -> Option<&DropTargets> {
        let state = self.windows.get(window)?;
        Some(&state.router.frame().drop_targets)
    }

    /// A new handle to name an element of this window with
    /// (`Div::element_handle`) and find it in [`Self::geometry`]. Release
    /// it once the element is gone.
    pub fn new_element_handle(&mut self) -> ElementHandle {
        self.windows.current.element_handles.allocate()
    }

    pub fn release_element_handle(&mut self, handle: ElementHandle) {
        self.windows.current.element_handles.release(handle);
    }

    /// The window's key bindings, resolved against the key contexts on
    /// the focus path; see
    /// [`UiAdapter::with_key_bindings`](crate::UiAdapter::with_key_bindings).
    /// Every window shares them. Replace them when the user edits the
    /// keymap.
    pub fn key_bindings_mut(&mut self) -> &mut KeyBindings {
        &mut self.shared.key_bindings
    }

    /// Drag `paths` out of the window as files
    /// ([`EventContext::start_drag_out`]). The platform takes the pointer,
    /// so the adapter ends its pointer capture: the drag that called this
    /// is cancelled ([`DragHandler::on_cancel`](crate::quark_ui::element::DragHandler::on_cancel))
    /// right after.
    pub fn start_drag_out<P: AsRef<std::path::Path>>(
        &mut self,
        paths: impl IntoIterator<Item = P>,
    ) -> Result<(), crate::platform::drag_out::DragOutError> {
        self.start_drag_out_with_options(paths, &Default::default())
    }

    /// [`Self::start_drag_out`] with a drag image or a Wayland seat
    /// ([`EventContext::start_drag_out_with_options`]).
    pub fn start_drag_out_with_options<P: AsRef<std::path::Path>>(
        &mut self,
        paths: impl IntoIterator<Item = P>,
        options: &crate::platform::drag_out::DragOutOptions,
    ) -> Result<(), crate::platform::drag_out::DragOutError> {
        self.window.start_drag_out_with_options(paths, options)?;
        self.windows.current.pointer.taken = true;
        Ok(())
    }

    /// Whether a pointer drag holds the pointer: a dragged tab, divider,
    /// or text selection in this window, or a drag session.
    pub fn is_dragging(&self) -> bool {
        self.windows.current.router.is_capturing() || self.shared.drag.is_some()
    }

    /// End the pointer drag, if any, without its release, as Escape does in
    /// most apps: its handler backs out (`DragHandler::on_cancel`), so a
    /// dragged tab drops nowhere. A drag session ends too, reported to
    /// [`UiApp::drag_session_ended`](crate::UiApp::drag_session_ended).
    pub fn cancel_drag(&mut self) {
        if self.windows.current.router.is_capturing() {
            self.windows.current.pointer.taken = true;
        }
        let shared = &mut *self.shared;
        if let Some(end) = drag::cancel(
            &mut shared.drag,
            &self.windows,
            self.window,
            &mut shared.queued,
        ) {
            shared.ended.push(end);
        }
    }

    /// `window`'s state, made now for a window open or opening that has
    /// had no callback yet.
    pub(super) fn state_in(&mut self, window: WindowHandle) -> Option<&mut WindowUiState> {
        if self.windows.get(window).is_none() {
            if !self.window.windows().contains(&window) {
                return None;
            }
            let state = Box::new(WindowUiState::new());
            self.windows.parked.push((Some(window), state));
        }
        self.windows.get_mut(window)
    }

    /// A sender for this app's messages; `M` must be the app's
    /// [`UiApp::Message`](crate::UiApp::Message).
    ///
    /// # Panics
    ///
    /// When `M` is another type.
    #[track_caller]
    pub fn sender<M: Send + 'static>(&self) -> UiSender<M> {
        self.sender
            .downcast_ref::<UiSender<M>>()
            .expect("UiContext::sender: M must be the app's UiApp::Message")
            .clone()
    }
}
