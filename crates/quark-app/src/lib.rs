//! Window, event loop, input normalization, and hot reload for Quark apps.
//!
//! `input/pointer.rs` and `input/keyboard.rs` are the source app's widget routing,
//! carried over with history and kept out of the module tree until a generic
//! router replaces them.

#[cfg(feature = "hot-reload")]
pub mod hot_reload;
pub mod input;
pub mod keymap;
#[cfg(target_os = "macos")]
pub mod macos_window;
mod runner;

pub use input::{InputEvent, InputNormalizer, KeyChord, KeyKind};
pub use runner::{
    App, EventContext, FrameContext, RunError, TrafficLights, Waker, WindowChrome, WindowOptions,
    run,
};
pub use winit;
