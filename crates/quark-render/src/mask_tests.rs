//! Pixel probes of alpha masks on isolated groups (edge fades).

use super::compositing_tests::{evidence, rect, render};
use super::*;
use crate::scene::{AlphaMask, FadeEdge, LayerPrimitive, RectPrimitive};
use quark::Color;

const WEB: RendererOptions = RendererOptions {
    compositing: UiCompositing::WebCompatible,
    text_rendering: TextRendering::Perceptual,
};

const RED: Color = Color::rgba(255, 0, 0, 255);

/// Black and white squares of `cell` pixels over `area`.
fn checkerboard(scene: &mut Scene, area: Rect, cell: f32) {
    let (cols, rows) = (
        (area.width / cell).ceil() as u32,
        (area.height / cell).ceil() as u32,
    );
    for row in 0..rows {
        for col in 0..cols {
            let light = (row + col) % 2 == 0;
            let v = if light { 255 } else { 0 };
            scene.rect(RectPrimitive {
                rect: rect(
                    area.x + col as f32 * cell,
                    area.y + row as f32 * cell,
                    cell,
                    cell,
                ),
                color: Color::rgba(v, v, v, 255),
            });
        }
    }
}

fn red(scene: &mut Scene, area: Rect) {
    scene.rect(RectPrimitive {
        rect: area,
        color: RED,
    });
}

/// The pixel at `(x, y)`, with how far it is from `expected`.
fn off(image: &image::RgbaImage, (x, y): (u32, u32), expected: [u8; 3]) -> u8 {
    let p = image.get_pixel(x, y).0;
    (0..3)
        .map(|i| p[i].abs_diff(expected[i]))
        .max()
        .unwrap_or(0)
}

// A right-edge fade over a checkerboard: opaque before the ramp, half way
// through it the red covers half of whichever cell lies below, and past
// its end the backdrop shows untouched. Two overlapping rects in the
// group fade as one, so their overlap is no redder than either alone.
#[test]
fn edge_fade_reveals_the_backdrop_and_fades_content_once() {
    let mut scene = Scene::default();
    let area = rect(0.0, 0.0, 120.0, 16.0);
    checkerboard(&mut scene, area, 8.0);
    let group = rect(0.0, 0.0, 100.0, 16.0);
    scene.push_mask(group, AlphaMask::fade_edge(group, FadeEdge::Right, 40.0));
    red(&mut scene, rect(0.0, 0.0, 70.0, 16.0));
    red(&mut scene, rect(50.0, 0.0, 50.0, 16.0));
    scene.pop_isolate();
    let Some(image) = render(&scene, (120, 16), WEB) else {
        return;
    };
    evidence("g6-fade-checkerboard", &image);
    // Before the ramp, overlap included.
    for x in [10, 55] {
        assert_eq!(off(&image, (x, 4), [255, 0, 0]), 0, "x {x}");
    }
    // Pixel 79 samples the mask at 79.5: alpha 1 - 19.5 / 40.
    let a = 1.0 - 19.5 / 40.0;
    let over = |below: f32| {
        [
            255.0 * a + below * (1.0 - a),
            below * (1.0 - a),
            below * (1.0 - a),
        ]
    };
    let as_bytes = |c: [f32; 3]| c.map(|v| v.round() as u8);
    assert!(
        off(&image, (79, 4), as_bytes(over(0.0))) <= 2,
        "{:?}",
        image.get_pixel(79, 4)
    );
    assert!(
        off(&image, (79, 12), as_bytes(over(255.0))) <= 2,
        "{:?}",
        image.get_pixel(79, 12)
    );
    // At the end of the ramp and outside the group, the checkerboard as
    // drawn (pixel 99 keeps 1/80 of the red).
    assert!(
        off(&image, (99, 4), [255, 252, 252]) <= 1,
        "{:?}",
        image.get_pixel(99, 4)
    );
    assert_eq!(off(&image, (110, 12), [255, 255, 255]), 0);
}

// A mask inside a half-opaque layer multiplies with it, and a rounded clip
// around the masked group still cuts its corners.
#[test]
fn masks_compose_with_group_opacity_and_rounded_clips() {
    let mut scene = Scene::default();
    scene.rect(RectPrimitive {
        rect: rect(0.0, 0.0, 64.0, 32.0),
        color: Color::rgba(255, 255, 255, 255),
    });
    scene.clip_rounded(rect(0.0, 0.0, 64.0, 32.0), [12.0; 4]);
    scene.push(Primitive::LayerStart(LayerPrimitive {
        opacity: 0.5,
        transform: Transform2D::IDENTITY,
    }));
    let group = rect(0.0, 0.0, 64.0, 32.0);
    scene.push_mask(group, AlphaMask::fade_edge(group, FadeEdge::Bottom, 8.0));
    scene.rect(RectPrimitive {
        rect: group,
        color: Color::rgba(0, 0, 0, 255),
    });
    scene.pop_isolate();
    scene.pop_layer();
    scene.pop_clip();
    let Some(image) = render(&scene, (64, 32), WEB) else {
        return;
    };
    // Above the ramp: the layer's half opacity alone.
    assert!(
        off(&image, (32, 16), [128, 128, 128]) <= 2,
        "{:?}",
        image.get_pixel(32, 16)
    );
    // Pixel row 27 samples the ramp at 27.5: mask 1 - 3.5 / 8, times 0.5.
    let expected = (255.0 * (1.0 - 0.5 * (1.0 - 3.5 / 8.0))) as u8;
    assert!(
        off(&image, (32, 27), [expected; 3]) <= 2,
        "{:?}",
        image.get_pixel(32, 27)
    );
    assert_eq!(
        off(&image, (0, 0), [255, 255, 255]),
        0,
        "corner not clipped"
    );
}

// At a fractional scale the ramp converts with the group: a fade of 20
// points at 1.5x spans 30 pixels and ends at the scaled edge.
#[test]
fn fade_ramp_scales_with_the_scene() {
    let mut scene = Scene::default();
    scene.rect(RectPrimitive {
        rect: rect(0.0, 0.0, 80.0, 10.0),
        color: Color::rgba(255, 255, 255, 255),
    });
    let group = rect(0.0, 0.0, 60.0, 10.0);
    scene.push_mask(group, AlphaMask::fade_edge(group, FadeEdge::Right, 20.0));
    scene.rect(RectPrimitive {
        rect: group,
        color: Color::rgba(0, 0, 0, 255),
    });
    scene.pop_isolate();
    for primitive in &mut scene.primitives {
        primitive.to_physical(1.5);
    }
    let Some(image) = render(&scene, (120, 15), WEB) else {
        return;
    };
    // Ramp from 60 to 90 pixels: pixel 74 at 74.5 is 14.5 / 30 faded.
    let at = |x| image.get_pixel(x, 7).0[0];
    assert_eq!(at(55), 0);
    assert!(at(74).abs_diff(123) <= 2, "{}", at(74));
    assert_eq!(at(91), 255);
}
