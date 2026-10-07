use std::collections::HashMap;
use std::sync::Arc;

use glyphon::{
    Attrs, AttrsList, AttrsOwned, Buffer, Color as GlyphonColor, FontSystem, TextArea, TextBounds,
};
use quark::scene::ShapedText;
use quark::{Color, FontKind};
use quark_text::{TextLayout, TextParams, TextStyle, TextSystem};

use crate::renderer::{ClippedRichText, ClippedText};
use crate::scene::{Rect, RectPrimitive, Scene, TextDecoration, TextDecorationKind};

/// Builds glyphon areas straight from the shaped layouts; nothing is shaped
/// here. Layouts must come from the `TextSystem` passed to glyphon's prepare,
/// because glyph cache keys carry that font database's face ids.
pub(super) fn prepare_text_areas<'a>(
    texts: &'a [ClippedText],
    rich_texts: &'a [ClippedRichText],
    recolored: &'a RecoloredBuffers,
) -> Vec<TextArea<'a>> {
    let mut areas = Vec::with_capacity(texts.len() + rich_texts.len());
    for text in texts {
        let primitive = &text.primitive;
        let Some(layout) = primitive.layout.downcast_ref::<TextLayout>() else {
            continue;
        };
        areas.push(text_area(
            layout,
            primitive.rect,
            text.clip,
            primitive.color,
        ));
    }
    for text in rich_texts {
        let primitive = &text.primitive;
        let Some(layout) = primitive.layout.downcast_ref::<TextLayout>() else {
            continue;
        };
        areas.push(rich_text_area(
            layout,
            primitive.rect,
            text.clip,
            primitive.default_color,
            &primitive.span_colors,
            recolored,
        ));
    }
    areas
}

fn text_area(layout: &TextLayout, origin: Rect, clip: Rect, color: Color) -> TextArea<'_> {
    TextArea {
        buffer: layout.buffer(),
        // The layout's hit-testing and carets already include this shift.
        left: origin.x + layout.buffer_x(),
        top: origin.y,
        // The buffer is already shaped at physical size.
        scale: 1.0,
        bounds: text_bounds(clip),
        default_color: glyphon_color(color),
        custom_glyphs: &[],
    }
}

/// `clip` in whole pixels. `as i32` saturates and maps NaN to 0; an
/// inverted or non-finite clip becomes empty bounds rather than a rect whose
/// right edge lies left of its left edge.
fn text_bounds(clip: Rect) -> TextBounds {
    let left = clip.x.round() as i32;
    let top = clip.y.round() as i32;
    TextBounds {
        left,
        top,
        right: (clip.right().round() as i32).max(left),
        bottom: (clip.bottom().round() as i32).max(top),
    }
}

fn span_color(span: u32, default_color: Color, span_colors: &[Color]) -> Color {
    match span.checked_sub(1) {
        Some(i) => span_colors
            .get(i as usize)
            .copied()
            .unwrap_or(default_color),
        None => default_color,
    }
}

/// The one color every glyph of `layout` draws in, or `None` when its spans
/// use more than one.
fn uniform_color(
    layout: &TextLayout,
    default_color: Color,
    span_colors: &[Color],
) -> Option<Color> {
    let mut runs = layout.glyph_runs();
    let first = runs.next().map_or(default_color, |run| {
        span_color(run.span, default_color, span_colors)
    });
    runs.all(|run| span_color(run.span, default_color, span_colors) == first)
        .then_some(first)
}

/// Copies of multi-colored layouts' buffers with each glyph's color baked
/// in, so a rich text primitive draws as one glyphon area whatever its line
/// and color count. glyphon colors glyphs from the buffer, and the layout's
/// own buffer is shared and immutable; drawing one area per same-colored
/// stretch of each line instead made glyphon walk the buffer's lines once
/// per stretch, quadratic in the line count.
///
/// Building a copy reshapes the layout once, since cosmic-text bakes colors
/// in while shaping. Colors do not change shaping, so the copy's glyphs sit
/// exactly where the layout's do. Copies live while drawn and a few frames
/// after.
#[derive(Default)]
pub(super) struct RecoloredBuffers {
    entries: HashMap<usize, Recolored>,
    frame: u64,
}

struct Recolored {
    /// Keeps the layout alive, so its address keys no other layout.
    _layout: ShapedText,
    default_color: Color,
    span_colors: Arc<[Color]>,
    buffer: Buffer,
    last_used: u64,
}

/// Frames a recolored copy may go undrawn before it is dropped.
const KEEP_UNUSED_RECOLORED_FRAMES: u64 = 120;

impl RecoloredBuffers {
    /// Build or reuse a copy for every multi-colored primitive in the
    /// frame's `targets` (the window and its offscreen layers).
    pub(super) fn prepare<'a>(
        &mut self,
        targets: impl IntoIterator<Item = &'a [ClippedRichText]>,
        text: &mut TextSystem,
    ) {
        self.frame += 1;
        for text_run in targets.into_iter().flatten() {
            let primitive = &text_run.primitive;
            let Some(layout) = primitive.layout.downcast_ref::<TextLayout>() else {
                continue;
            };
            let (default_color, span_colors) = (primitive.default_color, &primitive.span_colors);
            if uniform_color(layout, default_color, span_colors).is_some() {
                continue;
            }
            let key = std::ptr::from_ref(layout) as usize;
            if let Some(entry) = self.entries.get_mut(&key)
                && entry.default_color == default_color
                && (Arc::ptr_eq(&entry.span_colors, span_colors)
                    || entry.span_colors == *span_colors)
            {
                entry.last_used = self.frame;
                continue;
            }
            let buffer = recolor(layout, text.font_system_mut(), default_color, span_colors);
            self.entries.insert(
                key,
                Recolored {
                    _layout: primitive.layout.clone(),
                    default_color,
                    span_colors: span_colors.clone(),
                    buffer,
                    last_used: self.frame,
                },
            );
        }
        let frame = self.frame;
        self.entries
            .retain(|_, entry| frame - entry.last_used <= KEEP_UNUSED_RECOLORED_FRAMES);
    }

    fn get(&self, layout: &TextLayout) -> Option<&Buffer> {
        let key = std::ptr::from_ref(layout) as usize;
        self.entries.get(&key).map(|entry| &entry.buffer)
    }
}

/// A copy of `layout`'s buffer, reshaped with every glyph colored by its span.
fn recolor(
    layout: &TextLayout,
    font_system: &mut FontSystem,
    default_color: Color,
    span_colors: &[Color],
) -> Buffer {
    let source = layout.buffer();
    let mut buffer = Buffer::new_empty(source.metrics());
    buffer.set_wrap(font_system, source.wrap());
    let (width, height) = source.size();
    buffer.set_size(font_system, width, height);
    buffer.set_tab_width(font_system, source.tab_width());
    buffer.set_monospace_width(font_system, source.monospace_width());
    let colored = |attrs: Attrs<'_>| {
        let color = span_color(attrs.metadata as u32, default_color, span_colors);
        AttrsOwned::new(&attrs.color(glyphon_color(color)))
    };
    buffer.lines = source
        .lines
        .iter()
        .map(|line| {
            let source_attrs = line.attrs_list();
            let mut attrs = AttrsList::new(&colored(source_attrs.defaults()).as_attrs());
            for (range, span) in source_attrs.spans_iter() {
                attrs.add_span(range.clone(), &colored(span.as_attrs()).as_attrs());
            }
            let mut line = line.clone();
            line.set_attrs_list(attrs);
            line
        })
        .collect();
    for line in 0..buffer.lines.len() {
        buffer.line_layout(font_system, line);
    }
    buffer
}

fn rich_text_area<'a>(
    layout: &'a TextLayout,
    origin: Rect,
    clip: Rect,
    default_color: Color,
    span_colors: &[Color],
    recolored: &'a RecoloredBuffers,
) -> TextArea<'a> {
    if let Some(color) = uniform_color(layout, default_color, span_colors) {
        return text_area(layout, origin, clip, color);
    }
    let mut area = text_area(layout, origin, clip, default_color);
    // `RecoloredBuffers::prepare` ran over this frame's rich texts, so the
    // copy exists; without it the text still draws, in one color.
    if let Some(buffer) = recolored.get(layout) {
        area.buffer = buffer;
    }
    area
}

/// Quads (scene pixels, layout at `origin`) for one decoration: one per
/// visual line segment its range covers, spanning the glyphs of the range on
/// that line. Trailing whitespace on a line is skipped so a wrapped underline
/// does not run past the last word. Positions come from the line baseline and
/// the font size, since the layout does not carry the font's own
/// underline metrics.
pub fn text_decoration_rects(
    layout: &TextLayout,
    origin: (f32, f32),
    decoration: &TextDecoration,
) -> Vec<Rect> {
    let text = layout.text();
    let size = layout.style().font_size;
    let thickness = (size * 0.07).max(1.0);
    let offset = match decoration.kind {
        // Top of the quad relative to the baseline.
        TextDecorationKind::Underline => size * 0.12,
        TextDecorationKind::Strikethrough => -size * 0.28 - thickness * 0.5,
    };
    let (a, b) = (decoration.range.start, decoration.range.end.min(text.len()));
    let mut out = Vec::new();
    for line in layout.lines() {
        let start = a.max(line.byte_range.start);
        let mut end = b.min(line.byte_range.end);
        if start >= end {
            continue;
        }
        if let Some(trimmed) = text.get(start..end) {
            end = start + trimmed.trim_end().len();
        }
        if start >= end {
            continue;
        }
        let y = origin.1 + line.baseline + offset;
        for r in layout.selection_rects(start..end) {
            // selection_rects can return rects of neighbouring lines when the
            // range touches a line break; keep only this line's.
            if (r.y - line.top).abs() > 0.01 {
                continue;
            }
            out.push(Rect {
                x: origin.0 + r.x,
                y,
                width: r.width,
                height: thickness,
            });
        }
    }
    out
}

/// Paints `decorations` as solid quads. Call right after pushing the text
/// primitive so the lines draw over the glyphs in paint order.
pub fn push_text_decorations(
    scene: &mut Scene,
    layout: &TextLayout,
    origin: (f32, f32),
    decorations: &[TextDecoration],
) {
    for decoration in decorations {
        for rect in text_decoration_rects(layout, origin, decoration) {
            scene.rect(RectPrimitive {
                rect,
                color: decoration.color,
            });
        }
    }
}

/// Average advance of a digit in the monospace face, in physical pixels.
pub(super) fn measure_mono_char_width(text: &mut TextSystem, font_size: f32) -> f32 {
    let params = TextParams::new("0000000000", TextStyle::new(font_size).kind(FontKind::Mono));
    let Ok(layout) = text.layout(&params) else {
        return 8.0;
    };
    let advances = &layout.glyphs().advance;
    if advances.is_empty() {
        return 8.0;
    }
    advances.iter().sum::<f32>() / advances.len() as f32
}

pub(super) fn glyphon_color(color: Color) -> GlyphonColor {
    GlyphonColor::rgba(color.r, color.g, color.b, color.a)
}

pub(super) fn color_to_linear(color: Color) -> [f32; 4] {
    [
        srgb_to_linear(color.r),
        srgb_to_linear(color.g),
        srgb_to_linear(color.b),
        color.a as f32 / 255.0,
    ]
}

fn srgb_to_linear(channel: u8) -> f32 {
    let value = channel as f32 / 255.0;
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

/// One vendored-only system shared by the tests that shape and render;
/// building one parses every vendored font.
#[cfg(test)]
pub(crate) fn test_text() -> std::sync::MutexGuard<'static, TextSystem> {
    use std::sync::{Mutex, OnceLock};
    static TEXT: OnceLock<Mutex<TextSystem>> = OnceLock::new();
    TEXT.get_or_init(|| Mutex::new(TextSystem::vendored_only(&Default::default())))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use quark_text::FontSettings;

    /// Lowest and highest x of the glyphs whose bytes fall in `range` on `line`.
    fn glyph_span(layout: &TextLayout, line: usize, range: std::ops::Range<usize>) -> (f32, f32) {
        let g = layout.glyphs();
        (0..g.len())
            .filter(|&i| g.line[i] as usize == line)
            .filter(|&i| range.contains(&(g.byte_start[i] as usize)))
            .fold((f32::MAX, f32::MIN), |(lo, hi), i| {
                (lo.min(g.x[i]), hi.max(g.x[i] + g.advance[i]))
            })
    }

    // Regression guard for decoration geometry: each quad must lie inside the
    // decorated glyphs of its own line (not the whole line, not the next line),
    // underline below the baseline and strikethrough above it.
    #[test]
    fn decoration_quads_sit_within_their_glyph_runs() {
        let mut system = test_text();
        let text = "plain words then a decorated stretch that wraps onto the next line";
        let start = text.find("decorated").unwrap_or(0);
        let end = text.find(" line").unwrap_or(text.len());
        let params = TextParams::new(text, TextStyle::new(16.0)).wrap_width(Some(220.0));
        let layout = system.layout(&params).expect("layout");
        let origin = (10.0, 20.0);

        for kind in [
            TextDecorationKind::Underline,
            TextDecorationKind::Strikethrough,
        ] {
            let decoration = TextDecoration {
                range: start..end,
                kind,
                color: Color::rgba(255, 0, 0, 255),
            };
            let rects = text_decoration_rects(&layout, origin, &decoration);
            let covered: Vec<usize> = layout
                .lines()
                .enumerate()
                .filter(|(_, l)| l.byte_range.start < end && l.byte_range.end > start)
                .map(|(i, _)| i)
                .collect();
            assert!(covered.len() > 1, "fixture must wrap");
            assert_eq!(rects.len(), covered.len(), "{kind:?}: one quad per line");
            for (rect, &line_i) in rects.iter().zip(&covered) {
                let line = layout.line(line_i).expect("line");
                let (lo, hi) = glyph_span(&layout, line_i, start..end);
                let (x0, x1) = (rect.x - origin.0, rect.right() - origin.0);
                assert!(
                    x0 >= lo - 0.01 && x1 <= hi + 0.01,
                    "{kind:?} line {line_i}: {x0}..{x1} outside glyphs {lo}..{hi}"
                );
                let (y0, y1) = (rect.y - origin.1, rect.bottom() - origin.1);
                assert!(
                    y0 >= line.top && y1 <= line.top + line.height,
                    "{kind:?} outside line box"
                );
                match kind {
                    TextDecorationKind::Underline => assert!(y0 > line.baseline),
                    TextDecorationKind::Strikethrough => assert!(y1 < line.baseline),
                }
            }
        }
    }

    // Regression guard for color glyphs: an emoji must draw from its color
    // bitmap, not as a coverage mask tinted with the text color. The text is
    // white on black, so a strongly red pixel can only be the heart's own.
    #[test]
    fn color_emoji_draws_in_its_own_colors() {
        use crate::renderer::{RenderError, Renderer};
        use crate::scene::{Primitive, TextPrimitive};

        let mut renderer = match Renderer::new_headless(64, 64, 1.0) {
            Ok(renderer) => renderer,
            Err(RenderError::NoAdapter) => {
                assert!(
                    std::env::var_os("QUARK_REQUIRE_GPU").is_none(),
                    "QUARK_REQUIRE_GPU is set but no wgpu adapter is available"
                );
                return;
            }
            Err(error) => panic!("headless renderer failed: {error}"),
        };
        let mut text = test_text();
        let params = TextParams::new("\u{2764}\u{fe0f}", TextStyle::new(40.0));
        let layout = text.layout(&params).expect("layout");
        let mut scene = Scene::default();
        scene.push(Primitive::TextRun(TextPrimitive {
            rect: Rect {
                x: 4.0,
                y: 4.0,
                width: 56.0,
                height: 56.0,
            },
            layout: ShapedText::new(Arc::new(layout)),
            color: Color::rgba(255, 255, 255, 255),
        }));
        let pixels = renderer
            .render_to_rgba(&scene, &mut text, 64, 64)
            .expect("offscreen render");
        let red = pixels
            .chunks_exact(4)
            .filter(|p| p[0] > 160 && p[1] < 90 && p[2] < 90)
            .count();
        assert!(
            red > 50,
            "{red} red pixels: the emoji drew without its colors"
        );
    }

    // Regression: the default vendored faces (Geist, Geist Mono) have no
    // italic, so italic spans painted upright. They must be slanted
    // synthetically, while a family that ships an italic face (JetBrains
    // Mono) uses it instead of being slanted twice, including after the
    // fonts change on a running system.
    #[test]
    fn italic_spans_without_an_italic_face_are_slanted() {
        use quark::scene::FontStyle;
        use quark_text::TextSpan;
        use quark_text::cosmic_text::CacheKeyFlags;

        fn slanted(system: &mut TextSystem, kind: FontKind) -> bool {
            let span = TextSpan {
                range: 0..6,
                weight: None,
                style: Some(FontStyle::Italic),
                kind: Some(kind),
            };
            let params = TextParams::new("italic", TextStyle::new(14.0)).spans(vec![span]);
            let layout = system.layout(&params).expect("layout");
            let flags = &layout.glyphs().flags;
            flags.iter().all(|f| f.contains(CacheKeyFlags::FAKE_ITALIC))
        }

        // Its own system: changing fonts on the shared one would race other
        // tests.
        let mut system = TextSystem::vendored_only(&FontSettings::default());
        assert!(slanted(&mut system, FontKind::Ui), "Geist upright");
        assert!(slanted(&mut system, FontKind::Mono), "Geist Mono upright");
        system.set_font_settings(&FontSettings {
            mono_family: "JetBrains Mono".to_owned(),
            ..FontSettings::default()
        });
        assert!(
            !slanted(&mut system, FontKind::Mono),
            "JetBrains Mono slanted twice"
        );
    }
}
