//! The wgpu renderer for Quark scenes.
//!
//! A [`GpuContext`] holds the GPU state every window shares (instance,
//! adapter, device, queue, pipelines, image cache); each window's
//! [`Renderer`] draws `quark::Scene`s into its surface or into
//! [`OffscreenTarget`]s. Backends are Vulkan, Metal, and DX12, plus GLES on
//! Linux for machines without a Vulkan driver; the `WGPU_BACKEND`
//! environment variable picks one. The `headless-render` feature renders
//! without a window, for tests that read pixels back.
//!
//! Apps normally reach this crate through `quark-app`, which owns the
//! renderers.
pub mod icons;
mod path;
pub mod renderer;
mod shaders;
mod text;

/// Fonts are loaded and configured by quark-text; re-exported for callers
/// that reach them through the renderer.
pub use quark_text::fonts;

/// The scene types, re-exported from quark together with its `Rect`.
pub mod scene {
    pub use quark::Rect;
    pub use quark::Transform2D;
    pub use quark::path::*;
    pub use quark::scene::*;
}

pub use text::{push_text_decorations, text_decoration_rects};

pub use quark_text::TextSystem;
pub use renderer::{FrameStats, GpuContext, OffscreenTarget, RenderError, Renderer, TextMetrics};
pub use scene::{
    BlurRegionPrimitive, BorderPrimitive, ClipPrimitive, EffectQuadPrimitive, EffectType, FontKind,
    FontStyle, FontWeight, ImagePrimitive, LayerPrimitive, Path, PathBuilder, PathPrimitive,
    Primitive, Rect, RectPrimitive, RichTextPrimitive, RoundedRectPrimitive, Scene,
    ShadowPrimitive, ShapedText, StrokeStyle, TextDecoration, TextDecorationKind, TextPrimitive,
    Transform2D,
};
