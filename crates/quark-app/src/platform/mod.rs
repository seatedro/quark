//! Operating system integration beyond the window: notifications, single
//! instance handoff, deep links, and window state persistence.

pub mod desktop_entry;
#[cfg(feature = "notifications")]
pub mod notification;
pub mod single_instance;
