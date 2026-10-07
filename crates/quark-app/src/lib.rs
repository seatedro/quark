//! Window, event loop, input normalization, and hot reload for Quark apps.
//!
//! [`run`] drives an [`App`] that returns a `quark::Scene` per frame. Most
//! apps implement [`UiApp`] instead and start it with [`run_ui`]: the
//! adapter calls [`UiApp::view`] for a `quark_ui` element tree, then does
//! layout, paint, hit testing, focus, text editing, and accessibility
//! publishing, and hands each click or key action back to
//! [`UiApp::update`]. Threads send values back with a [`UiSender`].
//!
//! - [`platform`]: menus, notifications, badges, tray, file dialogs, single
//!   instance handoff, deep links, and window state persistence.
//! - [`keymap`]: key binding tables with user overrides.
//! - `testing` (feature `test-support`): `testing::UiTestHarness` runs a
//!   `UiApp` headlessly with a fake clock and clipboard.
//!
//! A complete app:
//!
//! ```no_run
//! use quark_app::quark_ui::accessibility::AccessibilityRole;
//! use quark_app::quark_ui::element::{AnyElement, IntoAnyElement, div, text};
//! use quark_app::quark_ui::style::Styled;
//! use quark_app::quark_ui::Action;
//! use quark_app::{UiApp, UiContext, ViewContext, WindowOptions};
//!
//! #[derive(Debug, Clone, PartialEq)]
//! enum Msg {
//!     Increment,
//! }
//!
//! impl From<Msg> for Action {
//!     fn from(msg: Msg) -> Self {
//!         Action::new(msg)
//!     }
//! }
//!
//! struct Counter {
//!     count: u32,
//! }
//!
//! impl UiApp for Counter {
//!     type Action = Msg;
//!     type Message = ();
//!
//!     fn view(&mut self, cx: &mut ViewContext) -> AnyElement {
//!         let colors = &cx.theme.colors;
//!         let (width, height) = cx.frame.size();
//!         div()
//!             .w(width)
//!             .h(height)
//!             .items_center()
//!             .justify_center()
//!             .gap(12.0)
//!             .bg(colors.background)
//!             .child(text(format!("Clicked {} times", self.count)).color(colors.text))
//!             .child(
//!                 div()
//!                     .accessibility_role(AccessibilityRole::Button)
//!                     .accessibility_label("Increment")
//!                     .test_id("counter.increment")
//!                     .on_click(Msg::Increment)
//!                     .px(16.0)
//!                     .h(36.0)
//!                     .items_center()
//!                     .rounded(8.0)
//!                     .bg(colors.accent)
//!                     .hover_bg(colors.accent_strong)
//!                     .child(text("Increment").color(colors.text_strong)),
//!             )
//!             .into_any()
//!     }
//!
//!     fn update(&mut self, msg: Msg, _cx: &mut UiContext) {
//!         match msg {
//!             Msg::Increment => self.count += 1,
//!         }
//!     }
//! }
//!
//! fn main() -> Result<(), quark_app::RunError> {
//!     quark_app::run_ui(
//!         Counter { count: 0 },
//!         WindowOptions {
//!             title: "Counter".into(),
//!             size: (480.0, 320.0),
//!             ..WindowOptions::default()
//!         },
//!     )
//! }
//! ```
//!
//! Work off the UI thread reports back through a [`UiSender`]; messages
//! reach [`UiApp::message`] on the UI thread in send order. The test drives
//! the app headlessly the way a user would:
//!
//! ```
//! use quark_app::quark_ui::accessibility::AccessibilityRole;
//! use quark_app::quark_ui::element::{AnyElement, IntoAnyElement, div, text};
//! use quark_app::quark_ui::style::Styled;
//! use quark_app::quark_ui::Action;
//! use quark_app::testing::{By, UiTestHarness};
//! use quark_app::{UiApp, UiContext, ViewContext};
//!
//! #[derive(Debug, Clone, PartialEq)]
//! struct Refresh;
//!
//! impl From<Refresh> for Action {
//!     fn from(msg: Refresh) -> Self {
//!         Action::new(msg)
//!     }
//! }
//!
//! /// A finished background job.
//! struct Loaded(String);
//!
//! #[derive(Default)]
//! struct Status {
//!     text: String,
//! }
//!
//! impl UiApp for Status {
//!     type Action = Refresh;
//!     type Message = Loaded;
//!
//!     fn view(&mut self, _cx: &mut ViewContext) -> AnyElement {
//!         div()
//!             .flex_col()
//!             .child(text(self.text.clone()))
//!             .child(
//!                 div()
//!                     .accessibility_role(AccessibilityRole::Button)
//!                     .accessibility_label("Refresh")
//!                     .on_click(Refresh)
//!                     .child(text("Refresh")),
//!             )
//!             .into_any()
//!     }
//!
//!     fn update(&mut self, _: Refresh, cx: &mut UiContext) {
//!         self.text = "Loading".into();
//!         let sender = cx.sender::<Loaded>();
//!         std::thread::spawn(move || {
//!             sender.send(Loaded("Loaded 3 items".into()));
//!         });
//!     }
//!
//!     fn message(&mut self, Loaded(text): Loaded, cx: &mut UiContext) {
//!         self.text = text;
//!         cx.window.request_redraw();
//!     }
//! }
//!
//! let mut ui = UiTestHarness::new(Status::default(), (320.0, 200.0), 1.0);
//! ui.click_node(By::role_name(AccessibilityRole::Button, "Refresh"));
//! // The worker thread may not have sent yet; a sender from the harness
//! // delivers on this thread, deterministically.
//! ui.send_message(Loaded("Loaded 3 items".into()));
//! assert!(ui.painted_text().lines().any(|line| line == "Loaded 3 items"));
//! ```
//!
//! Platform services are calls on the window context, and their results come
//! back as [`AppEvent`]s:
//!
//! ```no_run
//! use quark_app::platform::menu::{Menu, MenuAction};
//! use quark_app::quark_ui::element::{AnyElement, IntoAnyElement, div};
//! use quark_app::{AppEvent, UiApp, UiContext, ViewContext};
//!
//! struct Notes {
//!     unread: u32,
//! }
//!
//! impl UiApp for Notes {
//!     type Action = ();
//!     type Message = ();
//!
//!     fn init(&mut self, cx: &mut UiContext) {
//!         // The first menu is the macOS application menu. Linux shows no
//!         // native menu bar; the shortcut still reaches the app as a key.
//!         cx.window.set_menus(vec![
//!             Menu::app("Notes"),
//!             Menu::new("File", vec![MenuAction::new("new", "New Note").shortcut("mod+n").into()]),
//!             Menu::edit(),
//!         ]);
//!         cx.window.set_badge(Some(self.unread));
//!     }
//!
//!     fn app_event(&mut self, event: AppEvent, cx: &mut UiContext) {
//!         if let AppEvent::Menu(id) = event {
//!             if id == "new" {
//!                 self.unread = 0;
//!                 cx.window.set_badge(None);
//!             }
//!         }
//!     }
//!
//!     fn view(&mut self, _cx: &mut ViewContext) -> AnyElement {
//!         div().into_any()
//!     }
//!
//!     fn update(&mut self, _: (), _cx: &mut UiContext) {}
//! }
//! ```

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

#[cfg(feature = "hot-reload")]
pub mod hot_reload;
pub mod input;
// Bindings are quark-ui's `Binding`, which the router matches too.
#[cfg(all(test, feature = "test-support"))]
mod frame_budget;
#[cfg(feature = "ui")]
pub mod keymap;
#[cfg(target_os = "macos")]
pub mod macos_window;
mod panic_hook;
pub mod platform;
#[cfg(any(feature = "profile-puffin", feature = "profile-tracy"))]
mod profile;
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
