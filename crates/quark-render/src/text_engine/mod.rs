//! The glyph engine: rasterized glyphs in a texture atlas, their draw
//! instances, and the pipeline and shader that draw them.
//!
//! Absorbed from glyphon 0.10.0 (<https://github.com/grovesNL/glyphon>,
//! MIT OR Apache-2.0 OR Zlib; the license texts sit beside this file)
//! together with every quark patch it carried as `vendor/glyphon`: the
//! positioned-glyph prepare path, separate uploads, atlas growth by copy,
//! atlas clearing and epochs, per-draw offsets, work counters, linear
//! corrected coverage, encoded targets, and draw-time paint (fills and
//! backdrops). Upstream's custom glyphs, depth callbacks, and web color
//! mode, which quark never used, were left out. Shaping and layout stay in
//! cosmic-text through quark-text; swash rasterizes.

mod atlas;
mod error;
mod pipeline;
mod render;
mod viewport;

pub use atlas::AtlasStats;
pub(crate) use atlas::TextAtlas;
pub use error::{PrepareError, RenderError};
pub(crate) use pipeline::Cache;
pub(crate) use render::{GlyphFill, MAX_GLYPH_FILLS, TextRenderer};
pub(crate) use viewport::{MAX_DRAW_OFFSETS, Viewport};

use etagere::AllocId;
use quark_text::cosmic_text::{Buffer, CacheKey, Color};
use wgpu::{Device, Queue};

pub(crate) enum GpuCacheStatus {
    InAtlas {
        x: u16,
        y: u16,
        content_type: ContentType,
    },
    SkipRasterization,
}

pub(crate) struct GlyphDetails {
    width: u16,
    height: u16,
    gpu_cache: GpuCacheStatus,
    atlas_id: Option<AllocId>,
    top: i16,
    left: i16,
}

/// The kind of pixels a rasterized glyph holds.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum ContentType {
    /// Four bytes of RGBA per pixel.
    Color = 0,
    /// One byte of coverage per pixel.
    Mask = 1,
}

/// One glyph instance, as the vertex shader reads it.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct GlyphToRender {
    pos: [i32; 2],
    dim: [u16; 2],
    uv: [u16; 2],
    color: u32,
    content_type_with_srgb: [u16; 2],
    depth: f32,
    /// Draw-time paint; see [`PositionedGlyph::fill`].
    paint: u32,
}

/// The target resolution text is placed in.
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct Resolution {
    pub width: u32,
    pub height: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct Params {
    screen_resolution: Resolution,
    /// `[1 when the target holds sRGB-encoded values, 0]`.
    _pad: [u32; 2],
}

/// A clip rectangle in target pixels.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TextBounds {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

/// A shaped buffer to draw, for [`TextRenderer::prepare`].
#[derive(Clone)]
pub(crate) struct TextArea<'a> {
    /// The buffer whose layout runs are drawn.
    pub buffer: &'a Buffer,
    /// The buffer's left and top edges.
    pub left: f32,
    pub top: f32,
    /// Multiplies the buffer's positions.
    pub scale: f32,
    /// Glyphs are clipped to these bounds.
    pub bounds: TextBounds,
    /// The color of glyphs that carry none.
    pub default_color: Color,
}

/// A glyph already shaped, laid out, and placed, for
/// [`TextRenderer::prepare_glyphs`]. Its color is its own, so text in many
/// colors needs no buffer shaped with those colors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PositionedGlyph {
    /// What to rasterize: font, glyph, size, subpixel bins, and flags.
    pub cache_key: CacheKey,
    /// The pixel position `LayoutGlyph::physical` returns, with the line's
    /// rounded baseline already added to `y`.
    pub x: i32,
    pub y: i32,
    pub color: Color,
    /// Clip rectangle, as [`TextArea::bounds`].
    pub bounds: TextBounds,
    /// One plus the index of the renderer's [`GlyphFill`] that colors a
    /// monochrome glyph in place of `color` (whose alpha still fades it);
    /// zero for `color` alone. See [`TextRenderer::set_fills`].
    pub fill: u8,
    /// Perceptual coverage over a known opaque backdrop: its sRGB-encoded
    /// luminance. Overrides the correction a cache key's
    /// `LINEAR_CORRECTED` flag asks for. Ignored on an encoded target.
    pub backdrop: Option<u8>,
}

pub(crate) struct State<'a> {
    pub(crate) device: &'a Device,
    pub(crate) queue: &'a Queue,
}
