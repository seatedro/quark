//! Operating system integration beyond the window: notifications, single
//! instance handoff, deep links, and window state persistence.

pub mod desktop_entry;
#[cfg(feature = "dialogs")]
pub mod dialog;
#[cfg(feature = "notifications")]
pub mod notification;
pub mod single_instance;
#[cfg(target_os = "linux")]
pub(crate) mod theme;
#[cfg(feature = "tray")]
pub mod tray;
pub mod window_state;
