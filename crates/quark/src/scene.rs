//! Scene — immediate-mode primitive container emitted by the paint phase.
//!
//! Pure data: a `Vec<Primitive>` plus convenience builders. The renderer
//! consumes the scene; quark itself does not render.

use std::any::Any;
use std::fmt;
use std::ops::Range;
use std::sync::Arc;

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
            Self::ClipEnd
            | Self::ZIndexPush(_)
            | Self::ZIndexPop
            | Self::LayerBoundary
            | Self::LayerEnd => {}
        }
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
