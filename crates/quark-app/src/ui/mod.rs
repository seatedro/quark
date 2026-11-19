//! Lite adapter from quark-ui elements to the runner.
//!
//! Implement [`UiApp`] to describe each window as an element tree each
//! frame and handle your action type; [`run_ui`] (or [`UiAdapter`] with
//! [`run`](crate::run)) does layout, paint, hit testing, focus, text
//! editing, and accessibility publishing.
//!
//! The element tree is laid out in logical points, like the runner's scenes
//! and pointer events; text is shaped at the window's scale factor so it
//! rasterizes at physical size. Accessibility bounds stay logical under a
//! root transform that scales them to the physical pixels accesskit expects.
//!
//! The adapter redraws only for a reason it can name: the
//! hovered elements changed, focus moved, an action was dispatched, a text
//! field changed, the theme changed, or a callback wrote a signal in the
//! app's [`SignalStore`](quark::reactive::SignalStore)
//! ([`UiContext::signals`]). Animations schedule their own frames, and an
//! app that changes state elsewhere asks with `cx.window.request_redraw()`.
//!
//! # Signals
//!
//! State kept in [`Signal`](quark::reactive::Signal)s needs neither
//! redraw requests nor inputs hashes. A write from any callback redraws
//! every window, and a [`reactive`](quark_ui::element::reactive)
//! boundary rebuilds only when a signal it read changed; the others
//! replay. Focus and the theme are signals too ([`ViewContext::window_signals`]),
//! so a boundary that paints a focus ring rebuilds only when focus moves.
//!
//! # Windows
//!
//! One app serves every window it opens ([`crate::EventContext::open_window`]).
//! [`UiApp::view`] is called per window; branch on
//! [`ViewContext::window_handle`]. Each window keeps its own input routing,
//! geometry, element handles, focus, hover, text selection, IME
//! composition, accessibility tree and announcements, and animations, so an
//! idle window stays idle while another animates. The app, its messages,
//! signals, theme, and key bindings are shared: a theme change repaints
//! every window. Refer to things in another window as
//! `(WindowHandle, FocusId)` or `(WindowHandle, ElementHandle)`; the same
//! [`FocusId`](quark_ui::FocusId) in two windows names two elements, which
//! [`UiApp::edit_text_in`] tells apart.
//!
//! An IME composition belongs to its window. Focus moving onto an element
//! of another window, or the window closing, ends it: its preedit is
//! cancelled and anything the platform still delivers for it is dropped.

use crate::{EventContext, WindowHandle};

mod accessibility;
mod adapter;
mod app;
mod context;
mod drag;
mod focus;
mod frame;
mod ime;
mod input;
mod jump;
#[cfg(test)]
mod multi_window_tests;
mod pointer;
#[cfg(test)]
mod tests;
mod theme;
mod window;

pub use adapter::{UiAdapter, run_ui};
pub use app::{UiApp, UiSender};
pub use context::{UiContext, ViewContext};
pub use drag::{drag_location, drag_window_id};
pub use pointer::HoveredTooltip;
pub(crate) use window::WindowUiState;

/// Why the adapter asked for a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Redraw {
    /// The elements under the pointer changed, so hover styles did.
    Hover,
    Focus,
    /// An action reached the app.
    Action,
    /// A text field's text, selection, or composition changed.
    TextEdit,
    /// Input moved a scroll handle.
    Scroll,
    /// The tooltip under the pointer changed.
    Tooltip,
    Theme,
    /// The accessibility tree has something new to publish.
    Announce,
    /// Jump mode started, ended, or narrowed its labels.
    Jump,
    /// A callback wrote a signal.
    Signal,
}

/// Ask for a frame of `window`, or of the context's window without one.
/// Explicit, since the context may be bound to another window than the
/// one whose state changed.
fn redraw(reason: Redraw, window: Option<WindowHandle>, cx: &mut EventContext) {
    tracing::trace!(?reason, ?window, "redraw");
    match window {
        Some(window) => cx.request_redraw_window(window),
        None => cx.request_redraw(),
    }
}

fn redraw_all(reason: Redraw, cx: &mut EventContext) {
    tracing::trace!(?reason, "redraw every window");
    cx.request_redraw_all();
}
