use std::collections::HashMap;
use std::ops::ControlFlow;
use std::sync::Arc;

use glyphon::{
    Attrs, AttrsList, AttrsOwned, Buffer, Color as GlyphonColor, FontSystem, LayoutGlyph,
    PositionedGlyph, TextArea, TextAtlas, TextBounds,
};
use quark::scene::ShapedText;
use quark::{Color, FontKind};
use quark_text::{TextLayout, TextParams, TextStyle, TextSystem, TextSystemId};

use crate::renderer::{ClippedRichText, ClippedText, fade_color};
use crate::scene::{Rect, RectPrimitive, Scene, TextDecoration, TextDecorationKind};

/// How text primitives reach glyphon.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum TextPath {
    /// Each layout's glyphs, already positioned, in the color of their
    /// span. Changing colors copies and reshapes nothing.
    #[default]
    Positioned,
    /// One glyphon text area per primitive over the layout's buffer, with
    /// multi-colored layouts drawn from [`RecoloredBuffers`]. The fallback
    /// the positioned path is checked against.
    Buffer,
}

/// The text system whose fonts the renderer's glyph atlas and recolored
/// buffers were filled from. Both hold cosmic-text face ids, which another
/// system's font database hands out again from the start, so after a
/// replacement a cached glyph would draw for an unrelated glyph of the new
/// fonts. A font change within one system keeps its ids.
#[derive(Default)]
pub(super) struct GlyphOwner(Option<TextSystemId>);

impl GlyphOwner {
    /// Records `text` as the system this frame draws with, first emptying
    /// `atlas` and `recolored` when another system filled them.
    pub(super) fn adopt(
        &mut self,
        text: &TextSystem,
        atlas: &mut TextAtlas,
        recolored: &mut RecoloredBuffers,
    ) {
        let system = text.font_epoch().system;
        if self.0.replace(system).is_some_and(|old| old != system) {
            atlas.clear();
            recolored.entries.clear();
        }
    }
}

/// Append the glyphs of `texts` then `rich_texts` to `out`, and the
/// offsets their rows were placed at to `rows`; see
/// [`visit_positioned_glyphs`].
pub(super) fn push_positioned_glyphs(
    texts: &[ClippedText],
    rich_texts: &[ClippedRichText],
    out: &mut Vec<PositionedGlyph>,
    rows: &mut RowOffsets,
) {
    rows.clear();
    let _ = visit_glyphs(
        texts,
        rich_texts,
        |glyph| {
            out.push(glyph);
            ControlFlow::Continue(())
        },
        rows,
    );
}

/// Call `visit` with the glyphs of `texts` then `rich_texts` until it
/// breaks, in glyphon's order for the same primitives as text areas, so
/// overlapping glyphs blend the same way. Nothing is shaped or copied;
/// layouts must come from the `TextSystem` passed to glyphon's prepare,
/// because glyph cache keys carry that font database's face ids.
pub(super) fn visit_positioned_glyphs(
    texts: &[ClippedText],
    rich_texts: &[ClippedRichText],
    visit: impl FnMut(PositionedGlyph) -> ControlFlow<()>,
) -> ControlFlow<()> {
    visit_glyphs(texts, rich_texts, visit, &mut ())
}

fn visit_glyphs(
    texts: &[ClippedText],
    rich_texts: &[ClippedRichText],
    mut visit: impl FnMut(PositionedGlyph) -> ControlFlow<()>,
    rows: &mut impl RowSink,
) -> ControlFlow<()> {
    // Plain loops: nested `flat_map`s cost several times the glyph math.
    for text in texts {
        let primitive = &text.primitive;
        if let Some(layout) = primitive.layout.downcast_ref::<TextLayout>() {
            let color = glyphon_color(primitive.color);
            visit_layout_glyphs(
                layout,
                primitive.rect,
                text.clip,
                &mut visit,
                rows,
                |glyph| glyph.color_opt.unwrap_or(color),
            )?;
        }
        rows.end_text();
    }
    for text in rich_texts {
        let primitive = &text.primitive;
        if let Some(layout) = primitive.layout.downcast_ref::<TextLayout>() {
            let (default_color, span_colors) = (primitive.default_color, &primitive.span_colors);
            visit_layout_glyphs(
                layout,
                primitive.rect,
                text.clip,
                &mut visit,
                rows,
                |glyph| {
                    let color = span_color(glyph.metadata as u32, default_color, span_colors);
                    glyph
                        .color_opt
                        .unwrap_or(glyphon_color(fade_color(color, text.alpha)))
                },
            )?;
        }
        rows.end_text();
    }
    ControlFlow::Continue(())
}

/// Visit `layout`'s glyphs on lines that reach `clip`, placed as glyphon
/// places a text area's: the same physical position, rounded baseline,
/// and line culling, so both paths draw identical pixels. `rows` hears
/// every vertical offset added to `origin.y`: the top of each line
/// checked against the clip, and each glyph's own offset.
///
/// Positions come from the layout's cosmic-text buffer. The glyph columns
/// cannot replace it yet: their `phys_y` adds the baseline before
/// truncating, where glyphon rounds the baseline separately, which moves
/// glyphs on fractional baselines by a pixel. Drawing from the columns
/// needs quark-text to keep each glyph's rounded baseline apart from its
/// offset.
fn visit_layout_glyphs(
    layout: &TextLayout,
    origin: Rect,
    clip: Rect,
    visit: &mut impl FnMut(PositionedGlyph) -> ControlFlow<()>,
    rows: &mut impl RowSink,
    color: impl Fn(&LayoutGlyph) -> GlyphonColor,
) -> ControlFlow<()> {
    // The layout's hit-testing and carets already include this shift.
    let (left, top) = (origin.x + layout.buffer_x(), origin.y);
    let bounds = text_bounds(clip);
    let mut reached = false;
    for run in layout.buffer().layout_runs() {
        rows.offset(run.line_top);
        let start = (top + run.line_top) as i32;
        let end = start + run.line_height as i32;
        if !(start <= bounds.bottom && bounds.top <= end) {
            // Lines run top to bottom: past the visible ones, stop.
            if reached {
                break;
            }
            continue;
        }
        reached = true;
        let line_y = run.line_y.round() as i32;
        for glyph in run.glyphs {
            // What `physical` adds `top` to, before truncating.
            rows.offset(glyph.y - glyph.font_size * glyph.y_offset);
            // The buffer is already shaped at physical size.
            let physical = glyph.physical((left, top), 1.0);
            visit(PositionedGlyph {
                cache_key: physical.cache_key,
                x: physical.x,
                y: physical.y + line_y,
                color: color(glyph),
                bounds,
            })?;
        }
    }
    ControlFlow::Continue(())
}

/// Hears the vertical offsets [`visit_layout_glyphs`] places rows at.
trait RowSink {
    fn offset(&mut self, offset: f32);
    fn end_text(&mut self);
}

impl RowSink for () {
    fn offset(&mut self, _: f32) {}
    fn end_text(&mut self) {}
}

/// Per text of a run whose glyphs were visited, the distinct vertical
/// offsets the visit added to the text's top: every line top it checked
/// against the clip and every glyph's own offset. A glyph's row is that
/// sum truncated, so these decide exactly where the same texts land
/// when only their top changes, without visiting a glyph.
#[derive(Debug, Default)]
pub(super) struct RowOffsets {
    offsets: Vec<f32>,
    /// Per text, the end of its offsets.
    ends: Vec<u32>,
}

impl RowSink for RowOffsets {
    fn offset(&mut self, offset: f32) {
        let start = self.ends.last().map_or(0, |&end| end as usize);
        // Most glyphs share their line's offset; sort the rest out at the
        // end of the text.
        if self.offsets[start..].last().map(|o| o.to_bits()) != Some(offset.to_bits()) {
            self.offsets.push(offset);
        }
    }

    fn end_text(&mut self) {
        let start = self.ends.last().map_or(0, |&end| end as usize);
        let mine = &mut self.offsets[start..];
        mine.sort_unstable_by(f32::total_cmp);
        let len = start + dedup_bits(mine);
        self.offsets.truncate(len);
        self.ends.push(len as u32);
    }
}

/// Move the distinct values (by bits) of sorted `values` to its front and
/// return how many there are.
fn dedup_bits(values: &mut [f32]) -> usize {
    let mut len = 0;
    for i in 0..values.len() {
        if len == 0 || values[len - 1].to_bits() != values[i].to_bits() {
            values[len] = values[i];
            len += 1;
        }
    }
    len
}

impl RowOffsets {
    pub(super) fn clear(&mut self) {
        self.offsets.clear();
        self.ends.clear();
    }

    /// Whether text `index` of the visited run, placed at `was` and
    /// clipped to `was_clip`, then at `now` and clipped to `now_clip`
    /// with the same layout and colors, places exactly the same glyphs
    /// `dy` pixels lower: the same left edge, so the same columns and
    /// subpixel bins; every row offset lands `dy` lower once truncated,
    /// so the same lines pass the clip and every glyph's row moves by
    /// `dy`; and clip bounds moved by `dy`, inside `(width, height)` both
    /// times so glyphon's clamp to the viewport changes none.
    pub(super) fn moved_down(
        &self,
        index: usize,
        (was, was_clip): (Rect, Rect),
        (now, now_clip): (Rect, Rect),
        dy: i32,
        (width, height): (u32, u32),
    ) -> bool {
        let Some(&end) = self.ends.get(index) else {
            return false;
        };
        let start = index.checked_sub(1).map_or(0, |i| self.ends[i]);
        let (before, after) = (text_bounds(was_clip), text_bounds(now_clip));
        let (width, height) = (width as i32, height as i32);
        let inside =
            |b: TextBounds| b.left >= 0 && b.top >= 0 && b.right <= width && b.bottom <= height;
        // Rows far from the target could saturate or overflow on the way.
        const LIMIT: u32 = 1 << 30;
        was.x.to_bits() == now.x.to_bits()
            && after.left == before.left
            && after.right == before.right
            && i64::from(after.top) == i64::from(before.top) + i64::from(dy)
            && i64::from(after.bottom) == i64::from(before.bottom) + i64::from(dy)
            && inside(before)
            && inside(after)
            && self.offsets[start as usize..end as usize]
                .iter()
                .all(|&offset| {
                    let (a, b) = ((was.y + offset) as i32, (now.y + offset) as i32);
                    a.unsigned_abs() < LIMIT
                        && b.unsigned_abs() < LIMIT
                        && i64::from(b) == i64::from(a) + i64::from(dy)
                })
    }
}

/// Builds glyphon areas straight from the shaped layouts; nothing is shaped
/// here. Layouts must come from the `TextSystem` passed to glyphon's prepare,
/// because glyph cache keys carry that font database's face ids.
pub(super) fn prepare_text_areas<'a>(
    texts: &'a [ClippedText],
    rich_texts: &'a [ClippedRichText],
    recolored: &'a RecoloredBuffers,
) -> impl Iterator<Item = TextArea<'a>> + 'a {
    let plain = texts.iter().filter_map(|text| {
        let primitive = &text.primitive;
        let layout = primitive.layout.downcast_ref::<TextLayout>()?;
        Some(text_area(
            layout,
            primitive.rect,
            text.clip,
            primitive.color,
        ))
    });
    let rich = rich_texts.iter().filter_map(|text| {
        let primitive = &text.primitive;
        let layout = primitive.layout.downcast_ref::<TextLayout>()?;
        Some(rich_text_area(layout, text, recolored))
    });
    plain.chain(rich)
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
    alpha: f32,
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
            let alpha = text_run.alpha;
            if uniform_color(layout, default_color, span_colors).is_some() {
                continue;
            }
            let key = std::ptr::from_ref(layout) as usize;
            if let Some(entry) = self.entries.get_mut(&key)
                && entry.default_color == default_color
                && entry.alpha == alpha
                && (Arc::ptr_eq(&entry.span_colors, span_colors)
                    || entry.span_colors == *span_colors)
            {
                entry.last_used = self.frame;
                continue;
            }
            let buffer = recolor(
                layout,
                text.raster_font_system(),
                default_color,
                span_colors,
                alpha,
            );
            self.entries.insert(
                key,
                Recolored {
                    _layout: primitive.layout.clone(),
                    default_color,
                    span_colors: span_colors.clone(),
                    alpha,
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

/// A copy of `layout`'s buffer, reshaped with every glyph colored by its
/// span and faded by `alpha`.
fn recolor(
    layout: &TextLayout,
    font_system: &mut FontSystem,
    default_color: Color,
    span_colors: &[Color],
    alpha: f32,
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
        AttrsOwned::new(&attrs.color(glyphon_color(fade_color(color, alpha))))
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
    text: &ClippedRichText,
    recolored: &'a RecoloredBuffers,
) -> TextArea<'a> {
    let primitive = &text.primitive;
    let (origin, clip, alpha) = (primitive.rect, text.clip, text.alpha);
    let (default_color, span_colors) = (primitive.default_color, &primitive.span_colors);
    if let Some(color) = uniform_color(layout, default_color, span_colors) {
        return text_area(layout, origin, clip, fade_color(color, alpha));
    }
    let mut area = text_area(layout, origin, clip, fade_color(default_color, alpha));
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

/// `color`'s channels in 0..1, still sRGB-encoded and straight alpha. The
/// shaders decode them for a linear target, so cached draws suit targets
/// of either compositing mode.
pub(super) fn color_to_unit(color: Color) -> [f32; 4] {
    [color.r, color.g, color.b, color.a].map(|c| f32::from(c) / 255.0)
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
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| p[0] > 160 && p[1] < 90 && p[2] < 90)
            .count();
        assert!(
            red > 50,
            "{red} red pixels: the emoji drew without its colors"
        );
    }

    /// A font file with every `from` in its names changed to `to`, which
    /// must be as long, so a vendored-only system has no family by the new
    /// name.
    fn renamed(file: &[u8], from: &str, to: &str) -> Vec<u8> {
        let utf16 =
            |name: &str| -> Vec<u8> { name.encode_utf16().flat_map(u16::to_be_bytes).collect() };
        let mut bytes = file.to_vec();
        for (from, to) in [
            (from.as_bytes().to_vec(), to.as_bytes().to_vec()),
            (utf16(from), utf16(to)),
        ] {
            let mut at = 0;
            while let Some(i) = bytes[at..].windows(from.len()).position(|w| w == from) {
                bytes[at + i..at + i + from.len()].copy_from_slice(&to);
                at += i + from.len();
            }
        }
        bytes
    }

    // Regression: the glyph atlas is keyed by face id, and a replacement
    // text system's database hands out the old ids again. A glyph cached
    // from the old fonts drew where the new fonts have another glyph under
    // the same face and glyph id.
    #[test]
    fn replaced_text_system_draws_its_own_glyphs() {
        use crate::renderer::{RenderError, Renderer};
        use crate::scene::{Primitive, TextPrimitive};
        use quark_text::cosmic_text::fontdb;

        fn renderer() -> Option<Renderer> {
            match Renderer::new_headless(48, 48, 1.0) {
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
        /// A vendored-only system that draws its UI text in the font in
        /// `file`, loaded after the vendored faces under a new family.
        fn system(file: &[u8], from: &str, to: &str, family: &str) -> TextSystem {
            let mut system = TextSystem::vendored_only(&FontSettings {
                ui_family: family.into(),
                ..FontSettings::default()
            });
            system.load_font_data(Arc::new(renamed(file, from, to)));
            system
        }
        fn shape(text: &mut TextSystem, s: &str) -> TextLayout {
            text.layout(&TextParams::new(s, TextStyle::new(32.0)))
                .expect("layout")
        }
        fn first_glyph(layout: &TextLayout) -> Option<(fontdb::ID, u16)> {
            let run = layout.buffer().layout_runs().next()?;
            run.glyphs.first().map(|g| (g.font_id, g.glyph_id))
        }
        fn render(renderer: &mut Renderer, text: &mut TextSystem, s: &str) -> Vec<u8> {
            let layout = shape(text, s);
            let mut scene = Scene::default();
            scene.push(Primitive::TextRun(TextPrimitive {
                rect: Rect {
                    x: 4.0,
                    y: 4.0,
                    width: 40.0,
                    height: 40.0,
                },
                layout: ShapedText::new(Arc::new(layout)),
                color: Color::rgba(255, 255, 255, 255),
            }));
            renderer
                .render_to_rgba(&scene, text, 48, 48)
                .expect("offscreen render")
        }

        let Some(mut reused) = renderer() else {
            return;
        };
        let mut old = system(
            include_bytes!("../../quark-text/assets/fonts/Geist-Regular.otf"),
            "Geist",
            "Gaust",
            "Gaust",
        );
        let mut new = system(
            include_bytes!("../../quark-text/assets/fonts/SourceSans3-Regular.ttf"),
            "Source",
            "Sorcer",
            "Sorcer Sans 3",
        );
        // A character the old fonts shape to the same face and glyph id as
        // "M" in the new fonts, so both glyphs share an atlas key.
        let target = first_glyph(&shape(&mut new, "M"));
        assert!(target.is_some(), "M shapes to a glyph");
        let twin = ('!'..'\u{3000}')
            .map(String::from)
            .find(|c| first_glyph(&shape(&mut old, c)) == target)
            .expect("the loaded faces share an id and a glyph id");

        let twin_pixels = render(&mut reused, &mut old, &twin);
        let pixels = render(&mut reused, &mut new, "M");

        let mut fresh = renderer().expect("a second renderer");
        let expected = render(&mut fresh, &mut new, "M");
        assert!(
            twin_pixels != expected,
            "the fonts draw the glyphs differently"
        );
        assert!(pixels == expected, "drew the old fonts' glyph");
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
