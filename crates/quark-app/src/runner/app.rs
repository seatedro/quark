use super::*;
use crate::platform::material::{WindowBackground, WindowCorners};

/// An application driven by [`run`]. Scene, size, and pointer coordinates
/// are logical points; the runner scales each window's scene to physical
/// pixels with that window's [`FrameContext::scale_factor`]. Shape text with
/// [`FrameContext::layout_text`] so glyphs are rasterized at physical size.
///
/// Every window shares the one app. Each context names the window it is for
/// ([`FrameContext::window_handle`], [`EventContext::window_handle`]); an app
/// with one window can ignore it.
pub trait App: 'static {
    /// Called once after the first window and its renderer exist.
    fn init(&mut self, _cx: &mut EventContext) {}

    /// Build the scene for the next frame of the context's window.
    fn frame(&mut self, cx: &mut FrameContext) -> Scene;

    /// The runner hands each scene back once it is rendered, so the app can
    /// build the next one in the same buffer instead of allocating it.
    /// `window` is the window the scene was built for.
    fn recycle_scene(&mut self, _window: WindowHandle, _scene: Scene) {}

    fn event(&mut self, _event: InputEvent, _cx: &mut EventContext) {}

    /// Called after any [`Waker::wake`]. Wakes coalesce and may be spurious.
    fn wake(&mut self, _cx: &mut EventContext) {}

    /// Events about the app and its windows; see [`AppEvent`]. The context is
    /// bound to the window the event concerns, or else to the focused window.
    fn app_event(&mut self, _event: AppEvent, _cx: &mut EventContext) {}

    /// The context's window is about to close for `reason`:
    /// [`CloseReason::User`] when the user closed it, [`CloseReason::Quit`]
    /// when the Quit menu item asks every window. Return false to keep it
    /// open (for example, to ask about unsaved changes first). Closes from
    /// [`EventContext::close_window`] do not ask.
    fn close_requested(&mut self, _reason: CloseReason, _cx: &mut EventContext) -> bool {
        true
    }

    /// The accessibility tree to publish for `window`, if it changed.
    /// Called after [`App::frame`] for that window, while assistive tech
    /// listens to it.
    fn accessibility(&mut self, _window: WindowHandle) -> Option<TreeUpdate> {
        None
    }

    fn accessibility_action(&mut self, _request: ActionRequest, _cx: &mut EventContext) {}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WindowChrome {
    /// Native title bar and borders.
    #[default]
    System,
    /// The app draws its own title bar: a transparent full-size title bar on
    /// macOS (traffic lights stay), no decorations elsewhere.
    Custom,
}

/// Where macOS traffic lights sit, in logical points from the window's
/// top-left. Reapplied on resize and title changes, which reset them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrafficLights {
    pub left_margin: f32,
    pub center_y: f32,
}

#[derive(Debug, Clone)]
pub struct WindowOptions {
    pub title: String,
    /// Initial inner size in logical points.
    pub size: (f64, f64),
    pub min_size: Option<(f64, f64)>,
    pub chrome: WindowChrome,
    pub icon: Option<Icon>,
    pub fonts: FontSettings,
    pub traffic_lights: Option<TrafficLights>,
    /// Log panics (message, location, backtrace) through `tracing` and to a
    /// crash log in the platform state directory, then run the previous
    /// hook. Installed once per process by [`run`].
    pub panic_hook: bool,
    /// Save this window's size and position under this key when it closes
    /// and restore them when a window with the same key opens. See
    /// [`crate::platform::window_state`].
    pub persist_key: Option<String>,
    /// Initial outer position in desktop units (see [`DesktopPoint`]), as
    /// [`WindowPlacement::outer_position`] reports it. Ignored where windows
    /// cannot be positioned ([`PlatformCapabilities::window_positions`]),
    /// and when `persist_key` restores a saved position.
    pub position: Option<DesktopPoint>,
    /// Take keyboard focus when the window opens. Turn off for a window
    /// that appears under the pointer mid-drag and must not take focus from
    /// the window the drag started in; platforms that cannot open windows
    /// unfocused ignore it.
    pub active: bool,
    /// Open maximized. A `persist_key` window restores its saved state
    /// instead.
    pub maximized: bool,
    /// What shows behind the app's paint: an opaque color, a transparent
    /// surface, or a native material. Resolved against the platform; see
    /// [`crate::platform::material`] and [`EventContext::window_surface`].
    pub background: WindowBackground,
    /// The window's corner shape, as far as the platform allows.
    pub corners: WindowCorners,
}

impl Default for WindowOptions {
    fn default() -> Self {
        Self {
            title: "Quark".to_owned(),
            size: (1024.0, 768.0),
            min_size: None,
            chrome: WindowChrome::System,
            icon: None,
            fonts: FontSettings::default(),
            traffic_lights: None,
            panic_hook: true,
            persist_key: None,
            position: None,
            active: true,
            maximized: false,
            background: WindowBackground::default(),
            corners: WindowCorners::default(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error("could not start the event loop: {0}")]
    EventLoop(#[from] EventLoopError),
    #[error("could not create the native window: {0}")]
    Window(#[from] OsError),
    #[error("could not initialize the GPU renderer: {0}")]
    Renderer(#[from] RenderError),
}
