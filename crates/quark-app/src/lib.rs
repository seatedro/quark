//! Window, event loop, input normalization, and hot reload for Quark apps.

#[cfg(feature = "hot-reload")]
pub mod hot_reload;
pub mod input;
pub mod keymap;
#[cfg(target_os = "macos")]
pub mod macos_window;
mod panic_hook;
pub mod platform;
mod runner;
#[cfg(feature = "ui")]
pub mod ui;

pub use input::{InputEvent, InputNormalizer, KeyChord, KeyKind};
#[cfg(feature = "ui")]
pub use quark_ui;
#[cfg(feature = "clipboard-image")]
pub use runner::ClipboardImage;
pub use runner::{
    App, AppEvent, AppText, EventContext, FrameContext, RunError, TrafficLights, Waker,
    WindowChrome, WindowHandle, WindowOptions, run, scene_to_physical,
};
#[cfg(feature = "ui")]
pub use ui::{UiAdapter, UiApp, UiContext, ViewContext, run_ui};
pub use winit;
