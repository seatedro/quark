//! Pixel probes of patterned strokes, borders, decorations, and stripes.

use std::sync::Arc;

use super::compositing_tests::{evidence, rect, render};
use super::*;
use crate::scene::{
    LineCap, Path, PathPrimitive, RectPrimitive, StripesPrimitive, StrokePattern, StrokeStyle,
    StyledDecoration, TextDecoration, TextDecorationKind, TextDecorationStyle,
};
use crate::text::test_text;
use quark::Color;
use quark_text::{TextParams, TextStyle};

const INK: Color = Color::rgba(0, 0, 0, 255);

fn white(scene: &mut Scene, (w, h): (u32, u32)) {
    scene.rect(RectPrimitive {
        rect: rect(0.0, 0.0, w as f32, h as f32),
        color: Color::rgba(255, 255, 255, 255),
    });
}

fn inked(image: &image::RgbaImage, x: f32, y: f32) -> bool {
    image.get_pixel(x as u32, y as u32).0[0] < 128
}

/// A 100-point horizontal line at y 10, stroked 4 wide in `pattern`,
/// drawn at `scale`.
fn patterned_line(pattern: StrokePattern, cap: LineCap, scale: f32) -> Option<image::RgbaImage> {
    let size = ((110.0 * scale) as u32, (20.0 * scale) as u32);
    let mut scene = Scene::default();
    let mut line = Path::builder();
    line.move_to(0.0, 0.0).line_to(100.0, 0.0);
    scene.path(
        PathPrimitive::new(Arc::new(line.build()), [0.0, 10.0])
            .stroke(INK, StrokeStyle::new(4.0).cap(cap).pattern(pattern)),
    );
    for primitive in &mut scene.primitives {
        primitive.to_physical(scale);
    }
    scene.primitives.insert(
        0,
        Primitive::Rect(RectPrimitive {
            rect: rect(0.0, 0.0, size.0 as f32, size.1 as f32),
            color: Color::rgba(255, 255, 255, 255),
        }),
    );
    render(&scene, size, Default::default())
}

// Dots sit `spacing` apart from `offset`, round and as wide as the stroke,
// with clear gaps between, at every scale (lengths are logical).
#[test]
fn dotted_stroke_places_dots_by_logical_spacing() {
    let dotted = StrokePattern::Dotted {
        spacing: 10.0,
        offset: 5.0,
    };
    for scale in [1.0, 1.25, 2.0] {
        let Some(image) = patterned_line(dotted, LineCap::Butt, scale) else {
            return;
        };
        evidence(&format!("g9-dotted-{scale}x"), &image);
        let y = 10.0 * scale;
        for k in 0..10 {
            let center = (5.0 + 10.0 * k as f32) * scale;
            assert!(inked(&image, center, y), "{scale}x: no dot at {center}");
            // Round: the dot's corner box stays clear.
            assert!(
                !inked(&image, center + 1.8 * scale, y + 1.8 * scale),
                "{scale}x: square dot at {center}"
            );
        }
        for k in 1..10 {
            let gap = 10.0 * k as f32 * scale;
            assert!(!inked(&image, gap, y), "{scale}x: no gap at {gap}");
        }
    }
}

// A dash pattern with an offset starts partway into its first dash: 6 on,
// 4 off, shifted 2 along, puts dashes on [0, 4), [8, 14), [18, 24).
#[test]
fn dash_offset_shifts_the_pattern_phase() {
    let dashed = StrokePattern::Dashed {
        dash: 6.0,
        gap: 4.0,
        offset: 2.0,
    };
    let Some(image) = patterned_line(dashed, LineCap::Butt, 1.0) else {
        return;
    };
    let table = [
        (2.0, true),
        (6.0, false),
        (9.0, true),
        (13.0, true),
        (16.0, false),
        (21.0, true),
    ];
    for (x, ink) in table {
        assert_eq!(inked(&image, x, 10.0), ink, "x {x}");
    }
}

// Hostile patterns (a NaN or zero length, dots closer than their width)
// draw a solid stroke rather than nothing or an unbounded dash list.
#[test]
fn invalid_patterns_draw_solid() {
    let table = [
        StrokePattern::Dashed {
            dash: f32::NAN,
            gap: 4.0,
            offset: 0.0,
        },
        StrokePattern::Dashed {
            dash: 4.0,
            gap: 0.0,
            offset: 0.0,
        },
        StrokePattern::Dotted {
            spacing: 2.0,
            offset: 0.0,
        },
        StrokePattern::Dashed {
            dash: 1e-9,
            gap: 1e-9,
            offset: f32::INFINITY,
        },
    ];
    for pattern in table {
        let Some(image) = patterned_line(pattern, LineCap::Butt, 1.0) else {
            return;
        };
        for x in (1..99).step_by(3) {
            assert!(inked(&image, x as f32, 10.0), "{pattern:?}: gap at {x}");
        }
    }
}

// A dotted rounded border stays inside its rect, leaves gaps along its
// edges, and carries its dots around the rounded corners.
#[test]
fn dotted_border_follows_the_rounded_perimeter() {
    let size = (80, 60);
    let mut scene = Scene::default();
    white(&mut scene, size);
    let border = rect(10.0, 10.0, 60.0, 40.0);
    scene.path(PathPrimitive::border(
        border,
        3.0,
        [10.0; 4],
        INK,
        StrokePattern::dotted(8.0),
    ));
    let Some(image) = render(&scene, size, Default::default()) else {
        return;
    };
    evidence("g9-dotted-border", &image);
    for (x, y, pixel) in image.enumerate_pixels() {
        let outside = !(10..70).contains(&x) || !(10..50).contains(&y);
        assert!(!(outside && pixel.0[0] < 200), "ink outside at ({x}, {y})");
    }
    // The path runs 1.5 in, through the middle of row 11.
    let top: Vec<bool> = (20..60).map(|x| inked(&image, x as f32, 11.0)).collect();
    assert!(
        top.contains(&true) && top.contains(&false),
        "top edge {top:?}"
    );
    let corners = [(10, 10), (60, 10), (60, 40), (10, 40)];
    for (cx, cy) in corners {
        let any = (cx..cx + 10).any(|x| (cy..cy + 10).any(|y| inked(&image, x as f32, y as f32)));
        assert!(any, "no dot in the corner at ({cx}, {cy})");
    }
}

// A dotted underline over wrapped text restarts its pattern on every
// line: each line's first dot sits at the start of that line's segment.
#[test]
fn patterned_underline_restarts_on_each_wrapped_line() {
    let size = (240, 80);
    let text = "dotted link text that wraps onto another line";
    let params = TextParams::new(text, TextStyle::new(16.0)).wrap_width(Some(200.0));
    let layout = test_text().layout(&params).expect("layout");
    assert!(layout.lines().count() > 1, "fixture must wrap");
    let origin = (8.0, 8.0);
    let solid = TextDecoration {
        range: 0..text.len(),
        kind: TextDecorationKind::Underline,
        color: INK,
    };
    let segments = crate::text::text_decoration_rects(&layout, origin, &solid);
    let dotted = StyledDecoration {
        range: 0..text.len(),
        kind: TextDecorationKind::Underline,
        style: TextDecorationStyle::solid(INK)
            .pattern(StrokePattern::dotted(6.0))
            .thickness(3.0),
    };
    let mut scene = Scene::default();
    white(&mut scene, size);
    crate::text::push_styled_text_decorations(&mut scene, &layout, origin, &[dotted]);
    let Some(image) = render(&scene, size, Default::default()) else {
        return;
    };
    evidence("g9-dotted-underline", &image);
    let mut lines = 0;
    let mut seen = Vec::new();
    for line in segments.iter() {
        if seen.iter().any(|y: &f32| (y - line.y).abs() < 0.5) {
            continue;
        }
        seen.push(line.y);
        // The dotted line is thicker than the solid one; look around it.
        let rows = (line.y - 3.0) as u32..(line.y + 5.0) as u32;
        let first = (0..size.0).find(|&x| rows.clone().any(|y| inked(&image, x as f32, y as f32)));
        let Some(first) = first else {
            continue;
        };
        // Leftmost line segment start; a dot centered there inks from a
        // radius before.
        let start = segments
            .iter()
            .filter(|s| (s.y - line.y).abs() < 0.5)
            .map(|s| s.x)
            .fold(f32::MAX, f32::min);
        assert!(
            (first as f32 - (start - 1.5)).abs() <= 1.0,
            "line at {}: first dot at {first}, segment starts at {start}",
            line.y
        );
        lines += 1;
    }
    assert!(lines > 1);
}

// Stripes keep their logical period at every scale, clipped to the
// rounded rect: a band of the first color, then the second.
#[test]
fn stripes_keep_their_period_across_scales() {
    for scale in [1.0, 1.25, 2.0] {
        let size = ((60.0 * scale) as u32, (30.0 * scale) as u32);
        let mut scene = Scene::default();
        scene.stripes(StripesPrimitive {
            rect: rect(0.0, 0.0, 60.0, 30.0),
            corner_radii: [8.0; 4],
            angle: 0.0,
            period: 10.0,
            duty: 0.5,
            colors: [Color::rgba(255, 0, 0, 255), Color::rgba(0, 0, 255, 255)],
        });
        for primitive in &mut scene.primitives {
            primitive.to_physical(scale);
        }
        let Some(image) = render(&scene, size, Default::default()) else {
            return;
        };
        evidence(&format!("g9-stripes-{scale}x"), &image);
        let y = 15.0 * scale;
        for k in 1..5 {
            let red = image
                .get_pixel(((10.0 * k as f32 + 2.5) * scale) as u32, y as u32)
                .0;
            let blue = image
                .get_pixel(((10.0 * k as f32 + 7.5) * scale) as u32, y as u32)
                .0;
            assert!(red[0] > 240 && red[2] < 15, "{scale}x stripe {k}: {red:?}");
            assert!(blue[2] > 240 && blue[0] < 15, "{scale}x gap {k}: {blue:?}");
        }
        assert_eq!(image.get_pixel(0, 0).0[3], 255);
        assert!(image.get_pixel(0, 0).0[0] < 10, "corner not clipped");
    }
}
