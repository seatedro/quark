//! Layer planning.
//!
//! A layer fades and transforms its content as one group. Most layers never
//! touch a texture: one at zero opacity (or with a degenerate transform) is
//! skipped, an opaque one that only translates draws in place at whole
//! pixels, and a translated one with a single primitive draws in place with
//! that primitive's alpha scaled (one primitive cannot show through itself).
//! Everything else renders into a pooled offscreen texture sized to its
//! visible content plus a transparent pixel, then composites as one textured
//! quad through the layer's transform with its opacity. Isolated groups plan
//! the same way: in place, clipped to their bounds, unless a mask or a
//! compositing mode of their own needs a target.

use super::flatten::rect_union;
use crate::scene::{AlphaMask, Primitive, Rect, Transform2D, UiCompositing};

/// Opacity below this skips a layer; above one minus it, a layer is opaque.
const ALPHA_EPSILON: f32 = 1.0 / 512.0;

/// Largest side of a layer texture; larger content is cut off.
const MAX_LAYER_SIDE: f32 = 8192.0;

/// How a layer or isolated group draws this frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum LayerMode {
    /// Nothing of it shows.
    Skip,
    /// Draw the content in place, moved by whole pixels and faded.
    Inline { dx: f32, dy: f32, alpha: f32 },
    /// Render the content into a target of its own, then composite it.
    Offscreen(OffscreenLayer),
}

/// The target an offscreen layer renders into.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct OffscreenLayer {
    /// Whole-pixel origin, in the layer's content coordinates, and size of
    /// the region it renders.
    pub(crate) origin: (f32, f32),
    pub(crate) size: (u32, u32),
    /// Size of the pooled texture it renders into (at least `size`), once
    /// the frame acquires it.
    pub(crate) texture: (u32, u32),
    /// Frame index of its target, numbered from 1 in scene order.
    pub(crate) target: usize,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct LayerPlan {
    /// Index of the `LayerStart` in the scene.
    pub(crate) start: usize,
    /// Index of the matching `LayerEnd` (the scene length when missing).
    pub(crate) end: usize,
    pub(crate) mode: LayerMode,
    pub(crate) opacity: f32,
    transform: Transform2D,
    /// The layer's content composites in encoded sRGB (its own target
    /// holds encoded values).
    pub(crate) encoded: bool,
    /// An isolated group's bounds, in its content coordinates (which are
    /// its parent's): content outside them is clipped away.
    pub(crate) isolate: Option<Rect>,
    /// An isolated group's mask, in its content coordinates.
    pub(crate) mask: Option<AlphaMask>,
    /// Whether the group must render offscreen whatever its content: a
    /// mask, or a compositing mode other than its parent's.
    forced: bool,
}

impl LayerPlan {
    /// Map from the texels of `layer`, this plan's target, to the parent's
    /// content coordinates.
    pub(crate) fn composite(&self, layer: &OffscreenLayer) -> Transform2D {
        Transform2D::translate(layer.origin.0, layer.origin.1).then(self.transform)
    }
}

struct OpenLayer {
    plan: usize,
    clip_depth: usize,
    /// Visible content bounds so far, in the layer's content coordinates.
    bounds: Option<Rect>,
    /// Primitives drawn in the layer, with nested layers counting as many.
    drawables: u32,
}

/// Working storage of [`plan_layers`], kept so planning a frame with layers
/// allocates nothing once warm.
#[derive(Default)]
pub(crate) struct LayerScratch {
    open: Vec<OpenLayer>,
    /// Clip rects in the coordinates of the innermost open layer's content.
    clips: Vec<Rect>,
}

/// Plan every layer and isolated group of `scene` (in scene order) and
/// number the offscreen ones' targets from 1. Returns how many render
/// offscreen. The root target holds encoded values when `encoded`.
pub(crate) fn plan_layers(
    prims: &[Primitive],
    viewport: Rect,
    encoded: bool,
    plans: &mut Vec<LayerPlan>,
    scratch: &mut LayerScratch,
) -> usize {
    plans.clear();
    if !prims
        .iter()
        .any(|p| matches!(p, Primitive::LayerStart(_) | Primitive::IsolateStart(_)))
    {
        return 0;
    }
    let LayerScratch { open, clips } = scratch;
    open.clear();
    clips.clear();
    clips.push(viewport);
    for (index, primitive) in prims.iter().enumerate() {
        let parent_encoded = |open: &[OpenLayer], plans: &[LayerPlan]| {
            open.last().map_or(encoded, |l| plans[l.plan].encoded)
        };
        match primitive {
            Primitive::LayerStart(layer) => {
                let outer = *clips.last().expect("root clip");
                let content_clip = layer
                    .transform
                    .invert()
                    .map_or(Rect::default(), |inverse| inverse.map_rect_bounds(outer));
                let opacity = if layer.opacity.is_nan() {
                    0.0
                } else {
                    layer.opacity.clamp(0.0, 1.0)
                };
                plans.push(LayerPlan {
                    start: index,
                    end: prims.len(),
                    mode: LayerMode::Skip,
                    opacity,
                    transform: layer.transform,
                    encoded: parent_encoded(open, plans),
                    isolate: None,
                    mask: None,
                    forced: false,
                });
                open_layer(open, plans.len() - 1, clips, content_clip);
            }
            Primitive::IsolateStart(isolate) => {
                let outer = *clips.last().expect("root clip");
                let parent = parent_encoded(open, plans);
                let own = isolate
                    .compositing
                    .map_or(parent, |mode| mode == UiCompositing::WebCompatible);
                plans.push(LayerPlan {
                    start: index,
                    end: prims.len(),
                    mode: LayerMode::Skip,
                    opacity: 1.0,
                    transform: Transform2D::IDENTITY,
                    encoded: own,
                    isolate: Some(isolate.bounds),
                    mask: isolate.mask,
                    forced: isolate.mask.is_some() || own != parent,
                });
                let content_clip = outer.intersection(isolate.bounds).unwrap_or_default();
                open_layer(open, plans.len() - 1, clips, content_clip);
            }
            Primitive::LayerEnd | Primitive::IsolateEnd => {
                if let Some(layer) = open.pop() {
                    close_layer(layer, index, plans, open, clips);
                }
            }
            Primitive::ClipStart(clip) => {
                let top = *clips.last().expect("root clip");
                clips.push(top.intersection(clip.rect).unwrap_or_default());
            }
            Primitive::ClipEnd => {
                let floor = open.last().map_or(1, |layer| layer.clip_depth + 1);
                if clips.len() > floor {
                    clips.pop();
                }
            }
            _ => {
                if let Some(layer) = open.last_mut()
                    && let Some(bounds) = paint_bounds(primitive)
                {
                    layer.drawables += 1;
                    if let Some(visible) = bounds.intersection(*clips.last().expect("root clip")) {
                        layer.bounds =
                            Some(layer.bounds.map_or(visible, |b| rect_union(b, visible)));
                    }
                }
            }
        }
    }
    while let Some(layer) = open.pop() {
        close_layer(layer, prims.len(), plans, open, clips);
    }
    let mut targets = 0;
    for plan in plans.iter_mut() {
        if let LayerMode::Offscreen(layer) = &mut plan.mode {
            targets += 1;
            layer.target = targets;
        }
    }
    targets
}

/// Open plan `plan`, whose content is clipped to `content_clip`.
fn open_layer(open: &mut Vec<OpenLayer>, plan: usize, clips: &mut Vec<Rect>, content_clip: Rect) {
    open.push(OpenLayer {
        plan,
        clip_depth: clips.len(),
        bounds: None,
        drawables: 0,
    });
    clips.push(content_clip);
}

fn close_layer(
    layer: OpenLayer,
    end: usize,
    plans: &mut [LayerPlan],
    open: &mut [OpenLayer],
    clips: &mut Vec<Rect>,
) {
    clips.truncate(layer.clip_depth);
    let plan = &mut plans[layer.plan];
    plan.end = end;
    plan.mode = layer_mode(plan, layer.bounds, layer.drawables);
    let landed = match plan.mode {
        LayerMode::Skip => {
            // Layers nested in a skipped one are never reached.
            for nested in &mut plans[layer.plan + 1..] {
                nested.mode = LayerMode::Skip;
            }
            None
        }
        LayerMode::Inline { dx, dy, .. } => layer.bounds.map(|b| b.offset(dx, dy)),
        LayerMode::Offscreen(offscreen) => Some(plan.composite(&offscreen).map_rect_bounds(Rect {
            x: 0.0,
            y: 0.0,
            width: offscreen.size.0 as f32,
            height: offscreen.size.1 as f32,
        })),
    };
    if let Some(parent) = open.last_mut()
        && let Some(landed) = landed
    {
        // A nested group can overlap itself, so its parent cannot fade it
        // per primitive.
        parent.drawables += 2;
        if let Some(visible) = landed.intersection(*clips.last().expect("root clip")) {
            parent.bounds = Some(parent.bounds.map_or(visible, |b| rect_union(b, visible)));
        }
    }
}

/// How `plan` draws, its content covering `bounds` with `drawables`
/// primitives. A translation that draws in place is rounded to whole
/// pixels.
fn layer_mode(plan: &mut LayerPlan, bounds: Option<Rect>, drawables: u32) -> LayerMode {
    let Some(bounds) = bounds.filter(|b| {
        b.x.is_finite() && b.y.is_finite() && b.width.is_finite() && b.height.is_finite()
    }) else {
        return LayerMode::Skip;
    };
    if plan.opacity <= ALPHA_EPSILON || plan.transform.invert().is_none() {
        return LayerMode::Skip;
    }
    let transform = plan.transform;
    if plan.isolate.is_some() && !plan.forced {
        // Nothing to apply at composite time: draw in place, clipped.
        return LayerMode::Inline {
            dx: 0.0,
            dy: 0.0,
            alpha: 1.0,
        };
    }
    if transform.is_translation() && !plan.forced {
        // Whole-pixel moves keep text and edges as sharp as unmoved ones.
        let (dx, dy) = (transform.tx.round(), transform.ty.round());
        plan.transform = Transform2D::translate(dx, dy);
        if plan.opacity >= 1.0 - ALPHA_EPSILON {
            return LayerMode::Inline { dx, dy, alpha: 1.0 };
        }
        if drawables <= 1 {
            return LayerMode::Inline {
                dx,
                dy,
                alpha: plan.opacity,
            };
        }
    }
    // A transparent pixel around the content lets bilinear sampling
    // antialias the edges of a rotated or scaled layer.
    let x0 = bounds.x.floor() - 1.0;
    let y0 = bounds.y.floor() - 1.0;
    let width = (bounds.right().ceil() + 1.0 - x0).clamp(1.0, MAX_LAYER_SIDE);
    let height = (bounds.bottom().ceil() + 1.0 - y0).clamp(1.0, MAX_LAYER_SIDE);
    LayerMode::Offscreen(OffscreenLayer {
        origin: (x0, y0),
        size: (width as u32, height as u32),
        texture: (0, 0),
        target: 0,
    })
}

/// Whether a chunk of `prims` starts a layer, is drawn through one, or
/// sits inside one.
pub(crate) fn needs_expansion(prims: &[Primitive]) -> bool {
    let mut depth = 0u32;
    prims.iter().any(|primitive| match primitive {
        Primitive::LayerStart(_) | Primitive::IsolateStart(_) => {
            depth += 1;
            false
        }
        Primitive::LayerEnd | Primitive::IsolateEnd => {
            depth = depth.saturating_sub(1);
            false
        }
        Primitive::Chunk(chunk) => depth > 0 || chunk.needs_layer() || chunk.chunk.has_layers(),
        _ => false,
    })
}

/// Push `primitive` onto `out`, replacing a chunk that starts a layer or
/// sits inside one (`depth` layers deep) with its primitives, and a chunk
/// drawn through a layer with that layer around them.
pub(crate) fn expand(primitive: Primitive, depth: &mut u32, out: &mut Vec<Primitive>) {
    match primitive {
        Primitive::LayerStart(_) | Primitive::IsolateStart(_) => *depth += 1,
        Primitive::LayerEnd | Primitive::IsolateEnd => *depth = depth.saturating_sub(1),
        Primitive::Chunk(ref chunk) if chunk.needs_layer() => {
            out.push(Primitive::LayerStart(chunk.layer_primitive()));
            *depth += 1;
            chunk.for_each_placed(|placed| expand(placed, depth, out));
            *depth -= 1;
            out.push(Primitive::LayerEnd);
            return;
        }
        Primitive::Chunk(ref chunk) if *depth > 0 || chunk.chunk.has_layers() => {
            let [dx, dy] = chunk.layer_shift();
            chunk.for_each_placed(|mut placed| {
                placed.offset(dx, dy);
                expand(placed, depth, out);
            });
            return;
        }
        _ => {}
    }
    out.push(primitive);
}

/// Scene bounds a primitive can paint, or `None` for state changes.
fn paint_bounds(primitive: &Primitive) -> Option<Rect> {
    // Glyphs can overhang their layout box (italics, accents).
    const GLYPH_OVERHANG: f32 = 4.0;
    match primitive {
        Primitive::Rect(p) => Some(p.rect),
        Primitive::RoundedRect(p) => Some(p.rect),
        Primitive::Border(p) => Some(p.rect),
        Primitive::Shadow(p) => {
            let reach = (p.blur_radius * 0.5).max(0.5) * 3.0;
            Some(Rect {
                x: p.rect.x + p.offset[0] - reach,
                y: p.rect.y + p.offset[1] - reach,
                width: p.rect.width + reach * 2.0,
                height: p.rect.height + reach * 2.0,
            })
        }
        Primitive::TextRun(p) => Some(p.rect.inset(-GLYPH_OVERHANG)),
        Primitive::RichTextRun(p) => Some(p.rect.inset(-GLYPH_OVERHANG)),
        Primitive::StyledText(p) => Some(p.rect.inset(-GLYPH_OVERHANG)),
        Primitive::Stripes(p) => Some(p.rect),
        Primitive::Icon(p) => Some(p.rect),
        Primitive::Image(p) => Some(p.rect),
        Primitive::EffectQuad(p) => Some(p.rect),
        Primitive::BlurRegion(p) => Some(p.rect),
        Primitive::Path(p) => Some(p.bounds()),
        Primitive::ClipStart(_)
        | Primitive::ClipEnd
        | Primitive::ZIndexPush(_)
        | Primitive::ZIndexPop
        | Primitive::LayerStart(_)
        | Primitive::LayerEnd
        | Primitive::IsolateStart(_)
        | Primitive::IsolateEnd
        | Primitive::LayerBoundary
        | Primitive::Chunk(_) => None,
    }
}
