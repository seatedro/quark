//! Frame cost of a representative UI scene (sidebar rows, cards with
//! shadows and borders, an image, a scrim, and a faded layer holding a
//! blurred panel), for comparing compositing modes. Ignored: run with
//! `--ignored --nocapture` and read the report. Times are wall time of
//! `render_to_rgba`, readback included; pooled bytes are the offscreen
//! textures the pool holds after a frame.

use std::sync::Arc;
use std::time::Instant;

use super::*;
use crate::scene::{
    BlurRegionPrimitive, BorderPrimitive, ImagePrimitive, Primitive, RectPrimitive,
    RoundedRectPrimitive, ShapedText, TextPrimitive,
};
use quark::Color;
use quark_text::{TextParams, TextStyle};

fn scene(text: &mut TextSystem, s: f32, w: f32, h: f32) -> Scene {
    let mut scene = Scene::default();
    let r = |x: f32, y: f32, w: f32, h: f32| Rect {
        x: x * s,
        y: y * s,
        width: w * s,
        height: h * s,
    };
    scene.rect(RectPrimitive {
        rect: Rect {
            x: 0.0,
            y: 0.0,
            width: w,
            height: h,
        },
        color: Color::rgba(250, 250, 250, 255),
    });
    scene.rect(RectPrimitive {
        rect: r(0.0, 0.0, 260.0, h / s),
        color: Color::rgba(236, 236, 240, 255),
    });
    let layout = text
        .layout(&TextParams::new(
            "Sidebar row with a fairly ordinary title",
            TextStyle::new(13.0 * s),
        ))
        .expect("layout");
    let layout = ShapedText::new(Arc::new(layout));
    for row in 0..40 {
        let y = 8.0 + row as f32 * 28.0;
        if row % 7 == 3 {
            scene.rounded_rect(RoundedRectPrimitive::uniform(
                r(8.0, y, 244.0, 26.0),
                6.0 * s,
                Color::rgba(0, 0, 0, 20),
            ));
        }
        scene.push(Primitive::TextRun(TextPrimitive {
            rect: r(16.0, y + 5.0, 230.0, 18.0),
            layout: layout.clone(),
            color: Color::rgba(30, 30, 35, 255),
        }));
    }
    for card in 0..12 {
        let (x, y) = (
            300.0 + (card % 3) as f32 * 300.0,
            40.0 + (card / 3) as f32 * 180.0,
        );
        scene.shadow(crate::scene::ShadowPrimitive {
            rect: r(x, y, 280.0, 160.0),
            blur_radius: 12.0 * s,
            corner_radius: 10.0 * s,
            offset: [0.0, 2.0 * s],
            color: Color::rgba(0, 0, 0, 40),
        });
        scene.rounded_rect(RoundedRectPrimitive::uniform(
            r(x, y, 280.0, 160.0),
            10.0 * s,
            Color::rgba(255, 255, 255, 255),
        ));
        scene.border(BorderPrimitive::uniform(
            r(x, y, 280.0, 160.0),
            s,
            10.0 * s,
            Color::rgba(0, 0, 0, 30),
        ));
        for line in 0..4 {
            scene.push(Primitive::TextRun(TextPrimitive {
                rect: r(x + 12.0, y + 12.0 + line as f32 * 22.0, 256.0, 18.0),
                layout: layout.clone(),
                color: Color::rgba(40, 40, 50, 255),
            }));
        }
    }
    let pixels: Arc<[u8]> = (0..64 * 64)
        .flat_map(|i| [(i % 64 * 4) as u8, (i / 64 * 4) as u8, 128, 255])
        .collect();
    scene.image(ImagePrimitive {
        rect: r(300.0, 760.0, 128.0, 128.0),
        width: 64,
        height: 64,
        rgba: pixels,
        cache_key: 0xfeed,
    });
    scene.rect(RectPrimitive {
        rect: Rect {
            x: 0.0,
            y: 0.0,
            width: w,
            height: h,
        },
        color: Color::rgba(0, 0, 0, 90),
    });
    scene.push_layer(0.9, Transform2D::IDENTITY);
    scene.blur_region(BlurRegionPrimitive {
        rect: r(400.0, 200.0, 480.0, 320.0),
        blur_radius: 24.0 * s,
        corner_radii: [12.0 * s; 4],
    });
    scene.rounded_rect(RoundedRectPrimitive::uniform(
        r(400.0, 200.0, 480.0, 320.0),
        12.0 * s,
        Color::rgba(255, 255, 255, 200),
    ));
    scene.push(Primitive::TextRun(TextPrimitive {
        rect: r(420.0, 220.0, 440.0, 18.0),
        layout,
        color: Color::rgba(20, 20, 20, 255),
    }));
    scene.pop_layer();
    scene
}

fn report(label: &str, configure: impl Fn(&mut Renderer)) {
    let gpu = GpuContext::headless().expect("gpu");
    let mut text = crate::text::test_text();
    for s in [1.0f32, 2.0] {
        let (w, h) = ((1280.0 * s) as u32, (800.0 * s) as u32);
        let mut renderer = Renderer::headless_with_gpu(&gpu, w, h, f64::from(s));
        configure(&mut renderer);
        let scene = scene(&mut text, s, w as f32, h as f32);
        let mut times = Vec::new();
        let mut pool_bytes = 0u64;
        for frame in 0..25 {
            let start = Instant::now();
            renderer
                .render_to_rgba(&scene, &mut text, w, h)
                .expect("render");
            if frame >= 5 {
                times.push(start.elapsed().as_secs_f64() * 1000.0);
            }
            pool_bytes = pool_bytes.max(
                renderer
                    .texture_pool
                    .textures
                    .iter()
                    .map(|t| u64::from(t.width) * u64::from(t.height) * 4)
                    .sum(),
            );
        }
        times.sort_by(f64::total_cmp);
        let target = u64::from(w) * u64::from(h) * 4;
        println!(
            "PERF {label} {s}x {w}x{h}: median {:.2} ms, p90 {:.2} ms; target {:.1} MiB, pooled {:.1} MiB",
            times[times.len() / 2],
            times[times.len() * 9 / 10],
            target as f64 / 1048576.0,
            pool_bytes as f64 / 1048576.0,
        );
    }
}

#[test]
#[ignore = "benchmark; run with --ignored --nocapture"]
fn perf_compositing_modes() {
    report("linear", |_| {});
    let web = RendererOptions {
        compositing: UiCompositing::WebCompatible,
        ..RendererOptions::default()
    };
    report("web", |r| r.set_options(web));
    report("web-converted", |r| {
        r.set_options(web);
        r.direct_encoded = false;
    });
}
