//! Glue between [`crate::UiAdapter`] and quark-ui's devtools: input
//! mapping, the HUD sample, and the overlay pass.

use std::hash::{Hash, Hasher};
use std::time::Instant;

use quark::reactive::SignalStore;
use quark::scene::Scene;
use quark_ui::element::InputRouter;
use quark_ui::hud::HudSample;
use quark_ui::inspector::{Devtools, DevtoolsInput, OverlayContext, PhaseTimings};
use quark_ui::theme::Theme;

use crate::input::{PointerButton, UiInput};
use crate::{EventContext, FrameContext};

/// Offer `input` to devtools first. Returns `true` when the app must not
/// see it.
pub(crate) fn intercept(devtools: &mut Devtools, input: &UiInput, cx: &mut EventContext) -> bool {
    let binding;
    let input = match input {
        UiInput::PointerMove { x, y } => DevtoolsInput::PointerMoved { x: *x, y: *y },
        UiInput::PointerLeave => DevtoolsInput::PointerLeft,
        UiInput::PointerDown(PointerButton::Primary) => DevtoolsInput::PointerDown,
        UiInput::PointerUp(PointerButton::Primary) => DevtoolsInput::PointerUp,
        UiInput::Wheel { .. } => DevtoolsInput::Wheel,
        UiInput::Key(key) => {
            binding = key.to_string();
            DevtoolsInput::Key(&binding)
        }
        _ => return false,
    };
    let response = devtools.handle(input);
    if response.redraw {
        cx.request_redraw();
    }
    response.consumed
}

/// What the adapter knows about the frame it just painted.
pub(crate) struct PaintedFrame<'a> {
    pub router: &'a InputRouter,
    pub theme: &'a Theme,
    pub signals: &'a SignalStore,
    pub build_us: u64,
    pub phases: PhaseTimings,
}

/// Record the frame in the HUD and draw the overlay over `scene`.
pub(crate) fn finish_frame(
    devtools: &mut Devtools,
    mut scene: Scene,
    cx: &mut FrameContext,
    frame: PaintedFrame,
) -> Scene {
    let window = window_key(cx);
    let render = cx.last_render_stats();
    let (width, height) = cx.size();
    let scale_factor = cx.scale_factor();
    let text = cx.text();
    let sample = HudSample {
        build_us: frame.build_us,
        layout_us: frame.phases.layout_us,
        paint_us: frame.phases.paint_us,
        render_cpu_us: render.cpu_us,
        acquire_us: render.acquire_us,
        present_us: render.present_us,
        primitive_count: scene.len(),
        text_entries: text.layouts.len(),
        ..HudSample::default()
    };
    devtools.record_frame(window, Instant::now(), sample, text.layouts.stats());
    let input = frame.router.frame();
    devtools.paint_overlay(
        &mut scene,
        OverlayContext {
            theme: frame.theme,
            scale_factor,
            text: &mut text.system,
            layouts: &mut text.layouts,
            signals: frame.signals,
            width,
            height,
            semantic: &input.semantic,
            hits: &input.hits,
            window,
        },
    );
    devtools.sync_text_stats(text.layouts.stats());
    scene
}

fn window_key(cx: &FrameContext) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    cx.window_handle().hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use quark::Color;
    use quark_render::{RenderError, Renderer};
    use quark_text::{LayoutCache, TextSystem};
    use quark_ui::element::{ElementContext, IntoAnyElement, div, render_element};
    use quark_ui::style::Styled;

    use super::*;

    const HIGHLIGHT: [u8; 3] = [64, 156, 255];
    const BACKGROUND: Color = Color::rgba(20, 20, 20, 255);

    fn is_highlight(pixel: [u8; 4]) -> bool {
        pixel[..3]
            .iter()
            .zip(HIGHLIGHT)
            .all(|(got, want)| got.abs_diff(want) <= 12)
    }

    // Regression: the hover highlight was drawn somewhere other than the
    // hovered element's bounds (or not at all).
    #[test]
    fn hover_highlight_outlines_the_hovered_element() {
        let (width, height) = (600u32, 300u32);
        let mut renderer = match Renderer::new_headless(width, height, 1.0) {
            Ok(renderer) => renderer,
            Err(RenderError::NoAdapter) => {
                assert!(
                    std::env::var_os("QUARK_REQUIRE_GPU").is_none(),
                    "QUARK_REQUIRE_GPU is set but no wgpu adapter is available"
                );
                eprintln!("skipping: no wgpu adapter available");
                return;
            }
            Err(error) => panic!("headless renderer failed: {error}"),
        };
        let mut text = TextSystem::vendored_only(&Default::default());
        let mut layouts = LayoutCache::default();
        let theme = Theme::default_dark();
        let signals = SignalStore::new();
        let mut devtools = Devtools::default();
        devtools.inspector = true;
        // The box spans x 40..140, y 60..110.
        devtools.handle(DevtoolsInput::PointerMoved { x: 90.0, y: 85.0 });

        let mut root = div()
            .w(width as f32)
            .h(height as f32)
            .bg(BACKGROUND)
            .child(
                div()
                    .absolute()
                    .left(40.0)
                    .top(60.0)
                    .w(100.0)
                    .h(50.0)
                    .bg(Color::rgba(90, 90, 90, 255)),
            )
            .into_any();
        let mut scene = Scene::default();
        let mut ecx = ElementContext::new(&theme, 1.0, &mut text, &mut layouts, None, &signals);
        devtools.begin_frame(&mut ecx.devtools);
        render_element(&mut root, &mut scene, &mut ecx, width as f32, height as f32);
        devtools.end_frame(&mut ecx.devtools);
        let mut router = InputRouter::default();
        router.set_frame(ecx.take_input_frame());
        let input = router.frame();
        devtools.paint_overlay(
            &mut scene,
            OverlayContext {
                theme: &theme,
                scale_factor: 1.0,
                text: &mut text,
                layouts: &mut layouts,
                signals: &signals,
                width: width as f32,
                height: height as f32,
                semantic: &input.semantic,
                hits: &input.hits,
                window: 0,
            },
        );

        let rgba = renderer
            .render_to_rgba(&scene, &mut text, width, height)
            .expect("offscreen render");
        let pixel = |x: u32, y: u32| {
            let i = ((y * width + x) * 4) as usize;
            [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
        };
        // On each edge of the box, then just outside it.
        for (x, y) in [(40, 85), (139, 85), (90, 60), (90, 109)] {
            assert!(
                is_highlight(pixel(x, y)),
                "edge ({x}, {y}): {:?}",
                pixel(x, y)
            );
        }
        for (x, y) in [(36, 85), (143, 85), (90, 113)] {
            assert!(
                !is_highlight(pixel(x, y)),
                "outside ({x}, {y}): {:?}",
                pixel(x, y)
            );
        }
    }
}
