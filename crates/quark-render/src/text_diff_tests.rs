//! Differential evidence for text engine migrations: a fixed matrix of
//! text scenes (scales, compositing modes, coverage policies, fills,
//! terminal-style text, SVG text, layers, clips, scrolling, atlas
//! pressure, and font system changes), rendered frame by frame. Ignored:
//! with `QUARK_TEXT_DIFF_DIR` set it writes every frame's RGBA bytes
//! there, so two builds of the renderer can be compared byte for byte
//! (`cmp` the directories). Nothing here is a checked-in golden.

use std::sync::Arc;

use super::*;
use crate::scene::{
    IconPrimitive, RectPrimitive, RichTextPrimitive, ShapedText, ShimmerSpec, StyledTextPrimitive,
    TextBackdrop, TextFill, TextGradient, TextPrimitive,
};
use crate::text::{TextPath, test_text};
use quark::scene::{FontStyle, FontWeight};
use quark::{Color, FontKind};
use quark_text::{FontSettings, TextParams, TextSpan, TextStyle};

const INK: Color = Color::rgba(28, 30, 36, 255);
const PAPER: Color = Color::rgba(250, 249, 246, 255);
const TERM_BG: Color = Color::rgba(22, 24, 29, 255);

fn rect(x: f32, y: f32, width: f32, height: f32) -> Rect {
    Rect {
        x,
        y,
        width,
        height,
    }
}

/// Which text system a frame draws with.
#[derive(Clone, Copy, PartialEq)]
enum Fonts {
    /// The shared vendored-only default.
    Shared,
    /// Source Sans 3 and JetBrains Mono, a second system.
    Other,
}

struct Case {
    name: String,
    size: (u32, u32),
    options: RendererOptions,
    path: TextPath,
    /// Cap on the device's texture size, to fill the atlas.
    texture_limit: Option<u32>,
    frames: Vec<(Scene, Fonts)>,
}

/// Shapes text for the scenes at a device scale: layouts are shaped at
/// physical size and placed in physical pixels, as the app hands them over.
struct Shaper<'a> {
    text: &'a mut TextSystem,
    scale: f32,
}

impl Shaper<'_> {
    fn layout(&mut self, s: &str, style: TextStyle, wrap: Option<f32>) -> ShapedText {
        self.layout_spans(s, style, wrap, Vec::new())
    }

    fn layout_spans(
        &mut self,
        s: &str,
        style: TextStyle,
        wrap: Option<f32>,
        spans: Vec<TextSpan>,
    ) -> ShapedText {
        let params = TextParams::new(s, style)
            .spans(spans)
            .wrap_width(wrap)
            .scale_factor(self.scale);
        ShapedText::new(Arc::new(self.text.layout(&params).expect("layout")))
    }

    /// `r` in logical pixels, to physical.
    fn r(&self, x: f32, y: f32, w: f32, h: f32) -> Rect {
        let s = self.scale;
        rect(x * s, y * s, w * s, h * s)
    }
}

/// Paragraphs exercising shaping and raster features: wrapping, bidi,
/// combining marks, ligatures, weights (static and variable), synthetic
/// italic and bold, color emoji with presentation selectors, and rich
/// spans.
fn prose(shaper: &mut Shaper, scene: &mut Scene, origin: (f32, f32), width: f32) {
    let (x, y) = origin;
    scene.rect(RectPrimitive {
        rect: shaper.r(0.0, 0.0, 10_000.0, 10_000.0),
        color: PAPER,
    });
    let wrapped = shaper.layout(
        "Office efficiency: fi fl ffi -> => != a\u{301}e\u{308} \u{5e9}\u{5dc}\u{5d5}\u{5dd} wraps \
         across several lines of ordinary interface text.",
        TextStyle::new(13.0),
        Some(width),
    );
    scene.text(TextPrimitive {
        rect: shaper.r(x, y, width, 60.0),
        layout: wrapped,
        color: INK,
    });
    let text = "Bold Light 350 Italic thick emoji \u{2764}\u{fe0f} \u{2764}\u{fe0e} \u{1f600}";
    let mut spans = Vec::new();
    for (word, weight, style) in [
        ("Bold", Some(FontWeight::Bold), None),
        ("Light", Some(FontWeight::Light), None),
        ("350", Some(FontWeight::Numeric(350)), None),
        ("Italic", None, Some(FontStyle::Italic)),
    ] {
        let start = text.find(word).expect("word");
        spans.push(TextSpan {
            range: start..start + word.len(),
            weight,
            style,
            kind: None,
            size: None,
            letter_spacing: None,
        });
    }
    let styled = shaper.layout_spans(text, TextStyle::new(15.0), None, spans.clone());
    scene.rich_text(RichTextPrimitive {
        rect: shaper.r(x, y + 64.0, width, 24.0),
        layout: styled,
        default_color: INK,
        span_colors: Arc::from([Color::rgba(200, 30, 40, 255), Color::rgba(20, 120, 60, 255)]),
    });
    let thick = shaper.layout_spans(text, TextStyle::new(11.0).thicken(true), None, spans);
    scene.text(TextPrimitive {
        rect: shaper.r(x, y + 90.0, width, 18.0),
        layout: thick,
        color: INK,
    });
    let inter = shaper.layout(
        "Inter variable 450 and 650",
        TextStyle::new(14.0)
            .family(Some("Inter"))
            .weight(FontWeight::Numeric(650)),
        None,
    );
    scene.text(TextPrimitive {
        rect: shaper.r(x, y + 110.0, width, 20.0),
        layout: inter,
        color: INK,
    });
}

/// Terminal-style text: monospace, thickened, linearly corrected over its
/// cell background, letter spaced, inside a linear compositing island,
/// with block cells drawn as quads as the terminal draws its sprites.
fn terminal(shaper: &mut Shaper, scene: &mut Scene, origin: (f32, f32)) {
    let (x, y) = origin;
    let bounds = shaper.r(x, y, 220.0, 70.0);
    scene.push_compositing_island(bounds, UiCompositing::Linear);
    scene.rect(RectPrimitive {
        rect: bounds,
        color: TERM_BG,
    });
    let style = TextStyle::new(12.0)
        .kind(FontKind::Mono)
        .thicken(true)
        .linear_correction(Some(24))
        .letter_spacing(0.02);
    for (row, (line, color)) in [
        (
            "$ cargo test -p quark-render",
            Color::rgba(220, 220, 220, 255),
        ),
        (
            "   Compiling quark v0.1.0 \u{2713}",
            Color::rgba(80, 200, 120, 255),
        ),
        (
            "error[E0308]: mismatched types",
            Color::rgba(240, 90, 90, 255),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let layout = shaper.layout(line, style, None);
        scene.text(TextPrimitive {
            rect: shaper.r(x + 4.0, y + 4.0 + row as f32 * 16.0, 212.0, 16.0),
            layout,
            color,
        });
    }
    for cell in 0..6 {
        scene.rect(RectPrimitive {
            rect: shaper.r(x + 4.0 + cell as f32 * 7.25, y + 54.0, 7.25, 14.0),
            color: Color::rgba(200, 200, 200, 255),
        });
    }
    scene.pop_isolate();
}

/// Styled text: a gradient, a shimmer at a fixed phase, and perceptual
/// coverage over known and unknown backdrops.
fn styled(shaper: &mut Shaper, scene: &mut Scene, origin: (f32, f32), phase: f32) {
    let (x, y) = origin;
    let s = shaper.scale;
    let layout = shaper.layout("Shimmering status text", TextStyle::new(16.0), None);
    let shimmer = ShimmerSpec::new(Color::rgba(90, 90, 100, 255), Color::rgba(250, 60, 60, 255))
        .band_width(40.0)
        .phase(phase);
    scene.styled_text(StyledTextPrimitive::new(
        shaper.r(x, y, 200.0, 22.0),
        layout.clone(),
        TextFill::Shimmer(shimmer).scaled(s),
    ));
    let gradient = TextGradient {
        start: [0.0, 0.0],
        end: [180.0, 0.0],
        from: Color::rgba(255, 0, 80, 255),
        to: Color::rgba(0, 80, 255, 255),
    };
    scene.styled_text(StyledTextPrimitive::new(
        shaper.r(x, y + 22.0, 200.0, 22.0),
        layout.clone(),
        TextFill::LinearGradient(gradient).scaled(s),
    ));
    scene.rect(RectPrimitive {
        rect: shaper.r(x, y + 44.0, 200.0, 44.0),
        color: Color::rgba(30, 32, 40, 255),
    });
    scene.styled_text(
        StyledTextPrimitive::new(
            shaper.r(x + 2.0, y + 44.0, 200.0, 22.0),
            layout.clone(),
            TextFill::Solid(Color::rgba(235, 235, 240, 255)),
        )
        .backdrop(TextBackdrop::Opaque(Color::rgba(30, 32, 40, 255))),
    );
    scene.styled_text(StyledTextPrimitive::new(
        shaper.r(x + 2.0, y + 66.0, 200.0, 22.0),
        layout,
        TextFill::Solid(Color::rgba(235, 235, 240, 255)),
    ));
}

/// Fractional and negative origins, a clip cutting through glyphs,
/// overlapping translucent text, text inside a faded layer between
/// quads, and an SVG badge with text.
fn placement(shaper: &mut Shaper, scene: &mut Scene, origin: (f32, f32)) {
    let (x, y) = origin;
    let layout = shaper.layout("Clipped glyphs gjpqy", TextStyle::new(18.0), None);
    scene.text(TextPrimitive {
        rect: shaper.r(-3.3, y - 5.7, 200.0, 24.0),
        layout: layout.clone(),
        color: INK,
    });
    scene.clip(shaper.r(x + 10.0, y + 22.0, 90.0, 13.0));
    scene.text(TextPrimitive {
        rect: shaper.r(x + 0.4, y + 20.6, 200.0, 24.0),
        layout: layout.clone(),
        color: Color::rgba(0, 90, 200, 255),
    });
    scene.pop_clip();
    for (i, color) in [
        Color::rgba(220, 0, 0, 160),
        Color::rgba(0, 160, 0, 160),
        Color::rgba(0, 0, 220, 160),
    ]
    .into_iter()
    .enumerate()
    {
        scene.text(TextPrimitive {
            rect: shaper.r(x + 1.25 * i as f32, y + 40.0 + 0.5 * i as f32, 200.0, 24.0),
            layout: layout.clone(),
            color,
        });
    }
    scene.push_layer(0.6, Transform2D::IDENTITY);
    scene.rect(RectPrimitive {
        rect: shaper.r(x, y + 66.0, 120.0, 20.0),
        color: Color::rgba(255, 220, 120, 255),
    });
    scene.text(TextPrimitive {
        rect: shaper.r(x + 2.0, y + 66.0, 200.0, 24.0),
        layout,
        color: INK,
    });
    scene.rect(RectPrimitive {
        rect: shaper.r(x + 60.0, y + 74.0, 20.0, 4.0),
        color: Color::rgba(0, 0, 0, 255),
    });
    scene.pop_layer();
    scene.push(Primitive::Icon(IconPrimitive {
        rect: shaper.r(x + 130.0, y + 66.0, 32.0, 16.0),
        name: r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 16"><rect x="0.5" y="0.5" width="31" height="15" rx="4" fill="none" stroke="currentColor"/><text x="6" y="12.5" font-size="11" fill="currentColor">AB</text></svg>"#.to_owned(),
        color: INK,
    }));
}

fn full_scene(shaper: &mut Shaper, phase: f32) -> Scene {
    let mut scene = Scene::default();
    prose(shaper, &mut scene, (6.0, 6.0), 230.0);
    terminal(shaper, &mut scene, (250.0, 6.0));
    styled(shaper, &mut scene, (250.0, 84.0), phase);
    placement(shaper, &mut scene, (6.0, 140.0));
    scene
}

/// A list of rows, scrolled by `offset` logical pixels.
fn list(shaper: &mut Shaper, offset: f32) -> Scene {
    let mut scene = Scene::default();
    scene.rect(RectPrimitive {
        rect: shaper.r(0.0, 0.0, 10_000.0, 10_000.0),
        color: PAPER,
    });
    scene.clip(shaper.r(0.0, 10.0, 300.0, 200.0));
    for row in 0..24 {
        let layout = shaper.layout(
            &format!("Row {row}: a sidebar title with some detail"),
            TextStyle::new(13.0),
            None,
        );
        scene.text(TextPrimitive {
            rect: shaper.r(8.0, 12.0 + row as f32 * 20.0 - offset, 280.0, 18.0),
            layout,
            color: INK,
        });
    }
    scene.pop_clip();
    scene
}

fn cases(other: &mut TextSystem) -> Vec<Case> {
    let mut cases = Vec::new();
    let mut text = test_text();
    let modes = [
        (
            "linear-perceptual",
            RendererOptions {
                compositing: UiCompositing::Linear,
                text_rendering: TextRendering::Perceptual,
            },
        ),
        (
            "linear-linear",
            RendererOptions {
                compositing: UiCompositing::Linear,
                text_rendering: TextRendering::Linear,
            },
        ),
        (
            "web",
            RendererOptions {
                compositing: UiCompositing::WebCompatible,
                text_rendering: TextRendering::Perceptual,
            },
        ),
    ];
    for scale in [1.0f32, 1.25, 1.5, 2.0] {
        let size = ((480.0 * scale) as u32, (240.0 * scale) as u32);
        for (mode, options) in modes {
            for path in [TextPath::Positioned, TextPath::Buffer] {
                if path == TextPath::Buffer && (mode != "linear-perceptual" || scale > 1.5) {
                    continue;
                }
                let mut shaper = Shaper {
                    text: &mut text,
                    scale,
                };
                // Kept runs, then a shimmer phase change.
                let frames = vec![
                    (full_scene(&mut shaper, 0.25), Fonts::Shared),
                    (full_scene(&mut shaper, 0.25), Fonts::Shared),
                    (full_scene(&mut shaper, 0.6), Fonts::Shared),
                ];
                cases.push(Case {
                    name: format!("full-{scale}x-{mode}-{path:?}"),
                    size,
                    options,
                    path,
                    texture_limit: None,
                    frames,
                });
            }
        }
        // Whole-pixel and fractional scrolls of kept rows.
        let mut shaper = Shaper {
            text: &mut text,
            scale,
        };
        let frames = [0.0, 20.0, 23.0, 23.5, 3.0]
            .map(|offset| (list(&mut shaper, offset / scale), Fonts::Shared))
            .into();
        cases.push(Case {
            name: format!("scroll-{scale}x"),
            size: ((320.0 * scale) as u32, (220.0 * scale) as u32),
            options: RendererOptions::default(),
            path: TextPath::Positioned,
            texture_limit: None,
            frames,
        });
    }

    // Atlas growth and eviction on a 256 px device: kept text, then
    // large glyphs that fill the atlas, then the kept text again.
    let mut shaper = Shaper {
        text: &mut text,
        scale: 1.0,
    };
    let kept = list(&mut shaper, 0.0);
    let mut crowded = kept.clone();
    let large = shaper.layout(
        "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz",
        TextStyle::new(64.0),
        Some(300.0),
    );
    crowded.text(TextPrimitive {
        rect: rect(0.0, 40.0, 300.0, 180.0),
        layout: large,
        color: Color::rgba(0, 0, 0, 255),
    });
    let huge = shaper.layout("0123456789", TextStyle::new(110.0), Some(300.0));
    let mut overflowing = kept.clone();
    overflowing.text(TextPrimitive {
        rect: rect(0.0, 0.0, 300.0, 220.0),
        layout: huge,
        color: Color::rgba(0, 0, 0, 255),
    });
    let frames = vec![
        (kept.clone(), Fonts::Shared),
        (crowded.clone(), Fonts::Shared),
        (kept.clone(), Fonts::Shared),
        (crowded, Fonts::Shared),
        (overflowing, Fonts::Shared),
        (kept, Fonts::Shared),
    ];
    cases.push(Case {
        name: "atlas-pressure".into(),
        size: (256, 220),
        options: RendererOptions::default(),
        path: TextPath::Positioned,
        texture_limit: Some(256),
        frames,
    });

    // Two text systems on one renderer, and back.
    let frames = (0..3)
        .map(|i| {
            let fonts = if i == 1 { Fonts::Other } else { Fonts::Shared };
            (fonts, i)
        })
        .collect::<Vec<_>>();
    let frames = frames
        .into_iter()
        .map(|(fonts, _)| {
            let system = match fonts {
                Fonts::Shared => &mut *text,
                Fonts::Other => &mut *other,
            };
            let mut shaper = Shaper {
                text: system,
                scale: 1.0,
            };
            (full_scene(&mut shaper, 0.4), fonts)
        })
        .collect();
    cases.push(Case {
        name: "two-text-systems".into(),
        size: (480, 240),
        options: RendererOptions::default(),
        path: TextPath::Positioned,
        texture_limit: None,
        frames,
    });
    cases
}

/// The second text system of the matrix.
fn other_fonts() -> TextSystem {
    TextSystem::vendored_only(&FontSettings {
        ui_family: "Source Sans 3".into(),
        mono_family: "JetBrains Mono".into(),
        ..FontSettings::default()
    })
}

/// Every frame of `case`, drawn by one renderer whose atlas has `limits`
/// (the defaults for `None`); `None` without a GPU.
fn render_case(
    case: &Case,
    other: &mut TextSystem,
    limits: Option<TextAtlasLimits>,
) -> Option<(Vec<Vec<u8>>, TextAtlasStats)> {
    let gpu = match case.texture_limit {
        Some(limit) => GpuContext::headless_with_limits(wgpu::Limits {
            max_texture_dimension_2d: limit,
            ..wgpu::Limits::default()
        }),
        None => GpuContext::headless(),
    };
    let gpu = match gpu {
        Ok(gpu) => gpu,
        Err(error) => {
            assert!(
                std::env::var_os("QUARK_REQUIRE_GPU").is_none(),
                "QUARK_REQUIRE_GPU is set but no wgpu adapter is available: {error}"
            );
            return None;
        }
    };
    let mut renderer = Renderer::headless_with_gpu(&gpu, case.size.0, case.size.1, 1.0);
    renderer.set_options(case.options);
    renderer.text_path = case.path;
    if let Some(limits) = limits {
        renderer.set_text_atlas_limits(limits);
    }
    let frames = case
        .frames
        .iter()
        .map(|(scene, fonts)| {
            let mut shared = test_text();
            let system = match fonts {
                Fonts::Shared => &mut *shared,
                Fonts::Other => &mut *other,
            };
            renderer
                .render_to_rgba(scene, system, case.size.0, case.size.1)
                .expect("render")
        })
        .collect();
    Some((frames, renderer.text_atlas_stats()))
}

// An atlas far too small for the matrix's frames (pages of 128 pixels, no
// room for a resident color page, a few mask pages) draws every frame
// exactly as a roomy one: glyphs spread over many pages and dedicated
// textures, evicted while unpinned, and drawn in overflow mode ordinal by
// ordinal in scene order, across layers, islands, clips, and scrolls.
#[test]
fn a_tiny_atlas_draws_the_pixels_of_a_roomy_one() {
    let side = 128u64;
    let tiny = TextAtlasLimits {
        page_size: side as u32,
        target_bytes: side * side,
        // One color and one mask overflow page in reserve, two mask pages.
        hard_limit_bytes: side * side * (4 + 1 + 2),
    };
    let mut other = other_fonts();
    let cases = cases(&mut other);
    let mut work = TextAtlasStats::default();
    for case in cases.iter().filter(|c| !c.name.contains("1.25x")) {
        let Some((roomy, _)) = render_case(case, &mut other, None) else {
            return;
        };
        let (tight, stats) = render_case(case, &mut other, Some(tiny)).expect("renderer");
        work = work + stats;
        for (i, (a, b)) in roomy.iter().zip(&tight).enumerate() {
            let differing = a
                .as_chunks::<4>()
                .0
                .iter()
                .zip(b.as_chunks::<4>().0)
                .filter(|(p, q)| p != q)
                .count();
            assert_eq!(differing, 0, "{} frame {i}: pixels differ", case.name);
        }
    }
    eprintln!("{work:?}");
    // The atlas did the work the test is for.
    assert!(work.overflow_segments > work.overflow_frames, "{work:?}");
    assert!(work.evictions > 0 && work.pages_released > 0, "{work:?}");
}

#[test]
#[ignore = "migration evidence; writes frames to QUARK_TEXT_DIFF_DIR"]
fn dump_text_matrix() {
    let Some(dir) = std::env::var_os("QUARK_TEXT_DIFF_DIR") else {
        eprintln!("QUARK_TEXT_DIFF_DIR is not set; nothing written");
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir).expect("output directory");
    let mut other = other_fonts();
    let cases = cases(&mut other);
    let mut written = 0;
    for case in &cases {
        let Some(frames) = render_case(case, &mut other, None) else {
            return;
        };
        for (i, pixels) in frames.0.iter().enumerate() {
            std::fs::write(dir.join(format!("{}-{i}.rgba", case.name)), pixels)
                .expect("write frame");
            written += 1;
        }
    }
    eprintln!("wrote {written} frames to {}", dir.display());
}
