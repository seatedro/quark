pub mod icons;
pub mod renderer;
mod shaders;
mod text;

/// Fonts are loaded and configured by quark-text; re-exported for callers
/// that reach them through the renderer.
pub use quark_text::fonts;

/// The scene types, re-exported from quark together with its `Rect`.
pub mod scene {
    pub use quark::Rect;
    pub use quark::scene::*;
}

pub use text::{push_text_decorations, text_decoration_rects};

pub use quark_text::TextSystem;
pub use renderer::{FrameStats, GpuContext, OffscreenTarget, RenderError, Renderer, TextMetrics};
pub use scene::{
    BlurRegionPrimitive, BorderPrimitive, ClipPrimitive, EffectQuadPrimitive, EffectType, FontKind,
    FontStyle, FontWeight, ImagePrimitive, Primitive, Rect, RectPrimitive, RichTextPrimitive,
    RoundedRectPrimitive, Scene, ShadowPrimitive, ShapedText, TextDecoration, TextDecorationKind,
    TextPrimitive,
};
