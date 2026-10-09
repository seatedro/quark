//! Operating system integration beyond the window: menus, notifications,
//! badges, single instance handoff, deep links, dragging files out, docking
//! drags across windows, window state persistence, crash reports, native
//! window materials and app-drawn chrome, and the opt-in launch at login,
//! global shortcuts, and telemetry.

#[cfg(feature = "autostart")]
pub mod autostart;
pub mod badge;
pub mod chrome;
pub mod crash;
pub mod deep_link;
pub mod desktop_entry;
#[cfg(feature = "dialogs")]
pub mod dialog;
#[cfg(feature = "ui")]
pub mod dock_drag;
pub mod drag_out;
#[cfg(feature = "components")]
pub mod drawn_menu;
#[cfg(feature = "global-shortcut")]
pub mod global_shortcut;
pub mod material;
#[cfg(feature = "ui")]
pub mod menu;
#[cfg(any(target_os = "macos", windows))]
pub(crate) mod native_menu;
#[cfg(feature = "notifications")]
pub mod notification;
pub mod placement;
pub mod single_instance;
#[cfg(feature = "telemetry")]
pub mod telemetry;
#[cfg(target_os = "linux")]
pub(crate) mod theme;
#[cfg(feature = "tray")]
pub mod tray;
pub mod window_state;
