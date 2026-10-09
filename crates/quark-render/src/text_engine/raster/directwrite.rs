//! The DirectWrite rasterizer (text-render design 4.4): grayscale coverage
//! from `IDWriteGlyphRunAnalysis`, COLR color layers composited in order,
//! and every other color format handed back to swash for the same face.
//!
//! Status: experimental. The Windows half builds and passes clippy for
//! `x86_64-pc-windows-msvc`, but has never run on Windows; its runtime
//! behavior (pixels, placement, variation instances, color layers) is
//! unverified until a Windows host runs the `windows_` tests below and the
//! native comparison harness. The portable half (the mapping from the
//! contract to DirectWrite's inputs and back) is unit tested everywhere.
//!
//! Faces come from the exact bytes cosmic-text shaped: every source, an
//! installed font included, becomes an in-memory font file over the shared
//! bytes, so the native face cannot be another file of the same name. The
//! face's glyph count is checked against the shaped one, and its variation
//! instance is built from [`PreparedFont::variations`] through
//! `IDWriteFontResource` (Windows 10 1809 and later; older systems fail
//! [`DirectWriteRasterizer::new`] and keep swash).
//!
//! Positions follow the contract: the glyph origin sits at the request's
//! subpixel offset from an integer pixel, which goes into the glyph
//! transform's translation, and the texture bounds DirectWrite reports
//! become the placement. Advances stay cosmic-text's: each request draws
//! one glyph with a zero advance.

// Without Windows only the tests use the portable half.
#![cfg_attr(
    not(all(windows, feature = "text-raster-directwrite")),
    allow(dead_code)
)]

use super::{ColorGlyphs, Hinting, MAX_BITMAP_BYTES, Placement, RasterError, SubpixelOffset};

// Selected by the text engine once stream A wires backend selection.
#[cfg(all(windows, feature = "text-raster-directwrite"))]
#[allow(unused_imports)]
pub(crate) use native::DirectWriteRasterizer;

/// cosmic-text's synthetic italic slant: swash skews outlines 14 degrees.
const ITALIC_DEGREES: f32 = 14.0;

/// `DWRITE_FONT_AXIS_TAG` of an OpenType axis tag:
/// `DWRITE_MAKE_FONT_AXIS_TAG` packs the tag's first character into the
/// low byte, the reverse of the big-endian tag bytes.
const fn axis_tag(tag: [u8; 4]) -> u32 {
    u32::from_le_bytes(tag)
}

/// `DWRITE_RENDERING_MODE1`, with its values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
enum RenderingMode {
    Default = 0,
    Aliased = 1,
    GdiClassic = 2,
    GdiNatural = 3,
    Natural = 4,
    NaturalSymmetric = 5,
    Outline = 6,
    NaturalSymmetricDownsampled = 7,
}

impl RenderingMode {
    fn from_raw(raw: i32) -> Self {
        match raw {
            1 => Self::Aliased,
            2 => Self::GdiClassic,
            3 => Self::GdiNatural,
            4 => Self::Natural,
            5 => Self::NaturalSymmetric,
            6 => Self::Outline,
            7 => Self::NaturalSymmetricDownsampled,
            _ => Self::Default,
        }
    }
}

/// `DWRITE_GRID_FIT_MODE`, with its values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
enum GridFit {
    Default = 0,
    Disabled = 1,
    Enabled = 2,
}

impl GridFit {
    fn from_raw(raw: i32) -> Self {
        match raw {
            1 => Self::Disabled,
            2 => Self::Enabled,
            _ => Self::Default,
        }
    }
}

/// The rendering and grid-fit modes a glyph's coverage is drawn with.
/// [`Hinting::Font`] takes the face's recommendation for the size, which
/// is how DirectWrite applies a font's hinting and `gasp` table, except
/// where that recommendation would not give grayscale coverage: aliased
/// (bilevel) ink, outline mode (which a glyph run analysis cannot draw),
/// and no choice at all become natural symmetric antialiasing.
fn coverage_mode(
    recommended: RenderingMode,
    grid: GridFit,
    hinting: Hinting,
) -> (RenderingMode, GridFit) {
    if hinting == Hinting::Disabled {
        return (RenderingMode::NaturalSymmetric, GridFit::Disabled);
    }
    let mode = match recommended {
        RenderingMode::Aliased | RenderingMode::Outline | RenderingMode::Default => {
            RenderingMode::NaturalSymmetric
        }
        mode => mode,
    };
    (mode, grid)
}

/// `DWRITE_GLYPH_IMAGE_FORMATS` flags.
mod formats {
    pub(super) const TRUETYPE: u32 = 0x1;
    pub(super) const CFF: u32 = 0x2;
    pub(super) const COLR: u32 = 0x4;
    pub(super) const SVG: u32 = 0x8;
    pub(super) const PNG: u32 = 0x10;
    pub(super) const JPEG: u32 = 0x20;
    pub(super) const TIFF: u32 = 0x40;
    pub(super) const PREMULTIPLIED_B8G8R8A8: u32 = 0x80;
    pub(super) const COLR_PAINT_TREE: u32 = 0x100;
    /// Color drawings this backend does not draw itself.
    pub(super) const IMAGES: u32 =
        SVG | PNG | JPEG | TIFF | PREMULTIPLIED_B8G8R8A8 | COLR_PAINT_TREE;
}

/// How a glyph is drawn, from the image formats its face has for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Drawing {
    /// Grayscale coverage of the outline.
    Outline,
    /// COLR layers, each outline's coverage in its palette color.
    Layers,
    /// A color drawing only swash or a renderer DirectWrite needs
    /// Direct2D for can draw (embedded bitmaps, SVG, COLRv1 paint trees):
    /// [`RasterError::UnsupportedFormat`], so the atlas asks swash for the
    /// same face and glyph rather than drawing it without color.
    Fallback,
}

fn drawing(image_formats: u32, color: ColorGlyphs) -> Drawing {
    if color == ColorGlyphs::Never {
        return Drawing::Outline;
    }
    if image_formats & formats::COLR != 0 {
        Drawing::Layers
    } else if image_formats & formats::IMAGES != 0 {
        Drawing::Fallback
    } else {
        Drawing::Outline
    }
}

/// A rectangle of device pixels, y down, `right` and `bottom` exclusive:
/// DirectWrite's texture bounds (a `RECT`) relative to the glyph's
/// integer origin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PixelRect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

impl PixelRect {
    fn is_empty(self) -> bool {
        self.right <= self.left || self.bottom <= self.top
    }

    fn width(self) -> u32 {
        self.right.abs_diff(self.left)
    }

    fn height(self) -> u32 {
        self.bottom.abs_diff(self.top)
    }

    fn union(self, other: Self) -> Self {
        Self {
            left: self.left.min(other.left),
            top: self.top.min(other.top),
            right: self.right.max(other.right),
            bottom: self.bottom.max(other.bottom),
        }
    }
}

/// The contract's placement of a texture with these bounds, `None` when
/// it has no pixels. The contract counts `top` up from the baseline where
/// DirectWrite counts down, so a glyph standing on the baseline has a
/// negative `bounds.top` and a positive placement `top`.
fn placement(bounds: PixelRect, bytes_per_pixel: u32) -> Result<Option<Placement>, RasterError> {
    if bounds.is_empty() {
        return Ok(None);
    }
    let (width, height) = (bounds.width(), bounds.height());
    let bytes = (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(bytes_per_pixel as usize));
    if bytes.is_none_or(|bytes| bytes > MAX_BITMAP_BYTES) {
        return Err(RasterError::TooLarge);
    }
    let top = bounds.top.checked_neg().ok_or(RasterError::TooLarge)?;
    Ok(Some(Placement {
        left: bounds.left,
        top,
        width,
        height,
    }))
}

/// The `DWRITE_MATRIX` that draws a glyph, as `[m11, m12, m21, m22, dx,
/// dy]` mapping `(x, y)` to `(m11 x + m21 y + dx, m12 x + m22 y + dy)` in
/// y-down device pixels: the subpixel offset as its translation, so the
/// slant leaves it alone, and for synthetic italic swash's slant, which
/// moves ink above the baseline right.
fn glyph_transform(italic: bool, subpixel: SubpixelOffset) -> [f32; 6] {
    let (dx, dy) = subpixel.pixels();
    let skew = if italic {
        -ITALIC_DEGREES.to_radians().tan()
    } else {
        0.0
    };
    [1.0, 0.0, skew, 1.0, dx, dy]
}

/// Paints one COLR layer over a premultiplied RGBA canvas covering
/// `canvas_rect`, source over (layers arrive bottom first): `coverage` is
/// the layer's one-byte coverage over `rect`, which lies inside the
/// canvas, and `color` its straight RGBA palette color, sRGB encoded as
/// COLR stores it. Composited in encoded values, as swash composites its
/// color outlines.
fn paint_layer(
    canvas: &mut [u8],
    canvas_rect: PixelRect,
    coverage: &[u8],
    rect: PixelRect,
    color: [f32; 4],
) {
    let color = color.map(|c| c.clamp(0.0, 1.0));
    let (canvas_width, width) = (canvas_rect.width() as usize, rect.width() as usize);
    let column = (rect.left - canvas_rect.left) as usize;
    for (row, line) in coverage.chunks_exact(width).enumerate() {
        let y = (rect.top - canvas_rect.top) as usize + row;
        let start = (y * canvas_width + column) * 4;
        let pixels = canvas[start..start + width * 4].as_chunks_mut::<4>().0;
        for (pixel, &cover) in pixels.iter_mut().zip(line) {
            if cover == 0 {
                continue;
            }
            let alpha = color[3] * f32::from(cover) / 255.0;
            let over = |source: f32, below: u8| {
                (source * 255.0 + f32::from(below) * (1.0 - alpha)).round() as u8
            };
            pixel[0] = over(color[0] * alpha, pixel[0]);
            pixel[1] = over(color[1] * alpha, pixel[1]);
            pixel[2] = over(color[2] * alpha, pixel[2]);
            pixel[3] = over(alpha, pixel[3]);
        }
    }
}

#[cfg(all(windows, feature = "text-raster-directwrite"))]
mod native {
    use std::collections::HashMap;
    use std::mem::ManuallyDrop;
    use std::ops::Range;
    use std::sync::Arc;

    use quark_text::fonts::{FontInstanceId, FontSource, FontSourceId, PreparedFont};
    use windows::Win32::Foundation::{DWRITE_E_NOCOLOR, RECT};
    use windows::Win32::Graphics::DirectWrite::*;
    use windows::core::{BOOL, IUnknown};
    use windows_core::implement;

    use super::super::{
        AlphaMode, BitmapContent, GlyphBitmap, GlyphRasterizer, RasterBackend, RasterCapabilities,
        RasterOutcome, RasterProfile, RasterRequest, RasterScratch,
    };
    use super::*;

    // The portable mirrors of DirectWrite's values agree with the SDK's.
    const _: () = {
        assert!(RenderingMode::Default as i32 == DWRITE_RENDERING_MODE1_DEFAULT.0);
        assert!(RenderingMode::Aliased as i32 == DWRITE_RENDERING_MODE1_ALIASED.0);
        assert!(RenderingMode::GdiClassic as i32 == DWRITE_RENDERING_MODE1_GDI_CLASSIC.0);
        assert!(RenderingMode::GdiNatural as i32 == DWRITE_RENDERING_MODE1_GDI_NATURAL.0);
        assert!(RenderingMode::Natural as i32 == DWRITE_RENDERING_MODE1_NATURAL.0);
        assert!(
            RenderingMode::NaturalSymmetric as i32 == DWRITE_RENDERING_MODE1_NATURAL_SYMMETRIC.0
        );
        assert!(RenderingMode::Outline as i32 == DWRITE_RENDERING_MODE1_OUTLINE.0);
        assert!(
            RenderingMode::NaturalSymmetricDownsampled as i32
                == DWRITE_RENDERING_MODE1_NATURAL_SYMMETRIC_DOWNSAMPLED.0
        );
        assert!(GridFit::Default as i32 == DWRITE_GRID_FIT_MODE_DEFAULT.0);
        assert!(GridFit::Disabled as i32 == DWRITE_GRID_FIT_MODE_DISABLED.0);
        assert!(GridFit::Enabled as i32 == DWRITE_GRID_FIT_MODE_ENABLED.0);
        assert!(formats::TRUETYPE == DWRITE_GLYPH_IMAGE_FORMATS_TRUETYPE.0 as u32);
        assert!(formats::CFF == DWRITE_GLYPH_IMAGE_FORMATS_CFF.0 as u32);
        assert!(formats::COLR == DWRITE_GLYPH_IMAGE_FORMATS_COLR.0 as u32);
        assert!(formats::SVG == DWRITE_GLYPH_IMAGE_FORMATS_SVG.0 as u32);
        assert!(formats::PNG == DWRITE_GLYPH_IMAGE_FORMATS_PNG.0 as u32);
        assert!(formats::JPEG == DWRITE_GLYPH_IMAGE_FORMATS_JPEG.0 as u32);
        assert!(formats::TIFF == DWRITE_GLYPH_IMAGE_FORMATS_TIFF.0 as u32);
        assert!(
            formats::PREMULTIPLIED_B8G8R8A8
                == DWRITE_GLYPH_IMAGE_FORMATS_PREMULTIPLIED_B8G8R8A8.0 as u32
        );
        assert!(formats::COLR_PAINT_TREE == DWRITE_GLYPH_IMAGE_FORMATS_COLR_PAINT_TREE.0 as u32);
        assert!(axis_tag(*b"wght") == DWRITE_FONT_AXIS_TAG_WEIGHT.0);
        assert!(axis_tag(*b"opsz") == DWRITE_FONT_AXIS_TAG_OPTICAL_SIZE.0);
    };

    /// Bumped whenever this backend's bitmap for a request may change.
    const REVISION: u32 = 1;

    /// Native faces kept, one per font instance, and in-memory font files,
    /// one per source. Past either, that cache starts over: a face costs a
    /// few table lookups to make again, and the bytes stay in quark-text.
    const MAX_FACES: usize = 256;
    const MAX_FILES: usize = 64;

    /// Keeps a font source's bytes alive while DirectWrite references
    /// them, so in-memory font files read the bytes cosmic-text shaped
    /// instead of a copy.
    #[implement()]
    struct SourceOwner {
        _bytes: Arc<dyn AsRef<[u8]> + Send + Sync>,
    }

    /// One COLR layer's coverage, in the rasterizer's layer buffer.
    struct Layer {
        rect: PixelRect,
        color: [f32; 4],
        pixels: Range<usize>,
    }

    /// Draws glyphs with DirectWrite. One per rasterizing thread: the
    /// factory is DirectWrite's shared, thread-safe one, and the face
    /// caches and buffers are this rasterizer's own, so it is not `Sync`
    /// (nor `Send`: windows-rs interfaces are neither). DirectWrite needs
    /// no COM apartment on the thread.
    pub(crate) struct DirectWriteRasterizer {
        factory: IDWriteFactory6,
        loader: IDWriteInMemoryFontFileLoader,
        files: HashMap<FontSourceId, IDWriteFontFile>,
        faces: HashMap<FontInstanceId, Result<IDWriteFontFace5, RasterError>>,
        layers: Vec<Layer>,
        layer_pixels: Vec<u8>,
    }

    fn platform(_: windows::core::Error) -> RasterError {
        RasterError::Platform
    }

    impl DirectWriteRasterizer {
        /// Fails with [`RasterError::Platform`] where DirectWrite lacks
        /// `IDWriteFactory6` (before Windows 10 1809).
        pub(crate) fn new() -> Result<Self, RasterError> {
            // SAFETY: creating the shared factory has no preconditions.
            let factory: IDWriteFactory6 =
                unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED) }.map_err(platform)?;
            // SAFETY: a plain factory call; the loader is registered once
            // and unregistered on drop, after every file made with it.
            let loader = unsafe { factory.CreateInMemoryFontFileLoader() }.map_err(platform)?;
            unsafe { factory.RegisterFontFileLoader(&loader) }.map_err(platform)?;
            Ok(Self {
                factory,
                loader,
                files: HashMap::new(),
                faces: HashMap::new(),
                layers: Vec::new(),
                layer_pixels: Vec::new(),
            })
        }

        fn file(&mut self, source: &FontSource) -> Result<IDWriteFontFile, RasterError> {
            if let Some(file) = self.files.get(&source.id()) {
                return Ok(file.clone());
            }
            if self.files.len() >= MAX_FILES {
                self.files.clear();
            }
            let bytes = source.data();
            let len = u32::try_from(bytes.len()).map_err(|_| RasterError::UnsupportedFont)?;
            let owner: IUnknown = SourceOwner {
                _bytes: source.shared_data().clone(),
            }
            .into();
            // SAFETY: `owner` holds the Arc that owns `bytes`, which are
            // immutable and do not move, for as long as the file lives.
            let file = unsafe {
                self.loader.CreateInMemoryFontFileReference(
                    &self.factory,
                    bytes.as_ptr().cast(),
                    len,
                    &owner,
                )
            }
            .map_err(platform)?;
            self.files.insert(source.id(), file.clone());
            Ok(file)
        }

        fn face(&mut self, font: &PreparedFont) -> Result<IDWriteFontFace5, RasterError> {
            if let Some(face) = self.faces.get(&font.instance()) {
                return face.clone();
            }
            if self.faces.len() >= MAX_FACES {
                self.faces.clear();
            }
            let face = self.make_face(font);
            self.faces.insert(font.instance(), face.clone());
            face
        }

        /// The native face of exactly `font`: its source's bytes, face
        /// index, and variation values. A face DirectWrite reads
        /// differently (another glyph count) is
        /// [`RasterError::UnsupportedFont`], for swash to draw.
        fn make_face(&mut self, font: &PreparedFont) -> Result<IDWriteFontFace5, RasterError> {
            let file = self.file(font.source())?;
            let mut supported = BOOL::default();
            let mut file_type = DWRITE_FONT_FILE_TYPE::default();
            let mut faces = 0u32;
            // SAFETY: every out pointer is a live local.
            unsafe { file.Analyze(&mut supported, &mut file_type, None, &mut faces) }
                .map_err(platform)?;
            if !supported.as_bool() || font.index() >= faces {
                return Err(RasterError::UnsupportedFont);
            }
            // SAFETY: plain factory and resource calls on live objects.
            let resource = unsafe { self.factory.CreateFontResource(&file, font.index()) }
                .map_err(|_| RasterError::UnsupportedFont)?;
            let axes: Vec<DWRITE_FONT_AXIS_VALUE> = font
                .variations()
                .iter()
                .map(|variation| DWRITE_FONT_AXIS_VALUE {
                    axisTag: DWRITE_FONT_AXIS_TAG(axis_tag(variation.tag)),
                    value: variation.value,
                })
                .collect();
            // Synthetic italic is a transform (swash's exact slant), not
            // DirectWrite's oblique simulation.
            let face = unsafe { resource.CreateFontFace(DWRITE_FONT_SIMULATIONS_NONE, &axes) }
                .map_err(|_| RasterError::UnsupportedFont)?;
            if u32::from(unsafe { face.GetGlyphCount() }) != font.glyph_count() {
                return Err(RasterError::UnsupportedFont);
            }
            Ok(face)
        }

        /// The modes `face` recommends for `em` pixels under `transform`,
        /// made grayscale by [`coverage_mode`].
        fn modes(
            face: &IDWriteFontFace5,
            em: f32,
            transform: &DWRITE_MATRIX,
            hinting: Hinting,
        ) -> Result<(RenderingMode, GridFit), RasterError> {
            let mut mode = DWRITE_RENDERING_MODE1::default();
            let mut grid = DWRITE_GRID_FIT_MODE::default();
            // SAFETY: the out pointers are live locals. 96 DPI makes a
            // DIP one device pixel, the unit of the physical em.
            unsafe {
                face.GetRecommendedRenderingMode(
                    em,
                    96.0,
                    96.0,
                    Some(transform),
                    false,
                    DWRITE_OUTLINE_THRESHOLD_ANTIALIASED,
                    DWRITE_MEASURING_MODE_NATURAL,
                    None::<&IDWriteRenderingParams>,
                    &mut mode,
                    &mut grid,
                )
            }
            .map_err(platform)?;
            Ok(coverage_mode(
                RenderingMode::from_raw(mode.0),
                GridFit::from_raw(grid.0),
                hinting,
            ))
        }

        /// Draws the layers of a COLR glyph into `scratch`. `None` when
        /// DirectWrite finds no color layers for it after all.
        fn layers(
            &mut self,
            run: &DWRITE_GLYPH_RUN,
            transform: &DWRITE_MATRIX,
            modes: (RenderingMode, GridFit),
            scratch: &mut RasterScratch,
        ) -> Result<Option<RasterOutcome>, RasterError> {
            let factory: &IDWriteFactory2 = &self.factory;
            // SAFETY: `run` and `transform` outlive the call; the
            // enumerator copies what it needs.
            let translated = unsafe {
                factory.TranslateColorGlyphRun(
                    0.0,
                    0.0,
                    run,
                    None,
                    DWRITE_MEASURING_MODE_NATURAL,
                    Some(transform),
                    0,
                )
            };
            let layers = match translated {
                Ok(layers) => layers,
                Err(error) if error.code() == DWRITE_E_NOCOLOR => return Ok(None),
                Err(error) => return Err(platform(error)),
            };
            self.layers.clear();
            self.layer_pixels.clear();
            // SAFETY: each current run stays valid until the next
            // `MoveNext`, and is only read before it.
            while unsafe { layers.MoveNext() }.map_err(platform)?.as_bool() {
                let layer = unsafe { &*layers.GetCurrentRun().map_err(platform)? };
                // A layer in the text's own color: paint, which a bitmap
                // cannot hold. swash draws such glyphs.
                if layer.paletteIndex == 0xFFFF {
                    return Err(RasterError::UnsupportedFormat);
                }
                let start = self.layer_pixels.len();
                let origin = (layer.baselineOriginX, layer.baselineOriginY);
                if let Some(rect) = coverage(
                    &self.factory,
                    &layer.glyphRun,
                    transform,
                    modes,
                    origin,
                    &mut self.layer_pixels,
                )? {
                    let color = layer.runColor;
                    self.layers.push(Layer {
                        rect,
                        color: [color.r, color.g, color.b, color.a],
                        pixels: start..self.layer_pixels.len(),
                    });
                }
            }
            let Some(bounds) = self
                .layers
                .iter()
                .map(|layer| layer.rect)
                .reduce(PixelRect::union)
            else {
                return Ok(Some(RasterOutcome::Empty));
            };
            let Some(placement) = placement(bounds, 4)? else {
                return Ok(Some(RasterOutcome::Empty));
            };
            let len = placement.width as usize * placement.height as usize * 4;
            scratch.pixels.clear();
            scratch.pixels.resize(len, 0);
            for layer in &self.layers {
                paint_layer(
                    &mut scratch.pixels,
                    bounds,
                    &self.layer_pixels[layer.pixels.clone()],
                    layer.rect,
                    layer.color,
                );
            }
            Ok(Some(RasterOutcome::Bitmap(GlyphBitmap {
                content: BitmapContent::Color {
                    alpha: AlphaMode::Premultiplied,
                },
                placement,
                stride: placement.width * 4,
                bytes: 0..len,
            })))
        }
    }

    impl Drop for DirectWriteRasterizer {
        fn drop(&mut self) {
            // Files made by the loader go before it is unregistered.
            self.faces.clear();
            self.files.clear();
            // SAFETY: the loader was registered with this factory in `new`.
            let _ = unsafe { self.factory.UnregisterFontFileLoader(&self.loader) };
        }
    }

    /// Appends the grayscale coverage of `run` drawn at `origin` under
    /// `transform` to `out`, one byte per pixel, and returns its bounds;
    /// `None` (and nothing appended) for a glyph with no ink.
    fn coverage(
        factory: &IDWriteFactory6,
        run: &DWRITE_GLYPH_RUN,
        transform: &DWRITE_MATRIX,
        (mode, grid): (RenderingMode, GridFit),
        origin: (f32, f32),
        out: &mut Vec<u8>,
    ) -> Result<Option<PixelRect>, RasterError> {
        // SAFETY: `run` and `transform` outlive the call.
        let analysis = unsafe {
            factory.CreateGlyphRunAnalysis(
                run,
                Some(transform),
                DWRITE_RENDERING_MODE1(mode as i32),
                DWRITE_MEASURING_MODE_NATURAL,
                DWRITE_GRID_FIT_MODE(grid as i32),
                DWRITE_TEXT_ANTIALIAS_MODE_GRAYSCALE,
                origin.0,
                origin.1,
            )
        }
        .map_err(platform)?;
        // With grayscale antialiasing the 1x1 texture holds coverage, not
        // bilevel ink, despite its name.
        let bounds = unsafe { analysis.GetAlphaTextureBounds(DWRITE_TEXTURE_ALIASED_1x1) }
            .map_err(platform)?;
        let rect = PixelRect {
            left: bounds.left,
            top: bounds.top,
            right: bounds.right,
            bottom: bounds.bottom,
        };
        let Some(placement) = placement(rect, 1)? else {
            return Ok(None);
        };
        let start = out.len();
        out.resize(
            start + placement.width as usize * placement.height as usize,
            0,
        );
        let texture = RECT {
            left: rect.left,
            top: rect.top,
            right: rect.right,
            bottom: rect.bottom,
        };
        // SAFETY: the slice holds exactly the texture's pixels.
        unsafe {
            analysis.CreateAlphaTexture(DWRITE_TEXTURE_ALIASED_1x1, &texture, &mut out[start..])
        }
        .map_err(platform)?;
        Ok(Some(rect))
    }

    /// Calls `draw` with a one-glyph run of `glyph` from `face` at `em`
    /// pixels and no advance, releasing the run's face reference after.
    fn with_run<R>(
        face: &IDWriteFontFace,
        glyph: u16,
        em: f32,
        draw: impl FnOnce(&DWRITE_GLYPH_RUN) -> R,
    ) -> R {
        let advance = 0.0f32;
        let mut run = DWRITE_GLYPH_RUN {
            fontFace: ManuallyDrop::new(Some(face.clone())),
            fontEmSize: em,
            glyphCount: 1,
            glyphIndices: &glyph,
            glyphAdvances: &advance,
            glyphOffsets: std::ptr::null(),
            isSideways: false.into(),
            bidiLevel: 0,
        };
        let result = draw(&run);
        // SAFETY: the run is not used after this, and its face reference
        // was added by the clone above.
        unsafe { ManuallyDrop::drop(&mut run.fontFace) };
        result
    }

    impl GlyphRasterizer for DirectWriteRasterizer {
        fn profile(&self) -> RasterProfile {
            RasterProfile {
                backend: RasterBackend::DirectWrite,
                revision: REVISION,
            }
        }

        fn capabilities(&self) -> RasterCapabilities {
            RasterCapabilities {
                color_outlines: true,
                // Embedded bitmaps fall back to swash.
                color_bitmaps: false,
                variations: true,
                // Synthetic italic is drawn; thickening is not.
                synthesis: false,
            }
        }

        fn rasterize(
            &mut self,
            font: &PreparedFont,
            request: &RasterRequest,
            scratch: &mut RasterScratch,
        ) -> Result<RasterOutcome, RasterError> {
            let glyph = request.glyph();
            if u32::from(glyph) >= font.glyph_count() {
                return Err(RasterError::InvalidGlyph);
            }
            let synthesis = font.synthesis();
            // Terminal thickening keeps swash's meaning until a native
            // equivalent is calibrated against it (design 4.3).
            if synthesis.thicken {
                return Err(RasterError::UnsupportedOptions);
            }
            let face = self.face(font)?;
            let em = request.physical_em();
            let [m11, m12, m21, m22, dx, dy] =
                glyph_transform(synthesis.italic, request.subpixel());
            let transform = DWRITE_MATRIX {
                m11,
                m12,
                m21,
                m22,
                dx,
                dy,
            };
            let options = request.options();
            let modes = Self::modes(&face, em, &transform, options.hinting)?;
            // SAFETY: plain queries of a live face.
            let drawing = if unsafe { face.IsColorFont() }.as_bool() {
                let ppem = em.round() as u32;
                let formats =
                    unsafe { face.GetGlyphImageFormats(glyph, ppem, ppem) }.map_err(platform)?;
                super::drawing(formats.0 as u32, options.color)
            } else {
                Drawing::Outline
            };
            with_run(&face, glyph, em, |run| {
                match drawing {
                    Drawing::Fallback => return Err(RasterError::UnsupportedFormat),
                    Drawing::Layers => {
                        if let Some(outcome) = self.layers(run, &transform, modes, scratch)? {
                            return Ok(outcome);
                        }
                    }
                    Drawing::Outline => {}
                }
                scratch.pixels.clear();
                let Some(rect) = coverage(
                    &self.factory,
                    run,
                    &transform,
                    modes,
                    (0.0, 0.0),
                    &mut scratch.pixels,
                )?
                else {
                    return Ok(RasterOutcome::Empty);
                };
                let placement = placement(rect, 1)?.ok_or(RasterError::Platform)?;
                Ok(RasterOutcome::Bitmap(GlyphBitmap {
                    content: BitmapContent::Mask,
                    placement,
                    stride: placement.width,
                    bytes: 0..scratch.pixels.len(),
                }))
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quark_text::cosmic_text::SubpixelBin;

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> PixelRect {
        PixelRect {
            left,
            top,
            right,
            bottom,
        }
    }

    // Variation values reach DirectWrite under its tag encoding, which
    // stores the tag's first character in the low byte; big-endian tags
    // would name axes the font does not have and silently draw defaults.
    #[test]
    fn axis_tags_use_directwrite_byte_order() {
        // DWRITE_FONT_AXIS_TAG_WEIGHT, _OPTICAL_SIZE, _WIDTH from dwrite_3.h.
        assert_eq!(axis_tag(*b"wght"), 0x7468_6777);
        assert_eq!(axis_tag(*b"opsz"), 0x7A73_706F);
        assert_eq!(axis_tag(*b"wdth"), 0x6874_6477);
    }

    // DirectWrite's texture bounds count y down from the baseline; the
    // contract's top counts up. A glyph standing on the baseline (an "H"
    // from 10 pixels above it to the baseline) gets a positive top, and its
    // left bearing carries over unchanged.
    #[test]
    fn texture_bounds_become_bearings_above_the_baseline() {
        assert_eq!(
            placement(rect(-1, -10, 7, 0), 1),
            Ok(Some(Placement {
                left: -1,
                top: 10,
                width: 8,
                height: 10,
            }))
        );
        // A descender reaching 3 pixels below the baseline.
        assert_eq!(
            placement(rect(0, -7, 5, 3), 1).map(|p| p.map(|p| (p.top, p.height))),
            Ok(Some((7, 10)))
        );
    }

    // No ink is an empty glyph, not an error, and a bitmap past the
    // contract's byte limit is refused before anything is allocated.
    #[test]
    fn empty_and_oversized_textures() {
        assert_eq!(placement(rect(3, -4, 3, 2), 1), Ok(None));
        assert_eq!(placement(rect(0, 0, 0, 0), 4), Ok(None));
        // 2048 x 2048 color pixels is exactly 16 MiB; one more row is not.
        assert!(placement(rect(0, -2048, 2048, 0), 4).is_ok());
        assert_eq!(
            placement(rect(0, -2049, 2048, 0), 4),
            Err(RasterError::TooLarge)
        );
        assert_eq!(
            placement(rect(i32::MIN, i32::MIN, i32::MAX, i32::MAX), 1),
            Err(RasterError::TooLarge)
        );
    }

    #[test]
    fn coverage_modes_stay_grayscale_and_honor_disabled_hinting() {
        use GridFit as G;
        use RenderingMode as M;
        let font = Hinting::Font;
        for (recommended, grid, hinting, expected) in [
            // The face's choice stands when it is a grayscale mode.
            (M::GdiClassic, G::Enabled, font, (M::GdiClassic, G::Enabled)),
            (M::Natural, G::Default, font, (M::Natural, G::Default)),
            (
                M::NaturalSymmetricDownsampled,
                G::Disabled,
                font,
                (M::NaturalSymmetricDownsampled, G::Disabled),
            ),
            // Bilevel, outline, and unresolved modes become grayscale,
            // keeping the face's grid fitting.
            (
                M::Aliased,
                G::Enabled,
                font,
                (M::NaturalSymmetric, G::Enabled),
            ),
            (
                M::Outline,
                G::Disabled,
                font,
                (M::NaturalSymmetric, G::Disabled),
            ),
            (
                M::Default,
                G::Enabled,
                font,
                (M::NaturalSymmetric, G::Enabled),
            ),
            // Unhinted text never snaps to the grid.
            (
                M::GdiClassic,
                G::Enabled,
                Hinting::Disabled,
                (M::NaturalSymmetric, G::Disabled),
            ),
        ] {
            assert_eq!(
                coverage_mode(recommended, grid, hinting),
                expected,
                "{recommended:?} {grid:?} {hinting:?}"
            );
        }
    }

    #[test]
    fn color_formats_choose_layers_outline_or_swash() {
        use formats::*;
        for (image_formats, color, expected) in [
            (TRUETYPE, ColorGlyphs::Prefer, Drawing::Outline),
            (CFF, ColorGlyphs::Prefer, Drawing::Outline),
            // COLRv0 layers win over the outline and any bitmap strike.
            (TRUETYPE | COLR, ColorGlyphs::Prefer, Drawing::Layers),
            (
                COLR | COLR_PAINT_TREE | PNG,
                ColorGlyphs::Prefer,
                Drawing::Layers,
            ),
            // Embedded bitmaps (CBDT, sbix), SVG, and COLRv1 alone go to
            // swash for the same face, even beside an outline.
            (PNG, ColorGlyphs::Prefer, Drawing::Fallback),
            (
                TRUETYPE | PREMULTIPLIED_B8G8R8A8,
                ColorGlyphs::Prefer,
                Drawing::Fallback,
            ),
            (TRUETYPE | SVG, ColorGlyphs::Prefer, Drawing::Fallback),
            (
                COLR_PAINT_TREE | TRUETYPE,
                ColorGlyphs::Prefer,
                Drawing::Fallback,
            ),
            (JPEG | TIFF, ColorGlyphs::Prefer, Drawing::Fallback),
            // Monochrome requests draw the outline whatever else exists.
            (TRUETYPE | COLR | PNG, ColorGlyphs::Never, Drawing::Outline),
        ] {
            assert_eq!(
                drawing(image_formats, color),
                expected,
                "{image_formats:#x} {color:?}"
            );
        }
    }

    fn apply([m11, m12, m21, m22, dx, dy]: [f32; 6], (x, y): (f32, f32)) -> (f32, f32) {
        (m11 * x + m21 * y + dx, m12 * x + m22 * y + dy)
    }

    // Synthetic italic leans ink above the baseline right by tan(14°) per
    // pixel of height, as swash's skew does, and the subpixel offset moves
    // every point by the same amount, slanted or not.
    #[test]
    fn italic_slants_right_above_the_baseline_around_the_subpixel_origin() {
        let offset = SubpixelOffset {
            x: SubpixelBin::Two,
            y: SubpixelBin::Zero,
        };
        let upright = glyph_transform(false, offset);
        assert_eq!(apply(upright, (3.0, -10.0)), (3.5, -10.0));
        let italic = glyph_transform(true, offset);
        let (x, y) = apply(italic, (0.0, -10.0));
        assert!((x - (0.5 + 2.4933)).abs() < 1e-3, "{x}");
        assert_eq!(y, -10.0);
        assert_eq!(apply(italic, (4.0, 0.0)), (4.5, 0.0));
    }

    // Layers composite bottom first, source over, premultiplied: a half
    // transparent blue layer over an opaque red one leaves an even mix
    // where they overlap and each color alone elsewhere, inside one
    // canvas that covers both.
    #[test]
    fn color_layers_composite_in_order() {
        let canvas_rect = rect(0, -2, 3, 0);
        let mut canvas = vec![0u8; 3 * 2 * 4];
        let pixel = |canvas: &[u8], x: usize, y: usize| -> [u8; 4] {
            let i = (y * 3 + x) * 4;
            canvas[i..i + 4].try_into().unwrap()
        };
        // Red covers the left two columns of the top row.
        paint_layer(
            &mut canvas,
            canvas_rect,
            &[255, 255],
            rect(0, -2, 2, -1),
            [1.0, 0.0, 0.0, 1.0],
        );
        // Blue at half opacity covers the right two columns of both rows.
        paint_layer(
            &mut canvas,
            canvas_rect,
            &[255, 255, 255, 255],
            rect(1, -2, 3, 0),
            [0.0, 0.0, 1.0, 0.5],
        );
        assert_eq!(pixel(&canvas, 0, 0), [255, 0, 0, 255]);
        assert_eq!(pixel(&canvas, 1, 0), [128, 0, 128, 255]);
        assert_eq!(pixel(&canvas, 2, 0), [0, 0, 128, 128]);
        assert_eq!(pixel(&canvas, 0, 1), [0, 0, 0, 0]);
        assert_eq!(pixel(&canvas, 1, 1), [0, 0, 128, 128]);
    }

    /// The first glyph of `text` laid out at 32 px, and its prepared font.
    #[cfg(all(windows, feature = "text-raster-directwrite"))]
    fn shaped(text: &str, family: Option<&'static str>) -> (quark_text::fonts::PreparedFont, u16) {
        use quark_text::fonts::FontRegistry;
        use quark_text::{TextParams, TextStyle};
        let mut system = crate::text::test_text();
        let layout = system
            .layout(&TextParams::new(text, TextStyle::new(32.0).family(family)))
            .expect("layout");
        let glyph = layout
            .buffer()
            .layout_runs()
            .flat_map(|run| run.glyphs.iter())
            .next()
            .expect("a glyph")
            .clone();
        let mut registry = FontRegistry::new(system.font_snapshot());
        let font = registry
            .prepare(glyph.font_id, glyph.font_weight, glyph.cache_key_flags)
            .expect("prepared font")
            .clone();
        (font, glyph.glyph_id)
    }

    // NOT YET RUN: needs a Windows host. A capital H of the bundled UI font
    // at 32 px draws as grayscale coverage standing on the baseline, its
    // top near the font's cap height (0.7 em), with partial coverage on
    // its edges.
    #[cfg(all(windows, feature = "text-raster-directwrite"))]
    #[test]
    fn windows_draws_grayscale_coverage_on_the_baseline() {
        use super::super::{
            BitmapContent, GlyphRasterizer, RasterOptions, RasterOutcome, RasterRequest,
            RasterScratch,
        };
        let (font, glyph) = shaped("H", None);
        let mut rasterizer = DirectWriteRasterizer::new().expect("DirectWrite");
        let request = RasterRequest::new(
            glyph,
            32.0,
            1.0,
            SubpixelOffset::ZERO,
            RasterOptions::default(),
        )
        .expect("request");
        let mut scratch = RasterScratch::default();
        let RasterOutcome::Bitmap(bitmap) = rasterizer
            .rasterize(&font, &request, &mut scratch)
            .expect("rasterize")
        else {
            panic!("H drew nothing");
        };
        assert_eq!(bitmap.content, BitmapContent::Mask);
        let p = bitmap.placement;
        assert!((20..=25).contains(&p.top), "top {}", p.top);
        assert!(p.height as i32 - p.top <= 1, "ink below the baseline");
        let pixels = scratch.bitmap(&bitmap);
        assert!(pixels.contains(&255));
        assert!(pixels.iter().any(|&c| c > 0 && c < 255));
    }

    // NOT YET RUN: needs a Windows host. The bundled emoji font is CBDT
    // bitmaps only, which this backend leaves to swash for the same face
    // rather than drawing a blank or monochrome glyph.
    #[cfg(all(windows, feature = "text-raster-directwrite"))]
    #[test]
    fn windows_hands_embedded_bitmap_emoji_to_swash() {
        use super::super::{GlyphRasterizer, RasterOptions, RasterRequest, RasterScratch};
        let (font, glyph) = shaped("😀", Some(quark_text::fonts::EMOJI_FAMILY));
        let mut rasterizer = DirectWriteRasterizer::new().expect("DirectWrite");
        let request = RasterRequest::new(
            glyph,
            32.0,
            1.0,
            SubpixelOffset::ZERO,
            RasterOptions::default(),
        )
        .expect("request");
        assert_eq!(
            rasterizer.rasterize(&font, &request, &mut RasterScratch::default()),
            Err(RasterError::UnsupportedFormat)
        );
    }
}
