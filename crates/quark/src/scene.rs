//! Scene — immediate-mode primitive container emitted by the paint phase.
//!
//! Pure data: a `Vec<Primitive>` plus convenience builders. The renderer
//! consumes the scene; quark itself does not render.

use std::any::Any;
use std::fmt;
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::color::Color;
use crate::geometry::Rect;
use crate::path::{FillRule, Path, StrokeStyle};
use crate::transform::Transform2D;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FontKind {
    #[default]
    Ui,
    Mono,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FontWeight {
    #[default]
    Normal,
    Medium,
    Semibold,
    Bold,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FontStyle {
    #[default]
    Normal,
    Italic,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Scene {
    pub primitives: Vec<Primitive>,
}

impl Scene {
    pub fn push(&mut self, primitive: Primitive) {
        self.primitives.push(primitive);
    }

    pub fn rect(&mut self, rect: RectPrimitive) {
        self.push(Primitive::Rect(rect));
    }

    pub fn rounded_rect(&mut self, rect: RoundedRectPrimitive) {
        self.push(Primitive::RoundedRect(rect));
    }

    pub fn border(&mut self, border: BorderPrimitive) {
        self.push(Primitive::Border(border));
    }

    pub fn shadow(&mut self, shadow: ShadowPrimitive) {
        self.push(Primitive::Shadow(shadow));
    }

    pub fn text(&mut self, text: TextPrimitive) {
        self.push(Primitive::TextRun(text));
    }

    pub fn rich_text(&mut self, text: RichTextPrimitive) {
        self.push(Primitive::RichTextRun(text));
    }

    pub fn image(&mut self, image: ImagePrimitive) {
        self.push(Primitive::Image(image));
    }

    pub fn blur_region(&mut self, blur: BlurRegionPrimitive) {
        self.push(Primitive::BlurRegion(blur));
    }

    pub fn effect_quad(&mut self, effect: EffectQuadPrimitive) {
        self.push(Primitive::EffectQuad(effect));
    }

    pub fn path(&mut self, path: PathPrimitive) {
        self.push(Primitive::Path(path));
    }

    /// Start a layer: everything pushed until the matching
    /// [`Self::pop_layer`] fades by `opacity` as one group and moves by
    /// `transform`. The renderer skips the layer at zero opacity, draws it
    /// in place when it is opaque and only translated, and otherwise
    /// composites it from an offscreen texture.
    pub fn push_layer(&mut self, opacity: f32, transform: Transform2D) {
        self.push(Primitive::LayerStart(LayerPrimitive { opacity, transform }));
    }

    pub fn pop_layer(&mut self) {
        self.push(Primitive::LayerEnd);
    }

    pub fn clip(&mut self, rect: Rect) {
        self.push(Primitive::ClipStart(ClipPrimitive {
            rect,
            corner_radii: [0.0; 4],
        }));
    }

    pub fn clip_rounded(&mut self, rect: Rect, corner_radii: [f32; 4]) {
        self.push(Primitive::ClipStart(ClipPrimitive { rect, corner_radii }));
    }

    pub fn pop_clip(&mut self) {
        self.push(Primitive::ClipEnd);
    }

    pub fn push_z_index(&mut self, z: i32) {
        self.push(Primitive::ZIndexPush(z));
    }

    pub fn pop_z_index(&mut self) {
        self.push(Primitive::ZIndexPop);
    }

    /// Draw `chunk` with its origin at `offset`; see [`ChunkPrimitive`].
    pub fn chunk(&mut self, chunk: &Arc<SceneChunk>, offset: [f32; 2]) {
        self.push(Primitive::Chunk(ChunkPrimitive {
            chunk: Arc::clone(chunk),
            offset,
            scale: None,
        }));
    }

    /// The scene with every chunk replaced by the primitives it draws, as
    /// they land. Allocates; for tests and tools that read primitives.
    pub fn expanded(&self) -> Vec<Primitive> {
        let mut out = Vec::with_capacity(self.primitives.len());
        for primitive in &self.primitives {
            primitive.expand_into(&mut out);
        }
        out
    }

    pub fn len(&self) -> usize {
        self.primitives.len()
    }

    pub fn is_empty(&self) -> bool {
        self.primitives.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Primitive {
    Rect(RectPrimitive),
    RoundedRect(RoundedRectPrimitive),
    Border(BorderPrimitive),
    Shadow(ShadowPrimitive),
    TextRun(TextPrimitive),
    RichTextRun(RichTextPrimitive),
    Icon(IconPrimitive),
    Image(ImagePrimitive),
    EffectQuad(EffectQuadPrimitive),
    /// Start a frosted-glass blur region. Content rendered before this
    /// primitive (within the given bounds) will be blurred and composited
    /// as a backdrop before children are painted on top.
    BlurRegion(BlurRegionPrimitive),
    /// A filled and/or stroked vector path.
    Path(PathPrimitive),
    ClipStart(ClipPrimitive),
    ClipEnd,
    /// Start a group that fades and transforms as one; see
    /// [`Scene::push_layer`]. Z-indices inside a layer order its content
    /// only, as a CSS stacking context does.
    LayerStart(LayerPrimitive),
    LayerEnd,
    /// Push a z-index context. Primitives inside render on top of lower z-indices.
    ZIndexPush(i32),
    /// Pop the current z-index context.
    ZIndexPop,
    LayerBoundary,
    /// Primitives recorded once and drawn in place of this one; see
    /// [`ChunkPrimitive`].
    Chunk(ChunkPrimitive),
}

impl Primitive {
    pub fn offset(&mut self, dx: f32, dy: f32) {
        match self {
            Self::Rect(p) => p.rect = p.rect.offset(dx, dy),
            Self::RoundedRect(p) => p.rect = p.rect.offset(dx, dy),
            Self::Border(p) => p.rect = p.rect.offset(dx, dy),
            Self::Shadow(p) => p.rect = p.rect.offset(dx, dy),
            Self::TextRun(p) => p.rect = p.rect.offset(dx, dy),
            Self::RichTextRun(p) => p.rect = p.rect.offset(dx, dy),
            Self::Icon(p) => p.rect = p.rect.offset(dx, dy),
            Self::Image(p) => p.rect = p.rect.offset(dx, dy),
            Self::EffectQuad(p) => p.rect = p.rect.offset(dx, dy),
            Self::BlurRegion(p) => p.rect = p.rect.offset(dx, dy),
            Self::ClipStart(p) => p.rect = p.rect.offset(dx, dy),
            Self::Path(p) => {
                p.origin[0] += dx;
                p.origin[1] += dy;
            }
            Self::LayerStart(p) => p.transform = p.transform.offset(dx, dy),
            Self::Chunk(p) => {
                // The offset applies before the scale.
                let s = p.scale.unwrap_or(1.0);
                p.offset[0] += dx / s;
                p.offset[1] += dy / s;
            }
            Self::ClipEnd
            | Self::ZIndexPush(_)
            | Self::ZIndexPop
            | Self::LayerBoundary
            | Self::LayerEnd => {}
        }
    }

    /// Multiply every coordinate by `s`, turning logical points into
    /// physical pixels. Rect edges snap to whole pixels so quads stay sharp
    /// and neighbours tile without seams; a non-empty rect or border never
    /// snaps away to nothing. Radii, blur, and shadow offsets scale
    /// unsnapped. Path origins snap like rect edges, and their geometry
    /// scales through the primitive's `scale`. Layer transforms keep their
    /// rotation and scale and move by scaled pixels. Pixel-based effect
    /// parameters (noise frequency) scale so an effect looks the same at
    /// every scale factor. A chunk keeps its recording and converts each
    /// primitive as it is drawn, after moving it by the chunk's offset.
    ///
    /// Text origins snap too. Glyphs are not resized here: a text layout
    /// must already be shaped at `s`, which puts its glyphs in physical
    /// pixels.
    pub fn to_physical(&mut self, s: f32) {
        self.make_physical(s, [0.0; 2]);
    }

    /// [`Self::to_physical`], with `shift` physical pixels added to every
    /// coordinate before it snaps.
    fn make_physical(&mut self, s: f32, shift: [f32; 2]) {
        let snapped = |rect| snap(rect, s, shift);
        match self {
            Self::Rect(p) => p.rect = snapped(p.rect),
            Self::RoundedRect(p) => {
                p.rect = snapped(p.rect);
                p.corner_radii = p.corner_radii.map(|r| r * s);
            }
            Self::Border(p) => {
                p.rect = snapped(p.rect);
                p.widths = p.widths.map(|w| snap_length(w, s));
                p.corner_radii = p.corner_radii.map(|r| r * s);
            }
            Self::Shadow(p) => {
                p.rect = snapped(p.rect);
                p.blur_radius *= s;
                p.corner_radius *= s;
                p.offset = p.offset.map(|o| o * s);
            }
            Self::TextRun(p) => p.rect = snapped(p.rect),
            Self::RichTextRun(p) => p.rect = snapped(p.rect),
            Self::Icon(p) => p.rect = snapped(p.rect),
            Self::Image(p) => p.rect = snapped(p.rect),
            Self::EffectQuad(p) => {
                p.rect = snapped(p.rect);
                p.corner_radius *= s;
                // Noise is sampled per physical pixel; its frequency is per point.
                if p.effect_type == EffectType::NoiseGradient && s > 0.0 {
                    p.params[0] /= s;
                }
            }
            Self::BlurRegion(p) => {
                p.rect = snapped(p.rect);
                p.blur_radius *= s;
                p.corner_radii = p.corner_radii.map(|r| r * s);
            }
            Self::Path(p) => {
                p.origin = [0, 1].map(|i| round_half_up(p.origin[i] * s + shift[i]));
                p.scale *= s;
            }
            Self::LayerStart(p) => {
                p.transform = p.transform.in_scaled_space(s).offset(shift[0], shift[1]);
            }
            Self::ClipStart(p) => {
                p.rect = snapped(p.rect);
                p.corner_radii = p.corner_radii.map(|r| r * s);
            }
            // Only `to_physical` reaches a chunk here, with no shift:
            // placing moves nested chunks itself.
            Self::Chunk(p) => p.scale = Some(p.scale.unwrap_or(1.0) * s),
            Self::ClipEnd
            | Self::ZIndexPush(_)
            | Self::ZIndexPop
            | Self::LayerEnd
            | Self::LayerBoundary => {}
        }
    }

    /// Push this primitive onto `out`, or a chunk's primitives as they
    /// land, recursively.
    fn expand_into(&self, out: &mut Vec<Primitive>) {
        match self {
            Self::Chunk(chunk) => chunk.for_each_placed(|placed| placed.expand_into(out)),
            other => out.push(other.clone()),
        }
    }
}

/// Scale, shift by `shift` physical pixels, and round both edges, so
/// adjacent rects share a pixel edge.
fn snap(rect: Rect, s: f32, [dx, dy]: [f32; 2]) -> Rect {
    let x0 = round_half_up(rect.x * s + dx);
    let y0 = round_half_up(rect.y * s + dy);
    let mut x1 = round_half_up((rect.x + rect.width) * s + dx);
    let mut y1 = round_half_up((rect.y + rect.height) * s + dy);
    if rect.width > 0.0 && x1 <= x0 {
        x1 = x0 + 1.0;
    }
    if rect.height > 0.0 && y1 <= y0 {
        y1 = y0 + 1.0;
    }
    Rect {
        x: x0,
        y: y0,
        width: x1 - x0,
        height: y1 - y0,
    }
}

/// `v` rounded to the nearest whole pixel, halves up. Unlike
/// [`f32::round`], which rounds halves away from zero, this commutes with
/// moving by whole pixels across zero, so a chunk snapped around its own
/// origin snaps like its primitives pushed where it lands.
fn round_half_up(v: f32) -> f32 {
    let r = v.round();
    // `v - r` is exact: `r` is within half a pixel of `v`.
    if v - r == 0.5 { r + 1.0 } else { r }
}

/// A stroke width in whole pixels, at least one when it is drawn at all.
fn snap_length(length: f32, s: f32) -> f32 {
    if length > 0.0 {
        (length * s).round().max(1.0)
    } else {
        0.0
    }
}

/// Source of chunk ids; zero is never handed out.
static NEXT_CHUNK_ID: AtomicU64 = AtomicU64::new(1);

/// Primitives recorded once (a cached subtree's paint output, relative to
/// its origin) and drawn by reference from any number of scenes through
/// [`ChunkPrimitive`]s. Scenes share it through an `Arc` and never change
/// it; the recorder refills it only while no scene holds it, which bumps
/// its generation. A renderer may therefore keep work derived from a chunk
/// under its `(id, generation)` and reuse it while both match.
#[derive(Debug)]
pub struct SceneChunk {
    id: u64,
    generation: u64,
    primitives: Vec<Primitive>,
    /// Whether any primitive, nested chunks included, starts a layer.
    has_layers: bool,
}

impl Default for SceneChunk {
    fn default() -> Self {
        Self {
            id: NEXT_CHUNK_ID.fetch_add(1, Ordering::Relaxed),
            generation: 0,
            primitives: Vec::new(),
            has_layers: false,
        }
    }
}

impl SceneChunk {
    pub fn new() -> Self {
        Self::default()
    }

    /// Unique for the life of the process.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Bumped every time the content changes.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn primitives(&self) -> &[Primitive] {
        &self.primitives
    }

    pub fn is_empty(&self) -> bool {
        self.primitives.is_empty()
    }

    /// Whether drawing the chunk starts a layer (here or in a nested
    /// chunk).
    pub fn has_layers(&self) -> bool {
        self.has_layers
    }

    /// Replace the content with `primitives`, keeping the buffer.
    pub fn replace(&mut self, primitives: impl IntoIterator<Item = Primitive>) {
        self.primitives.clear();
        self.primitives.extend(primitives);
        self.has_layers = self.primitives.iter().any(|p| match p {
            Primitive::LayerStart(_) => true,
            Primitive::Chunk(chunk) => chunk.chunk.has_layers,
            _ => false,
        });
        self.generation += 1;
    }

    /// Drop the content, keeping the buffer.
    pub fn clear(&mut self) {
        if !self.primitives.is_empty() {
            self.replace(std::iter::empty());
        }
    }
}

/// Draws a [`SceneChunk`]'s primitives in its place, each moved by
/// `offset` and then, once the scene is in physical pixels, converted by
/// [`Primitive::to_physical`] at `scale`. Drawing the chunk paints what
/// pushing those primitives here would (exactly, while the sums are exact
/// in `f32`); a nested chunk's offset adds to this one's.
///
/// In physical pixels each primitive snaps around the chunk's
/// [`pixel_origin`](Self::pixel_origin): shifted by its fraction, snapped,
/// then moved by its whole pixels. Two placements with the same fraction
/// therefore snap every edge alike, and differ by exactly the difference
/// of their whole pixels.
#[derive(Debug, Clone)]
pub struct ChunkPrimitive {
    pub chunk: Arc<SceneChunk>,
    /// Where the chunk's origin lands, before `scale`.
    pub offset: [f32; 2],
    /// The scale [`Primitive::to_physical`] converted the scene at; `None`
    /// while the scene is in logical points.
    pub scale: Option<f32>,
}

impl ChunkPrimitive {
    /// Call `f` with each of the chunk's primitives as it lands here.
    pub fn for_each_placed(&self, mut f: impl FnMut(Primitive)) {
        for primitive in &self.chunk.primitives {
            f(self.place(primitive));
        }
    }

    /// `primitive`, one of the chunk's, as it lands here.
    pub fn place(&self, primitive: &Primitive) -> Primitive {
        let mut placed = primitive.clone();
        if let Primitive::Chunk(inner) = &mut placed {
            // Offsets add in the outer chunk's units; the scale carries over.
            inner.offset[0] += self.offset[0];
            inner.offset[1] += self.offset[1];
            inner.scale = self.scale;
            return placed;
        }
        match self.pixel_origin() {
            None => placed.offset(self.offset[0], self.offset[1]),
            Some((whole, fraction)) => {
                placed.make_physical(self.scale.unwrap_or(1.0), fraction);
                placed.offset(whole[0], whole[1]);
            }
        }
        placed
    }

    /// Where the chunk's origin lands in physical pixels, as whole pixels
    /// and the fraction left over; `None` while the scene is in logical
    /// points.
    pub fn pixel_origin(&self) -> Option<([f32; 2], [f32; 2])> {
        let scale = self.scale?;
        let at = self.offset.map(|o| o * scale);
        let whole = at.map(f32::floor);
        Some((whole, [at[0] - whole[0], at[1] - whole[1]]))
    }
}

impl PartialEq for ChunkPrimitive {
    /// The same recording at the same place.
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.chunk, &other.chunk)
            && self.offset == other.offset
            && self.scale == other.scale
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct RectPrimitive {
    pub rect: Rect,
    pub color: Color,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct RoundedRectPrimitive {
    pub rect: Rect,
    /// Corner radii: [top-left, top-right, bottom-right, bottom-left].
    pub corner_radii: [f32; 4],
    pub color: Color,
}

impl RoundedRectPrimitive {
    pub fn uniform(rect: Rect, radius: f32, color: Color) -> Self {
        Self {
            rect,
            corner_radii: [radius; 4],
            color,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct BorderPrimitive {
    pub rect: Rect,
    /// Border widths: [top, right, bottom, left].
    pub widths: [f32; 4],
    /// Corner radii: [top-left, top-right, bottom-right, bottom-left].
    pub corner_radii: [f32; 4],
    pub color: Color,
}

impl BorderPrimitive {
    pub fn uniform(rect: Rect, width: f32, radius: f32, color: Color) -> Self {
        Self {
            rect,
            widths: [width; 4],
            corner_radii: [radius; 4],
            color,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ShadowPrimitive {
    pub rect: Rect,
    pub blur_radius: f32,
    pub corner_radius: f32,
    /// Offset applied to shadow position: [x, y].
    pub offset: [f32; 2],
    pub color: Color,
}

/// A shaped text layout (`quark_text::TextLayout`). Opaque here because
/// quark-text depends on quark; the renderer downcasts it. Equality is
/// identity, so an unchanged cached layout compares equal across frames.
#[derive(Clone)]
pub struct ShapedText(Arc<dyn Any + Send + Sync>);

impl ShapedText {
    pub fn new<T: Any + Send + Sync>(layout: Arc<T>) -> Self {
        Self(layout)
    }

    pub fn downcast_ref<T: Any>(&self) -> Option<&T> {
        self.0.downcast_ref()
    }
}

impl fmt::Debug for ShapedText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ShapedText")
            .field(&Arc::as_ptr(&self.0))
            .finish()
    }
}

impl PartialEq for ShapedText {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

/// Single-color text. `rect.x`/`rect.y` is the layout origin in scene pixels;
/// glyphs are clipped by the clip stack, not by `rect`.
#[derive(Debug, Clone, PartialEq)]
pub struct TextPrimitive {
    pub rect: Rect,
    pub layout: ShapedText,
    pub color: Color,
}

/// Text whose spans have their own colors. `span_colors[i]` colors the glyphs
/// of the layout's span `i` (`TextParams::spans[i]`); spans without an entry
/// use `default_color`. Colors never affect shaping, so changing them reuses
/// the same layout.
#[derive(Debug, Clone, PartialEq)]
pub struct RichTextPrimitive {
    pub rect: Rect,
    pub layout: ShapedText,
    pub default_color: Color,
    pub span_colors: Arc<[Color]>,
}

/// Which line a [`TextDecoration`] draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextDecorationKind {
    Underline,
    Strikethrough,
}

/// A line drawn along a byte range of a text layout. Decorations never
/// affect shaping; they are painted as solid quads right after the text they
/// decorate, so they keep the text's paint order and clip.
#[derive(Debug, Clone, PartialEq)]
pub struct TextDecoration {
    pub range: Range<usize>,
    pub kind: TextDecorationKind,
    pub color: Color,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct IconPrimitive {
    pub rect: Rect,
    pub name: String,
    pub color: Color,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ClipPrimitive {
    pub rect: Rect,
    /// Per-corner radii: [top-left, top-right, bottom-right, bottom-left].
    /// All zero = plain rectangular clip.
    pub corner_radii: [f32; 4],
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImagePrimitive {
    pub rect: Rect,
    pub width: u32,
    pub height: u32,
    /// Shared so painting the same image every frame does not copy pixels.
    /// May be empty when the renderer already holds `cache_key`.
    pub rgba: std::sync::Arc<[u8]>,
    pub cache_key: u64,
}

// ---------------------------------------------------------------------------
// EffectQuad — procedural background (GPU-computed per-pixel)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct BlurRegionPrimitive {
    pub rect: Rect,
    pub blur_radius: f32,
    /// Per-corner radii: [top-left, top-right, bottom-right, bottom-left].
    /// Outside the rounded rect the backdrop stays sharp.
    pub corner_radii: [f32; 4],
}

/// Group opacity and transform of a [`Primitive::LayerStart`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayerPrimitive {
    /// Multiplies the alpha of the composited group, so overlapping
    /// children do not show through each other.
    pub opacity: f32,
    /// Maps the layer's content, painted in untransformed scene
    /// coordinates, to where it lands; see [`Transform2D`].
    pub transform: Transform2D,
}

/// How a [`PathPrimitive`] fills its interior.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PathFill {
    pub color: Color,
    pub rule: FillRule,
}

/// How a [`PathPrimitive`] strokes its outline.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PathStroke {
    pub color: Color,
    pub style: StrokeStyle,
}

/// A vector path, filled then stroked, antialiased by the renderer. Path
/// point `(x, y)` lands at `origin + scale * (x, y)` in scene pixels; the
/// stroke width scales too. Painters use `scale` 1 and logical points;
/// converting the scene to physical pixels multiplies it by the window
/// scale, so a shared `Arc<Path>` never needs rebuilding.
#[derive(Debug, Clone, PartialEq)]
pub struct PathPrimitive {
    pub path: Arc<Path>,
    pub origin: [f32; 2],
    pub scale: f32,
    pub fill: Option<PathFill>,
    pub stroke: Option<PathStroke>,
}

impl PathPrimitive {
    pub fn new(path: Arc<Path>, origin: [f32; 2]) -> Self {
        Self {
            path,
            origin,
            scale: 1.0,
            fill: None,
            stroke: None,
        }
    }

    pub fn fill(mut self, color: Color) -> Self {
        self.fill = Some(PathFill {
            color,
            rule: FillRule::NonZero,
        });
        self
    }

    pub fn fill_rule(mut self, rule: FillRule) -> Self {
        if let Some(fill) = &mut self.fill {
            fill.rule = rule;
        }
        self
    }

    pub fn stroke(mut self, color: Color, style: StrokeStyle) -> Self {
        self.stroke = Some(PathStroke { color, style });
        self
    }

    /// Scene bounds of everything the primitive can paint.
    pub fn bounds(&self) -> Rect {
        let b = self.path.bounds();
        let reach = self.stroke.map_or(0.0, |s| s.style.reach());
        let s = self.scale;
        Rect {
            x: self.origin[0] + (b.x - reach) * s,
            y: self.origin[1] + (b.y - reach) * s,
            width: (b.width + reach * 2.0) * s,
            height: (b.height + reach * 2.0) * s,
        }
    }
}

/// Effect type for procedural background quads.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[repr(u32)]
pub enum EffectType {
    /// Simplex noise blended between two colors.
    #[default]
    NoiseGradient = 0,
    /// Linear gradient with configurable angle.
    LinearGradient = 1,
    /// Radial gradient — color_a at center, color_b at edge.
    RadialGradient = 2,
    /// Animated shimmer — diagonal highlight sweep.
    Shimmer = 3,
    /// Vignette — edge darkening/coloring.
    Vignette = 4,
    /// Color tint — flat semi-transparent color overlay.
    ColorTint = 5,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct EffectQuadPrimitive {
    pub rect: Rect,
    pub effect_type: EffectType,
    pub color_a: Color,
    pub color_b: Color,
    /// Effect-specific parameters: [param1, param2].
    /// - NoiseGradient: [scale, 0.0]
    /// - LinearGradient: [angle_radians, 0.0]
    pub params: [f32; 2],
    pub corner_radius: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    // A chunk paints what its primitives pushed where it lands paint. It
    // snaps them around its own whole-pixel origin, so a rounding that
    // treats half pixels either side of zero differently would put an
    // edge one pixel away from where the pushed primitive's edge snaps.
    #[test]
    fn a_placed_chunk_snaps_like_its_primitives_pushed_in_place() {
        let mut triangle = Path::builder();
        triangle
            .move_to(0.0, 0.0)
            .line_to(4.0, 0.0)
            .line_to(2.0, 3.0);
        let triangle = Arc::new(triangle.close().build());
        let red = Color::rgba(255, 0, 0, 255);
        // Each scale turns some of these into half pixels, on both sides
        // of zero once offset; every product is exact.
        let edges = [-2.0, -1.0, -0.75, -0.5, 0.25, 0.5, 1.0, 2.0];
        let offsets = [-1.0, 0.5, 1.0, 2.0, 3.0];
        for s in [1.0, 1.25, 1.5, 2.0] {
            for x in edges {
                for o in offsets {
                    let rect = Rect {
                        x,
                        y: -x,
                        width: 3.0,
                        height: 2.0,
                    };
                    let mut primitives = Scene::default();
                    primitives.clip(rect);
                    primitives.rect(RectPrimitive { rect, color: red });
                    primitives.path(PathPrimitive::new(triangle.clone(), [x, -x]));
                    primitives.pop_clip();
                    let mut chunk = SceneChunk::new();
                    chunk.replace(primitives.primitives.iter().cloned());
                    let mut chunked = Scene::default();
                    chunked.chunk(&Arc::new(chunk), [o, o]);

                    let mut pushed = primitives;
                    for p in &mut pushed.primitives {
                        p.offset(o, o);
                    }
                    for scene in [&mut chunked, &mut pushed] {
                        for p in &mut scene.primitives {
                            p.to_physical(s);
                        }
                    }

                    assert_eq!(chunked.expanded(), pushed.primitives, "{x} at {o}, {s}x");
                }
            }
        }
    }
}
