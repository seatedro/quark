//! Window, event loop, input normalization, and hot reload for Quark apps.

#[cfg(all(feature = "profile-puffin", feature = "profile-tracy"))]
compile_error!("enable only one of the profile-puffin and profile-tracy features");

/// A profiler scope plus a `tracing` span for the rest of the block, both
/// compiled only with a profiling feature.
#[allow(unused_macros)]
macro_rules! profile_scope {
    ($name:literal) => {
        #[cfg(any(feature = "profile-puffin", feature = "profile-tracy"))]
        profiling::scope!($name);
        #[cfg(any(feature = "profile-puffin", feature = "profile-tracy"))]
        let _profile_span = tracing::trace_span!($name).entered();
    };
}

#[cfg(feature = "devtools")]
mod devtools;
#[cfg(all(test, feature = "test-support"))]
mod drag_out_tests;

#[cfg(feature = "hot-reload")]
pub mod hot_reload;
pub mod input;
#[cfg(all(test, feature = "test-support"))]
mod key_context_tests;
// Bindings are quark-ui's `Binding`, which the router matches too.
#[cfg(all(test, feature = "test-support"))]
mod frame_budget;
#[cfg(feature = "ui")]
pub mod keymap;
#[cfg(all(test, feature = "test-support"))]
mod layout_tests;
#[cfg(target_os = "macos")]
pub mod macos_window;
#[cfg(all(test, feature = "test-support", feature = "components"))]
mod menu_bar_tests;
mod panic_hook;
pub mod platform;
#[cfg(any(feature = "profile-puffin", feature = "profile-tracy"))]
mod profile;
#[cfg(all(test, feature = "test-support"))]
mod reorder_tests;
mod runner;
#[cfg(all(test, feature = "test-support"))]
mod scroll_tests;
#[cfg(feature = "test-support")]
pub mod testing;
#[cfg(feature = "ui")]
pub mod ui;

pub use input::{InputEvent, InputNormalizer, KeyChord, KeyKind};
#[cfg(feature = "ui")]
pub use input::{PointerButton, UiInput};
#[cfg(feature = "ui")]
pub use quark_ui;
#[cfg(feature = "clipboard-image")]
pub use runner::ClipboardImage;
pub use runner::{
    App, AppEvent, AppText, EventContext, FrameContext, RunError, TrafficLights, Waker,
    WindowChrome, WindowHandle, WindowOptions, run, scene_to_physical,
};
#[cfg(feature = "ui")]
pub use ui::{UiAdapter, UiApp, UiContext, UiSender, ViewContext, run_ui};
pub use winit;
