//! Pixel probes of glyph-masked text fills: gradients and the shimmer
//! highlight, evaluated per pixel inside glyph coverage only.

use std::sync::Arc;

use super::compositing_tests::{evidence, rect, render, renderer};
use super::*;
use crate::scene::{
    RectPrimitive, ShapedText, ShimmerSpec, StyledTextPrimitive, TextFill, TextGradient,
};
use crate::text::test_text;
use quark::Color;
use quark_text::{TextParams, TextStyle};

const SIZE: (u32, u32) = (240, 48);
const BASE: Color = Color::rgba(60, 60, 60, 255);
const HIGHLIGHT: Color = Color::rgba(255, 0, 0, 255);

fn bars() -> ShapedText {
    let params = TextParams::new("IIIIIIIIIIIIIIIIIIIIIIIIIIIIII", TextStyle::new(32.0));
    ShapedText::new(Arc::new(test_text().layout(&params).expect("layout")))
}

fn scene(layout: &ShapedText, fill: TextFill) -> Scene {
    let mut scene = Scene::default();
    scene.rect(RectPrimitive {
        rect: rect(0.0, 0.0, SIZE.0 as f32, SIZE.1 as f32),
        color: Color::rgba(255, 255, 255, 255),
    });
    scene.styled_text(StyledTextPrimitive::new(
        rect(0.0, 4.0, SIZE.0 as f32, 40.0),
        layout.clone(),
        fill,
    ));
    scene
}

/// Per column, the redness (red minus green) of its most inked pixel, or
/// `None` for a column with no ink.
fn column_redness(image: &image::RgbaImage) -> Vec<Option<i32>> {
    (0..image.width())
        .map(|x| {
            (0..image.height())
                .map(|y| image.get_pixel(x, y).0)
                .filter(|p| p[1] < 160)
                .min_by_key(|p| p[1])
                .map(|p| i32::from(p[0]) - i32::from(p[1]))
        })
        .collect()
}

// The highlight band colors the glyphs it passes over and nothing else: at
// each phase the ink near the band's center is red, ink a band away is
// the base gray, and the white between glyphs stays white.
#[test]
fn shimmer_band_colors_only_the_glyphs_under_it() {
    let layout = bars();
    let band = 40.0;
    for phase in [0.3f32, 0.7] {
        let spec = ShimmerSpec::new(BASE, HIGHLIGHT)
            .band_width(band)
            .phase(phase);
        let Some(image) = render(
            &scene(&layout, TextFill::Shimmer(spec)),
            SIZE,
            Default::default(),
        ) else {
            return;
        };
        evidence(&format!("g4-shimmer-phase-{phase}"), &image);
        let center = -band / 2.0 + phase * (SIZE.0 as f32 + band);
        let redness = column_redness(&image);
        let inked = |range: std::ops::Range<f32>| {
            redness
                .iter()
                .enumerate()
                .filter(|(x, _)| range.contains(&(*x as f32)))
                .filter_map(|(_, r)| *r)
                .collect::<Vec<_>>()
        };
        let lit = inked(center - 6.0..center + 6.0);
        assert!(!lit.is_empty(), "phase {phase}: no glyph under the band");
        assert!(
            lit.iter().all(|&r| r > 80),
            "phase {phase}: band not red {lit:?}"
        );
        let far = [
            inked(0.0..center - band),
            inked(center + band..SIZE.0 as f32),
        ]
        .concat();
        assert!(!far.is_empty());
        assert!(
            far.iter().all(|&r| r.abs() < 8),
            "phase {phase}: highlight outside the band {far:?}"
        );
        assert_eq!(image.get_pixel(center as u32, 1).0, [255, 255, 255, 255]);
    }
}

// A gradient fill runs from its start color at the start point to its
// end color at the end, clamped beyond.
#[test]
fn gradient_fill_runs_from_start_to_end_color() {
    let layout = bars();
    let gradient = TextGradient {
        start: [40.0, 0.0],
        end: [200.0, 0.0],
        from: Color::rgba(255, 0, 0, 255),
        to: Color::rgba(0, 0, 255, 255),
    };
    let Some(image) = render(
        &scene(&layout, TextFill::LinearGradient(gradient)),
        SIZE,
        Default::default(),
    ) else {
        return;
    };
    evidence("g4-gradient", &image);
    let darkest = |range: std::ops::Range<u32>| {
        range
            .flat_map(|x| (0..SIZE.1).map(move |y| (x, y)))
            .map(|(x, y)| image.get_pixel(x, y).0)
            .min_by_key(|p| p[1])
            .expect("pixels")
    };
    let left = darkest(0..40);
    let right = darkest(200..SIZE.0);
    assert!(left[0] > 200 && left[2] < 60, "left {left:?}");
    assert!(right[2] > 200 && right[0] < 60, "right {right:?}");
}

// Advancing a shimmer's phase every frame changes only the run's fill
// uniform: preparing the frame's text allocates nothing and rasterizes no
// glyph. (Writing the fills while preparing allocated a staging copy every
// frame.)
#[test]
fn advancing_shimmer_phase_allocates_nothing() {
    let Some(mut renderer) = renderer(SIZE.0, SIZE.1, Default::default()) else {
        return;
    };
    let layout = bars();
    let at = |phase| {
        scene(
            &layout,
            TextFill::Shimmer(ShimmerSpec::new(BASE, HIGHLIGHT).phase(phase)),
        )
    };
    let mut text = test_text();
    for phase in [0.1, 0.2] {
        renderer
            .render_to_rgba(&at(phase), &mut text, SIZE.0, SIZE.1)
            .expect("render");
    }
    let later = at(0.6);
    renderer.flatten(&later, SIZE.0, SIZE.1);
    let frames = std::mem::take(&mut renderer.frames);
    let active = renderer.active_frames;
    let misses = renderer.text_atlas_stats().misses;
    let (ready, allocated) =
        quark_ui::test_alloc::count(|| renderer.prepare_frame_text(&frames[..active], &mut text));
    renderer.frames = frames;
    assert!(ready);
    assert_eq!(allocated, 0);
    assert_eq!(renderer.text_atlas_stats().misses, misses);
}
