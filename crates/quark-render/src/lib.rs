pub mod fonts;
pub mod icons;
pub mod renderer;
pub mod scene;
mod shaders;
mod text;

pub use text::{push_text_decorations, text_decoration_rects};

pub use quark_text::TextSystem;
pub use renderer::{FrameStats, GpuContext, OffscreenTarget, RenderError, Renderer, TextMetrics};
pub use scene::{
    BlurRegionPrimitive, BorderPrimitive, ClipPrimitive, EffectQuadPrimitive, EffectType, FontKind,
    FontStyle, FontWeight, ImagePrimitive, Primitive, Rect, RectPrimitive, RichTextPrimitive,
    RoundedRectPrimitive, Scene, ShadowPrimitive, ShapedText, TextDecoration, TextDecorationKind,
    TextPrimitive,
};
