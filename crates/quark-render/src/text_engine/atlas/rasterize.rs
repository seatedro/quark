//! Rasterizing a missed glyph: the native rasterizer, swash, or the font
//! system that shaped it, into the atlas's scratch pixels.

use std::ops::Range;

use quark_text::cosmic_text::{CacheKey, FontSystem, SwashContent};
use quark_text::fonts::PreparedFont;

use super::pages::bytes_per_pixel;
use super::{ContentType, GUTTER, GlyphAtlas, GlyphKey};
use crate::text_engine::backend::NativeOutcome;
use crate::text_engine::raster::{
    AlphaMode, BitmapContent, GlyphRasterizer, RasterError, RasterOutcome,
};

/// A rasterized glyph, its pixels in the atlas's scratch.
pub(super) struct Drawn {
    pub(super) kind: ContentType,
    pub(super) premultiplied: bool,
    pub(super) left: i32,
    pub(super) top: i32,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) stride: usize,
    /// The first plane's bytes; a bundle's later planes follow at
    /// `plane_len` steps.
    pub(super) bytes: Range<usize>,
    pub(super) planes: u32,
    pub(super) plane_len: usize,
}

impl Drawn {
    /// Rows of the glyph's rectangle in the atlas: every plane, each in a
    /// gutter of its own.
    pub(super) fn rows(&self) -> u32 {
        self.planes * (self.height + 2 * GUTTER)
    }
}

/// What rasterizing a glyph gave.
pub(super) enum Raster {
    Blank,
    Failed { transient: bool },
    Drawn(Drawn),
}

impl GlyphAtlas {
    /// Rasterize `glyph_key` (cosmic-text's `key`) with the rasterizer it
    /// names, `font` prepared for it.
    pub(super) fn rasterize(
        &mut self,
        font_system: &mut FontSystem,
        glyph_key: &GlyphKey,
        key: CacheKey,
        font: Option<&PreparedFont>,
    ) -> Raster {
        let (GlyphKey::Raster(raster), Some(font)) = (glyph_key, font) else {
            return self.legacy_raster(font_system, key);
        };
        if let Some(native) = &mut self.native
            && raster.profile == native.profile()
        {
            match native.rasterize(font, &raster.request, &mut self.scratch) {
                Ok(NativeOutcome::Single(outcome)) => return Self::raster_outcome(Ok(outcome)),
                Ok(NativeOutcome::Bundle {
                    placement,
                    stride,
                    planes,
                }) => {
                    return Raster::Drawn(Drawn {
                        kind: ContentType::Mask,
                        premultiplied: false,
                        left: placement.left,
                        top: placement.top,
                        width: placement.width,
                        height: placement.height,
                        stride: stride as usize,
                        plane_len: planes[0].len(),
                        bytes: planes[0].clone(),
                        planes: planes.len() as u32,
                    });
                }
                // A font the backend cannot use: swash draws all of it.
                Err(RasterError::UnsupportedFont) => {
                    self.native_unsupported.insert(font.instance());
                    self.stats.native_fallbacks += 1;
                }
                // A glyph or option it cannot draw: swash draws this
                // glyph, kept under the native key.
                Err(RasterError::UnsupportedFormat | RasterError::UnsupportedOptions) => {
                    self.stats.native_fallbacks += 1;
                }
                Err(error) => return Self::raster_outcome(Err(error)),
            }
        }
        let outcome = self
            .swash
            .rasterize(font, &raster.request, &mut self.scratch);
        Self::raster_outcome(outcome)
    }

    /// A single-bitmap outcome as the atlas admits it.
    fn raster_outcome(outcome: Result<RasterOutcome, RasterError>) -> Raster {
        match outcome {
            Ok(RasterOutcome::Empty) => Raster::Blank,
            Ok(RasterOutcome::Bitmap(bitmap)) => {
                let (kind, premultiplied) = match bitmap.content {
                    BitmapContent::Mask => (ContentType::Mask, false),
                    BitmapContent::Color { alpha } => {
                        (ContentType::Color, alpha == AlphaMode::Premultiplied)
                    }
                };
                Raster::Drawn(Drawn {
                    kind,
                    premultiplied,
                    left: bitmap.placement.left,
                    top: bitmap.placement.top,
                    width: bitmap.placement.width,
                    height: bitmap.placement.height,
                    stride: bitmap.stride as usize,
                    plane_len: bitmap.bytes.len(),
                    bytes: bitmap.bytes,
                    planes: 1,
                })
            }
            Err(error) => Raster::Failed {
                transient: error == RasterError::Platform,
            },
        }
    }

    /// Draws a glyph through the font system that shaped it, for a face
    /// the registry could not prepare.
    fn legacy_raster(&mut self, font_system: &mut FontSystem, key: CacheKey) -> Raster {
        self.stats.fallbacks += 1;
        let Some(image) = self.legacy.get_image_uncached(font_system, key) else {
            return Raster::Blank;
        };
        let (kind, premultiplied) = match image.content {
            SwashContent::Mask => (ContentType::Mask, false),
            SwashContent::Color => (
                ContentType::Color,
                matches!(
                    image.source,
                    quark_text::cosmic_text::SwashSource::ColorOutline(_)
                ),
            ),
            SwashContent::SubpixelMask => return Raster::Failed { transient: false },
        };
        let placement = image.placement;
        if placement.width == 0 || placement.height == 0 {
            return Raster::Blank;
        }
        let stride = placement.width as usize * bytes_per_pixel(kind) as usize;
        let len = stride * placement.height as usize;
        if len > image.data.len() {
            return Raster::Failed { transient: false };
        }
        self.scratch.pixels = image.data;
        Raster::Drawn(Drawn {
            kind,
            premultiplied,
            left: placement.left,
            top: placement.top,
            width: placement.width,
            height: placement.height,
            stride,
            bytes: 0..len,
            planes: 1,
            plane_len: len,
        })
    }
}
