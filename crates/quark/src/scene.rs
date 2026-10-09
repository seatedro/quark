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
use crate::path::{FillRule, Path, StrokePattern, StrokeStyle};
use crate::transform::Transform2D;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FontKind {
    #[default]
    Ui,
    Mono,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum FontWeight {
    #[default]
    Normal,
    Light,
    Medium,
    Semibold,
    Bold,
    /// An OpenType/CSS weight. Valid weights are 1 to 1000; layout clamps
    /// others into that range at its public boundary.
    Numeric(u16),
}

impl FontWeight {
    /// The OpenType weight the name stands for: Light 300, Normal 400,
    /// Medium 500, Semibold 600, Bold 700, and a numeric weight clamped to
    /// 1 to 1000.
    pub fn value(self) -> u16 {
        match self {
            Self::Light => 300,
            Self::Normal => 400,
            Self::Medium => 500,
            Self::Semibold => 600,
            Self::Bold => 700,
            Self::Numeric(weight) => weight.clamp(1, 1000),
        }
    }
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

    pub fn styled_text(&mut self, text: StyledTextPrimitive) {
        self.push(Primitive::StyledText(text));
    }

    pub fn stripes(&mut self, stripes: StripesPrimitive) {
        self.push(Primitive::Stripes(stripes));
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

    /// Start an isolated group: everything pushed until the matching
    /// [`Self::pop_isolate`] renders together into `isolate.bounds` (and
    /// nowhere else), then composites once through its mask and in its
    /// compositing mode; see [`IsolatePrimitive`].
    pub fn push_isolate(&mut self, isolate: IsolatePrimitive) {
        self.push(Primitive::IsolateStart(isolate));
    }

    /// [`Self::push_isolate`] fading its content by `mask`.
    pub fn push_mask(&mut self, bounds: Rect, mask: AlphaMask) {
        self.push_isolate(IsolatePrimitive {
            bounds,
            mask: Some(mask),
            compositing: None,
        });
    }

    /// [`Self::push_isolate`] rendering its content in `compositing`
    /// whatever the surface's mode, e.g. a terminal that keeps linear
    /// blending inside a web-compatible window.
    pub fn push_compositing_island(&mut self, bounds: Rect, compositing: UiCompositing) {
        self.push_isolate(IsolatePrimitive {
            bounds,
            mask: None,
            compositing: Some(compositing),
        });
    }

    pub fn pop_isolate(&mut self) {
        self.push(Primitive::IsolateEnd);
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
    /// Text with a coordinate-dependent fill, an explicit coverage policy,
    /// and per-span colors; see [`StyledTextPrimitive`].
    StyledText(StyledTextPrimitive),
    Icon(IconPrimitive),
    Image(ImagePrimitive),
    EffectQuad(EffectQuadPrimitive),
    /// Start a frosted-glass blur region. Content rendered before this
    /// primitive (within the given bounds) will be blurred and composited
    /// as a backdrop before children are painted on top.
    BlurRegion(BlurRegionPrimitive),
    /// A filled and/or stroked vector path.
    Path(PathPrimitive),
    /// Diagonal or straight stripes filling a rounded rect.
    Stripes(StripesPrimitive),
    ClipStart(ClipPrimitive),
    ClipEnd,
    /// Start a group that fades and transforms as one; see
    /// [`Scene::push_layer`]. Z-indices inside a layer order its content
    /// only, as a CSS stacking context does.
    LayerStart(LayerPrimitive),
    LayerEnd,
    /// Start an isolated group; see [`Scene::push_isolate`].
    IsolateStart(IsolatePrimitive),
    IsolateEnd,
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
            Self::StyledText(p) => p.rect = p.rect.offset(dx, dy),
            Self::Icon(p) => p.rect = p.rect.offset(dx, dy),
            Self::Image(p) => p.rect = p.rect.offset(dx, dy),
            Self::EffectQuad(p) => p.rect = p.rect.offset(dx, dy),
            Self::BlurRegion(p) => p.rect = p.rect.offset(dx, dy),
            Self::Stripes(p) => p.rect = p.rect.offset(dx, dy),
            Self::ClipStart(p) => p.rect = p.rect.offset(dx, dy),
            Self::IsolateStart(p) => p.offset(dx, dy),
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
            | Self::LayerEnd
            | Self::IsolateEnd => {}
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
            Self::StyledText(p) => {
                p.rect = snapped(p.rect);
                p.fill = p.fill.scaled(s);
            }
            Self::Stripes(p) => {
                p.rect = snapped(p.rect);
                p.corner_radii = p.corner_radii.map(|r| r * s);
                p.period *= s;
            }
            Self::IsolateStart(p) => {
                p.bounds = snapped(p.bounds);
                p.mask = p.mask.map(|mask| mask.to_physical(s, shift));
            }
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
            | Self::IsolateEnd
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
    /// Whether any primitive, nested chunks included, starts a layer or an
    /// isolated group.
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
            Primitive::LayerStart(_) | Primitive::IsolateStart(_) => true,
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

// ---------------------------------------------------------------------------
// Color and blending policy
// ---------------------------------------------------------------------------

/// How a render surface blends UI paint. Chosen per window or offscreen
/// render, never per element; a subtree can only switch through an isolated
/// group ([`Scene::push_compositing_island`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum UiCompositing {
    /// Authored sRGB colors are decoded and composited in linear light.
    /// The framework default: gradients and translucency stay physically
    /// even, and a 50% black scrim over white encodes to about 188.
    #[default]
    Linear,
    /// Source-over on encoded sRGB values, as browsers and Electron blend
    /// CSS: the same scrim encodes to about 128. Images, layers, and blur
    /// convert at their boundaries; glyph coverage is not corrected.
    WebCompatible,
}

/// How monochrome glyph coverage blends on a [`UiCompositing::Linear`]
/// surface. A web-compatible surface always uses plain coverage, since
/// blending encoded values already gives text its sRGB weight.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum TextRendering {
    /// Coverage used as alpha in linear light: thin, light strokes on dark
    /// text over light backgrounds. The appearance before perceptual
    /// coverage existed.
    Linear,
    /// Coverage adjusted so a glyph has the weight sRGB blending would give
    /// it, without its color fringes, when the backdrop is a known opaque
    /// color ([`TextBackdrop::Opaque`]). Falls back to `Linear` over an
    /// unknown backdrop.
    #[default]
    Perceptual,
}

/// What a text primitive draws over, for [`TextRendering::Perceptual`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TextBackdrop {
    /// Images, gradients, layers, or materials may lie behind the text.
    #[default]
    Unknown,
    /// An opaque color the painter knows lies behind every glyph (the
    /// nearest opaque background, with nothing painted between).
    Opaque(Color),
}

/// What a window's surface shows where the app paints nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceBackground {
    /// Cleared to this color every frame.
    Opaque(Color),
    /// Cleared to transparent, with premultiplied output, so the window
    /// system composites what lies behind (a native material or the
    /// desktop). Needs a surface alpha mode the platform may not offer.
    Transparent,
}

impl Default for SurfaceBackground {
    fn default() -> Self {
        Self::Opaque(Color::rgba(0, 0, 0, 255))
    }
}

/// Semantic kind of a native window material (G3). The native adapter maps
/// it to the platform's closest material; renderer-free so element code can
/// request regions without depending on the app crate.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum MaterialKind {
    #[default]
    WindowBackground,
    Sidebar,
    Content,
    Titlebar,
    HeaderView,
    Popover,
    Menu,
    Tooltip,
    Hud,
    Sheet,
    UnderWindow,
}

// ---------------------------------------------------------------------------
// Text fill
// ---------------------------------------------------------------------------

/// The color of a text primitive's monochrome glyphs, evaluated per pixel in
/// paragraph coordinates (relative to the primitive's `rect` origin) and
/// multiplied by glyph coverage. Color glyphs (emoji) keep their colors.
/// Fills never affect shaping, so changing one (or a shimmer's phase)
/// reuses the same layout and glyph rasters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TextFill {
    Solid(Color),
    LinearGradient(TextGradient),
    Shimmer(ShimmerSpec),
}

impl Default for TextFill {
    fn default() -> Self {
        Self::Solid(Color::TRANSPARENT)
    }
}

impl TextFill {
    /// The fill with its lengths and points multiplied by `s`.
    pub fn scaled(self, s: f32) -> Self {
        match self {
            Self::Solid(color) => Self::Solid(color),
            Self::LinearGradient(g) => Self::LinearGradient(TextGradient {
                start: g.start.map(|v| v * s),
                end: g.end.map(|v| v * s),
                ..g
            }),
            Self::Shimmer(spec) => Self::Shimmer(ShimmerSpec {
                band_width: spec.band_width * s,
                ..spec
            }),
        }
    }

    /// The color drawn where no gradient or highlight applies: what reduced
    /// motion or a renderer without fills shows.
    pub fn base_color(&self) -> Color {
        match self {
            Self::Solid(color) => *color,
            Self::LinearGradient(g) => g.from,
            Self::Shimmer(spec) => spec.base,
        }
    }
}

/// A two-stop linear gradient from `start` (in `from`) to `end` (in `to`),
/// in paragraph coordinates, clamped beyond both ends. Interpolates in the
/// surface's compositing space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextGradient {
    pub start: [f32; 2],
    pub end: [f32; 2],
    pub from: Color,
    pub to: Color,
}

/// Which way a shimmer highlight sweeps.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum ShimmerDirection {
    #[default]
    LeftToRight,
    RightToLeft,
}

/// A highlight band sweeping across text in `base` color. At `phase` 0 the
/// band sits just before the paragraph's leading edge, at 1 just past its
/// trailing edge, so a looping phase sweeps continuously. The painter
/// resolves `phase` from its animation clock (keyed by `key`) and paints
/// the static `base` color under reduced motion; the renderer keeps no time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShimmerSpec {
    pub base: Color,
    pub highlight: Color,
    /// Full width of the highlight band, in logical points.
    pub band_width: f32,
    /// One sweep, for [`Self::phase_at`]. 1.5 seconds by default, an example
    /// value rather than a measured platform one.
    pub duration_ms: u32,
    pub direction: ShimmerDirection,
    /// Stable identity of the animation, for the painter's clock.
    pub key: u64,
    /// Sweep progress in `[0, 1)`.
    pub phase: f32,
}

impl ShimmerSpec {
    pub fn new(base: Color, highlight: Color) -> Self {
        Self {
            base,
            highlight,
            band_width: 48.0,
            duration_ms: 1500,
            direction: ShimmerDirection::LeftToRight,
            key: 0,
            phase: 0.0,
        }
    }

    pub fn band_width(mut self, width: f32) -> Self {
        self.band_width = width;
        self
    }

    pub fn duration_ms(mut self, duration_ms: u32) -> Self {
        self.duration_ms = duration_ms;
        self
    }

    pub fn direction(mut self, direction: ShimmerDirection) -> Self {
        self.direction = direction;
        self
    }

    pub fn key(mut self, key: u64) -> Self {
        self.key = key;
        self
    }

    pub fn phase(mut self, phase: f32) -> Self {
        self.phase = phase;
        self
    }

    /// The phase at `now_ms` of a sweep that started at `start_ms` and
    /// repeats every `duration_ms`.
    pub fn phase_at(&self, start_ms: u64, now_ms: u64) -> f32 {
        let duration = u64::from(self.duration_ms.max(1));
        (now_ms.saturating_sub(start_ms) % duration) as f32 / duration as f32
    }
}

/// Text whose glyphs take a [`TextFill`], with an explicit coverage policy.
/// `span_colors[i]` overrides the fill for span `i + 1`'s glyphs as in
/// [`RichTextPrimitive`]; an empty slice fills every glyph. `rect.x`/`rect.y`
/// is the layout origin and the origin of the fill's coordinates; the
/// shimmer sweeps across `rect.width`.
#[derive(Debug, Clone, PartialEq)]
pub struct StyledTextPrimitive {
    pub rect: Rect,
    pub layout: ShapedText,
    pub fill: TextFill,
    pub span_colors: Arc<[Color]>,
    pub rendering: TextRendering,
    pub backdrop: TextBackdrop,
}

impl StyledTextPrimitive {
    /// Every glyph in `fill`, perceptual over an unknown backdrop (so
    /// linear until [`Self::backdrop`] names one).
    pub fn new(rect: Rect, layout: ShapedText, fill: TextFill) -> Self {
        Self {
            rect,
            layout,
            fill,
            span_colors: Arc::from([]),
            rendering: TextRendering::default(),
            backdrop: TextBackdrop::Unknown,
        }
    }

    pub fn span_colors(mut self, colors: Arc<[Color]>) -> Self {
        self.span_colors = colors;
        self
    }

    pub fn rendering(mut self, rendering: TextRendering) -> Self {
        self.rendering = rendering;
        self
    }

    pub fn backdrop(mut self, backdrop: TextBackdrop) -> Self {
        self.backdrop = backdrop;
        self
    }
}

// ---------------------------------------------------------------------------
// Decoration patterns
// ---------------------------------------------------------------------------

/// How a decoration line looks. `None` lengths take the defaults derived
/// from the font size (as solid [`TextDecoration`]s use).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextDecorationStyle {
    pub pattern: StrokePattern,
    /// Line thickness in the layout's units.
    pub thickness: Option<f32>,
    /// Distance from the baseline to the line's center, positive below.
    pub offset: Option<f32>,
    pub color: Color,
}

impl TextDecorationStyle {
    pub fn solid(color: Color) -> Self {
        Self {
            pattern: StrokePattern::Solid,
            thickness: None,
            offset: None,
            color,
        }
    }

    pub fn pattern(mut self, pattern: StrokePattern) -> Self {
        self.pattern = pattern;
        self
    }

    pub fn thickness(mut self, thickness: f32) -> Self {
        self.thickness = Some(thickness);
        self
    }

    pub fn offset(mut self, offset: f32) -> Self {
        self.offset = Some(offset);
        self
    }
}

/// A [`TextDecoration`] with a full [`TextDecorationStyle`]. A pattern
/// restarts at the start of each wrapped line and runs unbroken across a
/// range's spans on one line.
#[derive(Debug, Clone, PartialEq)]
pub struct StyledDecoration {
    pub range: Range<usize>,
    pub kind: TextDecorationKind,
    pub style: TextDecorationStyle,
}

impl From<TextDecoration> for StyledDecoration {
    fn from(decoration: TextDecoration) -> Self {
        Self {
            range: decoration.range,
            kind: decoration.kind,
            style: TextDecorationStyle::solid(decoration.color),
        }
    }
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

// ---------------------------------------------------------------------------
// Isolated groups: masks and compositing islands
// ---------------------------------------------------------------------------

/// Most stops an [`AlphaMask`] carries; more are dropped.
pub const MAX_MASK_STOPS: usize = 4;

/// Mask opacity `alpha` at `offset` along a mask's axis (0 at its start, 1
/// at its end).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct MaskStop {
    pub offset: f32,
    pub alpha: f32,
}

/// Up to [`MAX_MASK_STOPS`] stops in increasing offset, inline so a mask
/// never allocates.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct MaskStops {
    stops: [MaskStop; MAX_MASK_STOPS],
    len: u8,
}

impl MaskStops {
    /// The first [`MAX_MASK_STOPS`] of `stops`, sanitized: non-finite
    /// values become 0, offsets and alphas clamp to `[0, 1]`, and each
    /// offset is at least the previous one.
    pub fn new(stops: &[MaskStop]) -> Self {
        let mut out = Self::default();
        let mut floor = 0.0f32;
        for stop in stops.iter().take(MAX_MASK_STOPS) {
            let clean = |v: f32| {
                if v.is_finite() {
                    v.clamp(0.0, 1.0)
                } else {
                    0.0
                }
            };
            let offset = clean(stop.offset).max(floor);
            floor = offset;
            out.stops[out.len as usize] = MaskStop {
                offset,
                alpha: clean(stop.alpha),
            };
            out.len += 1;
        }
        out
    }

    pub fn as_slice(&self) -> &[MaskStop] {
        &self.stops[..self.len as usize]
    }

    /// The mask opacity at `t` along the axis: the stops interpolated
    /// linearly, the first and last held beyond the ends, and fully opaque
    /// with no stops.
    pub fn alpha_at(&self, t: f32) -> f32 {
        let stops = self.as_slice();
        let (Some(first), Some(last)) = (stops.first(), stops.last()) else {
            return 1.0;
        };
        if t <= first.offset {
            return first.alpha;
        }
        for pair in stops.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            if t <= b.offset {
                let span = b.offset - a.offset;
                let f = if span > 0.0 {
                    (t - a.offset) / span
                } else {
                    1.0
                };
                return a.alpha + (b.alpha - a.alpha) * f;
            }
        }
        last.alpha
    }
}

/// Which edge of a box a fade runs out at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FadeEdge {
    Top,
    Right,
    Bottom,
    Left,
}

/// Opacity applied to an isolated group's premultiplied color and alpha
/// as it composites, so overlapping content fades once, as a whole.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AlphaMask {
    /// Stops along the axis from `start` to `end`, in scene coordinates
    /// like the group's bounds.
    Linear {
        start: [f32; 2],
        end: [f32; 2],
        stops: MaskStops,
    },
}

impl AlphaMask {
    /// Opaque inside `bounds`, fading to transparent over the last `length`
    /// points before `edge`. A zero, negative, or non-finite length is no
    /// fade: opaque everywhere.
    pub fn fade_edge(bounds: Rect, edge: FadeEdge, length: f32) -> Self {
        if !(length.is_finite() && length > 0.0) {
            return Self::Linear {
                start: [bounds.x, bounds.y],
                end: [bounds.x, bounds.y],
                stops: MaskStops::new(&[MaskStop {
                    offset: 0.0,
                    alpha: 1.0,
                }]),
            };
        }
        let (x0, y0, x1, y1) = (bounds.x, bounds.y, bounds.right(), bounds.bottom());
        let (start, end) = match edge {
            FadeEdge::Right => ([x1 - length, y0], [x1, y0]),
            FadeEdge::Left => ([x0 + length, y0], [x0, y0]),
            FadeEdge::Bottom => ([x0, y1 - length], [x0, y1]),
            FadeEdge::Top => ([x0, y0 + length], [x0, y0]),
        };
        Self::Linear {
            start,
            end,
            stops: MaskStops::new(&[
                MaskStop {
                    offset: 0.0,
                    alpha: 1.0,
                },
                MaskStop {
                    offset: 1.0,
                    alpha: 0.0,
                },
            ]),
        }
    }

    /// The mask's opacity at scene point `p`. Degenerate axes (start equal
    /// to end) use the last stop everywhere past the start.
    pub fn alpha_at(&self, p: [f32; 2]) -> f32 {
        match self {
            Self::Linear { start, end, stops } => {
                let axis = [end[0] - start[0], end[1] - start[1]];
                let len2 = axis[0] * axis[0] + axis[1] * axis[1];
                let t = if len2 > 0.0 {
                    ((p[0] - start[0]) * axis[0] + (p[1] - start[1]) * axis[1]) / len2
                } else {
                    1.0
                };
                stops.alpha_at(t)
            }
        }
    }

    /// The mask in physical pixels, as [`Primitive::to_physical`] converts
    /// the group's bounds (without snapping, so the ramp keeps its length).
    pub fn to_physical(self, s: f32, shift: [f32; 2]) -> Self {
        match self {
            Self::Linear { start, end, stops } => Self::Linear {
                start: [start[0] * s + shift[0], start[1] * s + shift[1]],
                end: [end[0] * s + shift[0], end[1] * s + shift[1]],
                stops,
            },
        }
    }

    fn offset(self, dx: f32, dy: f32) -> Self {
        match self {
            Self::Linear { start, end, stops } => Self::Linear {
                start: [start[0] + dx, start[1] + dy],
                end: [end[0] + dx, end[1] + dy],
                stops,
            },
        }
    }
}

/// An isolated group ([`Primitive::IsolateStart`] to
/// [`Primitive::IsolateEnd`]). Its content renders on its own, clipped to
/// `bounds`, then composites once: multiplied by `mask` when there is one,
/// and blended by `compositing` (the surface's mode when `None`). Only the
/// visible part of `bounds` takes a pooled texture. Hit testing and
/// semantics are the painter's; a mask hides nothing from them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IsolatePrimitive {
    pub bounds: Rect,
    pub mask: Option<AlphaMask>,
    pub compositing: Option<UiCompositing>,
}

impl IsolatePrimitive {
    fn offset(&mut self, dx: f32, dy: f32) {
        self.bounds = self.bounds.offset(dx, dy);
        self.mask = self.mask.map(|mask| mask.offset(dx, dy));
    }
}

// ---------------------------------------------------------------------------
// Stripes
// ---------------------------------------------------------------------------

/// Parallel stripes filling a rounded rect: bands of `colors[0]` covering
/// `duty` of every `period` (logical points, measured across the stripes),
/// `colors[1]` between them. `angle` turns the stripes from vertical,
/// clockwise in radians; phase zero is at the rect's top-left corner.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StripesPrimitive {
    pub rect: Rect,
    pub corner_radii: [f32; 4],
    pub angle: f32,
    pub period: f32,
    pub duty: f32,
    pub colors: [Color; 2],
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

    /// A border of `width` along `rect`'s rounded perimeter drawn in
    /// `pattern`, staying inside `rect` like a [`BorderPrimitive`]: the
    /// path runs `width / 2` in, with radii reduced to match.
    pub fn border(
        rect: Rect,
        width: f32,
        corner_radii: [f32; 4],
        color: Color,
        pattern: StrokePattern,
    ) -> Self {
        let half = width.max(0.0) * 0.5;
        let inner = Rect {
            x: half,
            y: half,
            width: (rect.width - width).max(0.0),
            height: (rect.height - width).max(0.0),
        };
        let radii = corner_radii.map(|r| (r - half).max(0.0));
        let style = StrokeStyle::new(width)
            .join(crate::path::LineJoin::Round)
            .pattern(pattern);
        Self::new(Arc::new(Path::rounded_rect(inner, radii)), [rect.x, rect.y]).stroke(color, style)
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

    // Hostile mask stops (NaN, out of order, out of range, too many) must
    // sanitize to a bounded, monotonic ramp, so a renderer never reads an
    // unbounded or unordered stop list.
    #[test]
    fn mask_stops_sanitize_hostile_input() {
        let stop = |offset, alpha| MaskStop { offset, alpha };
        let stops = MaskStops::new(&[
            stop(0.5, f32::NAN),
            stop(0.2, 2.0),
            stop(f32::INFINITY, 0.5),
            stop(0.9, 0.0),
            stop(1.0, 1.0),
        ]);
        let got: Vec<(f32, f32)> = stops
            .as_slice()
            .iter()
            .map(|s| (s.offset, s.alpha))
            .collect();
        assert_eq!(got, [(0.5, 0.0), (0.5, 1.0), (0.5, 0.5), (0.9, 0.0)]);
        // Held beyond both ends, interpolated between.
        let table = [(-1.0, 0.0), (0.5, 0.0), (0.7, 0.25), (2.0, 0.0)];
        for (t, alpha) in table {
            assert!(
                (stops.alpha_at(t) - alpha).abs() < 1e-6,
                "{t}: {}",
                stops.alpha_at(t)
            );
        }
    }

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
