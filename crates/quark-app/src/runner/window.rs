use super::*;

/// Everything the runner owns for one native window. The runner holds a single
/// one today; a multi-window runner would keep a table of these by
/// [`WindowId`].
pub(super) struct WindowState {
    pub(super) window: Arc<Window>,
    pub(super) renderer: Renderer,
    pub(super) accessibility: AccessibilityAdapter,
    pub(super) scale_factor: f64,
    pub(super) surface_size: PhysicalSize<u32>,
    // Kept per window so a multi-window runner can open windows with
    // different chrome; nothing reads it while there is one window.
    #[allow(dead_code)]
    pub(super) chrome: WindowChrome,
    pub(super) traffic_lights: Option<TrafficLights>,
}

impl WindowState {
    pub(super) fn id(&self) -> WindowId {
        self.window.id()
    }

    pub(super) fn sync_metrics(&mut self, size: PhysicalSize<u32>, scale_factor: f64) {
        self.surface_size = size;
        self.scale_factor = scale_factor;
        self.renderer.resize(size.width, size.height, scale_factor);
        position_traffic_lights(&self.window, self.traffic_lights);
    }
}

#[cfg(target_os = "macos")]
pub(super) fn custom_chrome(attrs: WindowAttributes) -> WindowAttributes {
    use winit::platform::macos::WindowAttributesExtMacOS;
    attrs
        .with_titlebar_transparent(true)
        .with_fullsize_content_view(true)
        .with_title_hidden(true)
        .with_movable_by_window_background(false)
}

#[cfg(not(target_os = "macos"))]
pub(super) fn custom_chrome(attrs: WindowAttributes) -> WindowAttributes {
    attrs.with_decorations(false)
}

#[cfg(target_os = "macos")]
pub(super) fn position_traffic_lights(window: &Window, lights: Option<TrafficLights>) {
    if let Some(lights) = lights {
        crate::macos_window::position_traffic_lights(window, lights.left_margin, lights.center_y);
    }
}

#[cfg(not(target_os = "macos"))]
pub(super) fn position_traffic_lights(_window: &Window, _lights: Option<TrafficLights>) {}
