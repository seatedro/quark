//! Flattening a target's primitives into draw order.
//!
//! Draw order follows paint order across primitive kinds. Each z-layer is
//! split into ordered segments, one primitive kind per segment, and each
//! segment is one batched draw (per scissor clip). A primitive joins the
//! most recent segment of its kind unless something of another kind
//! painted after that segment overlaps it, in which case it opens a new
//! segment ([`zbatch`]). Disjoint content (list rows: background, text,
//! background, text) therefore stays in two batches, while a selection quad
//! painted over text gets its own segment above the text. Overlap tests
//! use the clipped bounds of each primitive, with a chunked bounding-box
//! prefilter so long frames stay near linear.
//!
//! [`Flattener::flatten`] walks the primitives, keeping the clip, z-index,
//! and inline layer stacks; [`draw`] converts and emits each primitive, and
//! [`chunks`] replays chunks flattened in earlier frames. The result is a
//! [`FlattenedScene`].

mod chunks;
mod draw;
mod flattened;
mod zbatch;

use std::ops::Range;

pub(crate) use draw::fade_color;
pub(crate) use flattened::{
    ClippedImage, ClippedLayer, ClippedRichText, ClippedText, DrawKey, DrawStep,
    FlattenedBlurRegion, FlattenedScene, GlyphPaint, KIND_COUNT, PrimKind,
};

use self::draw::{BandScratch, Draw, convert, emit, path_parts};
use self::zbatch::{SegmentSlot, ZBuilder};
use super::images::ImageCache;
use super::instances::{LayerInstance, PathInstance, SourceSpace};
use super::layers::{LayerMode, LayerPlan, OffscreenLayer};
use crate::icons::SvgFonts;
use crate::scene::{ClipPrimitive, Primitive, Rect, TextRendering, Transform2D};
use quark_text::TextSystem;

/// Reusable state for flattening targets, kept across frames.
#[derive(Debug, Default)]
pub(super) struct Flattener {
    builders: Vec<ZBuilder>,
    active: usize,
    clips: Vec<ActiveClip>,
    z_stack: Vec<i32>,
    seq: u32,
    /// Subtracted from scene coordinates: the target's texture origin, moved
    /// by the layers drawn in place around the current primitive.
    origin: (f32, f32),
    /// Opacity of the layers drawn in place around the current primitive.
    alpha: f32,
    /// `origin` and `alpha` outside each layer drawn in place.
    inline: Vec<((f32, f32), f32)>,
    /// Per isolated group drawn in place, the clip stack depth to return
    /// to at its end.
    isolates: Vec<usize>,
    /// Path segments of every target this frame, in segment texture order.
    segments: Vec<[f32; 4]>,
    band_scratch: BandScratch,
    /// Band instances of the path being drawn outside any chunk.
    path_bands: Vec<PathInstance>,
    /// Flattened chunks kept across frames.
    chunks: chunks::ChunkCache,
    /// The renderer's coverage policy for styled text.
    text_rendering: TextRendering,
    /// The target being flattened holds encoded values.
    encoded: bool,
    /// Fonts SVG text in icons draws with.
    svg_fonts: Option<SvgFonts>,
}

/// The primitives one target draws, and where.
pub(super) struct FlattenTarget<'a> {
    pub(super) prims: &'a [Primitive],
    pub(super) range: Range<usize>,
    /// Subtracted from scene coordinates, so an offscreen layer draws its
    /// content at its own texture origin.
    pub(super) origin: (f32, f32),
    /// The region of the target that shows, in its own pixels.
    pub(super) viewport: Rect,
    /// The target holds encoded values.
    pub(super) encoded: bool,
}

impl Flattener {
    /// Start a frame whose styled text draws under `text_rendering`.
    pub(super) fn begin_frame(&mut self, text_rendering: TextRendering) {
        self.text_rendering = text_rendering;
        self.segments.clear();
        self.chunks.begin_frame();
    }

    /// Snapshot `text`'s fonts for SVG text when they changed since the
    /// last snapshot (a new system or a font change), dropping flattened
    /// chunks whose icons were rasterized with the old ones.
    pub(super) fn adopt_svg_fonts(&mut self, text: &TextSystem) {
        let epoch = text.font_epoch();
        if self.svg_fonts.as_ref().map(SvgFonts::epoch) != Some(epoch) {
            if self.svg_fonts.is_some() {
                self.chunks = chunks::ChunkCache::default();
            }
            self.svg_fonts = Some(SvgFonts::new(text));
        }
    }

    /// Path segments of every target flattened this frame, which the
    /// renderer pads to whole texture rows as it uploads them.
    pub(super) fn segments_mut(&mut self) -> &mut Vec<[f32; 4]> {
        &mut self.segments
    }

    fn clip(&self) -> ActiveClip {
        *self.clips.last().expect("root clip")
    }

    fn z(&self) -> i32 {
        self.z_stack.last().copied().unwrap_or(0)
    }

    /// Flatten `target` into `out`'s draw steps. Layers draw per their
    /// plan in `plans`: skipped, inline, or as one composite of their
    /// texture (their content is a target of its own). Icons whose key
    /// `images` holds are not rasterized again.
    pub(super) fn flatten(
        &mut self,
        target: &FlattenTarget<'_>,
        plans: &[LayerPlan],
        images: &ImageCache,
        out: &mut FlattenedScene,
    ) {
        out.clear();
        self.active = 0;
        self.seq = 0;
        self.clips.clear();
        self.clips.push(ActiveClip::root(target.viewport));
        self.z_stack.clear();
        self.z_stack.push(0);
        self.origin = target.origin;
        self.alpha = 1.0;
        self.inline.clear();
        self.isolates.clear();
        self.encoded = target.encoded;

        let prims = target.prims;
        let plan_at = |index: usize| {
            plans
                .binary_search_by_key(&index, |plan| plan.start)
                .ok()
                .map(|at| &plans[at])
        };
        let mut index = target.range.start;
        while index < target.range.end {
            let primitive = &prims[index];
            index += 1;
            match primitive {
                Primitive::Path(path) => self.draw_path(path, out),
                Primitive::Chunk(chunk) => chunks::draw_chunk(chunk, self, out),
                Primitive::Rect(_)
                | Primitive::RoundedRect(_)
                | Primitive::Border(_)
                | Primitive::Shadow(_)
                | Primitive::TextRun(_)
                | Primitive::RichTextRun(_)
                | Primitive::StyledText(_)
                | Primitive::Stripes(_)
                | Primitive::BlurRegion(_)
                | Primitive::EffectQuad(_)
                | Primitive::Image(_)
                | Primitive::Icon(_) => {
                    let draw = Draw {
                        shift: (-self.origin.0, -self.origin.1),
                        clip: self.clip(),
                        z: self.z(),
                        alpha: self.alpha,
                        bands: &[],
                        segment_base: 0,
                        run: 0,
                    };
                    // Icons already on the GPU skip rasterizing.
                    let rasterize = |key| !images.contains_key(&key);
                    if let Some(drawn) = convert(primitive, rasterize, self.svg_fonts.as_ref()) {
                        emit(drawn, &draw, self, out);
                    }
                }
                Primitive::ClipStart(ClipPrimitive { rect, corner_radii }) => {
                    let rect = rect.offset(-self.origin.0, -self.origin.1);
                    self.clips.push(self.clip().push(rect, *corner_radii));
                }
                Primitive::ClipEnd => {
                    if self.clips.len() > 1 {
                        self.clips.pop();
                    }
                }
                Primitive::ZIndexPush(z) => self.z_stack.push(*z),
                Primitive::ZIndexPop => {
                    if self.z_stack.len() > 1 {
                        self.z_stack.pop();
                    }
                }
                Primitive::LayerStart(_) => {
                    let Some(plan) = plan_at(index - 1) else {
                        continue;
                    };
                    match plan.mode {
                        LayerMode::Skip => index = plan.end + 1,
                        LayerMode::Inline { dx, dy, alpha } => {
                            self.inline.push((self.origin, self.alpha));
                            self.origin = (self.origin.0 - dx, self.origin.1 - dy);
                            self.alpha *= alpha;
                        }
                        LayerMode::Offscreen(layer) => {
                            self.push_layer(plan, &layer, &mut out.layers);
                            index = plan.end + 1;
                        }
                    }
                }
                Primitive::LayerEnd => {
                    if let Some((origin, alpha)) = self.inline.pop() {
                        self.origin = origin;
                        self.alpha = alpha;
                    }
                }
                Primitive::IsolateStart(isolate) => {
                    let Some(plan) = plan_at(index - 1) else {
                        continue;
                    };
                    match plan.mode {
                        LayerMode::Skip => index = plan.end + 1,
                        LayerMode::Inline { .. } => {
                            // In place, clipped to its bounds.
                            self.isolates.push(self.clips.len());
                            let rect = isolate.bounds.offset(-self.origin.0, -self.origin.1);
                            self.clips.push(self.clip().push(rect, [0.0; 4]));
                        }
                        LayerMode::Offscreen(layer) => {
                            self.push_layer(plan, &layer, &mut out.layers);
                            index = plan.end + 1;
                        }
                    }
                }
                Primitive::IsolateEnd => {
                    if let Some(depth) = self.isolates.pop() {
                        self.clips.truncate(depth.max(1));
                    }
                }
                Primitive::LayerBoundary => {}
            }
        }
        self.slice_into_steps(out);
    }

    /// Append the bands of `path`'s fill and stroke, when any of it shows.
    fn draw_path(&mut self, path: &crate::scene::PathPrimitive, out: &mut FlattenedScene) {
        // Bands are worth building only for a visible path.
        let shift = (-self.origin.0, -self.origin.1);
        let clip = self.clip();
        if path
            .bounds()
            .offset(shift.0, shift.1)
            .intersection(clip.scissor)
            .is_none()
        {
            return;
        }
        let mut bands = std::mem::take(&mut self.path_bands);
        bands.clear();
        let mut parts = [None, None];
        path_parts(
            path,
            &mut self.segments,
            &mut self.band_scratch,
            &mut bands,
            &mut parts,
        );
        let draw = Draw {
            shift,
            clip,
            z: self.z(),
            alpha: self.alpha,
            bands: &bands,
            segment_base: 0,
            run: 0,
        };
        for part in parts.into_iter().flatten() {
            emit(part, &draw, self, out);
        }
        self.path_bands = bands;
    }

    /// Append the composite of offscreen layer `plan` into the current
    /// target.
    fn push_layer(
        &mut self,
        plan: &LayerPlan,
        layer: &OffscreenLayer,
        out: &mut Vec<ClippedLayer>,
    ) {
        let map = plan
            .composite(layer)
            .then(Transform2D::translate(-self.origin.0, -self.origin.1));
        let (w, h) = (layer.size.0 as f32, layer.size.1 as f32);
        let clip = self.clip();
        let footprint = map.map_rect_bounds(Rect {
            x: 0.0,
            y: 0.0,
            width: w,
            height: h,
        });
        let Some(bounds) = footprint.intersection(clip.scissor) else {
            return;
        };
        let key = self.place(self.z(), PrimKind::Layer, bounds);
        // Isolated groups never transform, so the mask only moves with the
        // target origin.
        let (mask_axis, mask_offsets, mask_alphas) =
            LayerInstance::mask_fields(plan.mask.as_ref(), (-self.origin.0, -self.origin.1));
        let (clip_bounds, clip_radii) = clip.rounded_attributes();
        out.push(ClippedLayer {
            key,
            instance: LayerInstance {
                linear: [map.a, map.b, map.c, map.d],
                offset_size: [map.tx, map.ty, w, h],
                params: [
                    1.0 / layer.texture.0.max(1) as f32,
                    1.0 / layer.texture.1.max(1) as f32,
                    plan.opacity * self.alpha,
                    SourceSpace::between(plan.encoded, self.encoded).code(),
                ],
                clip_bounds,
                clip_radii,
                mask_axis,
                mask_offsets,
                mask_alphas,
            },
            clip: clip.scissor,
            target: layer.target,
        });
    }

    /// Sort `out` into draw order, then walk the z-layers' segment lists
    /// to slice its arrays into steps.
    fn slice_into_steps(&mut self, out: &mut FlattenedScene) {
        out.sort();
        let builders = &mut self.builders[..self.active];
        builders.sort_unstable_by_key(|builder| builder.z);
        let mut cursors = [0usize; KIND_COUNT];
        let mut rich_cursor = 0usize;
        for builder in builders.iter() {
            for (segment, slot) in builder.segments.iter().enumerate() {
                let (z, segment) = (builder.z, segment as u32);
                match *slot {
                    SegmentSlot::Blur(region) => out.steps.push(DrawStep::Blur(region)),
                    SegmentSlot::Batch(kind) => {
                        let cursor = &mut cursors[kind as usize];
                        let items = match kind {
                            PrimKind::Shadow => {
                                take_run(&out.shadows, cursor, z, segment, |i| i.key)
                            }
                            PrimKind::Effect => {
                                take_run(&out.effect_quads, cursor, z, segment, |i| i.key)
                            }
                            PrimKind::Quad => take_run(&out.quads, cursor, z, segment, |i| i.key),
                            PrimKind::Image => take_run(&out.images, cursor, z, segment, |i| i.key),
                            PrimKind::Text => take_run(&out.texts, cursor, z, segment, |i| i.key),
                            PrimKind::Path => take_run(&out.paths, cursor, z, segment, |i| i.key),
                            PrimKind::Layer => take_run(&out.layers, cursor, z, segment, |i| i.key),
                        };
                        let rich = if kind == PrimKind::Text {
                            take_run(&out.rich_texts, &mut rich_cursor, z, segment, |i| i.key)
                        } else {
                            0..0
                        };
                        if !items.is_empty() || !rich.is_empty() {
                            out.steps.push(DrawStep::Batch { kind, items, rich });
                        }
                    }
                }
            }
        }
    }
}

/// Advance `cursor` over the items keyed to (`z`, `segment`) and return them.
fn take_run<T>(
    items: &[T],
    cursor: &mut usize,
    z: i32,
    segment: u32,
    key: impl Fn(&T) -> DrawKey,
) -> Range<u32> {
    let start = *cursor;
    while *cursor < items.len() {
        let k = key(&items[*cursor]);
        if k.z != z || k.segment != segment {
            break;
        }
        *cursor += 1;
    }
    start as u32..*cursor as u32
}

/// The clip in effect at a point of the walk.
///
/// `scissor` is the intersection of every rect clip active there; it maps
/// directly to `set_scissor_rect` for rectangular culling, and is empty
/// when those clips share no pixel. `rounded` is the innermost rounded
/// clip, if any: a rect clip pushed inside a rounded one keeps it, so
/// descendants of a rounded container are still clipped to its rounded
/// boundary. Nested rounded clips keep only the inner one, which is good
/// enough since they are rare.
#[derive(Debug, Clone, Copy, PartialEq)]
struct ActiveClip {
    scissor: Rect,
    rounded: Option<RoundedClip>,
}

/// A clip rect with at least one rounded corner.
#[derive(Debug, Clone, Copy, PartialEq)]
struct RoundedClip {
    rect: Rect,
    /// [top left, top right, bottom right, bottom left].
    radii: [f32; 4],
}

impl RoundedClip {
    /// `rect` with `radii`, or `None` when no corner is rounded.
    fn new(rect: Rect, radii: [f32; 4]) -> Option<Self> {
        radii
            .iter()
            .any(|&r| r > 0.0)
            .then_some(Self { rect, radii })
    }
}

impl ActiveClip {
    /// Nothing shows.
    const EMPTY: Self = Self {
        scissor: Rect {
            x: 0.0,
            y: 0.0,
            width: 0.0,
            height: 0.0,
        },
        rounded: None,
    };

    fn root(viewport: Rect) -> Self {
        Self {
            scissor: viewport,
            rounded: None,
        }
    }

    /// This clip with `rect`, rounded by `corner_radii`, pushed inside it.
    fn push(&self, rect: Rect, corner_radii: [f32; 4]) -> Self {
        match self.scissor.intersection(rect) {
            Some(scissor) => Self {
                scissor,
                rounded: RoundedClip::new(rect, corner_radii).or(self.rounded),
            },
            None => Self::EMPTY,
        }
    }

    /// The `clip_bounds` and `clip_radii` instance attributes: all zero
    /// without a rounded clip.
    fn rounded_attributes(&self) -> ([f32; 4], [f32; 4]) {
        match self.rounded {
            Some(RoundedClip { rect, radii }) => ([rect.x, rect.y, rect.width, rect.height], radii),
            None => ([0.0; 4], [0.0; 4]),
        }
    }
}

/// The smallest rect holding both `a` and `b`.
pub(super) fn rect_union(a: Rect, b: Rect) -> Rect {
    let x = a.x.min(b.x);
    let y = a.y.min(b.y);
    Rect {
        x,
        y,
        width: a.right().max(b.right()) - x,
        height: a.bottom().max(b.bottom()) - y,
    }
}

/// Whether `a` and `b` share area.
fn rects_overlap(a: Rect, b: Rect) -> bool {
    a.x < b.right() && b.x < a.right() && a.y < b.bottom() && b.y < a.bottom()
}

/// `scene` flattened into one target showing `viewport`, as the
/// renderer's own target would be.
#[cfg(test)]
pub(super) fn flatten_scene(
    scene: &crate::scene::Scene,
    viewport: Rect,
    images: &ImageCache,
) -> FlattenedScene {
    let mut out = FlattenedScene::default();
    let mut plans = Vec::new();
    super::layers::plan_layers(
        &scene.primitives,
        viewport,
        false,
        &mut plans,
        &mut super::layers::LayerScratch::default(),
    );
    let target = FlattenTarget {
        prims: &scene.primitives,
        range: 0..scene.primitives.len(),
        origin: (0.0, 0.0),
        viewport,
        encoded: false,
    };
    Flattener::default().flatten(&target, &plans, images, &mut out);
    out
}
