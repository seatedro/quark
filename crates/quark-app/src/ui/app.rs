//! The app side of the adapter: [`UiApp`], what an app implements, and
//! [`UiSender`], how other threads reach it.

use std::any::Any;
use std::sync::mpsc::Sender;
use std::sync::{Arc, OnceLock};

use quark_ui::FocusId;
use quark_ui::element::{AnyElement, DragEnd};
use quark_ui::text_input::{TextEditCommand, TextEditOutcome};

use super::{UiContext, ViewContext};
use crate::{AppEvent, CloseReason, InputEvent, Waker, WindowHandle};

/// An app described as quark-ui elements.
///
/// Elements carry actions as type-erased [`Action`](quark_ui::Action)s;
/// the adapter downcasts each to [`UiApp::Action`] and passes it to
/// [`UiApp::update`]. Actions of other types (such as `NoopAction`) are
/// dropped.
///
/// A text field needs no input code: give it a `focus_target` and implement
/// [`UiApp::edit_text`] (and [`UiApp::set_preedit`] for IME). While it has
/// focus, the adapter turns typed text, IME commits, editing keys, paste,
/// and pointer selection into [`TextEditCommand`]s for it.
///
/// Every window the app opens shares it; contexts name the window they are
/// for. See the [module docs](super#windows).
pub trait UiApp: 'static {
    type Action: Any + Clone;

    /// Values other threads send through a [`UiSender`] (socket reads,
    /// finished jobs). Use `()` when the app takes none.
    type Message: Send + 'static;

    /// Called once after the first window and renderer exist.
    fn init(&mut self, _cx: &mut UiContext) {}

    /// Build the element tree for the next frame of the context's window
    /// ([`ViewContext::window_handle`]).
    fn view(&mut self, cx: &mut ViewContext) -> AnyElement;

    /// Handle an action emitted by a click, scroll, drag, key binding, or
    /// assistive tech, in the context's window.
    fn update(&mut self, action: Self::Action, cx: &mut UiContext);

    /// A value sent through a [`UiSender`], delivered on the UI thread in
    /// send order, once whatever windows are open. Redraw with
    /// `cx.window.request_redraw()` if it changed what the view shows.
    fn message(&mut self, _message: Self::Message, _cx: &mut UiContext) {}

    /// Events about the app and its windows: theme changes, URLs from later
    /// launches, notification and tray clicks, opened and closed windows
    /// and dialogs. The adapter has already applied a
    /// [`AppEvent::ThemeChanged`] to its theme when this runs.
    fn app_event(&mut self, _event: AppEvent, _cx: &mut UiContext) {}

    /// `window` opened, before its first frame; the context is bound to
    /// it. The first window gets this right after [`Self::init`]. Called
    /// before [`Self::app_event`] gets the [`AppEvent::WindowOpened`].
    fn window_opened(&mut self, _window: WindowHandle, _cx: &mut UiContext) {}

    /// `window` closed for `reason`, or never opened
    /// ([`CloseReason::OpenFailed`]); every window gets this once. The
    /// context is bound to another window, if any is open, and the closed
    /// window's focus and geometry still answer through
    /// [`UiContext::focus_in`] and [`UiContext::geometry_in`]. Its IME
    /// composition and a drag session started in it have already ended.
    /// Called before [`Self::app_event`] gets the
    /// [`AppEvent::WindowClosed`].
    fn window_closed(&mut self, _window: WindowHandle, _reason: CloseReason, _cx: &mut UiContext) {}

    /// The user asked to close the window. Return false to keep it open.
    fn close_requested(&mut self, _cx: &mut UiContext) -> bool {
        true
    }

    /// The context's window is about to close for `reason`
    /// ([`App::close_requested`](crate::App::close_requested)):
    /// [`CloseReason::User`] for its close button, [`CloseReason::Quit`]
    /// when quitting asks every window. Return false to keep it open.
    /// Defaults to [`Self::close_requested`].
    fn window_close_requested(&mut self, _reason: CloseReason, cx: &mut UiContext) -> bool {
        self.close_requested(cx)
    }

    /// Assistive tech set the value of the text field `target` in the
    /// context's window.
    fn set_text_value(&mut self, _target: FocusId, _value: String, _cx: &mut UiContext) {}

    /// Apply `command` to the model behind the text field `target` and
    /// return what it did; `TextField::apply` and `Editor::apply` do both.
    /// The adapter writes the outcome's `clipboard_write` through
    /// [`UiApp::write_clipboard`] and redraws when the text or selection
    /// changed. Pointer selection steps may arrive while a frame is built,
    /// so this gets no context. An app with fields in several windows
    /// implements [`Self::edit_text_in`] instead.
    fn edit_text(&mut self, _target: FocusId, _command: TextEditCommand) -> TextEditOutcome {
        TextEditOutcome::default()
    }

    /// [`Self::edit_text`] for the field `target` in `window`, which the
    /// adapter calls. Defaults to [`Self::edit_text`].
    fn edit_text_in(
        &mut self,
        _window: WindowHandle,
        target: FocusId,
        command: TextEditCommand,
    ) -> TextEditOutcome {
        self.edit_text(target, command)
    }

    /// The IME composition in the text field `target` changed
    /// (`TextField::set_preedit`). A composition that ends without a commit
    /// arrives as [`TextEditCommand::CancelPreedit`] instead.
    fn set_preedit(&mut self, _target: FocusId, _text: String, _cursor: Option<(usize, usize)>) {}

    /// [`Self::set_preedit`] for the field `target` in `window`, which the
    /// adapter calls. Defaults to [`Self::set_preedit`].
    fn set_preedit_in(
        &mut self,
        _window: WindowHandle,
        target: FocusId,
        text: String,
        cursor: Option<(usize, usize)>,
    ) {
        self.set_preedit(target, text, cursor);
    }

    /// Text to paste into a text field. Defaults to the system clipboard.
    fn read_clipboard(&mut self, cx: &mut UiContext) -> Option<String> {
        cx.window.clipboard_text()
    }

    /// Text a copy or cut in a text field produced. Defaults to the system
    /// clipboard.
    fn write_clipboard(&mut self, text: String, cx: &mut UiContext) {
        cx.window.set_clipboard_text(&text);
    }

    /// Sees every input event first. Return `true` to stop the adapter's own
    /// handling (clicks, wheel, keys, text editing, Tab focus traversal).
    /// A transport moving the drag session ([`UiContext::update_drag`])
    /// hooks pointer input here.
    /// Elements that take typed text without a text field, like a
    /// terminal, read it here.
    fn event(&mut self, _event: &InputEvent, _cx: &mut UiContext) -> bool {
        false
    }

    /// Called after any [`crate::Waker::wake`], once pending messages have
    /// been delivered.
    fn wake(&mut self, _cx: &mut UiContext) {}

    /// The adapter ended the drag session by itself, cancelled: Escape, its
    /// source window closed, a release [`UiContext::finish_drag`] was not
    /// called for, [`UiContext::cancel_drag`], or a new session replacing
    /// it. `end` carries the payload back; the handler's actions were
    /// already delivered. Sessions the app ends with
    /// [`UiContext::finish_drag`] or [`UiContext::cancel_drag_session`]
    /// return their end there instead.
    fn drag_session_ended(&mut self, _end: DragEnd, _cx: &mut UiContext) {}
}

/// Sends [`UiApp::Message`]s to the UI thread from any thread. Clone it
/// into each worker. Messages sent before the window opens are delivered
/// right after [`UiApp::init`].
pub struct UiSender<M> {
    pub(super) sender: Sender<M>,
    pub(super) waker: Arc<OnceLock<Waker>>,
}

impl<M> Clone for UiSender<M> {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
            waker: Arc::clone(&self.waker),
        }
    }
}

impl<M: Send> UiSender<M> {
    /// Queue `message` for [`UiApp::message`] and wake the UI thread.
    /// Returns false once the app is gone.
    pub fn send(&self, message: M) -> bool {
        if self.sender.send(message).is_err() {
            return false;
        }
        if let Some(waker) = self.waker.get() {
            waker.wake();
        }
        true
    }
}
