//! Window, event loop, input routing, and hot reload for Quark apps.
//!
//! `app.rs` and `input/{mod,pointer,keyboard,scroll}.rs` are carried over from
//! diffy with history and are not yet in the module tree: they still reference
//! diffy's state and actions. They join as the generic app runner lands.

#[cfg(feature = "hot-reload")]
pub mod hot_reload;
pub mod keymap;
#[cfg(target_os = "macos")]
pub mod macos_window;
