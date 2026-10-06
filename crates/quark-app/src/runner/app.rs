use super::*;

/// An application driven by [`run`]. Scene and pointer coordinates are
/// physical pixels; use [`FrameContext::scale_factor`] to size content.
pub trait App: 'static {
    /// Called once after the window and renderer exist.
    fn init(&mut self, _cx: &mut EventContext) {}

    /// Build the scene for the next frame.
    fn frame(&mut self, cx: &mut FrameContext) -> Scene;

    fn event(&mut self, _event: InputEvent, _cx: &mut EventContext) {}

    /// Called after any [`Waker::wake`]. Wakes coalesce and may be spurious.
    fn wake(&mut self, _cx: &mut EventContext) {}

    /// The accessibility tree to publish after each frame, if it changed.
    fn accessibility(&mut self) -> Option<TreeUpdate> {
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
