//! Pixel probes of the color and blending contract: what each compositing
//! mode writes for fills, groups, images, and glyphs. Expected values are
//! fixed numbers worked out by hand from the sRGB transfer function, never
//! the renderer's own conversions.

use std::sync::Arc;

use super::*;
use crate::scene::{
    ImagePrimitive, LayerPrimitive, RectPrimitive, ShapedText, StyledTextPrimitive, TextBackdrop,
    TextFill, TextPrimitive,
};
use crate::text::test_text;
use quark::Color;
use quark_text::{TextParams, TextStyle};

const WEB: RendererOptions = RendererOptions {
    compositing: UiCompositing::WebCompatible,
    text_rendering: TextRendering::Perceptual,
};
const LINEAR: RendererOptions = RendererOptions {
    compositing: UiCompositing::Linear,
    text_rendering: TextRendering::Perceptual,
};

pub(super) fn renderer(width: u32, height: u32, options: RendererOptions) -> Option<Renderer> {
    match Renderer::new_headless(width, height, 1.0) {
        Ok(mut renderer) => {
            renderer.set_options(options);
            Some(renderer)
        }
        Err(RenderError::NoAdapter) => {
            assert!(
                std::env::var_os("QUARK_REQUIRE_GPU").is_none(),
                "QUARK_REQUIRE_GPU is set but no wgpu adapter is available"
            );
            None
        }
        Err(error) => panic!("headless renderer failed: {error}"),
    }
}

pub(super) fn draw(renderer: &mut Renderer, scene: &Scene, size: (u32, u32)) -> image::RgbaImage {
    let pixels = renderer
        .render_to_rgba(scene, &mut test_text(), size.0, size.1)
        .expect("offscreen render");
    image::RgbaImage::from_raw(size.0, size.1, pixels).expect("pixel buffer size")
}

pub(super) fn render(
    scene: &Scene,
    size: (u32, u32),
    options: RendererOptions,
) -> Option<image::RgbaImage> {
    let mut renderer = renderer(size.0, size.1, options)?;
    Some(draw(&mut renderer, scene, size))
}

/// Saves `image` as review evidence under `QUARK_EVIDENCE_DIR` when it is
/// set; tests never read it back.
pub(super) fn evidence(name: &str, image: &image::RgbaImage) {
    if let Some(dir) = std::env::var_os("QUARK_EVIDENCE_DIR") {
        let path = std::path::Path::new(&dir).join(format!("{name}.png"));
        let _ = std::fs::create_dir_all(&dir);
        let _ = image.save(path);
    }
}

pub(super) fn rect(x: f32, y: f32, width: f32, height: f32) -> Rect {
    Rect {
        x,
        y,
        width,
        height,
    }
}

fn fill(scene: &mut Scene, area: Rect, color: Color) {
    scene.rect(RectPrimitive { rect: area, color });
}

fn near(actual: [u8; 4], expected: [u8; 3], tolerance: u8) -> bool {
    (0..3).all(|i| actual[i].abs_diff(expected[i]) <= tolerance)
}

// Source-over of a translucent fill. Linear values: decode, mix, encode.
// 50% black over white: 0.5 linear encodes to 188. 40% green over gray
// 100 (0.1274 linear): red/blue 0.6 * 0.1274 = 0.0765 -> 79, green
// 0.4 + 0.0765 = 0.4765 -> 184. Encoded values mix the bytes directly.
#[test]
fn translucent_fills_blend_in_the_surface_space() {
    let cases = [
        (
            "black scrim",
            Color::rgba(255, 255, 255, 255),
            Color::rgba(0, 0, 0, 128),
            [188, 188, 188],
            [127, 127, 127],
        ),
        (
            "green over gray",
            Color::rgba(100, 100, 100, 255),
            Color::rgba(0, 255, 0, 102),
            [79, 184, 79],
            [60, 162, 60],
        ),
    ];
    for (name, below, above, linear, web) in cases {
        let mut scene = Scene::default();
        fill(&mut scene, rect(0.0, 0.0, 16.0, 16.0), below);
        fill(&mut scene, rect(0.0, 0.0, 16.0, 16.0), above);
        for (options, expected) in [(LINEAR, linear), (WEB, web)] {
            let Some(image) = render(&scene, (16, 16), options) else {
                return;
            };
            let pixel = image.get_pixel(8, 8).0;
            assert!(
                near(pixel, expected, 2),
                "{name} in {:?}: {pixel:?}, expected {expected:?}",
                options.compositing
            );
        }
    }
}

// A half-opaque group of two overlapping opaque rects fades as one, in the
// surface's space: 188 in linear light, 128 encoded, and the overlap no
// darker than the rest.
#[test]
fn group_opacity_fades_as_one_in_the_surface_space() {
    let mut scene = Scene::default();
    fill(
        &mut scene,
        rect(0.0, 0.0, 40.0, 20.0),
        Color::rgba(255, 255, 255, 255),
    );
    scene.push(Primitive::LayerStart(LayerPrimitive {
        opacity: 0.5,
        transform: Transform2D::IDENTITY,
    }));
    fill(
        &mut scene,
        rect(0.0, 0.0, 30.0, 20.0),
        Color::rgba(0, 0, 0, 255),
    );
    fill(
        &mut scene,
        rect(10.0, 0.0, 30.0, 20.0),
        Color::rgba(0, 0, 0, 255),
    );
    scene.pop_layer();
    for (options, expected) in [(LINEAR, 188), (WEB, 128)] {
        let Some(image) = render(&scene, (40, 20), options) else {
            return;
        };
        for x in [5, 20, 35] {
            let pixel = image.get_pixel(x, 10).0;
            assert!(
                near(pixel, [expected; 3], 2),
                "{:?} at x {x}: {pixel:?}",
                options.compositing
            );
        }
    }
}

/// A 4x4 image: opaque orange, with a fully transparent right column and
/// a half-transparent (premultiplied) black bottom row.
fn edged_image() -> Primitive {
    let mut rgba = Vec::new();
    for y in 0..4 {
        for x in 0..4 {
            rgba.extend_from_slice(match (x, y) {
                (3, _) => &[0, 0, 0, 0],
                (_, 3) => &[0, 0, 0, 128],
                _ => &[200, 100, 50, 255],
            });
        }
    }
    Primitive::Image(ImagePrimitive {
        rect: rect(0.0, 0.0, 4.0, 4.0),
        width: 4,
        height: 4,
        rgba: Arc::from(rgba),
        cache_key: 0x1ed6e,
    })
}

// Image texels keep their encoded bytes on a web-compatible surface (the
// sampled linear value converts back at the composite), and a
// half-transparent edge blends in the surface's space.
#[test]
fn image_texels_composite_in_the_surface_space() {
    let mut scene = Scene::default();
    fill(
        &mut scene,
        rect(0.0, 0.0, 4.0, 4.0),
        Color::rgba(255, 255, 255, 255),
    );
    scene.push(edged_image());
    for (options, edge) in [(LINEAR, 188), (WEB, 127)] {
        let Some(image) = render(&scene, (4, 4), options) else {
            return;
        };
        let compositing = options.compositing;
        let inside = image.get_pixel(1, 1).0;
        assert!(
            near(inside, [200, 100, 50], 1),
            "{compositing:?}: {inside:?}"
        );
        let clear = image.get_pixel(3, 1).0;
        assert!(
            near(clear, [255, 255, 255], 0),
            "{compositing:?}: {clear:?}"
        );
        let half = image.get_pixel(1, 3).0;
        assert!(near(half, [edge; 3], 2), "{compositing:?}: {half:?}");
    }
}

// Over a transparent window background the image's own alpha reaches the
// target in both modes: zero at the clear column, half at the faded row.
#[test]
fn image_edge_alpha_survives_a_transparent_background() {
    let mut scene = Scene::default();
    scene.push(edged_image());
    for options in [LINEAR, WEB] {
        let Some(mut renderer) = renderer(4, 4, options) else {
            return;
        };
        renderer.set_surface_background(SurfaceBackground::Transparent);
        let image = draw(&mut renderer, &scene, (4, 4));
        let compositing = options.compositing;
        assert_eq!(image.get_pixel(3, 1).0, [0, 0, 0, 0], "{compositing:?}");
        assert_eq!(image.get_pixel(1, 1).0[3], 255, "{compositing:?}");
        let half = image.get_pixel(1, 3).0[3];
        assert!(half.abs_diff(128) <= 1, "{compositing:?}: alpha {half}");
    }
}

// Without a non-sRGB view of the window texture, an encoded frame draws
// into an encoded texture and converts once; the pixels must match.
#[test]
fn encoded_frames_match_through_the_conversion_pass() {
    let mut scene = Scene::default();
    fill(
        &mut scene,
        rect(0.0, 0.0, 24.0, 24.0),
        Color::rgba(240, 240, 240, 255),
    );
    fill(
        &mut scene,
        rect(4.0, 4.0, 16.0, 16.0),
        Color::rgba(20, 80, 200, 150),
    );
    scene.push(edged_image());
    let Some(mut direct) = renderer(24, 24, WEB) else {
        return;
    };
    let expected = draw(&mut direct, &scene, (24, 24));
    let Some(mut converted) = renderer(24, 24, WEB) else {
        return;
    };
    converted.direct_encoded = false;
    let actual = draw(&mut converted, &scene, (24, 24));
    for (a, b) in actual.pixels().zip(expected.pixels()) {
        assert!(near(a.0, [b.0[0], b.0[1], b.0[2]], 1), "{a:?} vs {b:?}");
    }
}

fn layout(text: &str, size: f32) -> ShapedText {
    let params = TextParams::new(text, TextStyle::new(size));
    ShapedText::new(Arc::new(test_text().layout(&params).expect("layout")))
}

/// Ink of black "WMWM" glyphs over white: the summed darkening of the
/// text's pixels.
fn ink(image: &image::RgbaImage) -> u64 {
    image.pixels().map(|p| u64::from(255 - p.0[1])).sum::<u64>()
}

fn black_text(backdrop: TextBackdrop, rendering: TextRendering) -> Scene {
    let mut scene = Scene::default();
    fill(
        &mut scene,
        rect(0.0, 0.0, 96.0, 32.0),
        Color::rgba(255, 255, 255, 255),
    );
    scene.styled_text(
        StyledTextPrimitive::new(
            rect(4.0, 4.0, 88.0, 24.0),
            layout("WMWMWM", 16.0),
            TextFill::Solid(Color::rgba(0, 0, 0, 255)),
        )
        .backdrop(backdrop)
        .rendering(rendering),
    );
    scene
}

// Perceptual coverage gives dark text on a known light backdrop the
// weight sRGB blending gives (more ink than linear coverage), and falls
// back to linear coverage when the backdrop is unknown.
#[test]
fn perceptual_text_corrects_only_over_a_known_backdrop() {
    let white = TextBackdrop::Opaque(Color::rgba(255, 255, 255, 255));
    let ink_of = |backdrop, rendering| {
        render(&black_text(backdrop, rendering), (96, 32), LINEAR).map(|image| ink(&image))
    };
    let (Some(linear), Some(perceptual), Some(unknown)) = (
        ink_of(white, TextRendering::Linear),
        ink_of(white, TextRendering::Perceptual),
        ink_of(TextBackdrop::Unknown, TextRendering::Perceptual),
    ) else {
        return;
    };
    assert!(
        perceptual > linear * 11 / 10,
        "perceptual {perceptual} vs linear {linear}"
    );
    assert_eq!(unknown, linear, "an unknown backdrop must not be guessed");
}

// A web-compatible surface already blends text in sRGB space, so a known
// backdrop must not correct coverage a second time.
#[test]
fn web_compatible_text_is_not_corrected_twice() {
    let white = TextBackdrop::Opaque(Color::rgba(255, 255, 255, 255));
    let draw_web = |rendering| render(&black_text(white, rendering), (96, 32), WEB);
    let (Some(perceptual), Some(linear)) = (
        draw_web(TextRendering::Perceptual),
        draw_web(TextRendering::Linear),
    ) else {
        return;
    };
    assert_eq!(ink(&perceptual), ink(&linear));
    // And it carries sRGB weight: more ink than linear-light coverage.
    let Some(linear_surface) = render(&black_text(white, TextRendering::Linear), (96, 32), LINEAR)
    else {
        return;
    };
    assert!(ink(&perceptual) > ink(&linear_surface));
}

/// Terminal-like content: a dark cell background and light text whose
/// layout asks for the terminal's linear-corrected coverage.
fn terminal_cells(scene: &mut Scene) {
    fill(
        scene,
        rect(8.0, 8.0, 112.0, 32.0),
        Color::rgba(30, 30, 46, 255),
    );
    fill(
        scene,
        rect(40.0, 8.0, 24.0, 32.0),
        Color::rgba(180, 60, 60, 255),
    );
    let style = TextStyle::new(16.0).linear_correction(Some(40));
    let params = TextParams::new("ls -la", style);
    let layout = test_text().layout(&params).expect("layout");
    scene.push(Primitive::TextRun(TextPrimitive {
        rect: rect(12.0, 14.0, 100.0, 20.0),
        layout: ShapedText::new(Arc::new(layout)),
        color: Color::rgba(220, 220, 230, 255),
    }));
}

// A terminal kept in a linear island of a web-compatible window draws the
// pixels it draws in a linear window: cell colors, glyph coverage, and its
// linear correction all survive the one conversion.
#[test]
fn linear_island_keeps_terminal_pixels_in_a_web_window() {
    let mut linear_scene = Scene::default();
    fill(
        &mut linear_scene,
        rect(0.0, 0.0, 128.0, 48.0),
        Color::rgba(250, 250, 250, 255),
    );
    terminal_cells(&mut linear_scene);
    let mut web_scene = Scene::default();
    fill(
        &mut web_scene,
        rect(0.0, 0.0, 128.0, 48.0),
        Color::rgba(250, 250, 250, 255),
    );
    web_scene.push_compositing_island(rect(8.0, 8.0, 112.0, 32.0), UiCompositing::Linear);
    terminal_cells(&mut web_scene);
    web_scene.pop_isolate();
    let (Some(expected), Some(actual)) = (
        render(&linear_scene, (128, 48), LINEAR),
        render(&web_scene, (128, 48), WEB),
    ) else {
        return;
    };
    evidence("g18-terminal-island-linear", &expected);
    evidence("g18-terminal-island-web", &actual);
    for y in 8..40 {
        for x in 8..120 {
            let (a, b) = (actual.get_pixel(x, y).0, expected.get_pixel(x, y).0);
            assert!(near(a, [b[0], b[1], b[2]], 1), "({x}, {y}): {a:?} vs {b:?}");
        }
    }
}
