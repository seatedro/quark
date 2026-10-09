//! Which rasterizer draws a renderer's glyphs: the public selection and
//! the native backends behind it. Swash is always built: it is Linux's
//! rasterizer, the explicit rollback everywhere, and the fallback for any
//! glyph or font a native backend cannot draw.

#[allow(unused_imports)]
use super::raster::GlyphRasterizer;
use super::raster::{
    PreparedFont, RasterError, RasterOutcome, RasterProfile, RasterRequest, RasterScratch,
};

/// Which rasterizer draws glyph bitmaps. Layout (advances, line breaks,
/// carets) is quark-text's whichever is chosen; only ink changes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TextRasterizer {
    /// The platform's preferred rasterizer once it has passed its
    /// acceptance gates: CoreText with [`TextSmoothing::System`] on macOS,
    /// swash elsewhere (DirectWrite awaits its Windows runtime checks).
    #[default]
    Auto,
    /// swash on every platform: the deterministic rasterizer, and the
    /// rollback from a native one.
    Swash,
    /// CoreText (macOS only), smoothing as given.
    CoreText(TextSmoothing),
    /// DirectWrite grayscale (Windows with the `text-raster-directwrite`
    /// feature).
    DirectWrite,
}

/// How CoreText smooths (darkens) glyph coverage.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TextSmoothing {
    /// As the user's settings say: off when font smoothing is turned off,
    /// else CoreText's smoothing for the text's own gray, five masks per
    /// glyph from which the shader picks by the paint's luminance.
    #[default]
    System,
    /// Plain antialiased coverage.
    Disabled,
    /// Smoothing for text of one gray whatever the paint: 0 black to 4
    /// white, in quarter steps.
    Fixed(u8),
}

/// The rasterizer [`TextRasterizer::Auto`] stands for on this platform.
pub(crate) fn auto() -> TextRasterizer {
    if cfg!(target_os = "macos") {
        TextRasterizer::CoreText(TextSmoothing::System)
    } else {
        TextRasterizer::Swash
    }
}

/// A native rasterizer's drawing of one glyph.
#[cfg_attr(
    not(any(target_os = "macos", all(windows, feature = "text-raster-directwrite"))),
    allow(dead_code)
)]
pub(crate) enum NativeOutcome {
    Single(RasterOutcome),
    /// Five smoothed coverage planes (CoreText's system smoothing), one
    /// placement and stride.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    Bundle {
        placement: super::raster::Placement,
        stride: u32,
        planes: [std::ops::Range<usize>; 5],
    },
}

/// The native rasterizer a renderer draws with, beside swash.
#[cfg_attr(
    not(any(target_os = "macos", all(windows, feature = "text-raster-directwrite"))),
    allow(dead_code)
)]
pub(crate) enum Native {
    #[cfg(target_os = "macos")]
    CoreText(super::raster::coretext::CoreTextRasterizer),
    #[cfg(all(windows, feature = "text-raster-directwrite"))]
    DirectWrite(super::raster::directwrite::DirectWriteRasterizer),
}

impl Native {
    /// The native rasterizer `choice` asks for: `Ok(None)` for swash,
    /// an error when this build or platform has none.
    pub(crate) fn new(choice: TextRasterizer) -> Result<Option<Self>, RasterError> {
        match choice {
            TextRasterizer::Auto => Self::new(auto()),
            TextRasterizer::Swash => Ok(None),
            #[cfg(target_os = "macos")]
            TextRasterizer::CoreText(smoothing) => {
                use super::raster::coretext::{CoreTextRasterizer, Foreground, SmoothingProfile};
                let profile = match smoothing {
                    TextSmoothing::System => SmoothingProfile::System,
                    TextSmoothing::Disabled => SmoothingProfile::Disabled,
                    TextSmoothing::Fixed(level) => {
                        SmoothingProfile::Fixed(Foreground::ALL[usize::from(level.min(4))])
                    }
                };
                Ok(Some(Self::CoreText(CoreTextRasterizer::new(profile))))
            }
            #[cfg(all(windows, feature = "text-raster-directwrite"))]
            TextRasterizer::DirectWrite => Ok(Some(Self::DirectWrite(
                super::raster::directwrite::DirectWriteRasterizer::new()?,
            ))),
            #[allow(unreachable_patterns)]
            _ => Err(RasterError::UnsupportedOptions),
        }
    }

    pub(crate) fn profile(&self) -> RasterProfile {
        match *self {
            #[cfg(target_os = "macos")]
            Self::CoreText(ref r) => r.profile(),
            #[cfg(all(windows, feature = "text-raster-directwrite"))]
            Self::DirectWrite(ref r) => r.profile(),
        }
    }

    pub(crate) fn rasterize(
        &mut self,
        font: &PreparedFont,
        request: &RasterRequest,
        scratch: &mut RasterScratch,
    ) -> Result<NativeOutcome, RasterError> {
        // Without a native backend in this build the enum is empty.
        let _ = (&font, &request, &scratch);
        match *self {
            #[cfg(target_os = "macos")]
            Self::CoreText(ref mut r) => {
                // System smoothing draws masks as bundles; color glyphs,
                // never smoothed, through the single-bitmap contract.
                if r.smoothing_profile() == super::raster::coretext::SmoothingProfile::System {
                    match r.rasterize_bundle(font, request, scratch) {
                        Ok(Some(bundle)) => {
                            return Ok(NativeOutcome::Bundle {
                                placement: bundle.placement,
                                stride: bundle.stride,
                                planes: bundle.planes,
                            });
                        }
                        Ok(None) => return Ok(NativeOutcome::Single(RasterOutcome::Empty)),
                        Err(RasterError::UnsupportedOptions) => {}
                        Err(error) => return Err(error),
                    }
                }
                r.rasterize(font, request, scratch)
                    .map(NativeOutcome::Single)
            }
            #[cfg(all(windows, feature = "text-raster-directwrite"))]
            Self::DirectWrite(ref mut r) => r
                .rasterize(font, request, scratch)
                .map(NativeOutcome::Single),
        }
    }
}
