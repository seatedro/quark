//! Primitives converted into draws, and draws clipped, faded, and placed
//! into a target's flattened scene.

use std::ops::Range;
use std::sync::{Arc, LazyLock};

use super::flattened::{ClippedEffectQuad, ClippedPath, ClippedQuad, ClippedShadow};
use super::{
    ActiveClip, ClippedImage, ClippedRichText, ClippedText, FlattenedBlurRegion, FlattenedScene,
    Flattener, GlyphPaint, PrimKind,
};
use crate::icons::SvgFonts;
use crate::path::{Band, push_bands, rule_code, stroke_outline, to_kurbo};
use crate::renderer::instances::{
    EFFECT_STRIPES, EffectQuadInstance, PathInstance, QuadInstance, ShadowInstance,
};
use crate::scene::{
    ImagePrimitive, PathPrimitive, Primitive, Rect, RichTextPrimitive, StyledTextPrimitive,
    TextPrimitive,
};
use crate::text::color_to_unit;

/// One primitive's draw before clipping and placement, in the coordinates
/// it was converted at. Chunks keep these across frames.
#[derive(Debug, Clone)]
pub(super) enum Drawn {
    Quad(QuadInstance),
    Shadow(ShadowInstance),
    Effect(EffectQuadInstance),
    Image(ImagePrimitive),
    Text(TextPrimitive),
    RichText(RichTextPrimitive),
    StyledText(StyledTextPrimitive),
    /// A path's fill or stroke: every band shares one key, placed with the
    /// bounds of the whole path.
    Path {
        area: Rect,
        bands: Range<u32>,
    },
    Blur(FlattenedBlurRegion),
}

impl Drawn {
    /// The extent it can draw, where it was converted.
    pub(super) fn extent(&self) -> Rect {
        match self {
            Drawn::Quad(instance) => bounds_rect(instance.bounds),
            Drawn::Shadow(instance) => bounds_rect(instance.draw_bounds),
            Drawn::Effect(instance) => bounds_rect(instance.bounds),
            Drawn::Image(image) => image.rect,
            Drawn::Text(text) => text.rect,
            Drawn::RichText(text) => text.rect,
            Drawn::StyledText(text) => text.rect,
            Drawn::Path { area, .. } => *area,
            Drawn::Blur(blur) => blur.rect,
        }
    }
}

/// How a [`Drawn`] lands this frame.
pub(super) struct Draw<'a> {
    /// Added to the drawn coordinates.
    pub(super) shift: (f32, f32),
    pub(super) clip: ActiveClip,
    pub(super) z: i32,
    /// Opacity of the layers drawn in place around it.
    pub(super) alpha: f32,
    /// Band instances that `Drawn::Path` ranges index, and what their
    /// segment starts are relative to in the frame's segments.
    pub(super) bands: &'a [PathInstance],
    pub(super) segment_base: u32,
    /// Id of the chunk it is drawn from, zero outside chunks: texts of one
    /// chunk prepare their glyphs together.
    pub(super) run: u64,
}

/// The draw of a primitive other than a path, a chunk, or a state change,
/// unclipped and unfaded. `rasterize` says whether an icon's pixels are
/// needed (its texture is not on the GPU).
pub(super) fn convert(
    primitive: &Primitive,
    rasterize: impl FnOnce(u64) -> bool,
    fonts: Option<&SvgFonts>,
) -> Option<Drawn> {
    let quad = |rect: Rect, background, border_color, corner_radii, border_widths| {
        Drawn::Quad(QuadInstance {
            bounds: [rect.x, rect.y, rect.width, rect.height],
            background,
            border_color,
            corner_radii,
            border_widths,
            clip_bounds: [0.0; 4],
            clip_radii: [0.0; 4],
        })
    };
    Some(match primitive {
        Primitive::Rect(rect) => quad(
            rect.rect,
            color_to_unit(rect.color),
            [0.0; 4],
            [0.0; 4],
            [0.0; 4],
        ),
        Primitive::RoundedRect(rect) => quad(
            rect.rect,
            color_to_unit(rect.color),
            [0.0; 4],
            rect.corner_radii,
            [0.0; 4],
        ),
        Primitive::Border(border) => quad(
            border.rect,
            [0.0; 4],
            color_to_unit(border.color),
            border.corner_radii,
            border.widths,
        ),
        Primitive::Shadow(shadow) => convert_shadow(shadow),
        Primitive::TextRun(text) => Drawn::Text(text.clone()),
        Primitive::RichTextRun(text) => Drawn::RichText(text.clone()),
        Primitive::StyledText(text) => Drawn::StyledText(text.clone()),
        Primitive::BlurRegion(blur) => Drawn::Blur(FlattenedBlurRegion {
            rect: blur.rect,
            blur_radius: blur.blur_radius,
            corner_radii: blur.corner_radii,
        }),
        Primitive::EffectQuad(effect) => {
            let rect = effect.rect;
            Drawn::Effect(EffectQuadInstance {
                bounds: [rect.x, rect.y, rect.width, rect.height],
                color_a: color_to_unit(effect.color_a),
                color_b: color_to_unit(effect.color_b),
                params: [
                    effect.effect_type as u32 as f32,
                    effect.params[0],
                    effect.params[1],
                    effect.corner_radius,
                ],
                clip_bounds: [0.0; 4],
                clip_radii: [0.0; 4],
                extra: [0.0; 4],
            })
        }
        Primitive::Stripes(stripes) => convert_stripes(stripes),
        Primitive::Image(img) => Drawn::Image(img.clone()),
        Primitive::Icon(icon) => convert_icon(icon, rasterize, fonts),
        Primitive::Path(_)
        | Primitive::Chunk(_)
        | Primitive::ClipStart(_)
        | Primitive::ClipEnd
        | Primitive::LayerStart(_)
        | Primitive::LayerEnd
        | Primitive::IsolateStart(_)
        | Primitive::IsolateEnd
        | Primitive::ZIndexPush(_)
        | Primitive::ZIndexPop
        | Primitive::LayerBoundary => return None,
    })
}

fn convert_shadow(shadow: &quark::scene::ShadowPrimitive) -> Drawn {
    let sigma = (shadow.blur_radius * 0.5).max(0.5);
    let expansion = sigma * 3.0;
    let rect = shadow.rect;
    let [offset_x, offset_y] = shadow.offset;
    Drawn::Shadow(ShadowInstance {
        draw_bounds: [
            rect.x + offset_x - expansion,
            rect.y + offset_y - expansion,
            rect.width + expansion * 2.0,
            rect.height + expansion * 2.0,
        ],
        shadow_bounds: [
            rect.x + offset_x,
            rect.y + offset_y,
            rect.width,
            rect.height,
        ],
        color: color_to_unit(shadow.color),
        params: [sigma, shadow.corner_radius, 0.0, 0.0],
        clip_bounds: [0.0; 4],
        clip_radii: [0.0; 4],
    })
}

fn convert_stripes(stripes: &quark::scene::StripesPrimitive) -> Drawn {
    let rect = stripes.rect;
    let clean = |v: f32, fallback: f32| if v.is_finite() { v } else { fallback };
    Drawn::Effect(EffectQuadInstance {
        bounds: [rect.x, rect.y, rect.width, rect.height],
        color_a: color_to_unit(stripes.colors[0]),
        color_b: color_to_unit(stripes.colors[1]),
        params: [
            EFFECT_STRIPES,
            clean(stripes.angle, 0.0),
            // Under a pixel, stripes would alias into noise.
            clean(stripes.period, 1.0).max(1.0),
            stripes.corner_radii.iter().copied().fold(0.0, f32::max),
        ],
        clip_bounds: [0.0; 4],
        clip_radii: [0.0; 4],
        extra: [clean(stripes.duty, 0.5).clamp(0.0, 1.0), 0.0, 0.0, 0.0],
    })
}

fn convert_icon(
    icon: &quark::scene::IconPrimitive,
    rasterize: impl FnOnce(u64) -> bool,
    fonts: Option<&SvgFonts>,
) -> Drawn {
    // Whole pixels; moving by whole pixels keeps them whole.
    let rect = Rect {
        x: icon.rect.x.round(),
        y: icon.rect.y.round(),
        width: icon.rect.width.round(),
        height: icon.rect.height.round(),
    };
    let px_size = icon.rect.width.max(icon.rect.height).ceil() as u32;
    let cache_key = crate::icons::cache_key_with(&icon.name, px_size, icon.color, fonts);
    // Once uploaded, the cache key alone is enough to draw.
    let (rgba, width, height) = if rasterize(cache_key) {
        crate::icons::rasterize_svg_with(&icon.name, px_size, icon.color, fonts)
    } else {
        (empty_rgba(), 0, 0)
    };
    Drawn::Image(ImagePrimitive {
        rect,
        width,
        height,
        rgba,
        cache_key,
    })
}

/// Scratch buffers for flattening paths into bands.
#[derive(Debug, Default)]
pub(super) struct BandScratch {
    bands: Vec<Band>,
    points: Vec<[f32; 4]>,
}

/// Append the band instances of `path`'s fill, then its stroke, to
/// `bands`, their segments to `segments` (band segment starts index it),
/// and describe each in `parts`. Unclipped and unfaded, at the path's own
/// origin.
pub(super) fn path_parts(
    path: &PathPrimitive,
    segments: &mut Vec<[f32; 4]>,
    scratch: &mut BandScratch,
    bands: &mut Vec<PathInstance>,
    parts: &mut [Option<Drawn>; 2],
) {
    if path.path.is_empty() || !(path.scale.is_finite() && path.scale > 0.0) {
        return;
    }
    let area = path.bounds();
    let origin = path.origin;
    let geometry = to_kurbo(&path.path, path.scale);
    let fill = path
        .fill
        .map(|fill| (geometry.clone(), fill.color, rule_code(fill.rule)));
    let stroke = path
        .stroke
        .filter(|stroke| stroke.style.width > 0.0)
        .map(|stroke| {
            let outline = stroke_outline(&geometry, &stroke.style, path.scale);
            // Stroke outlines wind consistently and are meant for nonzero.
            (outline, stroke.color, 0.0)
        });
    for (part, outline) in parts.iter_mut().zip([fill, stroke]) {
        let Some((outline, color, rule)) = outline else {
            continue;
        };
        scratch.bands.clear();
        if !push_bands(&outline, segments, &mut scratch.bands, &mut scratch.points) {
            continue;
        }
        let color = color_to_unit(color);
        let start = bands.len() as u32;
        bands.extend(scratch.bands.iter().map(|band| {
            let rect = band.rect.offset(origin[0], origin[1]);
            PathInstance {
                bounds: [rect.x, rect.y, rect.width, rect.height],
                origin_rule: [origin[0], origin[1], rule, 0.0],
                color,
                segments: [band.start, band.count, 0, 0],
                clip_bounds: [0.0; 4],
                clip_radii: [0.0; 4],
            }
        }));
        *part = Some(Drawn::Path {
            area,
            bands: start..bands.len() as u32,
        });
    }
}

/// Clip, place, and append `drawn` as `draw` says.
pub(super) fn emit(drawn: Drawn, draw: &Draw<'_>, fl: &mut Flattener, out: &mut FlattenedScene) {
    let (dx, dy) = draw.shift;
    let clip = draw.clip;
    let alpha = draw.alpha;
    let shift = |bounds: &mut [f32; 4]| {
        bounds[0] += dx;
        bounds[1] += dy;
    };
    let (clip_bounds, clip_radii) = clip.rounded_attributes();
    match drawn {
        Drawn::Quad(mut instance) => {
            shift(&mut instance.bounds);
            let Some(bounds) = bounds_rect(instance.bounds).intersection(clip.scissor) else {
                return;
            };
            let key = fl.place(draw.z, PrimKind::Quad, bounds);
            instance.background = fade(instance.background, alpha);
            instance.border_color = fade(instance.border_color, alpha);
            (instance.clip_bounds, instance.clip_radii) = (clip_bounds, clip_radii);
            out.quads.push(ClippedQuad {
                key,
                instance,
                clip: clip.scissor,
            });
        }
        Drawn::Shadow(mut instance) => {
            shift(&mut instance.draw_bounds);
            shift(&mut instance.shadow_bounds);
            let Some(bounds) = bounds_rect(instance.draw_bounds).intersection(clip.scissor) else {
                return;
            };
            let key = fl.place(draw.z, PrimKind::Shadow, bounds);
            instance.color = fade(instance.color, alpha);
            (instance.clip_bounds, instance.clip_radii) = (clip_bounds, clip_radii);
            out.shadows.push(ClippedShadow {
                key,
                instance,
                clip: clip.scissor,
            });
        }
        Drawn::Effect(mut instance) => {
            shift(&mut instance.bounds);
            let Some(bounds) = bounds_rect(instance.bounds).intersection(clip.scissor) else {
                return;
            };
            let key = fl.place(draw.z, PrimKind::Effect, bounds);
            instance.color_a = fade(instance.color_a, alpha);
            instance.color_b = fade(instance.color_b, alpha);
            (instance.clip_bounds, instance.clip_radii) = (clip_bounds, clip_radii);
            out.effect_quads.push(ClippedEffectQuad {
                key,
                instance,
                clip: clip.scissor,
            });
        }
        Drawn::Image(image) => {
            let rect = image.rect.offset(dx, dy);
            let Some(bounds) = rect.intersection(clip.scissor) else {
                return;
            };
            let key = fl.place(draw.z, PrimKind::Image, bounds);
            out.images.push(ClippedImage {
                key,
                primitive: ImagePrimitive { rect, ..image },
                clip: clip.scissor,
                alpha,
            });
        }
        Drawn::Text(text) => {
            let rect = text.rect.offset(dx, dy);
            let Some(intersection) = rect.intersection(clip.scissor) else {
                return;
            };
            let key = fl.place(draw.z, PrimKind::Text, intersection);
            out.texts.push(ClippedText {
                key,
                primitive: TextPrimitive {
                    rect,
                    color: fade_color(text.color, alpha),
                    ..text
                },
                clip: intersection,
                run: draw.run,
            });
        }
        Drawn::RichText(text) => {
            let rect = text.rect.offset(dx, dy);
            let Some(intersection) = rect.intersection(clip.scissor) else {
                return;
            };
            let key = fl.place(draw.z, PrimKind::Text, intersection);
            out.rich_texts.push(ClippedRichText {
                key,
                primitive: RichTextPrimitive { rect, ..text },
                clip: intersection,
                alpha,
                run: draw.run,
                paint: GlyphPaint::default(),
            });
        }
        Drawn::StyledText(text) => emit_styled_text(text, draw, fl, out),
        Drawn::Blur(blur) => {
            if let Some(rect) = blur.rect.offset(dx, dy).intersection(clip.scissor) {
                fl.barrier(draw.z, FlattenedBlurRegion { rect, ..blur });
            }
        }
        Drawn::Path { area, bands } => emit_path(area, bands, draw, fl, out),
    }
}

fn emit_styled_text(
    text: quark::scene::StyledTextPrimitive,
    draw: &Draw<'_>,
    fl: &mut Flattener,
    out: &mut FlattenedScene,
) {
    let (dx, dy) = draw.shift;
    let rect = text.rect.offset(dx, dy);
    let Some(intersection) = rect.intersection(draw.clip.scissor) else {
        return;
    };
    let key = fl.place(draw.z, PrimKind::Text, intersection);
    let paint = GlyphPaint::of(&text, rect, fl.text_rendering);
    out.rich_texts.push(ClippedRichText {
        key,
        primitive: RichTextPrimitive {
            rect,
            layout: text.layout,
            default_color: text.fill.base_color(),
            span_colors: text.span_colors,
        },
        clip: intersection,
        alpha: draw.alpha,
        run: draw.run,
        paint,
    });
}

fn emit_path(
    area: Rect,
    bands: std::ops::Range<u32>,
    draw: &Draw<'_>,
    fl: &mut Flattener,
    out: &mut FlattenedScene,
) {
    let (dx, dy) = draw.shift;
    let clip = draw.clip;
    let Some(visible) = area.offset(dx, dy).intersection(clip.scissor) else {
        return;
    };
    let (clip_bounds, clip_radii) = clip.rounded_attributes();
    // Fill and stroke each take their own key, so the stroke draws
    // on top; the bands of one shape never overlap.
    let key = fl.place(draw.z, PrimKind::Path, visible);
    for band in &draw.bands[bands.start as usize..bands.end as usize] {
        let mut instance = *band;
        instance.bounds[0] += dx;
        instance.bounds[1] += dy;
        if bounds_rect(instance.bounds)
            .intersection(clip.scissor)
            .is_none()
        {
            continue;
        }
        instance.origin_rule[0] += dx;
        instance.origin_rule[1] += dy;
        instance.segments[0] += draw.segment_base;
        instance.color = fade(instance.color, draw.alpha);
        (instance.clip_bounds, instance.clip_radii) = (clip_bounds, clip_radii);
        out.paths.push(ClippedPath {
            key,
            instance,
            clip: clip.scissor,
        });
    }
}

/// `[x, y, width, height]` as a rect.
fn bounds_rect([x, y, width, height]: [f32; 4]) -> Rect {
    Rect {
        x,
        y,
        width,
        height,
    }
}

/// Fade a straight-alpha color by `alpha`.
fn fade(mut color: [f32; 4], alpha: f32) -> [f32; 4] {
    color[3] *= alpha;
    color
}

/// `color` faded by `alpha`, as text glyphs fade.
pub(crate) fn fade_color(color: quark::Color, alpha: f32) -> quark::Color {
    if alpha >= 1.0 {
        return color;
    }
    quark::Color {
        a: (f32::from(color.a) * alpha).round() as u8,
        ..color
    }
}

/// Shared empty pixel buffer for icons whose texture is already uploaded.
fn empty_rgba() -> Arc<[u8]> {
    static EMPTY: LazyLock<Arc<[u8]>> = LazyLock::new(|| Arc::from([]));
    EMPTY.clone()
}
