//! Quark Workbench: a deterministic coding-agent client built from quark's
//! document, composer, dock, and window primitives. See README.md.
//!
//! The binary (`src/main.rs`) and the tests share these entry points:
//! [`Workbench::new`] builds the app for some [`Options`], [`adapter`]
//! wraps it with the workbench theme and key bindings, and
//! [`window_options`] sizes the main window.

pub mod app;
pub mod assets;
pub mod composer;
pub mod contracts;
pub mod design;
pub mod dock;
pub mod fixtures;
pub mod model;
pub mod options;
pub mod overlays;
pub mod perf;
pub mod scenario;
pub mod settings;
pub mod shell;
pub mod timeline;

use quark_app::quark_ui::key_context::KeyBindings;
use quark_app::{TrafficLights, UiAdapter, WindowChrome, WindowOptions};

pub use app::{Message, Workbench};
pub use contracts::Options;

pub const APP_TITLE: &str = "Quark Workbench";
/// Initial content size and minimum size (design section 3).
pub const INITIAL_SIZE: (f64, f64) = (1440.0, 900.0);
pub const MIN_SIZE: (f64, f64) = (960.0, 640.0);

/// Every command's default binding, resolved to `Msg::Command`.
pub fn key_bindings() -> KeyBindings {
    let mut bindings = KeyBindings::new();
    for spec in contracts::COMMANDS {
        if let Some(keys) = spec.binding {
            bindings
                .bind(keys, None, spec.id)
                .unwrap_or_else(|e| panic!("binding {keys:?} of {}: {e:?}", spec.name));
        }
    }
    bindings
}

/// `app` in an adapter with the workbench themes and key bindings.
pub fn adapter(app: Workbench) -> UiAdapter<Workbench> {
    let (light, dark) = design::themes_for(app.options.theme);
    UiAdapter::new(app, APP_TITLE)
        .with_themes(light, dark)
        .with_key_bindings(key_bindings())
}

/// The main window: custom chrome with native traffic lights on macOS,
/// system decorations elsewhere (design section 6).
pub fn window_options(app: &Workbench) -> WindowOptions {
    let macos = cfg!(target_os = "macos");
    let options = WindowOptions {
        title: APP_TITLE.into(),
        size: INITIAL_SIZE,
        min_size: Some(MIN_SIZE),
        chrome: if macos {
            WindowChrome::Custom
        } else {
            WindowChrome::System
        },
        traffic_lights: macos.then_some(TrafficLights {
            left_margin: 16.0,
            center_y: design::tokens::TOP_BAR / 2.0,
        }),
        ..WindowOptions::default()
    };
    app.dock.windows.main_window_options(options)
}
