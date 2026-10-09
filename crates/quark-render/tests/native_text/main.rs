//! Text rendering comparison and measurement harness (text-render design,
//! sections 8.2 and 8.3).
//!
//! Deterministic fixtures (UI strings, code, CJK, color emoji, variable and
//! static weights) render offscreen at scales 1, 1.5, and 2 through each
//! rasterizer the platform offers. Per fixture, scale, and rasterizer the
//! report records ink coverage of the frame, glyphs rasterized and bitmap
//! bytes uploaded, atlas textures and bytes, allocations of the first and
//! of repeated frames, and frame times. Every frame is also rendered
//! without its text, by a second renderer, so the text's share of frame
//! time and allocations can be read off.
//!
//! Frame times are wall time of `render_to_rgba`, readback included, on
//! whatever adapter the machine has: a software adapter's numbers are CPU
//! diagnostics, not GPU performance. Allocations are this thread's, from
//! `quark_ui::test_alloc`; wgpu's worker threads are not counted.
//!
//! The report test is ignored; `tools/text-render/report.sh` runs it and
//! documents its environment variables.

#![cfg(feature = "headless-render")]

mod fixtures;
mod measure;
mod report;

use std::path::PathBuf;
use std::time::Instant;

use quark_render::{GpuContext, Renderer};
use quark_text::TextSystem;
use quark_ui::test_alloc::{self, Counting};

use fixtures::{Fixture, SCALES};
use report::Row;

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// The rasterizers this platform renders with: swash alone until the
/// renderer can select another.
const BACKENDS: &[&str] = &["swash"];

/// Frames drawn after the first before measuring repeated frames.
const WARMUP: usize = 5;

fn gpu() -> Option<GpuContext> {
    match GpuContext::headless() {
        Ok(gpu) => Some(gpu),
        Err(error) if std::env::var("QUARK_REQUIRE_GPU").as_deref() == Ok("1") => {
            panic!("no GPU device: {error}")
        }
        Err(_) => None,
    }
}

/// The adapter the headless context picks, asked the same way.
fn adapter_description() -> String {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::from_env_or_default());
    let adapter = pollster::block_on(async {
        match instance
            .request_adapter(&wgpu::RequestAdapterOptions::default())
            .await
        {
            Ok(adapter) => Some(adapter),
            Err(_) => instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    force_fallback_adapter: true,
                    ..Default::default()
                })
                .await
                .ok(),
        }
    });
    adapter.map_or_else(
        || "none".to_owned(),
        |adapter| {
            let info = adapter.get_info();
            format!(
                "{} ({:?}, {:?}, {} {})",
                info.name, info.backend, info.device_type, info.driver, info.driver_info
            )
        },
    )
}

/// Render one fixture at one scale: a first frame, warmup, then `frames`
/// repeated frames alternating with the control scene. Returns the row
/// and the first frame's pixels.
fn measure(
    gpu: &GpuContext,
    text: &mut TextSystem,
    fixture: &Fixture,
    scale: f32,
    backend: &'static str,
    frames: usize,
) -> (Row, Vec<u8>) {
    let prepared = fixture.prepare(text, scale);
    let (w, h) = (prepared.width, prepared.height);
    let mut renderer = Renderer::headless_with_gpu(gpu, w, h, f64::from(scale));
    let mut control = Renderer::headless_with_gpu(gpu, w, h, f64::from(scale));
    let frame = |renderer: &mut Renderer, scene, text: &mut TextSystem| {
        let start = Instant::now();
        let (pixels, allocs) = test_alloc::count(|| {
            renderer
                .render_to_rgba(scene, text, w, h)
                .expect("render fixture")
        });
        (pixels, allocs, start.elapsed().as_secs_f64() * 1000.0)
    };

    let (pixels, cold_allocs, cold_ms) = frame(&mut renderer, &prepared.scene, text);
    let cold = renderer.text_atlas_stats();
    frame(&mut control, &prepared.control, text);
    for _ in 0..WARMUP {
        frame(&mut renderer, &prepared.scene, text);
        frame(&mut control, &prepared.control, text);
    }
    let (mut warm_ms, mut control_ms) = (Vec::new(), Vec::new());
    let (mut warm_allocs, mut control_allocs) = (Vec::new(), Vec::new());
    for _ in 0..frames {
        let (_, allocs, ms) = frame(&mut renderer, &prepared.scene, text);
        warm_allocs.push(allocs);
        warm_ms.push(ms);
        let (_, allocs, ms) = frame(&mut control, &prepared.control, text);
        control_allocs.push(allocs);
        control_ms.push(ms);
    }
    let end = renderer.text_atlas_stats();
    let (atlas_textures, atlas_bytes) = measure::atlas_memory(end, fixture.content);
    let row = Row {
        fixture: fixture.name,
        scale,
        backend,
        width: w,
        height: h,
        glyphs: prepared.glyphs,
        cold_misses: cold.misses,
        cold_upload_bytes: cold.upload_bytes,
        cold_allocs,
        cold_ms,
        warm_misses: end.misses - cold.misses,
        warm_upload_bytes: end.upload_bytes - cold.upload_bytes,
        warm_allocs: measure::median(&mut warm_allocs),
        control_allocs: measure::median(&mut control_allocs),
        warm: measure::summarize(&mut warm_ms),
        control: measure::summarize(&mut control_ms),
        evictions: end.evictions,
        growths: end.growths,
        atlas_textures,
        atlas_bytes,
        ink: measure::ink(&pixels, w),
    };
    (row, pixels)
}

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).map(PathBuf::from)
}

#[test]
#[ignore = "report; run through tools/text-render/report.sh"]
fn report_text_render() {
    let Some(gpu) = gpu() else {
        eprintln!("no GPU device; no report");
        return;
    };
    let frames = std::env::var("QUARK_TEXT_FRAMES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(100);
    let out_dir = env_path("QUARK_TEXT_REPORT_DIR");
    if let Some(dir) = &out_dir {
        std::fs::create_dir_all(dir).expect("create report directory");
    }
    let mut text = fixtures::text_system();
    let mut rows = Vec::new();
    for fixture in fixtures::fixtures() {
        for scale in SCALES {
            for &backend in BACKENDS {
                let (row, pixels) = measure(&gpu, &mut text, &fixture, scale, backend, frames);
                if let Some(dir) = &out_dir {
                    image::RgbaImage::from_raw(row.width, row.height, pixels)
                        .expect("frame matches its size")
                        .save(dir.join(report::image_name(&row.key())))
                        .expect("save frame");
                }
                rows.push(row);
            }
        }
    }
    let header = [
        (
            "revision",
            std::env::var("QUARK_TEXT_REVISION").unwrap_or_else(|_| "unknown".to_owned()),
        ),
        ("adapter", adapter_description()),
        (
            "build",
            format!(
                "{} {}-{}",
                if cfg!(debug_assertions) {
                    "debug"
                } else {
                    "release"
                },
                std::env::consts::OS,
                std::env::consts::ARCH
            ),
        ),
        ("frames", frames.to_string()),
    ];
    let report = report::render(&header, &rows);
    println!("{report}");
    if let Some(dir) = &out_dir {
        std::fs::write(dir.join("report.tsv"), &report).expect("write report");
    }

    let Some(baseline_dir) = env_path("QUARK_TEXT_BASELINE_DIR") else {
        return;
    };
    let baseline =
        std::fs::read_to_string(baseline_dir.join("report.tsv")).expect("read the baseline report");
    let candidate_dir = out_dir.unwrap_or_default();
    let (lines, failures) = report::compare(
        &report::parse(&baseline),
        &report::parse(&report),
        &baseline_dir,
        &candidate_dir,
    );
    println!("comparison with {}:", baseline_dir.display());
    for line in &lines {
        println!("  {line}");
    }
    for failure in &failures {
        println!("  GATE {failure}");
    }
    if std::env::var("QUARK_TEXT_GATE").as_deref() == Ok("1") {
        assert!(failures.is_empty(), "{} gate failures", failures.len());
    }
}

// Once a glyph is in the atlas, drawing the same frame again rasterizes
// and uploads nothing, for every content kind and scale (design 8.3,
// "warm identical scene").
#[test]
fn a_repeated_frame_rasterizes_and_uploads_no_glyph() {
    let Some(gpu) = gpu() else {
        return;
    };
    let mut text = fixtures::text_system();
    for fixture in fixtures::fixtures() {
        for scale in SCALES {
            let prepared = fixture.prepare(&mut text, scale);
            let (w, h) = (prepared.width, prepared.height);
            let mut renderer = Renderer::headless_with_gpu(&gpu, w, h, f64::from(scale));
            let mut draw = |text: &mut TextSystem| {
                renderer
                    .render_to_rgba(&prepared.scene, text, w, h)
                    .expect("render fixture");
                renderer.text_atlas_stats()
            };
            let first = draw(&mut text);
            let again = draw(&mut text);
            assert!(first.misses > 0, "{}@{scale} drew no glyph", fixture.name);
            assert_eq!(
                (again.misses, again.upload_bytes),
                (first.misses, first.upload_bytes),
                "{}@{scale}",
                fixture.name
            );
        }
    }
}
