//! Probes of the backdrop blur: impulse symmetry, the Gaussian's width,
//! and pooled scratch targets of other sizes. The diagnosis of the frosted
//! panel complaint (G17) rests on these.

use super::compositing_tests::{draw, evidence, rect, renderer};
use super::*;
use crate::scene::{BlurRegionPrimitive, RectPrimitive};
use quark::Color;

const SIZE: (u32, u32) = (96, 96);

/// Linear light of an sRGB-encoded byte, from the transfer function.
fn linear(v: u8) -> f64 {
    let c = f64::from(v) / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn blur(scene: &mut Scene, area: Rect, blur_radius: f32) {
    scene.blur_region(BlurRegionPrimitive {
        rect: area,
        blur_radius,
        corner_radii: [0.0; 4],
    });
}

fn black(scene: &mut Scene) {
    scene.rect(RectPrimitive {
        rect: rect(0.0, 0.0, SIZE.0 as f32, SIZE.1 as f32),
        color: Color::rgba(0, 0, 0, 255),
    });
}

fn white(scene: &mut Scene, area: Rect) {
    scene.rect(RectPrimitive {
        rect: area,
        color: Color::rgba(255, 255, 255, 255),
    });
}

/// The green bytes along row 47 (or column 47, when `vertical`), through
/// the impulse centered on the corner at (48, 48).
fn bytes(image: &image::RgbaImage, vertical: bool) -> Vec<u8> {
    (0..SIZE.0)
        .map(|i| {
            let p = if vertical {
                image.get_pixel(47, i)
            } else {
                image.get_pixel(i, 47)
            };
            p.0[1]
        })
        .collect()
}

/// Linear intensity along row 48.
fn profile(image: &image::RgbaImage) -> Vec<f64> {
    (0..SIZE.0)
        .map(|x| linear(image.get_pixel(x, 48).0[1]))
        .collect()
}

// An 8-pixel white square centered on (48, 48), blurred: bytes at
// opposite offsets match and the linear centroid stays on the center, both
// ways, below and above the radius where the kernel starts spacing its
// samples apart.
#[test]
fn blurred_impulse_stays_symmetric_and_centered() {
    for radius in [8.0, 40.0] {
        let mut scene = Scene::default();
        black(&mut scene);
        white(&mut scene, rect(44.0, 44.0, 8.0, 8.0));
        blur(
            &mut scene,
            rect(0.0, 0.0, SIZE.0 as f32, SIZE.1 as f32),
            radius,
        );
        let Some(mut renderer) = renderer(SIZE.0, SIZE.1, Default::default()) else {
            return;
        };
        let image = draw(&mut renderer, &scene, SIZE);
        evidence(&format!("g17-impulse-r{radius}"), &image);
        for vertical in [false, true] {
            let line = bytes(&image, vertical);
            for k in 0..40 {
                let (a, b) = (line[47 - k], line[48 + k]);
                assert!(
                    a.abs_diff(b) <= 1,
                    "radius {radius}, vertical {vertical}, offset {k}: {a} vs {b}"
                );
            }
            let total: f64 = line.iter().map(|&v| linear(v)).sum();
            let centroid = line
                .iter()
                .enumerate()
                .map(|(i, &v)| (i as f64 + 0.5) * linear(v))
                .sum::<f64>()
                / total;
            assert!(
                (centroid - 48.0).abs() < 0.1,
                "radius {radius}, vertical {vertical}: centroid {centroid}"
            );
        }
    }
}

/// Pixels the linear intensity of a blurred black-to-white step takes to
/// rise from 10% to 90% along row 48.
fn rise(image: &image::RgbaImage) -> f64 {
    let line = profile(image);
    let crossing = |level: f64| {
        let i = line
            .iter()
            .position(|&v| v >= level)
            .expect("reaches level");
        let (a, b) = (line[i - 1], line[i]);
        (i - 1) as f64 + 0.5 + (level - a) / (b - a)
    };
    crossing(0.9) - crossing(0.1)
}

// The blur is a Gaussian of sigma half the blur radius: a step edge rises
// from 10% to 90% over 2.563 sigma, whatever the radius. Past the radius
// where samples spread apart, weights taken at unscaled distances made the
// kernel nearly flat and cut it at one sigma, a much narrower blur.
#[test]
fn blurred_step_rises_over_the_gaussian_width() {
    for radius in [8.0f32, 24.0, 40.0] {
        let mut scene = Scene::default();
        black(&mut scene);
        white(&mut scene, rect(48.0, 0.0, 48.0, SIZE.1 as f32));
        blur(
            &mut scene,
            rect(0.0, 0.0, SIZE.0 as f32, SIZE.1 as f32),
            radius,
        );
        let Some(mut renderer) = renderer(SIZE.0, SIZE.1, Default::default()) else {
            return;
        };
        let image = draw(&mut renderer, &scene, SIZE);
        let expected = 2.563 * f64::from(radius) * 0.5;
        let got = rise(&image);
        assert!(
            (got - expected).abs() <= 0.15 * expected,
            "radius {radius}: rise {got}, expected {expected}"
        );
    }
}

// A blur whose scratch textures come from a pool holding larger textures
// (left by bigger windows or layers) blurs the same pixels as a fresh
// renderer: each pass reads its source with that texture's own size.
#[test]
fn blur_after_a_larger_pooled_target_draws_the_same_pixels() {
    let mut scene = Scene::default();
    black(&mut scene);
    white(&mut scene, rect(40.0, 20.0, 16.0, 56.0));
    blur(&mut scene, rect(16.0, 16.0, 64.0, 64.0), 12.0);
    let Some(mut fresh) = renderer(SIZE.0, SIZE.1, Default::default()) else {
        return;
    };
    let expected = draw(&mut fresh, &scene, SIZE);
    let Some(mut pooled) = renderer(SIZE.0, SIZE.1, Default::default()) else {
        return;
    };
    // The scene texture fits exactly; the next scratch is three times as
    // large.
    let exact = pooled.acquire_offscreen(SIZE.0, SIZE.1);
    let large = pooled.acquire_offscreen(SIZE.0 * 3, SIZE.1 * 3);
    pooled.release_offscreen(exact);
    pooled.release_offscreen(large);
    let actual = draw(&mut pooled, &scene, SIZE);
    evidence("g17-pooled-expected", &expected);
    evidence("g17-pooled-actual", &actual);
    for (x, y, pixel) in actual.enumerate_pixels() {
        let want = expected.get_pixel(x, y).0;
        let off = (0..3)
            .map(|i| pixel.0[i].abs_diff(want[i]))
            .max()
            .unwrap_or(0);
        assert!(off <= 1, "({x}, {y}): {:?} vs {want:?}", pixel.0);
    }
}
