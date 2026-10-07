use glyphon::{Color as GlyphonColor, TextArea, TextBounds};
use quark::{Color, FontKind};
use quark_text::{TextLayout, TextParams, TextStyle, TextSystem};

use crate::renderer::{ClippedRichText, ClippedText};
use crate::scene::Rect;

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
