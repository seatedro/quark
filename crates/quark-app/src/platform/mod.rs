//! Operating system integration beyond the window: menus, notifications,
//! badges, single instance handoff, deep links, dragging files out, and
//! window state persistence.

pub mod badge;
pub mod deep_link;
pub mod desktop_entry;
#[cfg(feature = "dialogs")]
pub mod dialog;
pub mod drag_out;
#[cfg(feature = "ui")]
pub mod menu;
#[cfg(any(target_os = "macos", windows))]
pub(crate) mod native_menu;
#[cfg(feature = "notifications")]
pub mod notification;
pub mod single_instance;
#[cfg(target_os = "linux")]
pub(crate) mod theme;
#[cfg(feature = "tray")]
pub mod tray;
pub mod window_state;
