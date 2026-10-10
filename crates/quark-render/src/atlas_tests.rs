//! The paged glyph atlas through the renderer: residency under pressure,
//! uploads, oversized glyphs, font replacement, and evacuation. Pixels are
//! compared with a fresh renderer drawing the same frame.

use std::sync::Arc;

use super::*;
use crate::scene::{RectPrimitive, ShapedText, TextPrimitive};
use crate::text::test_text;
use quark::{Color, FontKind};
use quark_text::{FontSettings, TextParams, TextStyle};

const SIZE: (u32, u32) = (240, 160);
const INK: Color = Color::rgba(20, 20, 30, 255);

fn rect(x: f32, y: f32, width: f32, height: f32) -> Rect {
    Rect {
        x,
        y,
        width,
        height,
    }
}

fn renderer(limits: Option<TextAtlasLimits>) -> Option<Renderer> {
    match Renderer::new_headless(SIZE.0, SIZE.1, 1.0) {
        Ok(mut renderer) => {
            if let Some(limits) = limits {
                renderer.set_text_atlas_limits(limits);
            }
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

fn text(system: &mut TextSystem, s: &str, style: TextStyle, at: (f32, f32)) -> Primitive {
    let layout = system.layout(&TextParams::new(s, style)).expect("layout");
    Primitive::TextRun(TextPrimitive {
        // Tall enough for any size here: the renderer culls a text whose
        // rect is off the target.
        rect: rect(at.0, at.1, SIZE.0 as f32, 600.0),
        layout: ShapedText::new(Arc::new(layout)),
        color: INK,
    })
}

fn scene(primitives: impl IntoIterator<Item = Primitive>) -> Scene {
    let mut scene = Scene::default();
    scene.rect(RectPrimitive {
        rect: rect(0.0, 0.0, SIZE.0 as f32, SIZE.1 as f32),
        color: Color::rgba(255, 255, 255, 255),
    });
    for primitive in primitives {
        scene.push(primitive);
    }
    scene
}

fn draw(renderer: &mut Renderer, scene: &Scene, system: &mut TextSystem) -> Vec<u8> {
    renderer
        .render_to_rgba(scene, system, SIZE.0, SIZE.1)
        .expect("render")
}

fn fresh(scene: &Scene, system: &mut TextSystem) -> Vec<u8> {
    draw(&mut renderer(None).expect("renderer"), scene, system)
}

/// Distinct printable characters `from..to` as one string.
fn chars(from: u8, to: u8) -> String {
    (from..to).map(char::from).collect()
}

/// Pages of 128 pixels with room for one resident mask page.
fn one_mask_page() -> TextAtlasLimits {
    let side = 128u64;
    TextAtlasLimits {
        page_size: side as u32,
        target_bytes: side * side,
        hard_limit_bytes: side * side * (1 + 4 + 1),
    }
}

// Regression guard for pin-before-miss: a run kept from the last frame (a
// recorded chunk's) draws its vertices again, so the glyphs they point at
// must survive the misses of the frame. Unpinned, they would be the least
// recently used (nothing prepared them since the first frame), and the new
// text's evictions would place other glyphs where they were.
#[test]
fn a_kept_runs_glyphs_survive_the_misses_of_its_frame() {
    let Some(mut renderer) = renderer(Some(one_mask_page())) else {
        return;
    };
    let mut system = test_text();
    // Text of a chunk draws as a run of its own.
    let chunk = {
        let mut chunk = quark::scene::SceneChunk::new();
        chunk.replace(vec![text(
            &mut system,
            "Kept",
            TextStyle::new(20.0),
            (4.0, 4.0),
        )]);
        Arc::new(chunk)
    };
    let kept = |_: &mut TextSystem| {
        let mut scene = Scene::default();
        scene.chunk(&chunk, [0.0, 0.0]);
        scene.primitives
    };
    let mono = TextStyle::new(20.0).kind(FontKind::Mono);
    let frames = [
        scene(kept(&mut system)),
        scene(kept(&mut system).into_iter().chain([text(
            &mut system,
            &chars(b'a', b'z'),
            mono,
            (4.0, 40.0),
        )])),
        scene(kept(&mut system).into_iter().chain([text(
            &mut system,
            &chars(b'A', b'Z'),
            mono,
            (4.0, 70.0),
        )])),
    ];
    for (i, frame) in frames.iter().enumerate() {
        let drawn = draw(&mut renderer, frame, &mut system);
        assert!(
            drawn == fresh(frame, &mut system),
            "frame {i}: kept text drew other glyphs"
        );
    }
    let stats = renderer.text_atlas_stats();
    assert!(stats.evictions > 0, "nothing evicted");
    assert_eq!(stats.overflow_frames, 0, "a frame overflowed");
}

// A frame drawn again, and then scrolled by whole pixels, rasterizes and
// uploads nothing: every glyph stays resident in its page.
#[test]
fn repeated_and_scrolled_frames_upload_no_glyphs() {
    let Some(mut renderer) = renderer(None) else {
        return;
    };
    let mut system = test_text();
    let at = |system: &mut TextSystem, y: f32| {
        let mut scene = scene([]);
        scene.clip(rect(0.0, 0.0, SIZE.0 as f32, SIZE.1 as f32));
        for row in 0..8 {
            scene.push(text(
                system,
                &format!("Row {row} of a list \u{2764}\u{fe0f}"),
                TextStyle::new(14.0),
                (4.0, y + row as f32 * 20.0),
            ));
        }
        scene.pop_clip();
        scene
    };
    draw(&mut renderer, &at(&mut system, 0.0), &mut system);
    let warm = renderer.text_atlas_stats();
    for y in [0.0, -7.0, -20.0] {
        draw(&mut renderer, &at(&mut system, y), &mut system);
    }
    let after = renderer.text_atlas_stats();
    assert_eq!(after.misses, warm.misses, "rasterized again");
    assert_eq!(after.upload_bytes, warm.upload_bytes, "uploaded again");
    assert_eq!(after.copy_commands, warm.copy_commands);
    assert_eq!(after.transfer_chunks, warm.transfer_chunks);
}

// A glyph larger than a page gets a texture of its own and draws as in a
// roomy atlas; one larger than the device allows is dropped alone, and the
// rest of the frame's text still draws.
#[test]
fn oversized_glyphs_draw_alone_or_drop_alone() {
    let mut system = test_text();
    let big = scene([
        text(&mut system, "W", TextStyle::new(110.0), (4.0, 0.0)),
        text(&mut system, "small", TextStyle::new(14.0), (140.0, 10.0)),
    ]);
    let Some(mut paged) = renderer(Some(TextAtlasLimits {
        page_size: 64,
        ..TextAtlasLimits::default()
    })) else {
        return;
    };
    assert!(draw(&mut paged, &big, &mut system) == fresh(&big, &mut system));
    assert_eq!(paged.text_atlas_memory().dedicated_pages, 1);

    let gpu = GpuContext::headless_with_limits(wgpu::Limits {
        max_texture_dimension_2d: 256,
        ..wgpu::Limits::default()
    })
    .expect("capped device");
    let mut capped = Renderer::headless_with_gpu(&gpu, SIZE.0, SIZE.1, 1.0);
    let huge = scene([
        text(&mut system, "W", TextStyle::new(400.0), (4.0, -100.0)),
        text(&mut system, "small", TextStyle::new(14.0), (140.0, 10.0)),
    ]);
    let drawn = draw(&mut capped, &huge, &mut system);
    assert_eq!(capped.text_atlas_stats().dropped_glyphs, 1);
    let alone = scene([text(
        &mut system,
        "small",
        TextStyle::new(14.0),
        (140.0, 10.0),
    )]);
    assert!(
        drawn == fresh(&alone, &mut system),
        "the small text did not draw"
    );
}

// Glyphs are keyed by their exact font bytes and instance, not by the face
// ids of one font database: a replacement text system with the same fonts
// draws from the glyphs already rasterized.
#[test]
fn a_replacement_text_system_reuses_rasterized_glyphs() {
    let Some(mut renderer) = renderer(None) else {
        return;
    };
    let frame = |system: &mut TextSystem| {
        scene([text(system, "Same fonts", TextStyle::new(18.0), (4.0, 4.0))])
    };
    let mut first = TextSystem::vendored_only(&FontSettings::default());
    let expected = draw(&mut renderer, &frame(&mut first), &mut first);
    let misses = renderer.text_atlas_stats().misses;
    let mut second = TextSystem::vendored_only(&FontSettings::default());
    let drawn = draw(&mut renderer, &frame(&mut second), &mut second);
    assert_eq!(renderer.text_atlas_stats().misses, misses);
    assert!(drawn == expected);
}

// Evacuating a sparse page moves its glyphs into another page on the GPU:
// nothing is rasterized again, the page is released, and text drawn from
// the moved glyphs has the same pixels.
#[test]
fn evacuated_glyphs_draw_unchanged_without_rasterizing() {
    let Some(mut renderer) = renderer(Some(TextAtlasLimits {
        page_size: 64,
        ..TextAtlasLimits::default()
    })) else {
        return;
    };
    let mut system = test_text();
    // Monospace at a whole-pixel advance, so every glyph of a string sits
    // in the same subpixel bin wherever it starts.
    let mono = TextStyle::new(10.0).kind(FontKind::Mono);
    let filler = chars(b'!', b'~');
    let tail = &filler[filler.len() - 15..];
    let kept = |system: &mut TextSystem, y| text(system, "Kept", TextStyle::new(14.0), (4.0, y));
    let first = scene([
        kept(&mut system, 4.0),
        text(&mut system, &filler, mono, (0.0, 40.0)),
    ]);
    draw(&mut renderer, &first, &mut system);
    let pages = renderer.text_atlas_memory().mask_pages;
    assert!(pages >= 3, "the filler spans {pages} pages");
    // Only "Kept" and the filler's tail stay in use, leaving sparse pages.
    let second = scene([
        kept(&mut system, 4.0),
        text(&mut system, tail, mono, (0.0, 80.0)),
    ]);
    draw(&mut renderer, &second, &mut system);
    renderer.atlas.evict_unused();
    let pages = renderer.text_atlas_memory().mask_pages;
    renderer.atlas.request_evacuation();
    let misses = renderer.text_atlas_stats().misses;
    // Moved: drawn somewhere new, so prepared from the moved glyphs.
    let third = scene([
        kept(&mut system, 20.0),
        text(&mut system, tail, mono, (0.0, 100.0)),
    ]);
    let drawn = draw(&mut renderer, &third, &mut system);
    let stats = renderer.text_atlas_stats();
    assert_eq!(stats.evacuated_pages, 1);
    assert_eq!(stats.misses, misses, "rasterized again");
    assert_eq!(renderer.text_atlas_memory().mask_pages, pages - 1);
    assert!(
        drawn == fresh(&third, &mut system),
        "moved glyphs drew wrong"
    );
}

// Blank glyphs (spaces) are remembered without atlas space.
#[test]
fn blank_glyphs_take_no_page() {
    let Some(mut renderer) = renderer(None) else {
        return;
    };
    let mut system = test_text();
    draw(
        &mut renderer,
        &scene([text(&mut system, "    ", TextStyle::new(14.0), (4.0, 4.0))]),
        &mut system,
    );
    let memory = renderer.text_atlas_memory();
    assert!(renderer.text_atlas_stats().misses > 0);
    assert_eq!(
        (memory.mask_pages, memory.color_pages, memory.resident),
        (0, 0, 0)
    );
}

// A rasterizer the build or platform lacks is refused, and the renderer
// keeps drawing with the one it had.
#[cfg(not(target_os = "macos"))]
#[test]
fn an_unavailable_rasterizer_is_refused() {
    let Some(mut renderer) = renderer(None) else {
        return;
    };
    let wanted = TextRasterizer::CoreText(TextSmoothing::System);
    assert!(matches!(
        renderer.set_text_rasterizer(wanted),
        Err(RenderError::TextRasterizerUnavailable(r)) if r == wanted
    ));
    assert_eq!(
        renderer.text_rasterizer(),
        (TextRasterizer::Swash, TextRasterizer::Swash)
    );
}

// CoreText's system smoothing admits five coverage planes per glyph; the
// shader draws the one smoothed for the text's own gray, so white and
// black text each draw as the fixed smoothing for their gray does.
#[cfg(target_os = "macos")]
#[test]
fn system_smoothing_draws_the_plane_of_the_text_gray() {
    let mut system = test_text();
    let frame = |system: &mut TextSystem, ink: Color, paper: Color| {
        let mut scene = Scene::default();
        scene.rect(RectPrimitive {
            rect: rect(0.0, 0.0, SIZE.0 as f32, SIZE.1 as f32),
            color: paper,
        });
        let layout = system
            .layout(&TextParams::new("Smoothed llll 0123", TextStyle::new(13.0)))
            .expect("layout");
        scene.text(TextPrimitive {
            rect: rect(4.0, 4.0, SIZE.0 as f32, 40.0),
            layout: ShapedText::new(Arc::new(layout)),
            color: ink,
        });
        scene
    };
    let draw_with = |rasterizer, scene: &Scene, system: &mut TextSystem| {
        let mut renderer = renderer(None)?;
        renderer.set_text_rasterizer(rasterizer).expect("CoreText");
        Some(draw(&mut renderer, scene, system))
    };
    let white = Color::rgba(255, 255, 255, 255);
    let black = Color::rgba(0, 0, 0, 255);
    for (ink, paper, level) in [(white, black, 4), (black, white, 0)] {
        let scene = frame(&mut system, ink, paper);
        let Some(system_drawn) = draw_with(
            TextRasterizer::CoreText(TextSmoothing::System),
            &scene,
            &mut system,
        ) else {
            return;
        };
        let fixed = draw_with(
            TextRasterizer::CoreText(TextSmoothing::Fixed(level)),
            &scene,
            &mut system,
        )
        .expect("renderer");
        assert!(system_drawn == fixed, "level {level}: another plane drew");
    }
}

// A span's own size reaches its glyphs' raster keys on both text paths:
// "x" spanned at twice the size draws twice as tall as the plain "x"
// beside it, and as tall as text laid out at that size throughout.
#[test]
fn a_spans_own_size_draws_its_glyphs_at_that_size() {
    let mut system = test_text();
    let spanned = system
        .layout(
            &TextParams::new("x x", TextStyle::new(16.0)).spans(vec![quark_text::TextSpan {
                range: 2..3,
                weight: None,
                style: None,
                kind: None,
                size: Some(32.0),
                letter_spacing: None,
                keep_together: false,
            }]),
        )
        .expect("layout");
    let spanned = ShapedText::new(Arc::new(spanned));
    let scene = scene([Primitive::TextRun(TextPrimitive {
        rect: rect(4.0, 4.0, SIZE.0 as f32, 80.0),
        layout: spanned,
        color: INK,
    })]);
    // Inked rows of the columns `x0..x1`.
    let ink_height = |pixels: &[u8], x0: u32, x1: u32| {
        let rows: Vec<u32> = (0..SIZE.1)
            .filter(|&y| (x0..x1).any(|x| pixels[((y * SIZE.0 + x) * 4) as usize] < 128))
            .collect();
        rows.last().map_or(0, |last| last - rows[0] + 1)
    };
    for path in [
        crate::text::TextPath::Positioned,
        crate::text::TextPath::Buffer,
    ] {
        let Some(mut renderer) = renderer(None) else {
            return;
        };
        renderer.text_path = path;
        let pixels = draw(&mut renderer, &scene, &mut system);
        let (small, big) = (ink_height(&pixels, 0, 14), ink_height(&pixels, 14, 60));
        assert!(small > 4, "{path:?}: no plain x");
        assert!(
            big * 10 >= small * 18 && big * 10 <= small * 22,
            "{path:?}: spanned x is {big} px tall, plain {small}"
        );
    }
}
