//! Window, event loop, input normalization, and hot reload for Quark apps.

#[cfg(feature = "hot-reload")]
pub mod hot_reload;
pub mod input;
pub mod keymap;
#[cfg(target_os = "macos")]
pub mod macos_window;
mod panic_hook;
mod runner;
#[cfg(feature = "ui")]
pub mod ui;

pub use input::{InputEvent, InputNormalizer, KeyChord, KeyKind};
#[cfg(feature = "ui")]
pub use quark_ui;
pub use runner::{
    App, EventContext, FrameContext, RunError, TrafficLights, Waker, WindowChrome, WindowOptions,
    run,
};
#[cfg(feature = "ui")]
pub use ui::{UiAdapter, UiApp, UiContext, ViewContext, run_ui};
pub use winit;
