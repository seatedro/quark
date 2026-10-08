//! Scenes that draw recorded chunks against the same scenes expanded.

use std::sync::Arc;
use std::time::Instant;

use quark::scene::SceneChunk;
use quark_text::{TextParams, TextSpan, TextStyle};

use super::*;
use crate::scene::{
    BorderPrimitive, ImagePrimitive, LayerPrimitive, PathPrimitive, RectPrimitive,
    RoundedRectPrimitive, ShadowPrimitive, ShapedText,
};
use crate::text::test_text;

fn rect(x: f32, y: f32, width: f32, height: f32) -> Rect {
    Rect {
        x,
        y,
        width,
        height,
    }
}

fn color(r: u8, g: u8, b: u8) -> quark::Color {
    quark::Color::rgba(r, g, b, 255)
}

fn chunk(primitives: Vec<Primitive>) -> Arc<SceneChunk> {
    let mut chunk = SceneChunk::new();
    chunk.replace(primitives);
    Arc::new(chunk)
}

fn chunk_primitive(chunk: &Arc<SceneChunk>, offset: [f32; 2]) -> Primitive {
    let mut scene = Scene::default();
    scene.chunk(chunk, offset);
    scene.primitives.pop().expect("chunk")
}

/// The scene as the app hands it over: in physical pixels at `scale`.
fn physical(mut scene: Scene, scale: f32) -> Scene {
    for primitive in &mut scene.primitives {
        primitive.to_physical(scale);
    }
    scene
}

fn expanded(scene: &Scene) -> Scene {
    Scene {
        primitives: scene.expanded(),
    }
}

// ---------------------------------------------------------------------------
// Pixel parity
// ---------------------------------------------------------------------------

/// Headless renderer, or `None` when no adapter exists (a failure when
/// `QUARK_REQUIRE_GPU` is set).
fn gpu_renderer(width: u32, height: u32) -> Option<Renderer> {
    match Renderer::new_headless(width, height, 1.0) {
        Ok(renderer) => Some(renderer),
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

const SIZE: (u32, u32) = (200, 160);

/// A cached row's worth of everything a chunk can hold, at its origin: a
/// shadowed card, its border, plain and rich text, an image, a path, and
/// a nested chunk that clips its own text and lifts a badge to z 5.
fn card(text: &mut TextSystem) -> Vec<Primitive> {
    let label = |text: &mut TextSystem, s: &str| {
        let layout = text
            .layout(&TextParams::new(s.to_owned(), TextStyle::new(14.0)))
            .expect("layout");
        ShapedText::new(Arc::new(layout))
    };
    let nested = chunk(vec![
        Primitive::ClipStart(ClipPrimitive {
            rect: rect(0.0, 0.0, 50.0, 10.0),
            corner_radii: [0.0; 4],
        }),
        Primitive::TextRun(TextPrimitive {
            rect: rect(0.0, -2.0, 80.0, 18.0),
            layout: label(text, "clipped nested text"),
            color: color(255, 255, 0),
        }),
        Primitive::ClipEnd,
        Primitive::ZIndexPush(5),
        Primitive::RoundedRect(RoundedRectPrimitive::uniform(
            rect(40.0, 4.0, 30.0, 14.0),
            7.0,
            color(0, 200, 255),
        )),
        Primitive::ZIndexPop,
    ]);
    let mut triangle = crate::scene::Path::builder();
    triangle.move_to(0.0, 0.0);
    triangle.line_to(16.0, 0.0);
    triangle.line_to(8.0, 14.0);
    triangle.close();
    vec![
        Primitive::Shadow(ShadowPrimitive {
            rect: rect(4.0, 4.0, 120.0, 70.0),
            blur_radius: 6.0,
            corner_radius: 8.0,
            offset: [0.0, 3.0],
            color: quark::Color::rgba(0, 0, 0, 160),
        }),
        Primitive::RoundedRect(RoundedRectPrimitive::uniform(
            rect(4.0, 4.0, 120.0, 70.0),
            8.0,
            color(50, 50, 70),
        )),
        Primitive::Border(BorderPrimitive::uniform(
            rect(4.0, 4.0, 120.0, 70.0),
            1.5,
            8.0,
            color(200, 200, 220),
        )),
        Primitive::TextRun(TextPrimitive {
            rect: rect(10.0, 8.0, 110.0, 18.0),
            layout: label(text, "Chunked card"),
            color: color(255, 255, 255),
        }),
        Primitive::RichTextRun(RichTextPrimitive {
            rect: rect(10.0, 26.0, 110.0, 18.0),
            layout: label(text, "rich and plain"),
            default_color: color(255, 120, 120),
            span_colors: Arc::from([]),
        }),
        Primitive::Image(ImagePrimitive {
            rect: rect(100.0, 50.0, 16.0, 16.0),
            width: 2,
            height: 2,
            rgba: Arc::from([0, 255, 0, 255].repeat(4)),
            cache_key: 0xC0FFEE,
        }),
        Primitive::Path(
            PathPrimitive::new(Arc::new(triangle.build()), [76.0, 50.0]).fill(color(255, 160, 0)),
        ),
        chunk_primitive(&nested, [10.0, 48.0]),
    ]
}

/// A scene that draws `card` through a chunk at `at`, between primitives
/// it overlaps: a backdrop below, a rounded clip around it, and a bar
/// painted after it that the nested badge's z-index lifts it over.
fn framed(card: &Arc<SceneChunk>, at: [f32; 2], around: impl Fn(&mut Scene, Primitive)) -> Scene {
    let mut scene = Scene::default();
    scene.rect(RectPrimitive {
        rect: rect(0.0, 0.0, 200.0, 160.0),
        color: color(20, 20, 20),
    });
    scene.clip_rounded(rect(6.0, 6.0, 188.0, 148.0), [24.0; 4]);
    around(&mut scene, chunk_primitive(card, at));
    scene.rect(RectPrimitive {
        rect: rect(0.0, at[1] + 54.0, 200.0, 6.0),
        color: color(255, 0, 255),
    });
    scene.pop_clip();
    scene
}

fn just(scene: &mut Scene, chunk: Primitive) {
    scene.push(chunk);
}

/// Every pixel of `scene` drawn from its chunks, on a renderer that drew
/// `warm` first, against the scene expanded on a fresh renderer.
fn assert_draws_expanded(name: &str, warm: &Scene, scene: &Scene, text: &mut TextSystem) {
    let (w, h) = SIZE;
    let Some(mut chunked) = gpu_renderer(w, h) else {
        return;
    };
    chunked.render_to_rgba(warm, text, w, h).expect("render");
    let drawn = chunked.render_to_rgba(scene, text, w, h).expect("render");
    let mut fresh = gpu_renderer(w, h).expect("renderer");
    let expected = fresh
        .render_to_rgba(&expanded(scene), text, w, h)
        .expect("render");
    let pixels = |rgba: &[u8]| rgba.as_chunks::<4>().0.to_vec();
    let lit = pixels(&expected)
        .iter()
        .filter(|p| p[..3] != [20, 20, 20] && p[..3] != [0, 0, 0])
        .count();
    assert!(
        lit > 3000,
        "{name}: {lit} lit pixels, the fixture drew too little"
    );
    let differing = pixels(&drawn)
        .iter()
        .zip(pixels(&expected))
        .filter(|(a, b)| *a != b)
        .count();
    assert_eq!(
        differing, 0,
        "{name}: pixels differ from the expanded scene"
    );
}

// A chunk drawn again, moved, clipped, scaled, or grouped by layers, paints
// exactly what its primitives drawn one by one paint: draw order against
// the primitives around it, its own clips and z-index, and those around it.
#[test]
fn chunked_scenes_draw_the_expanded_scenes_pixels() {
    let mut text = test_text();
    let card = chunk(card(&mut text));
    let faded = |scene: &mut Scene, chunk: Primitive| {
        scene.push_layer(0.5, Transform2D::translate(3.0, 2.0));
        scene.push(chunk);
        scene.rect(RectPrimitive {
            rect: rect(20.0, 20.0, 30.0, 30.0),
            color: color(0, 255, 0),
        });
        scene.pop_layer();
    };
    let clipped = |scene: &mut Scene, chunk: Primitive| {
        scene.clip(rect(0.0, 30.0, 200.0, 40.0));
        scene.push(chunk);
        scene.pop_clip();
    };
    let layered = chunk(vec![
        Primitive::LayerStart(LayerPrimitive {
            opacity: 0.5,
            transform: Transform2D::IDENTITY,
        }),
        chunk_primitive(&card, [0.0, 0.0]),
        Primitive::LayerEnd,
    ]);
    let cases: [(&str, Scene, Scene); 6] = [
        (
            "moved",
            framed(&card, [10.0, 10.0], just),
            framed(&card, [40.0, 52.0], just),
        ),
        (
            "moved under a clip of its own",
            framed(&card, [10.0, 10.0], clipped),
            framed(&card, [30.0, 22.0], clipped),
        ),
        (
            "moved by whole pixels at 1.5x",
            physical(framed(&card, [10.0, 10.0], just), 1.5),
            physical(framed(&card, [12.0, 4.0], just), 1.5),
        ),
        (
            "moved by a fraction at 1.5x",
            physical(framed(&card, [10.0, 10.0], just), 1.5),
            physical(framed(&card, [10.5, 12.25], just), 1.5),
        ),
        (
            "in a faded layer",
            framed(&card, [10.0, 10.0], just),
            framed(&card, [20.0, 30.0], faded),
        ),
        (
            "holding a layer",
            framed(&layered, [10.0, 10.0], just),
            framed(&layered, [16.0, 40.0], just),
        ),
    ];
    for (name, warm, scene) in &cases {
        assert_draws_expanded(name, warm, scene, &mut text);
    }
}

// ---------------------------------------------------------------------------
// Measurement scenes
// ---------------------------------------------------------------------------

const TERMINAL_ROWS: usize = 50;
const CELL_H: f32 = 16.0;

/// A terminal as quark-terminal paints it: the grid chunk holds one chunk
/// per row (a row background, two cell backgrounds, an 80-column line in
/// four colors), and the cursor is a chunk of its own.
fn terminal_scene(text: &mut TextSystem) -> Scene {
    let palette: Arc<[quark::Color]> = Arc::from([
        color(120, 220, 120),
        color(230, 230, 230),
        color(240, 200, 90),
    ]);
    let rows = (0..TERMINAL_ROWS)
        .map(|i| {
            let line = format!("{i:04} output {i} of a build step, compiling crate number {i:<30}");
            let spans = [0..4, 5..11, 12..line.len()]
                .into_iter()
                .map(|range| TextSpan {
                    range,
                    weight: None,
                    style: None,
                    kind: None,
                })
                .collect::<Vec<_>>();
            let params = TextParams::new(line.as_str(), TextStyle::new(13.0)).spans(spans);
            let layout = ShapedText::new(Arc::new(text.layout(&params).expect("layout")));
            let row = chunk(vec![
                Primitive::Rect(RectPrimitive {
                    rect: rect(0.0, 0.0, 640.0, CELL_H),
                    color: color(20, 20, 24),
                }),
                Primitive::Rect(RectPrimitive {
                    rect: rect(40.0, 0.0, 8.0, CELL_H),
                    color: color(60, 60, 90),
                }),
                Primitive::Rect(RectPrimitive {
                    rect: rect(96.0, 0.0, 24.0, CELL_H),
                    color: color(90, 40, 40),
                }),
                Primitive::RichTextRun(RichTextPrimitive {
                    rect: rect(0.0, 0.0, 640.0, CELL_H),
                    layout,
                    default_color: color(200, 200, 200),
                    span_colors: palette.clone(),
                }),
            ]);
            chunk_primitive(&row, [0.0, i as f32 * CELL_H])
        })
        .collect();
    let grid = chunk(rows);
    let cursor = chunk(vec![Primitive::Rect(RectPrimitive {
        rect: rect(0.0, 0.0, 8.0, CELL_H),
        color: color(230, 230, 230),
    })]);
    let mut scene = Scene::default();
    scene.rect(RectPrimitive {
        rect: rect(0.0, 0.0, 800.0, 820.0),
        color: color(16, 16, 18),
    });
    scene.chunk(&grid, [8.0, 8.0]);
    scene.chunk(&cursor, [8.0 + 6.0 * 8.0, 8.0 + 49.0 * CELL_H]);
    physical(scene, 1.0)
}

const LIST_ROWS: usize = 60;
const ROW_H: f32 = 40.0;

/// A scrolled list: each row is a chunk (a rounded card, its border, a
/// title, a subtitle, a badge), clipped to an 800 pixel viewport.
fn list_scene(text: &mut TextSystem, scroll: f32) -> Scene {
    let mut scene = Scene::default();
    scene.rect(RectPrimitive {
        rect: rect(0.0, 0.0, 800.0, 820.0),
        color: color(30, 30, 34),
    });
    scene.clip(rect(0.0, 10.0, 600.0, 800.0));
    for i in 0..LIST_ROWS {
        let title = text
            .layout(&TextParams::new(
                format!("Conversation {i}"),
                TextStyle::new(14.0),
            ))
            .expect("layout");
        let subtitle = text
            .layout(&TextParams::new(
                format!("Last message in thread {i}, a few words long"),
                TextStyle::new(12.0),
            ))
            .expect("layout");
        let row = chunk(vec![
            Primitive::RoundedRect(RoundedRectPrimitive::uniform(
                rect(4.0, 2.0, 592.0, ROW_H - 4.0),
                6.0,
                color(44, 44, 52),
            )),
            Primitive::Border(BorderPrimitive::uniform(
                rect(4.0, 2.0, 592.0, ROW_H - 4.0),
                1.0,
                6.0,
                color(70, 70, 80),
            )),
            Primitive::TextRun(TextPrimitive {
                rect: rect(12.0, 4.0, 400.0, 18.0),
                layout: ShapedText::new(Arc::new(title)),
                color: color(240, 240, 240),
            }),
            Primitive::TextRun(TextPrimitive {
                rect: rect(12.0, 21.0, 400.0, 16.0),
                layout: ShapedText::new(Arc::new(subtitle)),
                color: color(170, 170, 180),
            }),
            Primitive::RoundedRect(RoundedRectPrimitive::uniform(
                rect(560.0, 12.0, 20.0, 14.0),
                7.0,
                color(80, 120, 220),
            )),
        ]);
        scene.chunk(&row, [0.0, 10.0 + i as f32 * ROW_H - scroll]);
    }
    scene.pop_clip();
    physical(scene, 1.0)
}

/// CPU time and allocations of preparing `scene` on `renderer`: flatten,
/// batch, upload, prepare text, and encode, with the GPU work submitted
/// outside the timing.
fn frame_cost(renderer: &mut Renderer, scene: &Scene, text: &mut TextSystem) -> (u64, u64, u64) {
    let (w, h) = (renderer.size.width, renderer.size.height);
    let target = renderer.texture_pool.acquire(&renderer.device, w, h);
    let view = renderer.texture_pool.view(&target).clone();
    let mut encoder = renderer
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    let started = Instant::now();
    let mut flattened = 0;
    let ((), allocations) = quark_ui::test_alloc::count(|| {
        renderer.texture_pool.begin_frame();
        renderer.instance_buffer_pool.begin_frame();
        renderer.flatten(scene, w, h);
        flattened = started.elapsed().as_micros() as u64;
        renderer
            .record_frame(&mut encoder, &view, text)
            .expect("record");
    });
    let micros = started.elapsed().as_micros() as u64;
    renderer.queue.submit(Some(encoder.finish()));
    let _ = renderer.device.poll(wgpu::PollType::wait_indefinitely());
    renderer.atlas.trim();
    renderer.texture_pool.release(target);
    (micros, flattened, allocations)
}

/// Median CPU time (all, flattening) and allocations of a repeated frame
/// of `scene`.
fn repeated_cost(scene: &Scene, text: &mut TextSystem) -> Option<(u64, u64, u64)> {
    let mut renderer = Renderer::new_headless(800, 820, 1.0).ok()?;
    for _ in 0..3 {
        frame_cost(&mut renderer, scene, text);
    }
    let mut samples: Vec<(u64, u64, u64)> = (0..40)
        .map(|_| frame_cost(&mut renderer, scene, text))
        .collect();
    samples.sort_unstable();
    let mut text_us: Vec<u64> = (0..40)
        .map(|_| {
            renderer.flatten(scene, 800, 820);
            let frames = std::mem::take(&mut renderer.frames);
            let active = renderer.active_frames;
            let mut steps = 0;
            for frame in &mut renderer.frames.iter_mut().chain([]) {
                let _ = frame;
            }
            let mut frames = frames;
            for frame in frames[..active].iter_mut() {
                build_batches(&frame.flat, &mut frame.batches);
                frame.text_base = steps;
                steps += frame.batches.text_steps;
            }
            renderer.fit_text_renderers(steps);
            let started = Instant::now();
            renderer.prepare_frame_text(&frames[..active], text);
            let us = started.elapsed().as_micros() as u64;
            renderer.frames = frames;
            renderer.atlas.trim();
            us
        })
        .collect();
    text_us.sort_unstable();
    println!("  text preparation {} us", text_us[20]);
    Some(samples[samples.len() / 2])
}

/// Prints the CPU cost of a repeated frame of a cached terminal and a
/// cached list, drawn from chunks and from the same scenes expanded. Run
/// with `--ignored --nocapture` on a GPU (lavapipe works).
#[test]
#[ignore = "measurement, prints a report"]
fn report_chunk_frame_cost() {
    let mut text = test_text();
    let scenes = [
        ("terminal", terminal_scene(&mut text)),
        ("list", list_scene(&mut text, 0.0)),
    ];
    for (name, scene) in &scenes {
        for (mode, scene) in [("expanded", expanded(scene)), ("chunked", scene.clone())] {
            let Some((micros, flattened, allocations)) = repeated_cost(&scene, &mut text) else {
                return;
            };
            println!(
                "{name:<9} {mode:<9} {:>4} primitives {micros:>5} us ({flattened:>4} us flattening) {allocations:>3} allocations",
                scene.len()
            );
        }
    }
}
