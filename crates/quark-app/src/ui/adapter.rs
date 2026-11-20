//! [`UiAdapter`]: runs a [`UiApp`] as an [`App`], selecting the state of
//! the window each callback is for and delivering what the app's calls
//! leave behind.

use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, OnceLock};

use accesskit::{ActionRequest, TreeUpdate};
use quark::reactive::SignalStore;
use quark::scene::Scene;
use quark_ui::element::{Delivery, DragEnd};
use quark_ui::key_context::KeyBindings;
use quark_ui::text_input::{TextEditCommand, TextEditOutcome, TextPointer, TextPointerEvent};
use quark_ui::theme::Theme;
use quark_ui::{Action, FocusId};

use super::accessibility::scale_tree;
use super::drag::{self, ActiveDrag};
use super::theme::ThemeChoice;
use super::window::{Parked, WindowStates, WindowUiState};
use super::{Redraw, UiApp, UiContext, UiSender, redraw, redraw_all};
use crate::input::UiInput;
use crate::{
    App, AppEvent, CloseReason, EventContext, FrameContext, InputEvent, RunError, WindowHandle,
    WindowOptions, run,
};

/// What every window shares.
pub(super) struct Shared {
    /// Names each window's accessibility tree unless the app names it.
    pub(super) name: String,
    pub(super) theme: Theme,
    pub(super) theme_choice: ThemeChoice,
    pub(super) signals: SignalStore,
    pub(super) key_bindings: KeyBindings,
    pub(super) drag: Option<ActiveDrag>,
    /// Actions a call into the app produced (drag handlers' handoff and
    /// end actions), delivered once it returns.
    pub(super) queued: Vec<Action>,
    /// Drag sessions the adapter ended, for [`UiApp::drag_session_ended`].
    pub(super) ended: Vec<DragEnd>,
    /// Windows whose focus the app set from another window's callback.
    pub(super) refocused: Vec<WindowHandle>,
}

/// Runs a [`UiApp`] as an [`App`].
pub struct UiAdapter<U: UiApp> {
    pub(crate) app: U,
    pub(super) shared: Shared,
    /// The window the adapter is acting for: every callback selects the
    /// window its context is bound to before touching window state.
    pub(super) win_handle: Option<WindowHandle>,
    pub(super) win: Box<WindowUiState>,
    /// `win` is the state the adapter was made with and no window has
    /// claimed yet: the first window adopts it.
    win_unclaimed: bool,
    /// Every other window's state.
    pub(super) parked: Parked,
    sender: UiSender<U::Message>,
    messages: Receiver<U::Message>,
}

/// Open a window titled by `options` and run `app` in it.
pub fn run_ui<U: UiApp>(app: U, options: WindowOptions) -> Result<(), RunError> {
    let adapter = UiAdapter::new(app, options.title.clone());
    run(adapter, options)
}

impl<U: UiApp> UiAdapter<U> {
    /// `name` labels the accessibility tree's window node. The theme
    /// follows the desktop's light or dark preference, dark until it is
    /// known.
    pub fn new(app: U, name: impl Into<String>) -> Self {
        let (sender, messages) = mpsc::channel();
        Self {
            app,
            shared: Shared {
                name: name.into(),
                theme: Theme::default_dark(),
                theme_choice: ThemeChoice::System {
                    light: Box::new(Theme::default_light()),
                    dark: Box::new(Theme::default_dark()),
                },
                signals: SignalStore::new(),
                key_bindings: KeyBindings::new(),
                drag: None,
                queued: Vec::new(),
                ended: Vec::new(),
                refocused: Vec::new(),
            },
            win_handle: None,
            win: Box::new(WindowUiState::new()),
            win_unclaimed: true,
            parked: Vec::new(),
            sender: UiSender {
                sender,
                waker: Arc::new(OnceLock::new()),
            },
            messages,
        }
    }

    /// Resolve key presses against `bindings` as well as the elements'
    /// own `on_key` handlers. A binding's predicate is tested against the
    /// key contexts (`key_context("editor")`) on the focus path; see
    /// [`quark_ui::key_context`]. A binding matched through a context
    /// deeper than the element handling the key wins; otherwise the
    /// element's handler does, and a binding without a predicate only gets
    /// keys no element handles.
    pub fn with_key_bindings(mut self, bindings: KeyBindings) -> Self {
        self.shared.key_bindings = bindings;
        self
    }

    pub fn app(&self) -> &U {
        &self.app
    }

    /// The app's signals, shared by every window; [`UiContext::signals`]
    /// inside the app.
    pub fn signals(&self) -> &SignalStore {
        &self.shared.signals
    }

    /// A sender for [`UiApp::message`], for threads started before
    /// [`run`]. Inside the app, [`UiContext::sender`] gives the same.
    pub fn sender(&self) -> UiSender<U::Message> {
        self.sender.clone()
    }

    /// Act for `window`: make its state the current one. Cheap, and free
    /// of allocation once every window has state.
    pub(super) fn select(&mut self, window: Option<WindowHandle>) {
        if self.win_handle == window {
            return;
        }
        if self.win_unclaimed && window.is_some() {
            self.win_unclaimed = false;
            self.win_handle = window;
            return;
        }
        let from = self.win_handle;
        match self.parked.iter().position(|(handle, _)| *handle == window) {
            Some(index) => {
                let slot = &mut self.parked[index];
                std::mem::swap(&mut slot.1, &mut self.win);
                slot.0 = from;
            }
            None => {
                let state = std::mem::replace(&mut self.win, Box::new(WindowUiState::new()));
                self.parked.push((from, state));
            }
        }
        self.win_handle = window;
    }

    pub(super) fn window_states(&self, window: WindowHandle) -> Option<&WindowUiState> {
        if self.win_handle == Some(window) {
            return Some(&self.win);
        }
        self.parked
            .iter()
            .find(|(handle, _)| *handle == Some(window))
            .map(|(_, state)| &**state)
    }

    /// Run `f` with the app and a [`UiContext`] over `cx`.
    pub(super) fn with_app<R>(
        &mut self,
        cx: &mut EventContext,
        f: impl FnOnce(&mut U, &mut UiContext) -> R,
    ) -> R {
        let mut ucx = UiContext {
            window: cx,
            windows: WindowStates {
                handle: self.win_handle,
                current: &mut self.win,
                parked: &mut self.parked,
            },
            shared: &mut self.shared,
            sender: &self.sender,
        };
        f(&mut self.app, &mut ucx)
    }

    /// The app's [`UiApp::edit_text_in`] for `window`, or
    /// [`UiApp::edit_text`] when the adapter acts for no window.
    pub(super) fn edit_text_in(
        app: &mut U,
        window: Option<WindowHandle>,
        target: FocusId,
        command: TextEditCommand,
    ) -> TextEditOutcome {
        match window {
            Some(window) => app.edit_text_in(window, target, command),
            None => app.edit_text(target, command),
        }
    }

    fn deliver_messages(&mut self, cx: &mut EventContext) {
        while let Ok(message) = self.messages.try_recv() {
            self.with_app(cx, |app, ucx| app.message(message, ucx));
        }
    }

    pub(super) fn dispatch(&mut self, actions: Vec<Action>, cx: &mut EventContext) {
        for action in actions {
            if let Some(event) = action.downcast_ref::<TextPointerEvent>() {
                let extend = cx.modifiers().shift_key();
                let now_ms = cx.elapsed().as_millis() as u64;
                if let Some((target, command)) =
                    self.win
                        .text_pointer
                        .event(*event, &self.win.text_areas, now_ms, extend)
                {
                    Self::edit_text_in(&mut self.app, self.win_handle, target, command);
                }
                continue;
            }
            let Some(action) = action.downcast_ref::<U::Action>().cloned() else {
                tracing::debug!("ignoring action of another type: {action:?}");
                continue;
            };
            self.with_app(cx, |app, ucx| app.update(action, ucx));
        }
        redraw(Redraw::Action, self.win_handle, cx);
        self.end_taken_pointer(cx);
    }

    /// Hand a routed event's actions to the app. A delivery without actions
    /// changed nothing, so it draws nothing.
    pub(super) fn deliver(&mut self, delivery: Delivery, cx: &mut EventContext) {
        if !delivery.actions.is_empty() {
            self.dispatch(delivery.actions, cx);
        } else if delivery.redraw {
            redraw(Redraw::Scroll, self.win_handle, cx);
        }
    }

    /// Deliver what calls into the app left for after them: drag handler
    /// actions, and drag sessions the adapter ended.
    fn flush_queued(&mut self, cx: &mut EventContext) {
        loop {
            if !self.shared.queued.is_empty() {
                let actions = std::mem::take(&mut self.shared.queued);
                self.dispatch(actions, cx);
            } else if !self.shared.ended.is_empty() {
                let end = self.shared.ended.remove(0);
                self.with_app(cx, |app, ucx| app.drag_session_ended(end, ucx));
            } else {
                return;
            }
        }
    }

    /// End the drag session, cancelled, on the adapter's own account.
    pub(super) fn end_session(&mut self, cx: &mut EventContext) {
        let windows = WindowStates {
            handle: self.win_handle,
            current: &mut self.win,
            parked: &mut self.parked,
        };
        let shared = &mut self.shared;
        if let Some(end) = drag::cancel(&mut shared.drag, &windows, cx, &mut shared.queued) {
            shared.ended.push(end);
        }
    }

    fn after_input(&mut self, window_blurred: bool, cx: &mut EventContext) {
        self.flush_queued(cx);
        // Signals are shared, so any window may show one; each frame clears
        // the flag once painted.
        if self.shared.signals.any_dirty() {
            redraw_all(Redraw::Signal, cx);
        }
        self.settle_focus(window_blurred, cx);
        // Focus the app moved in other windows from this callback.
        while let Some(window) = self.shared.refocused.pop() {
            let home = self.win_handle;
            self.select(Some(window));
            self.settle_focus(false, cx);
            self.select(home);
        }
    }

    /// `window` is closing: end its composition, so its field shows no
    /// preedit that no commit will ever end, and a drag session that
    /// started in it.
    fn window_closing(&mut self, window: WindowHandle, cx: &mut EventContext) {
        if drag::window_closing(&mut self.shared.drag, window) {
            self.end_session(cx);
        }
        let state = if self.win_handle == Some(window) {
            Some(&mut *self.win)
        } else {
            self.parked
                .iter_mut()
                .find(|(handle, _)| *handle == Some(window))
                .map(|(_, state)| &mut **state)
        };
        let Some(state) = state else {
            return;
        };
        state.text_pointer = TextPointer::default();
        if let Some(target) = state.ime.close() {
            self.app
                .edit_text_in(window, target, TextEditCommand::CancelPreedit);
        }
    }

    /// Drop a closed window's state.
    fn forget_window(&mut self, window: WindowHandle) {
        if self.win_handle == Some(window) {
            self.select(None);
        }
        let signals = &self.shared.signals;
        self.parked.retain_mut(|(handle, state)| {
            let keep = *handle != Some(window);
            if !keep {
                state.element_cache.release_signals(signals);
            }
            keep
        });
    }
}

impl<U: UiApp> App for UiAdapter<U> {
    fn init(&mut self, cx: &mut EventContext) {
        self.select(cx.window_handle());
        let _ = self.sender.waker.set(cx.waker().clone());
        self.with_app(cx, |app, ucx| app.init(ucx));
        self.deliver_messages(cx);
        self.after_input(false, cx);
    }

    fn frame(&mut self, cx: &mut FrameContext) -> Scene {
        self.render(cx)
    }

    fn event(&mut self, event: InputEvent, cx: &mut EventContext) {
        self.select(cx.window_handle());
        // Checked before the app's own hook, which may take IME events for
        // an element it drives itself (a terminal).
        self.orphan_moved_composition();
        if !self.ime_event_is_current(&event) {
            tracing::debug!("dropping IME input for an element focus left: {event:?}");
            return;
        }
        let input = UiInput::from_event(&event);
        let window_blurred = input == Some(UiInput::WindowFocus(false));
        self.handle_event(&event, input, cx);
        self.after_input(window_blurred, cx);
    }

    fn wake(&mut self, cx: &mut EventContext) {
        self.select(cx.window_handle());
        self.deliver_messages(cx);
        self.with_app(cx, |app, ucx| app.wake(ucx));
        self.after_input(false, cx);
    }

    fn app_event(&mut self, event: AppEvent, cx: &mut EventContext) {
        self.select(cx.window_handle());
        if let AppEvent::ThemeChanged(system) = &event
            && self.shared.follow_desktop(*system)
        {
            // The runner reports the desktop's theme once for the app.
            redraw_all(Redraw::Theme, cx);
        }
        match event {
            AppEvent::WindowOpened(window) => {
                self.shared.theme_native_window(window, cx);
                self.with_app(cx, |app, ucx| app.window_opened(window, ucx));
            }
            AppEvent::WindowClosed { window, reason } => {
                self.window_closing(window, cx);
                self.with_app(cx, |app, ucx| app.window_closed(window, reason, ucx));
            }
            _ => {}
        }
        let closed = match event {
            AppEvent::WindowClosed { window, .. } => Some(window),
            _ => None,
        };
        self.with_app(cx, |app, ucx| app.app_event(event, ucx));
        if let Some(window) = closed {
            self.forget_window(window);
        }
        self.after_input(false, cx);
    }

    fn close_requested(&mut self, reason: CloseReason, cx: &mut EventContext) -> bool {
        self.select(cx.window_handle());
        let close = self.with_app(cx, |app, ucx| app.window_close_requested(reason, ucx));
        self.after_input(false, cx);
        close
    }

    fn recycle_scene(&mut self, window: WindowHandle, scene: Scene) {
        if self.win_handle == Some(window) {
            self.win.spare.scene = scene;
        } else if let Some((_, state)) = self
            .parked
            .iter_mut()
            .find(|(handle, _)| *handle == Some(window))
        {
            state.spare.scene = scene;
        }
    }

    fn accessibility(&mut self, window: WindowHandle) -> Option<TreeUpdate> {
        // A full tree; accesskit diffs it against the last one.
        let mut update = self.logical_accessibility_tree(window)?;
        let scale = self.window_states(window)?.scale_factor;
        scale_tree(&mut update, scale);
        Some(update)
    }

    fn accessibility_action(&mut self, request: ActionRequest, cx: &mut EventContext) {
        self.select(cx.window_handle());
        self.handle_accessibility_action(request, cx);
        self.after_input(false, cx);
    }
}

/// What [`crate::testing`] reads from the adapter, per window. `None` for
/// a window the adapter has heard nothing of.
#[cfg(feature = "test-support")]
impl<U: UiApp> UiAdapter<U> {
    pub(crate) fn app_mut(&mut self) -> &mut U {
        &mut self.app
    }

    pub(crate) fn window_state(&self, window: WindowHandle) -> Option<&WindowUiState> {
        self.window_states(window)
    }
}
