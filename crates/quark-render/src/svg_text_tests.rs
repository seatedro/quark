//! Pixel probes of text inside SVG icons (G11).

use super::compositing_tests::{evidence, rect, renderer};
use super::*;
use crate::scene::{IconPrimitive, RectPrimitive};
use crate::text::test_text;
use quark::Color;
use quark_text::FontSettings;

/// A two-letter badge: a rounded outline and "AB" in the default family.
const BADGE: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 16"><rect x="0.5" y="0.5" width="31" height="15" rx="4" fill="none" stroke="currentColor"/><text x="6" y="12.5" font-size="11" fill="currentColor">AB</text></svg>"#;

fn badge_scene(scale: f32) -> (Scene, (u32, u32)) {
    let size = ((40.0 * scale) as u32, (24.0 * scale) as u32);
    let mut scene = Scene::default();
    scene.rect(RectPrimitive {
        rect: rect(0.0, 0.0, size.0 as f32, size.1 as f32),
        color: Color::rgba(255, 255, 255, 255),
    });
    scene.push(Primitive::Icon(IconPrimitive {
        rect: rect(4.0 * scale, 4.0 * scale, 32.0 * scale, 16.0 * scale),
        name: BADGE.to_owned(),
        color: Color::rgba(0, 0, 0, 255),
    }));
    (scene, size)
}

/// Dark pixels inside the badge's outline, where only its text can ink.
fn text_ink(image: &image::RgbaImage, scale: f32) -> usize {
    let (x0, x1) = ((7.0 * scale) as u32, (33.0 * scale) as u32);
    let (y0, y1) = ((7.0 * scale) as u32, (17.0 * scale) as u32);
    (x0..x1)
        .flat_map(|x| (y0..y1).map(move |y| (x, y)))
        .filter(|&(x, y)| image.get_pixel(x, y).0[0] < 128)
        .count()
}

// Text in an SVG icon draws in the text system's fonts at every scale;
// without them (default usvg options, text compiled out) it drew nothing.
#[test]
fn svg_text_badge_draws_at_every_scale() {
    for scale in [1.0, 1.5, 2.0] {
        let (scene, size) = badge_scene(scale);
        let Some(mut renderer) = renderer(size.0, size.1, Default::default()) else {
            return;
        };
        let pixels = renderer
            .render_to_rgba(&scene, &mut test_text(), size.0, size.1)
            .expect("render");
        let image = image::RgbaImage::from_raw(size.0, size.1, pixels).expect("size");
        evidence(&format!("g11-badge-{scale}x"), &image);
        let ink = text_ink(&image, scale);
        assert!(
            ink as f32 > 12.0 * scale * scale,
            "{scale}x: {ink} text pixels"
        );
    }
}

// The same SVG rasterized under another text system's fonts draws in
// those fonts: the raster cache keys text by font epoch, so the second
// system never reuses the first one's bitmap.
#[test]
fn svg_text_rasters_follow_the_text_system() {
    let (scene, size) = badge_scene(2.0);
    let draw = |renderer: &mut Renderer, text: &mut TextSystem| {
        let pixels = renderer
            .render_to_rgba(&scene, text, size.0, size.1)
            .expect("render");
        image::RgbaImage::from_raw(size.0, size.1, pixels).expect("size")
    };
    let mut geist = TextSystem::vendored_only(&FontSettings::default());
    let mut source = TextSystem::vendored_only(&FontSettings {
        ui_family: "Source Sans 3".into(),
        ..FontSettings::default()
    });
    let Some(mut reused) = renderer(size.0, size.1, Default::default()) else {
        return;
    };
    let first = draw(&mut reused, &mut geist);
    let second = draw(&mut reused, &mut source);
    let Some(mut fresh) = renderer(size.0, size.1, Default::default()) else {
        return;
    };
    let expected = draw(&mut fresh, &mut source);
    assert!(first != expected, "the families draw the badge differently");
    assert!(second == expected, "reused the other system's raster");
}
