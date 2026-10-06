use super::*;

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
