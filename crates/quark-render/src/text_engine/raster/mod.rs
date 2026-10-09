//! The rasterizer contract: what a glyph cache miss asks a backend to draw,
//! and the bitmap it gets back.
//!
//! The atlas calls [`GlyphRasterizer::rasterize`] once per missing
//! [`RasterKey`]; hits never reach a backend. A backend draws the glyph id
//! it is given from the [`PreparedFont`] it is given, exactly the face,
//! variation values, and synthesis the glyph was shaped with (see
//! [`quark_text::fonts::FontRegistry`]); it never looks a family up or
//! substitutes another face. Layout (advances, carets, line metrics) stays
//! cosmic-text's whatever the backend: a bitmap only places ink.
//!
//! Coordinates are device pixels. The glyph origin sits at an integer
//! pixel plus the request's [`SubpixelOffset`], which the bitmap already
//! includes; a bitmap's [`Placement`] puts its top-left pixel `left`
//! pixels right of and `top` pixels above that integer origin, so it is
//! drawn at `(origin_x + left, baseline_y - top)`.

// Until the atlas rasterizes through the contract (stream A); the swash
// backend's tests exercise it meanwhile.
#![allow(dead_code)]

pub(crate) mod swash;

use std::ops::Range;

// Its portable mapping is unit tested on every platform; the backend itself
// builds on Windows with text-raster-directwrite.
#[cfg(any(test, all(windows, feature = "text-raster-directwrite")))]
mod directwrite;

use quark_text::cosmic_text::{CacheKey, CacheKeyFlags, SubpixelBin};
pub(crate) use quark_text::fonts::{FontInstanceId, PreparedFont};

/// Which implementation drew a bitmap. Part of every [`RasterKey`], so
/// switching backends, or a backend changing what it draws, never reuses
/// the other's bitmaps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct RasterProfile {
    pub backend: RasterBackend,
    /// Changes whenever the backend's bitmap for the same request may
    /// change: its code, the library it wraps, or a smoothing calibration.
    pub revision: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum RasterBackend {
    Swash,
    CoreText,
    DirectWrite,
}

/// What a backend can draw, for choosing a fallback before asking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RasterCapabilities {
    /// Layered color outlines (COLR).
    pub color_outlines: bool,
    /// Embedded color bitmaps (CBDT, sbix).
    pub color_bitmaps: bool,
    /// Variation axis values from [`PreparedFont::variations`].
    pub variations: bool,
    /// Synthetic italic and thickening from [`PreparedFont::synthesis`].
    pub synthesis: bool,
}

/// How outlines snap to the pixel grid.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub(crate) enum Hinting {
    /// The backend's hinting for the font: swash runs the font's hints.
    #[default]
    Font,
    /// Unhinted outlines (cosmic-text's `DISABLE_HINTING`).
    Disabled,
}

/// Which of a glyph's drawings a backend uses.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub(crate) enum ColorGlyphs {
    /// A color outline in the font's first palette, else a color bitmap of
    /// the strike that fits best, else the monochrome outline: how text
    /// draws emoji.
    #[default]
    Prefer,
    /// Only the monochrome outline.
    Never,
}

/// Rasterization choices besides the font and position. Paint (color,
/// backdrop, opacity, linear correction, fills) is never part of it: the
/// shader applies paint to the same bitmap.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub(crate) struct RasterOptions {
    pub hinting: Hinting,
    pub color: ColorGlyphs,
}

/// Where in its pixel the glyph origin sits, in quarter pixels: quark
/// places glyphs at four horizontal positions per pixel and whole pixels
/// vertically, so `y` is [`SubpixelBin::Zero`] for quark-text layouts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct SubpixelOffset {
    pub x: SubpixelBin,
    pub y: SubpixelBin,
}

impl SubpixelOffset {
    pub(crate) const ZERO: Self = Self {
        x: SubpixelBin::Zero,
        y: SubpixelBin::Zero,
    };

    /// The offset in pixels, right and down.
    pub(crate) fn pixels(self) -> (f32, f32) {
        (self.x.as_float(), self.y.as_float())
    }
}

/// The largest physical em a request may ask for. A larger glyph's bitmap
/// would pass [`MAX_BITMAP_BYTES`] anyway, and refusing first keeps a
/// backend from allocating it.
pub(crate) const MAX_PHYSICAL_EM: f32 = 2048.0;

/// The most bytes one bitmap may have; a backend returns
/// [`RasterError::TooLarge`] past it.
pub(crate) const MAX_BITMAP_BYTES: usize = 16 << 20;

/// One glyph of one font to draw, everything but the font. Sizes are
/// physical: `physical_em = logical_em * scale`, multiplied once, by
/// whoever shaped the glyph (quark-text layouts are shaped at physical
/// size, so a glyph's `font_size` already is it). `scale` stays in the
/// request even though swash only reads the physical em: a native
/// backend's size-dependent choices may differ between 14 px at 2x and
/// 28 px at 1x.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct RasterRequest {
    glyph: u16,
    physical_em_bits: u32,
    scale_bits: u32,
    subpixel: SubpixelOffset,
    options: RasterOptions,
}

impl RasterRequest {
    /// Fails with [`RasterError::InvalidRequest`] unless `physical_em` and
    /// `scale` are finite and positive, and with
    /// [`RasterError::TooLarge`] past [`MAX_PHYSICAL_EM`].
    pub(crate) fn new(
        glyph: u16,
        physical_em: f32,
        scale: f32,
        subpixel: SubpixelOffset,
        options: RasterOptions,
    ) -> Result<Self, RasterError> {
        let valid = |v: f32| v.is_finite() && v > 0.0;
        if !valid(physical_em) || !valid(scale) {
            return Err(RasterError::InvalidRequest);
        }
        if physical_em > MAX_PHYSICAL_EM {
            return Err(RasterError::TooLarge);
        }
        Ok(Self {
            glyph,
            physical_em_bits: physical_em.to_bits(),
            scale_bits: scale.to_bits(),
            subpixel,
            options,
        })
    }

    /// The request for a glyph cosmic-text keyed as `key`, shaped at
    /// physical size under device scale `scale`. Its font is the registry's
    /// `prepare(key.font_id, key.font_weight, key.flags)`; the flags'
    /// background luminance is paint and stays out of the request. Pixel
    /// fonts are not supported.
    pub(crate) fn from_cache_key(key: &CacheKey, scale: f32) -> Result<Self, RasterError> {
        if key.flags.contains(CacheKeyFlags::PIXEL_FONT) {
            return Err(RasterError::UnsupportedOptions);
        }
        let hinting = if key.flags.contains(CacheKeyFlags::DISABLE_HINTING) {
            Hinting::Disabled
        } else {
            Hinting::Font
        };
        Self::new(
            key.glyph_id,
            f32::from_bits(key.font_size_bits),
            scale,
            SubpixelOffset {
                x: key.x_bin,
                y: key.y_bin,
            },
            RasterOptions {
                hinting,
                color: ColorGlyphs::Prefer,
            },
        )
    }

    pub(crate) fn glyph(&self) -> u16 {
        self.glyph
    }

    pub(crate) fn physical_em(&self) -> f32 {
        f32::from_bits(self.physical_em_bits)
    }

    pub(crate) fn scale(&self) -> f32 {
        f32::from_bits(self.scale_bits)
    }

    pub(crate) fn subpixel(&self) -> SubpixelOffset {
        self.subpixel
    }

    pub(crate) fn options(&self) -> RasterOptions {
        self.options
    }
}

/// Everything that decides a bitmap: the backend and its revision, the
/// exact font instance, and the request. Two equal keys always get equal
/// bitmaps, across text systems and font changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct RasterKey {
    pub profile: RasterProfile,
    pub instance: FontInstanceId,
    pub request: RasterRequest,
}

/// What a bitmap's pixels hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BitmapContent {
    /// One byte of coverage per pixel, for the shader to paint. Never sRGB
    /// color, and never per-channel (LCD) coverage, which no backend may
    /// return as a mask.
    Mask,
    /// Four bytes per pixel in R, G, B, A order, sRGB encoded, associated
    /// with alpha as `alpha` says.
    Color { alpha: AlphaMode },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AlphaMode {
    /// Color channels independent of alpha (swash's embedded bitmaps).
    Straight,
    /// Color channels already multiplied by alpha (swash's color
    /// outlines, composited layer over layer from transparent black).
    Premultiplied,
}

impl BitmapContent {
    pub(crate) fn bytes_per_pixel(self) -> u32 {
        match self {
            Self::Mask => 1,
            Self::Color { .. } => 4,
        }
    }
}

/// Where a bitmap sits relative to the glyph's integer origin: see the
/// module docs. `top` is positive above the baseline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Placement {
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
}

/// A drawn glyph. Its pixels are `bytes` of the [`RasterScratch`] it was
/// drawn into, top row first, rows `stride` bytes apart, until the scratch
/// is drawn into again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GlyphBitmap {
    pub content: BitmapContent,
    pub placement: Placement,
    pub stride: u32,
    pub bytes: Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RasterOutcome {
    /// Nothing to draw (a space, or a glyph with no drawing): cache it
    /// without atlas space.
    Empty,
    Bitmap(GlyphBitmap),
}

/// Why a backend drew nothing. Missing glyphs are shaping's business
/// (fallback), never a rasterizer's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(crate) enum RasterError {
    /// The backend cannot use this font's source (a native backend that
    /// cannot prove a native face holds these bytes). Fall back to swash
    /// for the same font.
    #[error("the rasterizer cannot use this font")]
    UnsupportedFont,
    /// The glyph's only drawing is in a format the backend cannot draw.
    #[error("the glyph's format is not supported")]
    UnsupportedFormat,
    /// The request asks for something the backend does not do.
    #[error("the raster options are not supported")]
    UnsupportedOptions,
    /// The glyph id is past the face's glyphs.
    #[error("no such glyph in the face")]
    InvalidGlyph,
    /// A size or scale that is not finite and positive.
    #[error("invalid raster request")]
    InvalidRequest,
    /// The bitmap would pass [`MAX_BITMAP_BYTES`] (or its size
    /// [`MAX_PHYSICAL_EM`]).
    #[error("the glyph bitmap is too large")]
    TooLarge,
    /// The platform failed this time; asking again later may succeed.
    #[error("the platform rasterizer failed")]
    Platform,
}

/// A backend's output buffer, reused for every glyph so steady
/// rasterization allocates nothing. Backends keep other temporaries in
/// themselves.
#[derive(Debug, Default)]
pub(crate) struct RasterScratch {
    pub(crate) pixels: Vec<u8>,
}

impl RasterScratch {
    /// The pixels of `bitmap`, drawn into this scratch last.
    pub(crate) fn bitmap(&self, bitmap: &GlyphBitmap) -> &[u8] {
        &self.pixels[bitmap.bytes.clone()]
    }
}

/// Draws glyphs for the atlas. Owned by the thread that rasterizes (one
/// per renderer at first), so it takes `&mut self` and need not be `Sync`;
/// the fonts it draws from are immutable and shareable.
pub(crate) trait GlyphRasterizer {
    fn profile(&self) -> RasterProfile;

    fn capabilities(&self) -> RasterCapabilities;

    /// Draws `request`'s glyph from `font` into `scratch`. A blank glyph is
    /// [`RasterOutcome::Empty`], not an error.
    fn rasterize(
        &mut self,
        font: &PreparedFont,
        request: &RasterRequest,
        scratch: &mut RasterScratch,
    ) -> Result<RasterOutcome, RasterError>;
}
