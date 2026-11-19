//! A target's frame flattened into draw order: the clipped instances of
//! each primitive kind, sorted by [`DrawKey`], and the [`DrawStep`]s that
//! slice them into batches.

use std::ops::Range;

use crate::renderer::images::linear_to_srgb_byte;
use crate::renderer::instances::{
    EffectQuadInstance, LayerInstance, PathInstance, QuadInstance, ShadowInstance,
};
use crate::scene::{
    ImagePrimitive, Rect, RichTextPrimitive, StyledTextPrimitive, TextPrimitive, TextRendering,
};
use crate::text::color_to_unit;
use crate::text_engine::GlyphFill;

/// The pipelines a primitive can draw with; each segment of a z-layer
/// holds one kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PrimKind {
    Shadow = 0,
    Effect = 1,
    Quad = 2,
    Image = 3,
    Text = 4,
    Path = 5,
    /// An offscreen layer composited into its parent.
    Layer = 6,
}

pub(crate) const KIND_COUNT: usize = 7;

/// Sort key that puts every primitive in its final draw position: z-layer,
/// then segment within the layer, then paint order within the segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub(crate) struct DrawKey {
    pub(crate) z: i32,
    pub(crate) segment: u32,
    pub(crate) seq: u32,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct FlattenedBlurRegion {
    /// Screen-space bounds of the blur region.
    pub(crate) rect: Rect,
    pub(crate) blur_radius: f32,
    /// Rounded mask of the blurred result: [tl, tr, br, bl].
    pub(crate) corner_radii: [f32; 4],
}

#[derive(Debug, Clone)]
pub(crate) enum DrawStep {
    /// One segment. `items` indexes the kind's sorted array in
    /// `FlattenedScene`; for text, `items` covers plain runs and `rich` covers
    /// rich runs (plain runs of a segment draw before its rich runs).
    Batch {
        kind: PrimKind,
        items: Range<u32>,
        rich: Range<u32>,
    },
    /// Blur everything drawn so far inside the region, then keep drawing on
    /// top of the result. "So far" means final draw order: lower z-layers and
    /// earlier segments of this layer, whatever their scene order. Outside
    /// the region's rounded corners the backdrop stays sharp.
    Blur(FlattenedBlurRegion),
}

/// Flattened scene in draw order. Kept on the renderer and refilled every
/// frame so its vectors keep their capacity.
#[derive(Debug, Default)]
pub(crate) struct FlattenedScene {
    pub(crate) shadows: Vec<ClippedShadow>,
    pub(crate) effect_quads: Vec<ClippedEffectQuad>,
    pub(crate) quads: Vec<ClippedQuad>,
    pub(crate) images: Vec<ClippedImage>,
    pub(crate) texts: Vec<ClippedText>,
    pub(crate) rich_texts: Vec<ClippedRichText>,
    pub(crate) paths: Vec<ClippedPath>,
    pub(crate) layers: Vec<ClippedLayer>,
    pub(crate) steps: Vec<DrawStep>,
}

impl FlattenedScene {
    pub(crate) fn clear(&mut self) {
        self.shadows.clear();
        self.effect_quads.clear();
        self.quads.clear();
        self.images.clear();
        self.texts.clear();
        self.rich_texts.clear();
        self.paths.clear();
        self.layers.clear();
        self.steps.clear();
    }

    /// Put every kind's array into draw order.
    pub(super) fn sort(&mut self) {
        self.shadows.sort_unstable_by_key(|item| item.key);
        self.effect_quads.sort_unstable_by_key(|item| item.key);
        self.quads.sort_unstable_by_key(|item| item.key);
        self.images.sort_unstable_by_key(|item| item.key);
        self.texts.sort_unstable_by_key(|item| item.key);
        self.rich_texts.sort_unstable_by_key(|item| item.key);
        self.paths.sort_by_key(|item| item.key);
        self.layers.sort_unstable_by_key(|item| item.key);
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ClippedShadow {
    pub(crate) key: DrawKey,
    pub(crate) instance: ShadowInstance,
    pub(crate) clip: Rect,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ClippedQuad {
    pub(crate) key: DrawKey,
    pub(crate) instance: QuadInstance,
    pub(crate) clip: Rect,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ClippedEffectQuad {
    pub(crate) key: DrawKey,
    pub(crate) instance: EffectQuadInstance,
    pub(crate) clip: Rect,
}

#[derive(Debug, Clone)]
pub(crate) struct ClippedImage {
    pub(crate) key: DrawKey,
    pub(crate) primitive: ImagePrimitive,
    pub(crate) clip: Rect,
    /// Opacity of an enclosing layer drawn in place.
    pub(crate) alpha: f32,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ClippedPath {
    pub(crate) key: DrawKey,
    pub(crate) instance: PathInstance,
    pub(crate) clip: Rect,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ClippedLayer {
    pub(crate) key: DrawKey,
    pub(crate) instance: LayerInstance,
    pub(crate) clip: Rect,
    /// Frame index of the layer's own target.
    pub(crate) target: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct ClippedText {
    pub(crate) key: DrawKey,
    pub(crate) primitive: TextPrimitive,
    pub(crate) clip: Rect,
    /// Id of the innermost chunk it was drawn from, zero outside chunks.
    pub(crate) run: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct ClippedRichText {
    pub(crate) key: DrawKey,
    pub(crate) primitive: RichTextPrimitive,
    pub(crate) clip: Rect,
    /// Inline layer opacity, applied to each glyph's color as it is
    /// prepared so a fade allocates no faded copy of the span colors.
    pub(crate) alpha: f32,
    /// Id of the innermost chunk it was drawn from, zero outside chunks.
    pub(crate) run: u64,
    /// Fill and coverage of a styled text; default for rich text.
    pub(crate) paint: GlyphPaint,
}

/// How a styled text's glyphs paint beyond their span colors.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct GlyphPaint {
    /// Gradient or shimmer, placed in the target's pixels; `None` paints
    /// the default color.
    pub(crate) fill: Option<GlyphFill>,
    /// sRGB-encoded luminance of the known opaque backdrop, for perceptual
    /// coverage.
    pub(crate) backdrop: Option<u8>,
}

impl GlyphPaint {
    /// Whether glyphs prepared with `self` are the glyphs `other`
    /// prepares: fill values (a shimmer's phase) live in the run's fill
    /// uniform, not in the vertices.
    pub(crate) fn same_vertices(&self, other: &Self) -> bool {
        self.backdrop == other.backdrop && self.fill.is_some() == other.fill.is_some()
    }

    /// The paint of `text` placed with its layout origin at `origin`, in
    /// target pixels, under `rendering` policy.
    pub(super) fn of(text: &StyledTextPrimitive, origin: Rect, policy: TextRendering) -> Self {
        use crate::scene::{ShimmerDirection, TextBackdrop, TextFill};
        let unit = color_to_unit;
        let fill = match text.fill {
            TextFill::Solid(_) => None,
            TextFill::LinearGradient(g) => Some(GlyphFill {
                axis: [
                    origin.x + g.start[0],
                    origin.y + g.start[1],
                    origin.x + g.end[0],
                    origin.y + g.end[1],
                ],
                color_a: unit(g.from),
                color_b: unit(g.to),
                params: [1.0, 0.0, 0.0, 0.0],
            }),
            TextFill::Shimmer(spec) => {
                let band = spec.band_width.max(0.0);
                let width = origin.width.max(0.0);
                let (start, end) = match spec.direction {
                    ShimmerDirection::LeftToRight => (origin.x, origin.right()),
                    ShimmerDirection::RightToLeft => (origin.right(), origin.x),
                };
                let phase = if spec.phase.is_finite() {
                    spec.phase.rem_euclid(1.0)
                } else {
                    0.0
                };
                Some(GlyphFill {
                    axis: [
                        start,
                        origin.y,
                        end + (end - start).signum() * 1.0e-3,
                        origin.y,
                    ],
                    color_a: unit(spec.base),
                    color_b: unit(spec.highlight),
                    // The band enters fully outside the leading edge and
                    // leaves fully past the trailing one.
                    params: [2.0, band * 0.5, -band * 0.5 + phase * (width + band), 0.0],
                })
            }
        };
        let backdrop = match (policy, text.rendering, text.backdrop) {
            (TextRendering::Perceptual, TextRendering::Perceptual, TextBackdrop::Opaque(c)) => {
                Some(srgb_luminance(c))
            }
            _ => None,
        };
        Self { fill, backdrop }
    }
}

/// Relative luminance of `c`, encoded back to sRGB as a byte (the form
/// glyph coverage correction reads).
fn srgb_luminance(c: quark::Color) -> u8 {
    let lin = |v: u8| {
        let v = f32::from(v) / 255.0;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    let l = 0.2126 * lin(c.r) + 0.7152 * lin(c.g) + 0.0722 * lin(c.b);
    linear_to_srgb_byte(l)
}
