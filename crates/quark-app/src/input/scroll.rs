use winit::event::MouseScrollDelta;

/// Convert a wheel delta to pixels, positive meaning content moves up (scroll
/// down). Line deltas scale by the target's line height and notch setting.
pub fn scroll_delta_to_px(delta: MouseScrollDelta, line_step_px: f32, lines_per_notch: f32) -> f32 {
    match delta {
        MouseScrollDelta::LineDelta(_, y) => -y * line_step_px * lines_per_notch,
        MouseScrollDelta::PixelDelta(position) => -(position.y as f32),
    }
}

#[cfg(test)]
mod tests {
    use winit::dpi::PhysicalPosition;

    use super::*;

    #[test]
    fn scroll_delta_to_px_preserves_magnitude_and_direction() {
        let line_delta = scroll_delta_to_px(MouseScrollDelta::LineDelta(0.0, 1.5), 20.0, 1.0);
        assert_eq!(line_delta, -30.0);

        let pixel_delta = scroll_delta_to_px(
            MouseScrollDelta::PixelDelta(PhysicalPosition::new(0.0, -12.5)),
            20.0,
            1.0,
        );
        assert_eq!(pixel_delta, 12.5);
    }
}
