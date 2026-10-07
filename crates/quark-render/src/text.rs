use glyphon::{Color as GlyphonColor, TextArea, TextBounds};
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
        push_rich_text_areas(
            &mut areas,
            layout,
            primitive.rect,
            text.clip,
            primitive.default_color,
            &primitive.span_colors,
        );
    }
    areas
}

fn text_area(layout: &TextLayout, origin: Rect, clip: Rect, color: Color) -> TextArea<'_> {
    TextArea {
        buffer: layout.buffer(),
        left: origin.x,
        top: origin.y,
        // The buffer is already shaped at physical size.
        scale: 1.0,
        bounds: TextBounds {
            left: clip.x.round() as i32,
            top: clip.y.round() as i32,
            right: clip.right().round() as i32,
            bottom: clip.bottom().round() as i32,
        },
        default_color: glyphon_color(color),
        custom_glyphs: &[],
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

/// glyphon colors a whole area with one default color, and the layout's
/// buffer is shared and immutable, so per-span colors are drawn as one area
/// per same-colored stretch of a line, each clipped to that stretch. Bounds
/// clip partial glyphs, so neighbouring stretches tile without gaps or
/// double drawing.
fn push_rich_text_areas<'a>(
    areas: &mut Vec<TextArea<'a>>,
    layout: &'a TextLayout,
    origin: Rect,
    clip: Rect,
    default_color: Color,
    span_colors: &[Color],
) {
    let mut runs = layout.glyph_runs();
    let first = runs.next().map_or(default_color, |run| {
        span_color(run.span, default_color, span_colors)
    });
    if runs.all(|run| span_color(run.span, default_color, span_colors) == first) {
        areas.push(text_area(layout, origin, clip, first));
        return;
    }

    let glyphs = layout.glyphs();
    let scale = layout.scale_factor();
    let line_count = layout.line_count();
    // (x0, x1, color) per run of the current line, in logical pixels.
    let mut stretches: Vec<(f32, f32, Color)> = Vec::new();
    let mut runs = layout.glyph_runs().peekable();
    while let Some(line) = runs.peek().map(|run| run.line) {
        stretches.clear();
        while let Some(run) = runs.next_if(|run| run.line == line) {
            let (x0, x1) = run
                .glyphs
                .clone()
                .fold((f32::MAX, f32::MIN), |(lo, hi), i| {
                    (lo.min(glyphs.x[i]), hi.max(glyphs.x[i] + glyphs.advance[i]))
                });
            stretches.push((x0, x1, span_color(run.span, default_color, span_colors)));
        }
        stretches.sort_by(|a, b| a.0.total_cmp(&b.0));
        stretches.dedup_by(|next, prev| {
            let same = next.2 == prev.2;
            if same {
                prev.1 = prev.1.max(next.1);
            }
            same
        });
        let Some(info) = layout.line(line) else {
            continue;
        };
        // Outer edges extend to the clip so overhanging ink is not cut.
        let top = if line == 0 {
            clip.y
        } else {
            origin.y + info.top * scale
        };
        let bottom = if line + 1 == line_count {
            clip.bottom()
        } else {
            origin.y + (info.top + info.height) * scale
        };
        for (k, &(x0, _, color)) in stretches.iter().enumerate() {
            let left = if k == 0 {
                clip.x
            } else {
                origin.x + x0 * scale
            };
            let right = stretches
                .get(k + 1)
                .map_or(clip.right(), |next| origin.x + next.0 * scale);
            let stretch = Rect {
                x: left,
                y: top,
                width: right - left,
                height: bottom - top,
            };
            if let Some(bounds) = stretch.intersection(clip) {
                areas.push(text_area(layout, origin, bounds, color));
            }
        }
    }
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
        let mut system = TextSystem::vendored_only(&FontSettings::default());
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
}
