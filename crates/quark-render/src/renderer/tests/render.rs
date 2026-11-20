use super::*;
use crate::scene::ShapedText;
use crate::text::test_text;
use quark_text::{TextParams, TextStyle};

// Regression: straight-alpha image pixels were blended as premultiplied,
// so soft edges of transparent images drew a bright fringe.
#[test]
fn images_upload_premultiplied_and_opaque_ones_unchanged() {
    let opaque = [10, 20, 30, 255, 200, 100, 50, 255];
    assert!(matches!(
        premultiply(&opaque),
        std::borrow::Cow::Borrowed(_)
    ));

    let soft = premultiply(&[255, 255, 255, 128, 255, 0, 0, 0]);
    assert_eq!(soft[3], 128);
    assert_eq!(&soft[4..], &[0, 0, 0, 0]);
    let half = linear_to_srgb_byte(128.0 / 255.0);
    assert_eq!(&soft[..3], &[half, half, half]);
}

// A transparent window must take the alpha mode matching premultiplied
// output when offered, and report an opaque fallback when only opaque
// composition exists.
#[test]
fn transparent_surfaces_prefer_premultiplied_alpha() {
    use wgpu::CompositeAlphaMode::{Inherit, Opaque, PostMultiplied, PreMultiplied};
    let table: [(
        &[wgpu::CompositeAlphaMode],
        Option<wgpu::CompositeAlphaMode>,
    ); 4] = [
        (
            &[Opaque, Inherit, PostMultiplied, PreMultiplied],
            Some(PreMultiplied),
        ),
        (&[Opaque, PostMultiplied], Some(PostMultiplied)),
        (&[Inherit, Opaque], Some(Inherit)),
        (&[Opaque], None),
    ];
    for (offered, expected) in table {
        assert_eq!(transparent_alpha_mode(offered), expected, "{offered:?}");
    }
}

#[test]
fn scissor_rect_clamps_to_target_and_rejects_degenerate_clips() {
    let cases = [
        (rect(807.0, 118.0, 844.0, 865.0), Some((807, 118, 843, 865))),
        (rect(-10.0, -5.0, 20.0, 10.0), Some((0, 0, 10, 5))),
        (rect(1650.0, 118.0, 10.0, 865.0), None),
        (rect(10.0, 10.0, -5.0, 5.0), None),
        (rect(f32::NAN, 0.0, 10.0, 10.0), None),
        (rect(0.0, 0.0, f32::INFINITY, 10.0), None),
    ];
    for (clip, expected) in cases {
        assert_eq!(scissor_rect(clip, 1650, 1050), expected, "{clip:?}");
    }
}

// A rectangular clip inside a rounded one must keep the rounded corners
// for its children, or a list inside a rounded card paints square
// corners over the card's.
#[test]
fn render_child_of_rect_clip_inside_rounded_clip_keeps_rounded_corners() {
    let area = rect(0.0, 0.0, 40.0, 40.0);
    let mut scene = Scene::default();
    scene.push(Primitive::ClipStart(ClipPrimitive {
        rect: area,
        corner_radii: [12.0; 4],
    }));
    scene.push(Primitive::ClipStart(ClipPrimitive {
        rect: area,
        corner_radii: [0.0; 4],
    }));
    scene.push(solid(area, 255, 255, 255));
    scene.push(Primitive::ClipEnd);
    scene.push(Primitive::ClipEnd);
    let Some(image) = render_pixels(&scene, 40, 40) else {
        return;
    };
    assert_eq!(
        image.get_pixel(1, 1).0,
        [0, 0, 0, 255],
        "corner not clipped"
    );
    assert_eq!(image.get_pixel(20, 20).0, [255, 255, 255, 255]);
}

// -- Headless GPU helpers ------------------------------------------------

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

fn render_pixels(scene: &Scene, width: u32, height: u32) -> Option<image::RgbaImage> {
    render_pixels_with(scene, &mut test_text(), width, height)
}

fn render_pixels_with(
    scene: &Scene,
    text: &mut TextSystem,
    width: u32,
    height: u32,
) -> Option<image::RgbaImage> {
    let mut renderer = gpu_renderer(width, height)?;
    let pixels = renderer
        .render_to_rgba(scene, text, width, height)
        .expect("offscreen render");
    Some(image::RgbaImage::from_raw(width, height, pixels).expect("pixel buffer size"))
}

fn rect(x: f32, y: f32, width: f32, height: f32) -> Rect {
    Rect {
        x,
        y,
        width,
        height,
    }
}

fn white_text(rect: Rect, text: &str) -> Primitive {
    let params = TextParams::new(text, TextStyle::new(16.0));
    let layout = test_text().layout(&params).expect("layout");
    Primitive::TextRun(TextPrimitive {
        rect,
        layout: ShapedText::new(Arc::new(layout)),
        color: quark::Color::rgba(255, 255, 255, 255),
    })
}

fn solid(rect: Rect, r: u8, g: u8, b: u8) -> Primitive {
    Primitive::Rect(crate::scene::RectPrimitive {
        rect,
        color: quark::Color::rgba(r, g, b, 255),
    })
}

fn any_light_pixel(image: &image::RgbaImage, area: Rect) -> bool {
    let (x0, y0) = (area.x as u32, area.y as u32);
    let (x1, y1) = (area.right() as u32, area.bottom() as u32);
    (y0..y1).any(|y| {
        (x0..x1).any(|x| {
            let p = image.get_pixel(x, y).0;
            p[0] > 128 && p[1] > 128 && p[2] > 128
        })
    })
}

// -- Draw order ------------------------------------------------------------

// Regression: each z-layer drew all quads, then images, then text, so a
// quad painted after text (selection highlight, overlay) sat under it.
#[test]
fn render_quad_painted_after_text_covers_text() {
    let band = rect(4.0, 4.0, 120.0, 24.0);
    let mut scene = Scene::default();
    scene.push(white_text(band, "WWWWWWWW"));
    scene.push(solid(band, 200, 0, 0));
    let Some(image) = render_pixels(&scene, 128, 32) else {
        return;
    };
    assert!(
        !any_light_pixel(&image, band),
        "text drew over the quad painted after it"
    );
}

// Guards the single-encoder path: every text run has its own text
// renderer, so preparing a later z-layer's text must not erase earlier text.
#[test]
fn render_text_in_two_z_layers_draws_both() {
    let low = rect(4.0, 4.0, 120.0, 24.0);
    let high = rect(4.0, 36.0, 120.0, 24.0);
    let mut scene = Scene::default();
    scene.push(white_text(low, "WWWWWWWW"));
    scene.push(Primitive::ZIndexPush(1));
    scene.push(white_text(high, "WWWWWWWW"));
    scene.push(Primitive::ZIndexPop);
    let Some(image) = render_pixels(&scene, 128, 64) else {
        return;
    };
    assert!(any_light_pixel(&image, low), "z=0 text is missing");
    assert!(any_light_pixel(&image, high), "z=1 text is missing");
}

fn count_pixels(image: &image::RgbaImage, area: Rect, pred: impl Fn([u8; 4]) -> bool) -> usize {
    let (x0, y0) = (area.x as u32, area.y as u32);
    let (x1, y1) = (area.right() as u32, area.bottom() as u32);
    (y0..y1)
        .flat_map(|y| (x0..x1).map(move |x| (x, y)))
        .filter(|&(x, y)| pred(image.get_pixel(x, y).0))
        .count()
}

// Each span must keep its own color and the other span's glyphs must not
// bleed over.
#[test]
fn render_rich_text_paints_each_span_in_its_color() {
    let text = "WWWWWWMMMMMM";
    let params = TextParams::new(text, TextStyle::new(20.0)).spans(vec![
        quark_text::TextSpan {
            range: 0..6,
            weight: None,
            style: None,
            kind: None,
            size: None,
            letter_spacing: None,
            keep_together: false,
        },
        quark_text::TextSpan {
            range: 6..12,
            weight: None,
            style: None,
            kind: None,
            size: None,
            letter_spacing: None,
            keep_together: false,
        },
    ]);
    let layout = test_text().layout(&params).expect("layout");
    let split = layout.caret(6).x;
    let mut scene = Scene::default();
    scene.rich_text(RichTextPrimitive {
        rect: rect(0.0, 0.0, 200.0, 32.0),
        layout: ShapedText::new(Arc::new(layout)),
        default_color: quark::Color::rgba(255, 255, 255, 255),
        span_colors: Arc::from([
            quark::Color::rgba(255, 0, 0, 255),
            quark::Color::rgba(0, 255, 0, 255),
        ]),
    });
    let Some(image) = render_pixels(&scene, 200, 32) else {
        return;
    };
    let red = |p: [u8; 4]| p[0] > 120 && p[1] < 40;
    let green = |p: [u8; 4]| p[1] > 120 && p[0] < 40;
    // One pixel of slack each side: area bounds round to whole pixels.
    let left = rect(0.0, 0.0, split.floor() - 1.0, 32.0);
    let right = rect(split.ceil() + 1.0, 0.0, 198.0 - split.ceil(), 32.0);
    assert!(count_pixels(&image, left, red) > 20, "first span not red");
    assert!(
        count_pixels(&image, right, green) > 20,
        "second span not green"
    );
    assert_eq!(count_pixels(&image, left, green), 0, "green left of split");
    assert_eq!(count_pixels(&image, right, red), 0, "red right of split");
}

fn span(range: std::ops::Range<usize>) -> quark_text::TextSpan {
    quark_text::TextSpan {
        range,
        weight: None,
        style: None,
        kind: None,
        size: None,
        letter_spacing: None,
        keep_together: false,
    }
}

const RED: quark::Color = quark::Color::rgba(255, 0, 0, 255);
const GREEN: quark::Color = quark::Color::rgba(0, 255, 0, 255);
const BLUE: quark::Color = quark::Color::rgba(0, 0, 255, 255);

/// White text shaped by `system`, for tests that hold the shared one.
fn white_text_with(system: &mut TextSystem, rect: Rect, text: &str) -> Primitive {
    let layout = system.layout(&TextParams::new(text, TextStyle::new(16.0)));
    Primitive::TextRun(TextPrimitive {
        rect,
        layout: ShapedText::new(Arc::new(layout.expect("layout"))),
        color: quark::Color::rgba(255, 255, 255, 255),
    })
}

/// Highlighted code: four lines whose keywords draw in `colors[0]` and
/// whose numbers draw in `colors[1]`, under plain white text.
fn code_scene(text: &mut TextSystem, layout: &ShapedText, colors: [quark::Color; 2]) -> Scene {
    let mut scene = Scene::default();
    scene.push(white_text_with(text, rect(4.0, 2.0, 200.0, 20.0), "plain"));
    scene.rich_text(RichTextPrimitive {
        rect: rect(4.0, 24.0, 200.0, 100.0),
        layout: layout.clone(),
        default_color: quark::Color::rgba(255, 255, 255, 255),
        span_colors: Arc::from(colors),
    });
    scene
}

fn code_layout(text: &mut TextSystem) -> ShapedText {
    let source = "let a = 1;\nlet b = 2;\nlet c = 3;\nlet d = 4;";
    // Span 1 is each `let`, span 2 each digit.
    let mut spans = Vec::new();
    for (start, _) in source.match_indices("let") {
        spans.push((start, span(start..start + 3)));
    }
    for (start, _) in source.match_indices(|c: char| c.is_ascii_digit()) {
        spans.push((start, span(start..start + 1)));
    }
    spans.sort_by_key(|(start, _)| *start);
    let params = TextParams::new(source, TextStyle::new(16.0))
        .spans(spans.into_iter().map(|(_, span)| span).collect::<Vec<_>>());
    ShapedText::new(Arc::new(text.layout(&params).expect("layout")))
}

/// Allocations of preparing `scene`'s text on `renderer`, which has
/// already drawn a frame. Flattening happens before counting.
fn count_text_preparation(
    renderer: &mut Renderer,
    scene: &Scene,
    text: &mut TextSystem,
) -> (FrameText, u64) {
    renderer.flatten(scene, CODE_SIZE.0, CODE_SIZE.1);
    let frames = std::mem::take(&mut renderer.frames);
    let active = renderer.active_frames;
    let result =
        quark_ui::test_alloc::count(|| renderer.prepare_frame_text(&frames[..active], text));
    renderer.frames = frames;
    result
}

const CODE_SIZE: (u32, u32) = (200, 128);

fn render_code(renderer: &mut Renderer, scene: &Scene, text: &mut TextSystem) -> image::RgbaImage {
    let (w, h) = CODE_SIZE;
    let pixels = renderer
        .render_to_rgba(scene, text, w, h)
        .expect("offscreen render");
    image::RgbaImage::from_raw(w, h, pixels).expect("pixel buffer size")
}

fn has_pixels(image: &image::RgbaImage, color: quark::Color) -> bool {
    let area = rect(0.0, 0.0, CODE_SIZE.0 as f32, CODE_SIZE.1 as f32);
    let near = |p: [u8; 4]| {
        [
            p[0].abs_diff(color.r),
            p[1].abs_diff(color.g),
            p[2].abs_diff(color.b),
        ]
        .iter()
        .all(|&d| d < 60)
    };
    count_pixels(image, area, near) > 10
}

// Preparing an unchanged frame's text with every glyph in the atlas
// reuses the text engine's vertex storage and walks the layouts in place.
#[test]
fn repeated_text_preparation_allocates_nothing() {
    let Some(mut renderer) = gpu_renderer(CODE_SIZE.0, CODE_SIZE.1) else {
        return;
    };
    let mut text = test_text();
    let scene = {
        let layout = code_layout(&mut text);
        code_scene(&mut text, &layout, [RED, GREEN])
    };
    render_code(&mut renderer, &scene, &mut text);

    let (ready, allocated) = count_text_preparation(&mut renderer, &scene, &mut text);
    assert_eq!(ready, FrameText::Resident);
    assert_eq!(allocated, 0);
    let image = render_code(&mut renderer, &scene, &mut text);
    assert!(has_pixels(&image, RED) && has_pixels(&image, GREEN));
}

// Regression: changing a rich text's colors copied and reshaped its
// layout to bake the new colors into a buffer.
#[test]
fn recoloring_text_allocates_nothing() {
    let Some(mut renderer) = gpu_renderer(CODE_SIZE.0, CODE_SIZE.1) else {
        return;
    };
    let mut text = test_text();
    let layout = code_layout(&mut text);
    let scene = code_scene(&mut text, &layout, [RED, GREEN]);
    render_code(&mut renderer, &scene, &mut text);

    let recolored = code_scene(&mut text, &layout, [BLUE, GREEN]);
    let (ready, allocated) = count_text_preparation(&mut renderer, &recolored, &mut text);
    assert_eq!(ready, FrameText::Resident);
    assert_eq!(allocated, 0);
    let image = render_code(&mut renderer, &recolored, &mut text);
    assert!(has_pixels(&image, BLUE), "keywords not recolored");
    assert!(!has_pixels(&image, RED), "old keyword color left");
}

/// The code layout alone in a layer of `opacity`. A single drawable,
/// so flattening folds the opacity into the text instead of drawing the
/// layer offscreen.
fn faded_code_scene(layout: &ShapedText, colors: [quark::Color; 3], opacity: f32) -> Scene {
    let mut scene = Scene::default();
    scene.push(layer(opacity, Transform2D::IDENTITY));
    scene.rich_text(RichTextPrimitive {
        rect: rect(4.0, 4.0, 190.0, 100.0),
        layout: layout.clone(),
        default_color: colors[0],
        span_colors: Arc::from([colors[1], colors[2]]),
    });
    scene.push(Primitive::LayerEnd);
    scene
}

// Regression: fading rich text collected a faded copy of its span
// colors every frame it was flattened.
#[test]
fn fading_rich_text_allocates_nothing() {
    let Some(mut renderer) = gpu_renderer(CODE_SIZE.0, CODE_SIZE.1) else {
        return;
    };
    let mut text = test_text();
    let layout = code_layout(&mut text);
    let colors = [quark::Color::rgba(255, 255, 255, 255), RED, GREEN];
    render_code(
        &mut renderer,
        &faded_code_scene(&layout, colors, 0.9),
        &mut text,
    );

    // A fade animation's next frame: same text, another opacity.
    let scene = faded_code_scene(&layout, colors, 0.5);
    let ((), flattening) = quark_ui::test_alloc::count(|| {
        renderer.flatten(&scene, CODE_SIZE.0, CODE_SIZE.1);
    });
    assert_eq!(flattening, 0);
    let (ready, preparing) = count_text_preparation(&mut renderer, &scene, &mut text);
    assert_eq!(ready, FrameText::Resident);
    assert_eq!(preparing, 0);
}

// Faded rich text draws what the same text with its colors' alpha
// already halved draws, on both text paths.
#[test]
fn faded_rich_text_draws_its_colors_at_reduced_alpha() {
    let mut text = test_text();
    let layout = code_layout(&mut text);
    let [white, red, blue] = [quark::Color::rgba(255, 255, 255, 255), RED, BLUE];
    let faded = faded_code_scene(&layout, [white, red, blue], 0.5);
    // 255 * 0.5, rounded.
    let half = |c: quark::Color| quark::Color::rgba(c.r, c.g, c.b, 128);
    let prefaded = faded_code_scene(&layout, [half(white), half(red), half(blue)], 1.0);
    for path in [TextPath::Positioned, TextPath::Buffer] {
        let Some(mut renderer) = gpu_renderer(CODE_SIZE.0, CODE_SIZE.1) else {
            return;
        };
        renderer.text_path = path;
        let faded = render_code(&mut renderer, &faded, &mut text);
        let prefaded = render_code(&mut renderer, &prefaded, &mut text);
        let lit = faded.pixels().filter(|p| p.0[..3] != [0, 0, 0]).count();
        assert!(
            lit > 200,
            "{path:?}: {lit} lit pixels, the fixture drew too little"
        );
        let differing = faded
            .pixels()
            .zip(prefaded.pixels())
            .filter(|(a, b)| a != b)
            .count();
        assert_eq!(differing, 0, "{path:?}: faded pixels differ");
    }
}

// The positioned path must draw what the buffer path draws from text areas:
// fractional origins, a clip through a line, an overflowing RTL line
// shifted inside its box, a color glyph, and multi-colored spans.
#[test]
fn positioned_text_draws_the_buffer_paths_pixels() {
    let (w, h) = (160, 160);
    let mut text = test_text();
    let mut scene = Scene::default();
    scene.push(white_text_with(
        &mut text,
        rect(3.4, 2.6, 150.0, 20.0),
        "Fractional origin",
    ));
    scene.push(Primitive::ClipStart(ClipPrimitive {
        rect: rect(0.0, 24.0, 160.0, 9.5),
        corner_radii: [0.0; 4],
    }));
    scene.push(white_text_with(
        &mut text,
        rect(2.0, 22.0, 150.0, 20.0),
        "Clipped mid line",
    ));
    scene.push(Primitive::ClipEnd);
    let rtl = TextParams::new("\u{1f600}\u{5e9}\u{5dc}\u{5d5}", TextStyle::new(14.0))
        .wrap_width(Some(1.0));
    let rtl = text.layout(&rtl).expect("layout");
    assert!(rtl.buffer_x() != 0.0, "fixture must shift its RTL lines");
    scene.push(Primitive::TextRun(TextPrimitive {
        rect: rect(120.5, 40.25, 30.0, 80.0),
        layout: ShapedText::new(Arc::new(rtl)),
        color: GREEN,
    }));
    let code = code_layout(&mut text);
    scene.rich_text(RichTextPrimitive {
        rect: rect(1.75, 44.5, 110.0, 100.0),
        layout: code,
        default_color: quark::Color::rgba(200, 200, 200, 255),
        span_colors: Arc::from([RED, BLUE]),
    });

    let render = |path: TextPath, text: &mut TextSystem| {
        let mut renderer = gpu_renderer(w, h)?;
        renderer.text_path = path;
        Some(
            renderer
                .render_to_rgba(&scene, text, w, h)
                .expect("offscreen render"),
        )
    };
    let Some(positioned) = render(TextPath::Positioned, &mut text) else {
        return;
    };
    let buffer = render(TextPath::Buffer, &mut text).expect("renderer");
    let lit = positioned
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|p| p[..3] != [0, 0, 0])
        .count();
    assert!(lit > 500, "{lit} lit pixels: the fixture drew too little");
    let differing = positioned
        .as_chunks::<4>()
        .0
        .iter()
        .zip(buffer.as_chunks::<4>().0)
        .filter(|(a, b)| a != b)
        .count();
    assert_eq!(differing, 0, "pixels differ between text paths");
}

// Regression: rich text drew one text area per same-colored stretch of
// each line, and the engine walks the buffer's lines for every area, so
// highlighted code cost grew with the square of its line count.
#[test]
fn multi_line_rich_text_prepares_one_text_area() {
    let source = "let a = 1;\nlet b = 2;\nlet c = 3;\nlet d = 4;";
    let spans = source
        .match_indices("let")
        .map(|(start, _)| quark_text::TextSpan {
            range: start..start + 3,
            weight: None,
            style: None,
            kind: None,
            size: None,
            letter_spacing: None,
            keep_together: false,
        })
        .collect::<Vec<_>>();
    let mut text = test_text();
    let params = TextParams::new(source, TextStyle::new(14.0)).spans(spans);
    let layout = text.layout(&params).expect("layout");
    assert_eq!(layout.line_count(), 4);
    let mut scene = Scene::default();
    scene.rich_text(RichTextPrimitive {
        rect: rect(0.0, 0.0, 200.0, 100.0),
        layout: ShapedText::new(Arc::new(layout)),
        default_color: quark::Color::rgba(255, 255, 255, 255),
        span_colors: Arc::from([quark::Color::rgba(255, 0, 0, 255); 4]),
    });
    let flat = flatten_scene(&scene, rect(0.0, 0.0, 200.0, 100.0), &ImageCache::new());
    let mut recolored = RecoloredBuffers::default();
    recolored.prepare([&flat.rich_texts[..]], &mut text);
    let areas = crate::text::prepare_text_areas(&flat.texts, &flat.rich_texts, &recolored);
    assert_eq!(areas.count(), 1);
}

// Splitting at every kind transition would cost one draw and one text
// renderer per list row; disjoint rows must stay in a quad and a text batch.
#[test]
fn flatten_disjoint_rows_keep_one_quad_and_one_text_batch() {
    let mut scene = Scene::default();
    scene.push(solid(rect(0.0, 0.0, 200.0, 400.0), 10, 10, 10));
    for row in 0..20 {
        let row_rect = rect(0.0, row as f32 * 20.0, 200.0, 20.0);
        scene.push(solid(row_rect, 30, 30, 30));
        scene.push(white_text(row_rect, "row"));
    }
    let flat = flatten_scene(&scene, rect(0.0, 0.0, 200.0, 400.0), &ImageCache::new());
    let kinds: Vec<PrimKind> = flat
        .steps
        .iter()
        .filter_map(|step| match step {
            DrawStep::Batch { kind, .. } => Some(*kind),
            DrawStep::Blur(_) => None,
        })
        .collect();
    assert_eq!(kinds, [PrimKind::Quad, PrimKind::Text]);
}

// -- Borders -----------------------------------------------------------------

// Regression: the border shader used the widest side on every side, so a
// bottom-only border drew a full outline.
#[test]
fn render_bottom_only_border_leaves_other_sides_empty() {
    let mut scene = Scene::default();
    scene.push(Primitive::Border(crate::scene::BorderPrimitive {
        rect: rect(4.0, 4.0, 40.0, 40.0),
        widths: [0.0, 0.0, 4.0, 0.0],
        corner_radii: [6.0; 4],
        color: quark::Color::rgba(255, 255, 255, 255),
    }));
    let Some(image) = render_pixels(&scene, 48, 48) else {
        return;
    };
    let lit = |x: u32, y: u32| image.get_pixel(x, y).0[0] > 128;
    assert!(lit(24, 42), "bottom border missing");
    assert!(!lit(24, 5), "top side drew a border");
    assert!(!lit(5, 24), "left side drew a border");
    assert!(!lit(42, 24), "right side drew a border");
}

// -- Blur --------------------------------------------------------------------

fn blur(rect: Rect, blur_radius: f32) -> Primitive {
    Primitive::BlurRegion(crate::scene::BlurRegionPrimitive {
        rect,
        blur_radius,
        corner_radii: [0.0; 4],
    })
}

/// Black left half, white right half, so a blur turns the seam gray.
fn push_seam(scene: &mut Scene, area: Rect) {
    let half = area.width / 2.0;
    scene.push(solid(rect(area.x, area.y, half, area.height), 0, 0, 0));
    scene.push(solid(
        rect(area.x + half, area.y, half, area.height),
        255,
        255,
        255,
    ));
}

fn is_gray(pixel: [u8; 4]) -> bool {
    (40..=215).contains(&pixel[0])
}

// Regression: only blur_regions[0] was honored; later regions stayed sharp.
#[test]
fn render_second_blur_region_blurs_its_content() {
    let mut scene = Scene::default();
    push_seam(&mut scene, rect(0.0, 0.0, 64.0, 32.0));
    push_seam(&mut scene, rect(0.0, 32.0, 64.0, 32.0));
    scene.push(blur(rect(0.0, 0.0, 64.0, 32.0), 12.0));
    scene.push(blur(rect(0.0, 32.0, 64.0, 32.0), 12.0));
    let Some(image) = render_pixels(&scene, 64, 64) else {
        return;
    };
    assert!(
        is_gray(image.get_pixel(33, 16).0),
        "first region not blurred"
    );
    assert!(
        is_gray(image.get_pixel(33, 48).0),
        "second region not blurred"
    );
}

// Regression: the blur path drew all text last, sharp over the blur, even
// text painted before the blur region.
#[test]
fn render_text_painted_before_blur_region_is_blurred() {
    let band = rect(4.0, 4.0, 120.0, 24.0);
    let mut scene = Scene::default();
    scene.push(white_text(band, "WWWWWWWW"));
    scene.push(blur(rect(0.0, 0.0, 128.0, 32.0), 24.0));
    let Some(image) = render_pixels(&scene, 128, 32) else {
        return;
    };
    let brightest = image.pixels().map(|p| p.0[0]).max().unwrap_or(0);
    assert!(brightest < 200, "sharp text survived the blur: {brightest}");
}

// Regression: the windowed blur path never drew images. render() and
// render_to_rgba now share one encoder path, so this probe covers it.
#[test]
fn render_image_after_blur_region_draws() {
    let mut scene = Scene::default();
    scene.push(blur(rect(0.0, 0.0, 32.0, 32.0), 8.0));
    scene.push(Primitive::Image(crate::scene::ImagePrimitive {
        rect: rect(8.0, 8.0, 16.0, 16.0),
        width: 2,
        height: 2,
        rgba: Arc::from(vec![255u8; 16]),
        cache_key: 7,
    }));
    let Some(image) = render_pixels(&scene, 32, 32) else {
        return;
    };
    assert!(image.get_pixel(16, 16).0[0] > 200, "image missing");
}

// Catches images sampled without mip levels: drawn at a third of its
// size, a one-pixel checkerboard must average to grey instead of
// landing on single black or white texels.
#[test]
fn render_downscaled_image_averages_its_texels() {
    let side = 48u32;
    let mut rgba = Vec::with_capacity((side * side * 4) as usize);
    for y in 0..side {
        for x in 0..side {
            let v = if (x + y) % 2 == 0 { 255 } else { 0 };
            rgba.extend_from_slice(&[v, v, v, 255]);
        }
    }
    let mut scene = Scene::default();
    scene.push(Primitive::Image(crate::scene::ImagePrimitive {
        rect: rect(0.0, 0.0, 16.0, 16.0),
        width: side,
        height: side,
        rgba: Arc::from(rgba),
        cache_key: 11,
    }));
    let Some(image) = render_pixels(&scene, 16, 16) else {
        return;
    };
    // Half black and half white in linear light is sRGB 188.
    for (x, y) in [(3, 3), (8, 8), (12, 5)] {
        let v = image.get_pixel(x, y).0[0];
        assert!(v.abs_diff(188) <= 16, "pixel ({x}, {y}) is {v}, not grey");
    }
}

// Rounded blur regions: the corner the radius cuts off keeps the sharp
// backdrop, a square corner is blurred.
#[test]
fn render_blur_region_leaves_its_rounded_corner_sharp() {
    let mut scene = Scene::default();
    for x in (0..64).step_by(2) {
        let shade = if x % 4 == 0 { 255 } else { 0 };
        scene.push(solid(rect(x as f32, 0.0, 2.0, 64.0), shade, shade, shade));
    }
    scene.push(Primitive::BlurRegion(crate::scene::BlurRegionPrimitive {
        rect: rect(0.0, 0.0, 64.0, 64.0),
        blur_radius: 8.0,
        corner_radii: [20.0, 0.0, 0.0, 0.0],
    }));
    let Some(image) = render_pixels(&scene, 64, 64) else {
        return;
    };
    let top_left = image.get_pixel(1, 1).0[0];
    assert!(
        top_left == 0 || top_left == 255,
        "rounded corner blurred: {top_left}"
    );
    assert!(is_gray(image.get_pixel(62, 1).0), "square corner sharp");
    assert!(is_gray(image.get_pixel(32, 32).0), "center sharp");
}

// -- Layers ------------------------------------------------------------------

fn layer(opacity: f32, transform: Transform2D) -> Primitive {
    Primitive::LayerStart(crate::scene::LayerPrimitive { opacity, transform })
}

// Group opacity: a faded group shows nothing of a child its other child
// covers (per-primitive alpha would let the green show through), and
// its text dims with it.
#[test]
fn render_opacity_layer_fades_children_and_text_as_one_group() {
    let mut scene = Scene::default();
    scene.push(layer(0.5, Transform2D::IDENTITY));
    scene.push(solid(rect(0.0, 0.0, 40.0, 40.0), 0, 255, 0));
    scene.push(solid(rect(0.0, 0.0, 40.0, 40.0), 0, 0, 255));
    let band = rect(44.0, 4.0, 120.0, 24.0);
    scene.push(white_text(band, "WWWWWWWW"));
    scene.push(Primitive::LayerEnd);
    let Some(image) = render_pixels(&scene, 168, 40) else {
        return;
    };
    let [r, g, b, _] = image.get_pixel(20, 20).0;
    // Half of linear blue over black, in sRGB.
    assert!(g < 4 && r < 4, "green showed through: {:?}", [r, g, b]);
    assert!((180..=196).contains(&b), "blue not half faded: {b}");
    let brightest = (44..164)
        .flat_map(|x| (4..28).map(move |y| (x, y)))
        .map(|(x, y)| image.get_pixel(x, y).0[0])
        .max()
        .unwrap_or(0);
    assert!(
        (150..=200).contains(&brightest),
        "text not faded: {brightest}"
    );
}

// A rotated layer moves its corners: a square turned 45 degrees about
// its center leaves its old corner empty and reaches past its old side.
#[test]
fn render_rotated_layer_moves_the_corners() {
    let mut scene = Scene::default();
    let rotate = Transform2D::rotate(std::f32::consts::FRAC_PI_4).around(50.0, 50.0);
    scene.push(layer(1.0, rotate));
    scene.push(solid(rect(30.0, 30.0, 40.0, 40.0), 255, 255, 255));
    scene.push(Primitive::LayerEnd);
    let Some(image) = render_pixels(&scene, 100, 100) else {
        return;
    };
    let lit = |x: u32, y: u32| image.get_pixel(x, y).0[0];
    assert_eq!(lit(31, 31), 0, "old corner still drawn");
    assert_eq!(lit(50, 50), 255, "center missing");
    // The diagonal now points along the axes: 28 px from the center.
    assert_eq!(lit(50 + 26, 50), 255, "tip missing");
    assert_eq!(lit(50, 50 - 26), 255, "tip missing");
    assert_eq!(lit(50 + 30, 50), 0, "drawn past the tip");
}

// -- Paths -------------------------------------------------------------------

fn path(build: impl FnOnce(&mut crate::scene::PathBuilder)) -> Arc<crate::scene::Path> {
    let mut builder = crate::scene::Path::builder();
    build(&mut builder);
    Arc::new(builder.build())
}

// A filled circle covers its inside, leaves its outside, and blends its
// edge pixels.
#[test]
fn render_path_fill_covers_inside_with_antialiased_edge() {
    let circle = path(|b| {
        b.arc(32.0, 32.0, 20.0, 0.0, std::f32::consts::TAU).close();
    });
    let mut scene = Scene::default();
    scene.path(
        crate::scene::PathPrimitive::new(circle, [0.0, 0.0])
            .fill(quark::Color::rgba(255, 255, 255, 255)),
    );
    let Some(image) = render_pixels(&scene, 64, 64) else {
        return;
    };
    let lit = |x: u32, y: u32| image.get_pixel(x, y).0[0];
    assert_eq!(lit(32, 32), 255, "center");
    assert_eq!(lit(32, 14), 255, "inside near the top");
    assert_eq!(lit(32, 10), 0, "outside above");
    assert_eq!(lit(5, 5), 0, "outside corner");
    // An aliased circle has no partly covered pixels; this one has a
    // ring of them around its 126 px perimeter.
    let partial = image
        .pixels()
        .filter(|p| (20..235).contains(&p.0[0]))
        .count();
    assert!(
        partial > 40,
        "edge not antialiased: {partial} partial pixels"
    );
}

// A stroke covers its width around the line and its round cap past the
// end, and nothing beyond.
#[test]
fn render_path_stroke_covers_its_width_and_caps() {
    let line = path(|b| {
        b.move_to(10.0, 20.0).line_to(50.0, 20.0);
    });
    let mut scene = Scene::default();
    scene.path(crate::scene::PathPrimitive::new(line, [0.0, 0.0]).stroke(
        quark::Color::rgba(255, 255, 255, 255),
        crate::scene::StrokeStyle::new(8.0).cap(crate::scene::LineCap::Round),
    ));
    let Some(image) = render_pixels(&scene, 64, 40) else {
        return;
    };
    let lit = |x: u32, y: u32| image.get_pixel(x, y).0[0];
    assert_eq!(lit(30, 17), 255, "inside the width");
    assert_eq!(lit(30, 22), 255, "inside the width");
    assert_eq!(lit(30, 26), 0, "past the width");
    assert_eq!(lit(52, 19), 255, "round cap missing");
    assert_eq!(lit(56, 19), 0, "drawn past the cap");
}

// Regression: a target dropped without `release_offscreen` stayed in use
// forever, which also stopped the pool from trimming any texture.
#[test]
fn dropped_offscreen_target_returns_to_the_pool() {
    let Some(mut renderer) = gpu_renderer(16, 16) else {
        return;
    };
    drop(renderer.acquire_offscreen(64, 64));
    let reused = renderer.acquire_offscreen(32, 32);
    assert_eq!((reused.width, reused.height), (64, 64));
}

// Regression: a minimized window (size 0) kept acquiring and
// reconfiguring its stale surface, and the runner redrew on every
// `SurfaceReconfigured`, spinning a core while minimized.
#[test]
fn render_at_zero_size_skips_the_surface() {
    let Some(mut renderer) = gpu_renderer(16, 16) else {
        return;
    };
    let scene = Scene::default();
    renderer.resize(0, 0, 1.0);
    let skipped = renderer.render(&scene, &mut test_text(), 0.0);
    assert_eq!(skipped.expect("zero-size frame"), FrameStats::default());
    // At a real size the same renderer reaches for its (absent) surface.
    renderer.resize(16, 16, 1.0);
    let drawn = renderer.render(&scene, &mut test_text(), 0.0);
    assert!(matches!(drawn, Err(RenderError::NoSurface)), "{drawn:?}");
}

// Regression: the upload trusted `rgba.len()`, so a buffer shorter than
// width * height * 4 panicked inside wgpu.
#[test]
fn render_image_with_short_pixel_buffer_draws_nothing() {
    let mut scene = Scene::default();
    scene.push(solid(rect(0.0, 0.0, 16.0, 16.0), 0, 0, 255));
    scene.push(Primitive::Image(crate::scene::ImagePrimitive {
        rect: rect(0.0, 0.0, 16.0, 16.0),
        width: 4,
        height: 4,
        rgba: Arc::from(vec![255u8; 16]),
        cache_key: 9,
    }));
    let Some(image) = render_pixels(&scene, 16, 16) else {
        return;
    };
    assert_eq!(image.get_pixel(8, 8).0, [0, 0, 255, 255]);
}

// Regression: a full glyph atlas failed the frame before the end-of-frame
// trim, so its glyphs stayed pinned and every later frame failed too. Now
// that frame draws everything but text and the next frame draws text.
#[test]
fn render_recovers_from_a_full_glyph_atlas() {
    // The text atlas caps itself at the device's texture size limit: at
    // 256px a few dozen large glyphs cannot fit in one frame.
    let limits = wgpu::Limits {
        max_texture_dimension_2d: 256,
        ..wgpu::Limits::default()
    };
    let gpu = match GpuContext::headless_with_limits(limits) {
        Ok(gpu) => gpu,
        Err(error) => {
            assert!(
                std::env::var_os("QUARK_REQUIRE_GPU").is_none(),
                "QUARK_REQUIRE_GPU is set but no capped device: {error}"
            );
            return;
        }
    };
    let mut renderer = Renderer::headless_with_gpu(&gpu, 256, 128, 1.0);
    let mut text = test_text();
    let shaped = |text: &mut TextSystem, s: &str, size: f32| {
        let layout = text.layout(&TextParams::new(s, TextStyle::new(size)));
        ShapedText::new(Arc::new(layout.expect("layout")))
    };
    let white = quark::Color::rgba(255, 255, 255, 255);

    let mut crowded = Scene::default();
    crowded.push(solid(rect(0.0, 0.0, 256.0, 128.0), 0, 0, 255));
    crowded.text(TextPrimitive {
        rect: rect(0.0, 0.0, 256.0, 128.0),
        layout: shaped(
            &mut text,
            "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789",
            120.0,
        ),
        color: white,
    });
    let pixels = renderer
        .render_to_rgba(&crowded, &mut text, 256, 128)
        .expect("a full atlas still renders the frame");
    let crowded = image::RgbaImage::from_raw(256, 128, pixels).expect("pixel buffer size");
    assert_eq!(
        crowded.get_pixel(250, 120).0,
        [0, 0, 255, 255],
        "quad missing"
    );

    let mut sparse = Scene::default();
    sparse.text(TextPrimitive {
        rect: rect(0.0, 0.0, 256.0, 128.0),
        layout: shaped(&mut text, "W", 48.0),
        color: white,
    });
    let pixels = renderer
        .render_to_rgba(&sparse, &mut text, 256, 128)
        .expect("next frame renders");
    let sparse = image::RgbaImage::from_raw(256, 128, pixels).expect("pixel buffer size");
    assert!(
        any_light_pixel(&sparse, rect(0.0, 0.0, 64.0, 64.0)),
        "text did not recover after the atlas filled"
    );
}

/// The frames around a glyph atlas growth: `Kept glyphs` alone, then
/// again beside enough large glyphs that the atlas adds 256 px pages
/// while the first text's glyphs are pinned, then alone once more.
/// Returns each frame's pixels and the atlas counters after it.
fn frames_around_atlas_growth() -> Option<Vec<(image::RgbaImage, TextAtlasStats)>> {
    const SIZE: (u32, u32) = (256, 128);
    let mut renderer = gpu_renderer(SIZE.0, SIZE.1)?;
    renderer.set_text_atlas_limits(TextAtlasLimits {
        page_size: 256,
        ..TextAtlasLimits::default()
    });
    let mut text = test_text();
    let kept = white_text_with(&mut text, rect(4.0, 4.0, 248.0, 24.0), "Kept glyphs");
    let large = text.layout(&TextParams::new(
        "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz",
        TextStyle::new(64.0),
    ));
    let large = Primitive::TextRun(TextPrimitive {
        rect: rect(0.0, 40.0, 256.0, 88.0),
        layout: ShapedText::new(Arc::new(large.expect("layout"))),
        color: quark::Color::rgba(255, 255, 255, 255),
    });
    let mut alone = Scene::default();
    alone.push(kept.clone());
    let mut crowded = alone.clone();
    crowded.push(large);
    let frames = [&alone, &crowded, &alone].map(|scene| {
        let pixels = renderer
            .render_to_rgba(scene, &mut text, SIZE.0, SIZE.1)
            .expect("offscreen render");
        let image = image::RgbaImage::from_raw(SIZE.0, SIZE.1, pixels).expect("pixel size");
        (image, renderer.text_atlas_stats())
    });
    let growths = frames[1].1.growths - frames[0].1.growths;
    assert!(growths > 0, "the large glyphs did not grow the atlas");
    Some(frames.into())
}

// Regression: growing the glyph atlas replaced its texture and
// rasterized and uploaded every cached glyph again. A new page must
// leave cached glyphs alone.
#[test]
fn atlas_growth_rasterizes_no_cached_glyph_again() {
    let Some(frames) = frames_around_atlas_growth() else {
        return;
    };
    let [first, grown, again] = [frames[0].1, frames[1].1, frames[2].1];
    assert_eq!(grown.rerasterized, first.rerasterized);
    // The kept glyphs are all still cached after the growth.
    assert_eq!(again.misses, grown.misses);
    assert_eq!(again.upload_bytes, grown.upload_bytes);
}

// Glyphs cached before the atlas grows draw the same pixels after it,
// both in the frame that grows it and in later ones.
#[test]
fn atlas_growth_keeps_cached_glyph_pixels() {
    let Some(frames) = frames_around_atlas_growth() else {
        return;
    };
    let [first, grown, again] = [&frames[0].0, &frames[1].0, &frames[2].0];
    let kept = rect(0.0, 0.0, 256.0, 32.0);
    assert!(count_pixels(first, kept, |p| p[0] > 128) > 50);
    let differing = |a: &image::RgbaImage, b: &image::RgbaImage, area: Rect| {
        (area.y as u32..area.bottom() as u32)
            .flat_map(|y| (area.x as u32..area.right() as u32).map(move |x| (x, y)))
            .filter(|&(x, y)| a.get_pixel(x, y) != b.get_pixel(x, y))
            .count()
    };
    assert_eq!(differing(first, grown, kept), 0, "frame that grew");
    let whole = rect(0.0, 0.0, 256.0, 128.0);
    assert_eq!(differing(first, again, whole), 0, "frame after");
}

// Two windows share one device: an image uploaded while drawing one
// window draws in another from its cache key alone, which only works
// when both use the same device and texture cache.
#[test]
fn image_uploaded_by_one_renderer_draws_in_another_on_the_same_gpu() {
    let gpu = match GpuContext::headless() {
        Ok(gpu) => gpu,
        Err(_) => {
            assert!(
                std::env::var_os("QUARK_REQUIRE_GPU").is_none(),
                "QUARK_REQUIRE_GPU is set but no wgpu adapter is available"
            );
            return;
        }
    };
    let image = |rgba: Arc<[u8]>| {
        let mut scene = Scene::default();
        scene.push(Primitive::Image(crate::scene::ImagePrimitive {
            rect: rect(0.0, 0.0, 16.0, 16.0),
            width: 1,
            height: 1,
            rgba,
            cache_key: 42,
        }));
        scene
    };
    let mut first = Renderer::headless_with_gpu(&gpu, 16, 16, 1.0);
    let mut second = Renderer::headless_with_gpu(&gpu, 16, 16, 1.0);
    let mut text = test_text();
    first
        .render_to_rgba(&image(Arc::from(vec![0, 255, 0, 255])), &mut text, 16, 16)
        .expect("first render");
    let pixels = second
        .render_to_rgba(&image(Arc::from([])), &mut text, 16, 16)
        .expect("second render");
    assert_eq!(&pixels[..4], &[0, 255, 0, 255]);
}
