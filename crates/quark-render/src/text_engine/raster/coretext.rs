//! The CoreText rasterizer: draws the exact glyph quark-text shaped with
//! `CTFontDrawGlyphs` into a bitmap context, from the same font bytes.
//!
//! Faces come from the [`PreparedFont`]'s shared bytes (never by family or
//! file lookup), wrapped without copying in a `CFData` that keeps the bytes
//! alive for as long as CoreText holds them. A collection member is chosen
//! by comparing its tables with the face the glyph was shaped from, and
//! every face is checked (glyph count, tables, character mapping) before
//! use; a face that fails is [`RasterError::UnsupportedFont`], which falls
//! back to swash. Every variation axis is set to the value shaping used and
//! CoreText's automatic optical size is turned off, so the outline drawn is
//! the outline measured.
//!
//! Glyphs are drawn at the font's logical size under a device-scale
//! transform, as AppKit draws text on a Retina display, with antialiasing
//! and fractional positioning on and CoreGraphics' own subpixel
//! quantization off (the request's quarter-pixel offset already is the
//! quantized position).
//!
//! Font smoothing is CoreText's stem darkening, which on macOS 27 depends
//! on the text's gray: measured on macbox (see
//! `tests/native_text/macos/`), AppKit's window text is byte-identical to a
//! DeviceGray bitmap context with smoothing on, and smoothed coverage of
//! white text carries about 23% more ink than black text's. So a smoothed
//! mask is drawn for one [`Foreground`] gray and recovered as
//! `(C - B) / (F - B)` from text gray `F` over background `B`.
//! [`SmoothingProfile::System`] draws all five grays as one
//! [`CoverageBundle`] for paint to pick from; the single-bitmap contract
//! carries [`SmoothingProfile::Disabled`] and [`SmoothingProfile::Fixed`].

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::Arc;

use quark_text::cosmic_text::skrifa::instance::Size;
use quark_text::cosmic_text::skrifa::{self, GlyphId, MetadataProvider};
use quark_text::fonts::{FaceId, FontInstanceId};

use super::{
    AlphaMode, BitmapContent, ColorGlyphs, GlyphBitmap, GlyphRasterizer, MAX_BITMAP_BYTES,
    Placement, PreparedFont, RasterBackend, RasterCapabilities, RasterError, RasterOutcome,
    RasterProfile, RasterRequest, RasterScratch,
};

/// Bumped whenever this backend's bitmaps for the same request change.
const REVISION: u32 = 1;

/// Synthetic italic's slant, as swash draws `FAKE_ITALIC`.
const ITALIC_DEGREES: f64 = 14.0;

/// Sized fonts kept per rasterizer; the least recently used beyond this
/// are released (their faces stay).
const MAX_SIZED_FONTS: usize = 64;

/// A text gray CoreText smooths for, in quarter steps from black (0) to
/// white (4). Smoothing darkens stems more the lighter the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct Foreground(u8);

impl Foreground {
    pub(crate) const BLACK: Self = Self(0);
    pub(crate) const WHITE: Self = Self(4);
    pub(crate) const ALL: [Self; 5] = [Self(0), Self(1), Self(2), Self(3), Self(4)];

    /// The level for text whose luminance, sRGB encoded, is `encoded`
    /// (0 to 255): the gray the calibration drew with, nearest first.
    pub(crate) fn nearest(encoded: u8) -> Self {
        Self(((u32::from(encoded) * 4 + 127) / 255) as u8)
    }

    fn gray(self) -> f64 {
        f64::from(self.0) / 4.0
    }

    /// The background coverage is recovered against: the far end of the
    /// range, so `F - B` is at least half of it.
    fn background(self) -> f64 {
        if self.0 < 2 { 1.0 } else { 0.0 }
    }
}

/// How masks are smoothed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum SmoothingProfile {
    /// What the user's settings ask for: no smoothing when "Use font
    /// smoothing when available" is off (`AppleFontSmoothing` 0), else
    /// CoreText's smoothing for each [`Foreground`], drawn together by
    /// [`CoreTextRasterizer::rasterize_bundle`].
    System,
    /// Smoothing for text of one gray, whatever the paint.
    Fixed(Foreground),
    /// Plain antialiased coverage.
    Disabled,
}

/// The profile [`SmoothingProfile::System`] resolves to now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Smoothing {
    Bundle,
    Fixed(Foreground),
    Disabled,
}

/// One glyph's smoothed masks for every [`Foreground`], sharing one
/// placement and stride: `planes[i]` is [`Foreground::ALL`]`[i]`'s bytes in
/// the scratch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CoverageBundle {
    pub placement: Placement,
    pub stride: u32,
    pub planes: [std::ops::Range<usize>; 5],
}

/// Draws glyphs with CoreText. Each rasterizer owns its contexts and font
/// objects; build one per rasterizing thread.
pub(crate) struct CoreTextRasterizer {
    smoothing: Smoothing,
    profile: SmoothingProfile,
    faces: HashMap<FaceId, Result<Face, RasterError>>,
    sized: HashMap<SizedKey, SizedFont>,
    clock: u64,
    gray: ColorSpace,
    srgb: ColorSpace,
    /// The 8-bit values CoreGraphics stores for each foreground's text and
    /// background grays, read back once.
    levels: [(u8, u8); 5],
    /// Gray context pixels before coverage is recovered from them.
    gray_pixels: Vec<u8>,
}

// SAFETY: every CoreFoundation object here is owned by this rasterizer
// alone (retained once, released in Drop) and never shared, so moving the
// whole rasterizer to another thread moves sole ownership. Fonts,
// descriptors, color spaces, and data are immutable and documented as safe
// to use from any thread; bitmap contexts are created and released inside
// one `rasterize` call. Not `Sync`: `rasterize` takes `&mut self`.
unsafe impl Send for CoreTextRasterizer {}

impl CoreTextRasterizer {
    pub(crate) fn new(profile: SmoothingProfile) -> Self {
        let smoothing = match profile {
            SmoothingProfile::System if system_smoothing_enabled() => Smoothing::Bundle,
            SmoothingProfile::System | SmoothingProfile::Disabled => Smoothing::Disabled,
            SmoothingProfile::Fixed(level) => Smoothing::Fixed(level),
        };
        // SAFETY: plain constructors; each owned result is released once by
        // ColorSpace's Drop.
        let (gray, srgb) = unsafe {
            (
                ColorSpace(CGColorSpaceCreateDeviceGray()),
                ColorSpace(CGColorSpaceCreateWithName(kCGColorSpaceSRGB)),
            )
        };
        let mut rasterizer = Self {
            smoothing,
            profile,
            faces: HashMap::new(),
            sized: HashMap::new(),
            clock: 0,
            gray,
            srgb,
            levels: [(0, 255); 5],
            gray_pixels: Vec::new(),
        };
        for (i, level) in Foreground::ALL.into_iter().enumerate() {
            rasterizer.levels[i] = (
                rasterizer.stored_gray(level.gray()),
                rasterizer.stored_gray(level.background()),
            );
        }
        rasterizer
    }

    /// The profile it was built with.
    pub(crate) fn smoothing_profile(&self) -> SmoothingProfile {
        self.profile
    }

    /// The byte a DeviceGray context stores for `gray`.
    fn stored_gray(&mut self, gray: f64) -> u8 {
        let mut pixel = [0u8; 4];
        // SAFETY: a 1x1 context over `pixel`, released before it goes.
        unsafe {
            let ctx = CGBitmapContextCreate(
                pixel.as_mut_ptr().cast(),
                1,
                1,
                8,
                4,
                self.gray.0,
                ALPHA_NONE,
            );
            if ctx.is_null() {
                return (gray * 255.0).round() as u8;
            }
            CGContextSetGrayFillColor(ctx, gray, 1.0);
            CGContextFillRect(ctx, rect(0.0, 0.0, 1.0, 1.0));
            CGContextRelease(ctx);
        }
        pixel[0]
    }

    /// Draws `request`'s glyph once per [`Foreground`] with smoothing on,
    /// into one union rectangle: what [`SmoothingProfile::System`] admits
    /// on a miss, so changing the text's color never draws again. `None`
    /// for a blank glyph; a color glyph is
    /// [`RasterError::UnsupportedOptions`] here and drawn by `rasterize`,
    /// which never smooths it.
    pub(crate) fn rasterize_bundle(
        &mut self,
        font: &PreparedFont,
        request: &RasterRequest,
        scratch: &mut RasterScratch,
    ) -> Result<Option<CoverageBundle>, RasterError> {
        let glyph = self.prepare(font, request)?;
        let Some(glyph) = glyph else {
            return Ok(None);
        };
        if glyph.color {
            return Err(RasterError::UnsupportedOptions);
        }
        let (w, h) = (glyph.width as usize, glyph.height as usize);
        let plane = w * h;
        if plane * 5 > MAX_BITMAP_BYTES {
            return Err(RasterError::TooLarge);
        }
        scratch.pixels.clear();
        scratch.pixels.resize(plane * 5, 0);
        for (i, level) in Foreground::ALL.into_iter().enumerate() {
            self.draw_mask(
                &glyph,
                Some(level),
                &mut scratch.pixels[i * plane..(i + 1) * plane],
            )?;
        }
        // Trim to the union of every plane's ink, keeping the origin.
        let ink = (0..5)
            .filter_map(|i| ink_bounds(&scratch.pixels[i * plane..(i + 1) * plane], w, h, 1))
            .reduce(|a, b| (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3)));
        let Some((x0, y0, x1, y1)) = ink else {
            return Ok(None);
        };
        let (tw, th) = (x1 - x0, y1 - y0);
        let mut out = 0;
        for i in 0..5 {
            for row in y0..y1 {
                let from = i * plane + row * w + x0;
                scratch.pixels.copy_within(from..from + tw, out);
                out += tw;
            }
        }
        scratch.pixels.truncate(out);
        let tplane = tw * th;
        Ok(Some(CoverageBundle {
            placement: Placement {
                left: glyph.left + x0 as i32,
                top: glyph.top - y0 as i32,
                width: tw as u32,
                height: th as u32,
            },
            stride: tw as u32,
            planes: std::array::from_fn(|i| i * tplane..(i + 1) * tplane),
        }))
    }

    /// The face, sized font, and device rectangle to draw `request` in, or
    /// `None` for a glyph with nothing to draw.
    fn prepare(
        &mut self,
        font: &PreparedFont,
        request: &RasterRequest,
    ) -> Result<Option<GlyphJob>, RasterError> {
        if u32::from(request.glyph()) >= font.glyph_count() {
            return Err(RasterError::InvalidGlyph);
        }
        let face = font.face();
        let face = self
            .faces
            .entry(face)
            .or_insert_with(|| Face::new(font))
            .as_ref()
            .map_err(|e| *e)?;
        let color = match request.options().color {
            ColorGlyphs::Prefer => face.has_color(request.glyph()),
            ColorGlyphs::Never if face.has_color(request.glyph()) => {
                // CoreText draws a glyph's color drawing whenever it has
                // one; swash draws its monochrome outline.
                return Err(RasterError::UnsupportedOptions);
            }
            ColorGlyphs::Never => false,
        };
        let scale = f64::from(request.scale());
        let size = f64::from(request.physical_em()) / scale;
        let ct_font = self.sized_font(font, size)?;
        let glyph = request.glyph();
        let mut bounds = rect(0.0, 0.0, 0.0, 0.0);
        // SAFETY: a live font, one glyph, one output rect.
        unsafe { CTFontGetBoundingRectsForGlyphs(ct_font, 0, &glyph, &mut bounds, 1) };
        if !(bounds.size.width > 0.0 && bounds.size.height > 0.0) {
            return Ok(None);
        }
        let (ox, oy) = request.subpixel().pixels();
        let (ox, oy) = (f64::from(ox), f64::from(oy));
        let synthesis = font.synthesis();
        // Room for antialiasing, smoothing's darkening, and thickening's
        // stroke beyond the outline's bounds.
        let thicken = if synthesis.thicken {
            f64::from(request.physical_em()) / 50.0
        } else {
            0.0
        };
        let pad = 2.0 + thicken.ceil();
        let x0 = (bounds.origin.x * scale + ox - pad).floor();
        let x1 = ((bounds.origin.x + bounds.size.width) * scale + ox + pad).ceil();
        // y is up here; the request's y offset moves the glyph down.
        let y0 = (bounds.origin.y * scale - oy - pad).floor();
        let y1 = ((bounds.origin.y + bounds.size.height) * scale - oy + pad).ceil();
        let (width, height) = (x1 - x0, y1 - y0);
        let bpp = if color { 4.0 } else { 1.0 };
        if !(width.is_finite() && height.is_finite())
            || width * height * bpp > MAX_BITMAP_BYTES as f64
        {
            return Err(RasterError::TooLarge);
        }
        Ok(Some(GlyphJob {
            font: ct_font,
            glyph,
            color,
            scale,
            thicken,
            // Where the glyph origin lands in the bitmap, device pixels from
            // its bottom-left corner.
            origin: (ox - x0, -oy - y0),
            left: x0 as i32,
            top: y1 as i32,
            width: width as u32,
            height: height as u32,
        }))
    }

    /// The font for `font`'s instance at `size` points, created once.
    fn sized_font(&mut self, font: &PreparedFont, size: f64) -> Result<CTFontRef, RasterError> {
        self.clock += 1;
        let key = SizedKey {
            instance: font.instance(),
            size: size.to_bits(),
        };
        if let Some(sized) = self.sized.get_mut(&key) {
            sized.used = self.clock;
            return Ok(sized.font);
        }
        if self.sized.len() >= MAX_SIZED_FONTS {
            let oldest = self
                .sized
                .iter()
                .min_by_key(|(_, s)| s.used)
                .map(|(k, _)| *k);
            if let Some(oldest) = oldest {
                self.sized.remove(&oldest);
            }
        }
        let face = self.faces[&font.face()].as_ref().map_err(|e| *e)?;
        let created = face.sized(font, size)?;
        self.sized.insert(
            key,
            SizedFont {
                font: created,
                used: self.clock,
            },
        );
        Ok(created)
    }

    /// Draws a mask of `job` into `out` (`job.width * job.height` bytes,
    /// top row first): plain coverage for `None`, else smoothed for text of
    /// that gray, recovered against its background.
    fn draw_mask(
        &mut self,
        job: &GlyphJob,
        smoothing: Option<Foreground>,
        out: &mut [u8],
    ) -> Result<(), RasterError> {
        let (w, h) = (job.width as usize, job.height as usize);
        let level = smoothing.unwrap_or(Foreground::BLACK);
        let (f, b) = self.levels[level.0 as usize];
        self.gray_pixels.clear();
        self.gray_pixels.resize(w * h, 0);
        // SAFETY: the context draws into `gray_pixels`, sized for it, and
        // is released before the buffer is read.
        unsafe {
            let ctx = CGBitmapContextCreate(
                self.gray_pixels.as_mut_ptr().cast(),
                w,
                h,
                8,
                w,
                self.gray.0,
                ALPHA_NONE,
            );
            if ctx.is_null() {
                return Err(RasterError::Platform);
            }
            CGContextSetGrayFillColor(ctx, level.background(), 1.0);
            CGContextFillRect(ctx, rect(0.0, 0.0, w as f64, h as f64));
            // Thickening is its own named widening (swash's); smoothing's
            // darkening on top would thicken twice.
            configure(ctx, smoothing.is_some() && job.thicken == 0.0);
            CGContextSetGrayFillColor(ctx, level.gray(), 1.0);
            CGContextSetGrayStrokeColor(ctx, level.gray(), 1.0);
            draw(ctx, job);
            CGContextRelease(ctx);
        }
        let span = i32::from(f) - i32::from(b);
        for (o, &c) in out.iter_mut().zip(&self.gray_pixels) {
            let a = ((i32::from(c) - i32::from(b)) * 255 + span / 2) / span;
            *o = a.clamp(0, 255) as u8;
        }
        Ok(())
    }

    /// Draws a color glyph as premultiplied sRGB RGBA into `scratch`.
    fn draw_color(
        &mut self,
        job: &GlyphJob,
        scratch: &mut RasterScratch,
    ) -> Result<(), RasterError> {
        let (w, h) = (job.width as usize, job.height as usize);
        scratch.pixels.clear();
        scratch.pixels.resize(w * h * 4, 0);
        // SAFETY: as in `draw_mask`, over the scratch's pixels.
        unsafe {
            let ctx = CGBitmapContextCreate(
                scratch.pixels.as_mut_ptr().cast(),
                w,
                h,
                8,
                w * 4,
                self.srgb.0,
                PREMULTIPLIED_RGBA,
            );
            if ctx.is_null() {
                return Err(RasterError::Platform);
            }
            configure(ctx, false);
            // Layers drawn in the text color take black; the shader cannot
            // tint a color glyph.
            CGContextSetRGBFillColor(ctx, 0.0, 0.0, 0.0, 1.0);
            draw(ctx, job);
            CGContextRelease(ctx);
        }
        Ok(())
    }
}

impl Drop for CoreTextRasterizer {
    fn drop(&mut self) {
        // Sized fonts before the faces whose data they draw from.
        self.sized.clear();
        self.faces.clear();
    }
}

impl GlyphRasterizer for CoreTextRasterizer {
    fn profile(&self) -> RasterProfile {
        let smoothing = match self.smoothing {
            Smoothing::Disabled => 0,
            Smoothing::Fixed(level) => 1 + u32::from(level.0),
            Smoothing::Bundle => 6,
        };
        RasterProfile {
            backend: RasterBackend::CoreText,
            revision: REVISION << 4 | smoothing,
        }
    }

    fn capabilities(&self) -> RasterCapabilities {
        RasterCapabilities {
            color_outlines: true,
            color_bitmaps: true,
            variations: true,
            synthesis: true,
        }
    }

    fn rasterize(
        &mut self,
        font: &PreparedFont,
        request: &RasterRequest,
        scratch: &mut RasterScratch,
    ) -> Result<RasterOutcome, RasterError> {
        let Some(job) = self.prepare(font, request)? else {
            return Ok(RasterOutcome::Empty);
        };
        let smoothing = match self.smoothing {
            Smoothing::Disabled => None,
            Smoothing::Fixed(level) => Some(level),
            // Color glyphs are not smoothed; one mask cannot carry every
            // foreground's coverage (`rasterize_bundle` draws them).
            Smoothing::Bundle if !job.color => return Err(RasterError::UnsupportedOptions),
            Smoothing::Bundle => None,
        };
        let (w, h) = (job.width as usize, job.height as usize);
        let (bpp, content) = if job.color {
            self.draw_color(&job, scratch)?;
            let content = BitmapContent::Color {
                alpha: AlphaMode::Premultiplied,
            };
            (4, content)
        } else {
            scratch.pixels.clear();
            scratch.pixels.resize(w * h, 0);
            self.draw_mask(&job, smoothing, &mut scratch.pixels)?;
            (1, BitmapContent::Mask)
        };
        // Alpha is the last byte of a color pixel.
        let channel = if job.color { 3 } else { 0 };
        let Some((x0, y0, x1, y1)) = ink_bounds_channel(&scratch.pixels, w, h, bpp, channel) else {
            if job.color {
                // A glyph with color data CoreText drew nothing of: a
                // format it cannot draw (CBDT, say). Swash may.
                return Err(RasterError::UnsupportedFormat);
            }
            return Ok(RasterOutcome::Empty);
        };
        let (tw, th) = (x1 - x0, y1 - y0);
        let mut out = 0;
        for row in y0..y1 {
            let from = (row * w + x0) * bpp;
            scratch.pixels.copy_within(from..from + tw * bpp, out);
            out += tw * bpp;
        }
        scratch.pixels.truncate(out);
        Ok(RasterOutcome::Bitmap(GlyphBitmap {
            content,
            placement: Placement {
                left: job.left + x0 as i32,
                top: job.top - y0 as i32,
                width: tw as u32,
                height: th as u32,
            },
            stride: (tw * bpp) as u32,
            bytes: 0..out,
        }))
    }
}

/// Whether the user's settings leave font smoothing on: CoreText smooths
/// unless `AppleFontSmoothing` is 0 (System Settings' "Use font smoothing
/// when available" off), for this app or globally. Reads, never writes,
/// the defaults.
pub(crate) fn system_smoothing_enabled() -> bool {
    // SAFETY: a CFString built from a static, a copied preference value
    // checked for its type before reading, and both released.
    unsafe {
        let key = cf_string("AppleFontSmoothing");
        let value = CFPreferencesCopyAppValue(key, kCFPreferencesCurrentApplication);
        CFRelease(key);
        if value.is_null() {
            return true;
        }
        let mut level: i64 = 1;
        if CFGetTypeID(value) == CFNumberGetTypeID() {
            CFNumberGetValue(value, NUMBER_SINT64, (&mut level as *mut i64).cast());
        }
        CFRelease(value);
        level != 0
    }
}

struct GlyphJob {
    font: CTFontRef,
    glyph: u16,
    color: bool,
    scale: f64,
    thicken: f64,
    origin: (f64, f64),
    left: i32,
    top: i32,
    width: u32,
    height: u32,
}

/// Antialiased, fractionally positioned, unquantized drawing; smoothing as
/// asked.
unsafe fn configure(ctx: CGContextRef, smooth: bool) {
    // SAFETY: the caller's live context.
    unsafe {
        CGContextSetAllowsAntialiasing(ctx, true);
        CGContextSetShouldAntialias(ctx, true);
        CGContextSetAllowsFontSmoothing(ctx, true);
        CGContextSetShouldSmoothFonts(ctx, smooth);
        CGContextSetAllowsFontSubpixelPositioning(ctx, true);
        CGContextSetShouldSubpixelPositionFonts(ctx, true);
        CGContextSetAllowsFontSubpixelQuantization(ctx, false);
        CGContextSetShouldSubpixelQuantizeFonts(ctx, false);
        CGContextSetTextMatrix(ctx, IDENTITY);
    }
}

/// Draws `job`'s glyph at its origin, in points under the device scale.
unsafe fn draw(ctx: CGContextRef, job: &GlyphJob) {
    // SAFETY: the caller's live context and the job's live font.
    unsafe {
        CGContextScaleCTM(ctx, job.scale, job.scale);
        if job.thicken > 0.0 {
            // Swash's embolden widens each side by a fiftieth of the em: a
            // stroke twice that wide, centered on the outline, in points.
            CGContextSetTextDrawingMode(ctx, TEXT_FILL_STROKE);
            CGContextSetLineWidth(ctx, 2.0 * job.thicken / job.scale);
            CGContextSetLineJoin(ctx, LINE_JOIN_ROUND);
        }
        let position = CGPoint {
            x: job.origin.0 / job.scale,
            y: job.origin.1 / job.scale,
        };
        CTFontDrawGlyphs(job.font, &job.glyph, &position, 1, ctx);
    }
}

/// The smallest `(x0, y0, x1, y1)` holding every nonzero byte of a
/// one-byte-per-pixel image, or `None` if it is blank.
fn ink_bounds(
    pixels: &[u8],
    w: usize,
    h: usize,
    bpp: usize,
) -> Option<(usize, usize, usize, usize)> {
    ink_bounds_channel(pixels, w, h, bpp, 0)
}

fn ink_bounds_channel(
    pixels: &[u8],
    w: usize,
    h: usize,
    bpp: usize,
    channel: usize,
) -> Option<(usize, usize, usize, usize)> {
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
    for y in 0..h {
        let row = &pixels[y * w * bpp..(y + 1) * w * bpp];
        for x in 0..w {
            if row[x * bpp + channel] != 0 {
                x0 = x0.min(x);
                x1 = x1.max(x + 1);
                y0 = y0.min(y);
                y1 = y1.max(y + 1);
            }
        }
    }
    (x1 > x0).then_some((x0, y0, x1, y1))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct SizedKey {
    instance: FontInstanceId,
    size: u64,
}

struct SizedFont {
    font: CTFontRef,
    used: u64,
}

impl Drop for SizedFont {
    fn drop(&mut self) {
        // SAFETY: created (owned) by `Face::sized`, released once.
        unsafe { CFRelease(self.font) };
    }
}

/// A verified CoreText face for one [`FaceId`], with what it takes to tell
/// color glyphs from the face's bytes.
struct Face {
    descriptor: CTFontDescriptorRef,
    data: Arc<dyn AsRef<[u8]> + Send + Sync>,
    index: u32,
}

impl Drop for Face {
    fn drop(&mut self) {
        // SAFETY: owned since `Face::new`, released once.
        unsafe { CFRelease(self.descriptor) };
    }
}

impl Face {
    /// The face of `font`'s bytes CoreText agrees is the shaped face.
    fn new(font: &PreparedFont) -> Result<Self, RasterError> {
        let data = font.source().shared_data().clone();
        let index = font.index();
        let bytes = (*data).as_ref();
        let shaped =
            skrifa::FontRef::from_index(bytes, index).map_err(|_| RasterError::UnsupportedFont)?;
        let cf_data = shared_cf_data(data.clone());
        // SAFETY: CoreText's documented create calls on a live CFData; the
        // chosen descriptor is retained, everything else released.
        let descriptor = unsafe {
            let candidates: Vec<CTFontDescriptorRef> = if bytes.starts_with(b"ttcf") {
                // One descriptor per member (and named instance) of the
                // collection, in no promised order.
                let array = CTFontManagerCreateFontDescriptorsFromData(cf_data);
                let mut all = Vec::new();
                if !array.is_null() {
                    for i in 0..CFArrayGetCount(array) {
                        let d = CFArrayGetValueAtIndex(array, i);
                        CFRetain(d);
                        all.push(d);
                    }
                    CFRelease(array);
                }
                all
            } else {
                // The plural API lists a variable font's named instances;
                // the singular one gives its default face.
                let d = CTFontManagerCreateFontDescriptorFromData(cf_data);
                if d.is_null() { Vec::new() } else { vec![d] }
            };
            CFRelease(cf_data);
            let mut chosen = None;
            for d in candidates {
                if chosen.is_none() && same_face(d, &shaped, font.glyph_count()) {
                    chosen = Some(d);
                } else {
                    CFRelease(d);
                }
            }
            chosen.ok_or(RasterError::UnsupportedFont)?
        };
        Ok(Self {
            descriptor,
            data,
            index,
        })
    }

    /// `font`'s instance at `size` points: every axis at the value shaping
    /// used, automatic optical size off, slanted for synthetic italic.
    fn sized(&self, font: &PreparedFont, size: f64) -> Result<CTFontRef, RasterError> {
        // SAFETY: CF objects built and released here; the font returned is
        // owned by the caller.
        unsafe {
            let mut keys = Vec::new();
            let mut values = Vec::new();
            let axes = font.variations();
            let (mut tags, mut nums) = (Vec::new(), Vec::new());
            for axis in axes {
                tags.push(cf_number_i64(i64::from(u32::from_be_bytes(axis.tag))));
                nums.push(cf_number_f64(f64::from(axis.value)));
            }
            let variation = CFDictionaryCreate(
                std::ptr::null(),
                tags.as_ptr(),
                nums.as_ptr(),
                tags.len() as isize,
                &kCFTypeDictionaryKeyCallBacks,
                &kCFTypeDictionaryValueCallBacks,
            );
            for v in tags.iter().chain(&nums) {
                CFRelease(*v);
            }
            if !axes.is_empty() {
                keys.push(kCTFontVariationAttribute);
                values.push(variation);
            }
            let none = cf_string("none");
            keys.push(kCTFontOpticalSizeAttribute);
            values.push(none);
            let attributes = CFDictionaryCreate(
                std::ptr::null(),
                keys.as_ptr(),
                values.as_ptr(),
                keys.len() as isize,
                &kCFTypeDictionaryKeyCallBacks,
                &kCFTypeDictionaryValueCallBacks,
            );
            CFRelease(variation);
            CFRelease(none);
            let descriptor = CTFontDescriptorCreateCopyWithAttributes(self.descriptor, attributes);
            CFRelease(attributes);
            let skew = CGAffineTransform {
                a: 1.0,
                b: 0.0,
                c: ITALIC_DEGREES.to_radians().tan(),
                d: 1.0,
                tx: 0.0,
                ty: 0.0,
            };
            let matrix = if font.synthesis().italic {
                &skew as *const _
            } else {
                std::ptr::null()
            };
            let ct_font = CTFontCreateWithFontDescriptor(descriptor, size, matrix);
            CFRelease(descriptor);
            if ct_font.is_null() {
                return Err(RasterError::Platform);
            }
            Ok(ct_font)
        }
    }

    /// Whether `glyph` has a color drawing (COLR layers, or an sbix or
    /// CBDT bitmap) that CoreText would draw instead of its outline.
    fn has_color(&self, glyph: u16) -> bool {
        let Ok(font) = skrifa::FontRef::from_index((*self.data).as_ref(), self.index) else {
            return false;
        };
        let id = GlyphId::new(u32::from(glyph));
        if font.color_glyphs().get(id).is_some() {
            return true;
        }
        font.bitmap_strikes()
            .glyph_for_size(Size::unscaled(), id)
            .is_some_and(|g| !matches!(g.data, skrifa::bitmap::BitmapData::Mask(_)))
    }
}

/// Whether CoreText's face `descriptor` holds the face quark-text shaped:
/// the same glyph count, the same `head`, `maxp`, `hhea`, and `name`
/// tables, and the same glyphs for a few characters. Names alone could
/// match another file.
unsafe fn same_face(
    descriptor: CTFontDescriptorRef,
    shaped: &skrifa::FontRef,
    glyph_count: u32,
) -> bool {
    // SAFETY: a live descriptor; the font and tables made here are released.
    unsafe {
        let font = CTFontCreateWithFontDescriptor(descriptor, 16.0, std::ptr::null());
        if font.is_null() {
            return false;
        }
        let mut same = CTFontGetGlyphCount(font) as u32 == glyph_count;
        for tag in [*b"head", *b"maxp", *b"hhea", *b"name"] {
            if !same {
                break;
            }
            let theirs = CTFontCopyTable(font, u32::from_be_bytes(tag), 0);
            let ours = shaped.table_data(skrifa::Tag::new(&tag));
            same = match (theirs.is_null(), ours) {
                (true, None) => true,
                (false, Some(ours)) => {
                    let len = CFDataGetLength(theirs) as usize;
                    let bytes = std::slice::from_raw_parts(CFDataGetBytePtr(theirs), len);
                    bytes == ours.as_bytes()
                }
                _ => false,
            };
            if !theirs.is_null() {
                CFRelease(theirs);
            }
        }
        let charmap = shaped.charmap();
        for ch in ['A', 'a', '0', 'g'] {
            if !same {
                break;
            }
            let unit = ch as u16;
            let mut glyph = 0u16;
            CTFontGetGlyphsForCharacters(font, &unit, &mut glyph, 1);
            same = charmap.map(ch).map_or(0, |g| g.to_u32()) == u32::from(glyph);
        }
        CFRelease(font);
        same
    }
}

/// A `CFData` over shared font bytes without copying them. The bytes stay
/// alive until CoreText releases the last reference to the data, however
/// long its caches keep it: the allocator that frees the data owns them.
fn shared_cf_data(data: Arc<dyn AsRef<[u8]> + Send + Sync>) -> CFDataRef {
    extern "C" fn release(info: *const c_void) {
        // SAFETY: `info` is the box leaked below, released exactly once
        // when the allocator is destroyed.
        drop(unsafe { Box::from_raw(info as *mut Arc<dyn AsRef<[u8]> + Send + Sync>) });
    }
    extern "C" fn deallocate(_ptr: *mut c_void, _info: *mut c_void) {}
    let bytes = (*data).as_ref();
    let (ptr, len) = (bytes.as_ptr(), bytes.len() as isize);
    let owner = Box::into_raw(Box::new(data));
    let context = CFAllocatorContext {
        version: 0,
        info: owner.cast(),
        retain: None,
        release: Some(release),
        copy_description: None,
        allocate: None,
        reallocate: None,
        deallocate: Some(deallocate),
        preferred_size: None,
    };
    // SAFETY: the allocator holds `owner` until destroyed; the data retains
    // the allocator, which this releases after handing it over.
    unsafe {
        let allocator = CFAllocatorCreate(std::ptr::null(), &context);
        let cf = CFDataCreateWithBytesNoCopy(std::ptr::null(), ptr, len, allocator);
        CFRelease(allocator);
        cf
    }
}

fn rect(x: f64, y: f64, w: f64, h: f64) -> CGRect {
    CGRect {
        origin: CGPoint { x, y },
        size: CGSize {
            width: w,
            height: h,
        },
    }
}

unsafe fn cf_string(s: &str) -> CFStringRef {
    // SAFETY: valid UTF-8 bytes of the given length.
    unsafe { CFStringCreateWithBytes(std::ptr::null(), s.as_ptr(), s.len() as isize, UTF8, 0) }
}

unsafe fn cf_number_i64(v: i64) -> CFTypeRef {
    // SAFETY: a value of the declared type.
    unsafe { CFNumberCreate(std::ptr::null(), NUMBER_SINT64, (&v as *const i64).cast()) }
}

unsafe fn cf_number_f64(v: f64) -> CFTypeRef {
    // SAFETY: a value of the declared type.
    unsafe { CFNumberCreate(std::ptr::null(), NUMBER_FLOAT64, (&v as *const f64).cast()) }
}

/// An owned color space, released on drop.
struct ColorSpace(CGColorSpaceRef);

impl Drop for ColorSpace {
    fn drop(&mut self) {
        // SAFETY: created owned in `CoreTextRasterizer::new`.
        unsafe { CGColorSpaceRelease(self.0) };
    }
}

// Raw bindings: CoreFoundation, CoreGraphics, and CoreText as their C
// headers declare them (CGFloat is f64 on every Mac quark runs on).

type CFTypeRef = *const c_void;
type CFAllocatorRef = *const c_void;
type CFDataRef = *const c_void;
type CFStringRef = *const c_void;
type CFArrayRef = *const c_void;
type CFDictionaryRef = *const c_void;
type CTFontRef = *const c_void;
type CTFontDescriptorRef = *const c_void;
type CGContextRef = *mut c_void;
type CGColorSpaceRef = *const c_void;

#[repr(C)]
#[derive(Clone, Copy)]
struct CGPoint {
    x: f64,
    y: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CGSize {
    width: f64,
    height: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CGRect {
    origin: CGPoint,
    size: CGSize,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CGAffineTransform {
    a: f64,
    b: f64,
    c: f64,
    d: f64,
    tx: f64,
    ty: f64,
}

const IDENTITY: CGAffineTransform = CGAffineTransform {
    a: 1.0,
    b: 0.0,
    c: 0.0,
    d: 1.0,
    tx: 0.0,
    ty: 0.0,
};

#[repr(C)]
struct CFAllocatorContext {
    version: isize,
    info: *mut c_void,
    retain: Option<extern "C" fn(*const c_void) -> *const c_void>,
    release: Option<extern "C" fn(*const c_void)>,
    copy_description: Option<extern "C" fn(*const c_void) -> CFStringRef>,
    allocate: Option<extern "C" fn(isize, usize, *mut c_void) -> *mut c_void>,
    reallocate: Option<extern "C" fn(*mut c_void, isize, usize, *mut c_void) -> *mut c_void>,
    deallocate: Option<extern "C" fn(*mut c_void, *mut c_void)>,
    preferred_size: Option<extern "C" fn(isize, usize, *mut c_void) -> isize>,
}

/// `kCGImageAlphaNone`.
const ALPHA_NONE: u32 = 0;
/// `kCGImageAlphaPremultipliedLast | kCGBitmapByteOrder32Big`: R, G, B, A
/// bytes in memory.
const PREMULTIPLIED_RGBA: u32 = 1 | (4 << 12);
/// `kCGTextFillStroke`.
const TEXT_FILL_STROKE: i32 = 2;
/// `kCGLineJoinRound`.
const LINE_JOIN_ROUND: i32 = 1;
/// `kCFNumberSInt64Type` and `kCFNumberFloat64Type`.
const NUMBER_SINT64: isize = 4;
const NUMBER_FLOAT64: isize = 6;
/// `kCFStringEncodingUTF8`.
const UTF8: u32 = 0x0800_0100;

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFTypeDictionaryKeyCallBacks: c_void;
    static kCFTypeDictionaryValueCallBacks: c_void;
    static kCFPreferencesCurrentApplication: CFStringRef;
    fn CFRetain(cf: CFTypeRef) -> CFTypeRef;
    fn CFRelease(cf: CFTypeRef);
    fn CFGetTypeID(cf: CFTypeRef) -> usize;
    fn CFNumberGetTypeID() -> usize;
    fn CFAllocatorCreate(
        allocator: CFAllocatorRef,
        context: *const CFAllocatorContext,
    ) -> CFAllocatorRef;
    fn CFDataCreateWithBytesNoCopy(
        allocator: CFAllocatorRef,
        bytes: *const u8,
        length: isize,
        deallocator: CFAllocatorRef,
    ) -> CFDataRef;
    fn CFDataGetLength(data: CFDataRef) -> isize;
    fn CFDataGetBytePtr(data: CFDataRef) -> *const u8;
    fn CFNumberCreate(allocator: CFAllocatorRef, kind: isize, value: *const c_void) -> CFTypeRef;
    fn CFNumberGetValue(number: CFTypeRef, kind: isize, value: *mut c_void) -> u8;
    fn CFStringCreateWithBytes(
        allocator: CFAllocatorRef,
        bytes: *const u8,
        length: isize,
        encoding: u32,
        external: u8,
    ) -> CFStringRef;
    fn CFDictionaryCreate(
        allocator: CFAllocatorRef,
        keys: *const CFTypeRef,
        values: *const CFTypeRef,
        count: isize,
        key_callbacks: *const c_void,
        value_callbacks: *const c_void,
    ) -> CFDictionaryRef;
    fn CFArrayGetCount(array: CFArrayRef) -> isize;
    fn CFArrayGetValueAtIndex(array: CFArrayRef, index: isize) -> CFTypeRef;
    fn CFPreferencesCopyAppValue(key: CFStringRef, application: CFStringRef) -> CFTypeRef;
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    static kCGColorSpaceSRGB: CFStringRef;
    fn CGColorSpaceCreateDeviceGray() -> CGColorSpaceRef;
    fn CGColorSpaceCreateWithName(name: CFStringRef) -> CGColorSpaceRef;
    fn CGColorSpaceRelease(space: CGColorSpaceRef);
    fn CGBitmapContextCreate(
        data: *mut c_void,
        width: usize,
        height: usize,
        bits_per_component: usize,
        bytes_per_row: usize,
        space: CGColorSpaceRef,
        bitmap_info: u32,
    ) -> CGContextRef;
    fn CGContextRelease(ctx: CGContextRef);
    fn CGContextSetGrayFillColor(ctx: CGContextRef, gray: f64, alpha: f64);
    fn CGContextSetGrayStrokeColor(ctx: CGContextRef, gray: f64, alpha: f64);
    fn CGContextSetRGBFillColor(ctx: CGContextRef, r: f64, g: f64, b: f64, a: f64);
    fn CGContextFillRect(ctx: CGContextRef, rect: CGRect);
    fn CGContextScaleCTM(ctx: CGContextRef, sx: f64, sy: f64);
    fn CGContextSetAllowsAntialiasing(ctx: CGContextRef, allows: bool);
    fn CGContextSetShouldAntialias(ctx: CGContextRef, should: bool);
    fn CGContextSetAllowsFontSmoothing(ctx: CGContextRef, allows: bool);
    fn CGContextSetShouldSmoothFonts(ctx: CGContextRef, should: bool);
    fn CGContextSetAllowsFontSubpixelPositioning(ctx: CGContextRef, allows: bool);
    fn CGContextSetShouldSubpixelPositionFonts(ctx: CGContextRef, should: bool);
    fn CGContextSetAllowsFontSubpixelQuantization(ctx: CGContextRef, allows: bool);
    fn CGContextSetShouldSubpixelQuantizeFonts(ctx: CGContextRef, should: bool);
    fn CGContextSetTextMatrix(ctx: CGContextRef, t: CGAffineTransform);
    fn CGContextSetTextDrawingMode(ctx: CGContextRef, mode: i32);
    fn CGContextSetLineWidth(ctx: CGContextRef, width: f64);
    fn CGContextSetLineJoin(ctx: CGContextRef, join: i32);
}

#[link(name = "CoreText", kind = "framework")]
unsafe extern "C" {
    static kCTFontVariationAttribute: CFStringRef;
    static kCTFontOpticalSizeAttribute: CFStringRef;
    fn CTFontManagerCreateFontDescriptorFromData(data: CFDataRef) -> CTFontDescriptorRef;
    fn CTFontManagerCreateFontDescriptorsFromData(data: CFDataRef) -> CFArrayRef;
    fn CTFontDescriptorCreateCopyWithAttributes(
        original: CTFontDescriptorRef,
        attributes: CFDictionaryRef,
    ) -> CTFontDescriptorRef;
    fn CTFontCreateWithFontDescriptor(
        descriptor: CTFontDescriptorRef,
        size: f64,
        matrix: *const CGAffineTransform,
    ) -> CTFontRef;
    fn CTFontGetGlyphCount(font: CTFontRef) -> isize;
    fn CTFontGetBoundingRectsForGlyphs(
        font: CTFontRef,
        orientation: u32,
        glyphs: *const u16,
        rects: *mut CGRect,
        count: isize,
    ) -> CGRect;
    fn CTFontDrawGlyphs(
        font: CTFontRef,
        glyphs: *const u16,
        positions: *const CGPoint,
        count: usize,
        ctx: CGContextRef,
    );
    fn CTFontCopyTable(font: CTFontRef, tag: u32, options: u32) -> CFDataRef;
    fn CTFontGetGlyphsForCharacters(
        font: CTFontRef,
        characters: *const u16,
        glyphs: *mut u16,
        count: isize,
    ) -> bool;
}
