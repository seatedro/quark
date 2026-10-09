//! App-drawn window chrome: moving a window from its own title bar
//! ([`EventContext::start_window_drag`](crate::EventContext::start_window_drag))
//! and the platform's title bar double-click
//! ([`EventContext::title_double_click`](crate::EventContext::title_double_click)).
//!
//! A drag hands the move to the window manager, so snapping, edge tiling,
//! and moving between displays behave as they do from a native title bar.
//! It must start while the button press that began it is being handled:
//! Wayland proves the press with its serial, and macOS reads the current
//! event.

/// Why a window drag did not start.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WindowDragError {
    /// The window is not open, or the handle is stale.
    #[error("the window is not open")]
    NotOpen,
    /// The platform cannot move windows for the app.
    #[error("the platform cannot start a window drag")]
    Unsupported,
    /// The window manager refused, for example because no button is held.
    #[error("the window drag did not start: {0}")]
    Platform(String),
}

/// What a double-click on a title bar did, following the platform and the
/// user's setting for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitleAction {
    /// Maximized (zoomed, on macOS) or restored the window.
    ToggleMaximize,
    Minimize,
    /// The user's setting is to do nothing, or the window is not open.
    Nothing,
}
