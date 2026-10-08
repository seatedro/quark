use super::*;

// ---------------------------------------------------------------------------
// SvgIcon — renders an SVG string as a rasterized image
// ---------------------------------------------------------------------------

pub struct SvgIcon {
    svg: &'static str,
    size: f32,
    color: Option<Color>,
}

/// `size` is a BASE (logical, pre-`ui_scale`) value — pass an `Ico::*` token or a base
/// pixel size. `SvgIcon` multiplies it by `ui_scale` internally so it matches scaled
/// text/spacing. Do NOT pass an already-scaled value like `theme.metrics.ui_small_font_size`
/// (which is post-scale) — that double-scales and renders jumbo. To match `text-sm`,
/// use `Ico::SM`, not `ui_small_font_size`.
pub fn svg_icon(svg: &'static str, size: f32) -> SvgIcon {
    SvgIcon {
        svg,
        size,
        color: None,
    }
}

impl SvgIcon {
    pub fn color(mut self, c: Color) -> Self {
        self.color = Some(c);
        self
    }

    pub fn size(mut self, s: f32) -> Self {
        self.size = s;
        self
    }
}

impl Element for SvgIcon {
    type LayoutState = ();
    type PrepaintState = ();

    fn request_layout(
        &mut self,
        engine: &mut LayoutEngine,
        cx: &mut ElementContext,
    ) -> (LayoutId, ()) {
        let scale = cx.theme.metrics.ui_scale();
        let effective = self.size * scale;
        let id = engine.request_layout(
            taffy::Style {
                size: taffy::Size {
                    width: taffy::Dimension::length(effective),
                    height: taffy::Dimension::length(effective),
                },
                flex_shrink: 0.0,
                ..Default::default()
            },
            &[],
        );
        (id, ())
    }

    fn prepaint(
        &mut self,
        _bounds: Bounds,
        _layout_state: &mut (),
        _engine: &LayoutEngine,
        _cx: &mut ElementContext,
    ) {
    }

    fn paint(
        &mut self,
        bounds: Bounds,
        _layout_state: &mut (),
        _prepaint_state: &mut (),
        _engine: &LayoutEngine,
        scene: &mut Scene,
        cx: &mut ElementContext,
    ) {
        let color = cx
            .icon_color_override()
            .unwrap_or_else(|| self.color.unwrap_or(cx.theme.colors.icon));
        // Rasterize at the window's device pixels, not at points: a bitmap
        // the size of the logical rect is stretched (and blurred) by the
        // scale factor when the scene turns physical.
        let (rect, px_w, px_h) = device_pixel_rect(bounds, cx.scale_factor);
        let px_size = px_w.max(px_h);
        let key = quark_render::icons::cache_key(self.svg, px_size, color);
        let (rgba, w, h) = quark_render::icons::rasterize_svg(self.svg, px_size, color);
        scene.image(quark_render::ImagePrimitive {
            rect,
            width: w,
            height: h,
            rgba,
            cache_key: key,
        });
    }
}

/// `bounds` snapped to whole device pixels at `scale`, as the logical rect
/// that [`quark_render::scene::Primitive::to_physical`] turns back into
/// exactly those pixels, and its size in pixels (at least 1 x 1). A bitmap
/// of that size then draws 1:1, at any scale factor.
fn device_pixel_rect(bounds: Bounds, scale: f32) -> (Bounds, u32, u32) {
    let s = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    let (x0, y0) = ((bounds.x * s).round(), (bounds.y * s).round());
    let w = ((bounds.x + bounds.width) * s).round() - x0;
    let h = ((bounds.y + bounds.height) * s).round() - y0;
    let (w, h) = (w.max(1.0), h.max(1.0));
    let rect = Bounds {
        x: x0 / s,
        y: y0 / s,
        width: w / s,
        height: h / s,
    };
    (rect, w as u32, h as u32)
}

impl IntoAnyElement for SvgIcon {
    fn into_any(self) -> AnyElement {
        element_into_any(self)
    }
}

// ---------------------------------------------------------------------------
// Raster image — pre-decoded RGBA bitmap sized to `size` x `size`.
// ---------------------------------------------------------------------------

pub struct RasterImage {
    rgba: std::sync::Arc<[u8]>,
    src_width: u32,
    src_height: u32,
    cache_key: u64,
    size: f32,
}

pub fn raster_image(
    rgba: std::sync::Arc<[u8]>,
    src_width: u32,
    src_height: u32,
    cache_key: u64,
    size: f32,
) -> RasterImage {
    RasterImage {
        rgba,
        src_width,
        src_height,
        cache_key,
        size,
    }
}

impl Element for RasterImage {
    type LayoutState = ();
    type PrepaintState = ();

    fn request_layout(
        &mut self,
        engine: &mut LayoutEngine,
        cx: &mut ElementContext,
    ) -> (LayoutId, ()) {
        let scale = cx.theme.metrics.ui_scale();
        let effective = self.size * scale;
        let id = engine.request_layout(
            taffy::Style {
                size: taffy::Size {
                    width: taffy::Dimension::length(effective),
                    height: taffy::Dimension::length(effective),
                },
                flex_shrink: 0.0,
                ..Default::default()
            },
            &[],
        );
        (id, ())
    }

    fn prepaint(
        &mut self,
        _bounds: Bounds,
        _layout_state: &mut (),
        _engine: &LayoutEngine,
        _cx: &mut ElementContext,
    ) {
    }

    fn paint(
        &mut self,
        bounds: Bounds,
        _layout_state: &mut (),
        _prepaint_state: &mut (),
        _engine: &LayoutEngine,
        scene: &mut Scene,
        cx: &mut ElementContext,
    ) {
        let (rect, _, _) = device_pixel_rect(bounds, cx.scale_factor);
        scene.image(quark_render::ImagePrimitive {
            rect,
            width: self.src_width,
            height: self.src_height,
            rgba: self.rgba.clone(),
            cache_key: self.cache_key,
        });
    }
}

impl IntoAnyElement for RasterImage {
    fn into_any(self) -> AnyElement {
        element_into_any(self)
    }
}

// ---------------------------------------------------------------------------
// Animated images — GIF, WebP, and APNG decoded off the UI thread
// ---------------------------------------------------------------------------

/// Caps on what decoding one animation may keep in memory. Decoding stops
/// at the first frame past a cap; the frames before it still play.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnimationLimits {
    /// Decoded RGBA bytes across all frames.
    pub max_bytes: usize,
    pub max_frames: usize,
}

impl Default for AnimationLimits {
    fn default() -> Self {
        Self {
            max_bytes: 64 << 20,
            max_frames: 1_000,
        }
    }
}

/// One decoded frame: full-canvas RGBA8 pixels shown for `delay_ms`.
#[derive(Clone)]
pub struct ImageFrame {
    pub rgba: Arc<[u8]>,
    pub delay_ms: u32,
    /// Identifies the frame's pixels to the renderer's texture cache.
    pub cache_key: u64,
}

/// Every frame of an animation, all `width` x `height`.
pub struct ImageFrames {
    pub width: u32,
    pub height: u32,
    frames: Vec<ImageFrame>,
    loop_ms: u64,
    truncated: bool,
}

/// Delays this short are treated as unset, as browsers do: many GIFs say
/// 0 or 10 ms and expect the default.
const MIN_FRAME_DELAY_MS: u32 = 20;
const DEFAULT_FRAME_DELAY_MS: u32 = 100;

impl ImageFrames {
    /// Frames the caller decoded: `(rgba, delay_ms)` pairs, each
    /// `width * height * 4` bytes. Frames of another size are dropped.
    /// `key` distinguishes this animation's textures from other images'.
    pub fn from_rgba(width: u32, height: u32, frames: Vec<(Vec<u8>, u32)>, key: u64) -> Self {
        let mut out = Self {
            width,
            height,
            frames: Vec::with_capacity(frames.len()),
            loop_ms: 0,
            truncated: false,
        };
        let expected = width as usize * height as usize * 4;
        for (rgba, delay_ms) in frames {
            if rgba.len() == expected {
                out.push(rgba, delay_ms, key);
            }
        }
        out
    }

    fn push(&mut self, rgba: Vec<u8>, delay_ms: u32, key: u64) {
        let delay_ms = if delay_ms < MIN_FRAME_DELAY_MS {
            DEFAULT_FRAME_DELAY_MS
        } else {
            delay_ms
        };
        let index = self.frames.len() as u64;
        self.frames.push(ImageFrame {
            rgba: Arc::from(rgba),
            delay_ms,
            // Distinct, nonzero keys per frame of one animation.
            cache_key: (key ^ index.wrapping_mul(0x9e37_79b9_7f4a_7c15)) | 1,
        });
        self.loop_ms += u64::from(delay_ms);
    }

    pub fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    pub fn frame(&self, index: usize) -> Option<&ImageFrame> {
        self.frames.get(index)
    }

    /// True when decoding stopped at an [`AnimationLimits`] cap.
    pub fn truncated(&self) -> bool {
        self.truncated
    }

    /// The frame showing `elapsed_ms` into the (looping) animation, and how
    /// long until the next frame replaces it.
    pub fn frame_at(&self, elapsed_ms: u64) -> (usize, u64) {
        if self.frames.len() < 2 || self.loop_ms == 0 {
            return (0, u64::MAX);
        }
        let mut t = elapsed_ms % self.loop_ms;
        for (index, frame) in self.frames.iter().enumerate() {
            let delay = u64::from(frame.delay_ms);
            if t < delay {
                return (index, delay - t);
            }
            t -= delay;
        }
        (0, u64::from(self.frames[0].delay_ms))
    }
}

/// Decode a GIF, animated WebP, or APNG (any other format the `image`
/// crate reads becomes one frame), stopping at `limits`.
#[cfg(any(feature = "images", test))]
pub fn decode_animation(bytes: &[u8], limits: AnimationLimits) -> Option<ImageFrames> {
    use ::image::AnimationDecoder;
    use ::image::codecs::{gif::GifDecoder, png::PngDecoder, webp::WebPDecoder};
    use std::io::Cursor;

    let key = {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::hash::DefaultHasher::new();
        bytes.hash(&mut hasher);
        hasher.finish()
    };
    let frames: Option<::image::Frames<'_>> = match ::image::guess_format(bytes).ok()? {
        ::image::ImageFormat::Gif => Some(GifDecoder::new(Cursor::new(bytes)).ok()?.into_frames()),
        ::image::ImageFormat::WebP => {
            let decoder = WebPDecoder::new(Cursor::new(bytes)).ok()?;
            decoder.has_animation().then(|| decoder.into_frames())
        }
        ::image::ImageFormat::Png => {
            let decoder = PngDecoder::new(Cursor::new(bytes)).ok()?;
            if decoder.is_apng().ok()? {
                Some(decoder.apng().ok()?.into_frames())
            } else {
                None
            }
        }
        _ => None,
    };
    let Some(frames) = frames else {
        let image = ::image::load_from_memory(bytes).ok()?.into_rgba8();
        let (width, height) = image.dimensions();
        let still = ImageFrames::from_rgba(width, height, vec![(image.into_raw(), 0)], key);
        return (!still.is_empty()
            && still.width as usize * still.height as usize * 4 <= limits.max_bytes)
            .then_some(still);
    };
    let mut out: Option<ImageFrames> = None;
    let mut bytes_kept = 0usize;
    for frame in frames {
        let Ok(frame) = frame else {
            break;
        };
        let (numer, denom) = frame.delay().numer_denom_ms();
        let delay_ms = numer.checked_div(denom).unwrap_or(0);
        let buffer = frame.into_buffer();
        let (width, height) = buffer.dimensions();
        let size = buffer.as_raw().len();
        let out = out.get_or_insert_with(|| ImageFrames {
            width,
            height,
            frames: Vec::new(),
            loop_ms: 0,
            truncated: false,
        });
        if out.frames.len() >= limits.max_frames || bytes_kept + size > limits.max_bytes {
            out.truncated = true;
            break;
        }
        if (width, height) != (out.width, out.height) {
            continue;
        }
        bytes_kept += size;
        out.push(buffer.into_raw(), delay_ms, key);
    }
    out.filter(|frames| !frames.is_empty())
}

enum DecodeState {
    Pending(Option<std::thread::JoinHandle<()>>),
    Ready(Arc<ImageFrames>),
    Failed,
}

struct AnimatedShared {
    state: std::sync::Mutex<DecodeState>,
    /// Clock time of the first painted frame, `u64::MAX` until then.
    started_ms: std::sync::atomic::AtomicU64,
}

/// An animated image shared by the app and the elements that show it.
/// Decoding runs on a worker thread; until it finishes, elements show
/// nothing and check back every [`DECODE_POLL_MS`] while visible.
///
/// Playback follows the frame clock from the first frame the image is
/// painted. An element asks for a repaint only when its next frame is due
/// and it is painted inside its clip, so a hidden or scrolled-away image
/// costs nothing. With reduced motion it shows the first frame only.
#[derive(Clone)]
pub struct AnimatedImage {
    shared: Arc<AnimatedShared>,
}

/// How often a visible element checks on an image still decoding.
pub const DECODE_POLL_MS: u64 = 50;

impl AnimatedImage {
    /// Start decoding `bytes` (GIF, WebP, APNG, or a still image) on a
    /// worker thread.
    #[cfg(any(feature = "images", test))]
    pub fn decode(bytes: Vec<u8>, limits: AnimationLimits) -> Self {
        Self::decode_with(move || decode_animation(&bytes, limits))
    }

    /// Run `decode` (a loader's own decoder, or a fetch then decode) on a
    /// worker thread; `None` marks the image failed.
    pub fn decode_with(decode: impl FnOnce() -> Option<ImageFrames> + Send + 'static) -> Self {
        let image = Self::with_state(DecodeState::Pending(None));
        let shared = Arc::clone(&image.shared);
        let worker = std::thread::Builder::new()
            .name("quark-image-decode".into())
            .spawn(move || {
                let state = match decode().filter(|frames| !frames.is_empty()) {
                    Some(frames) => DecodeState::Ready(Arc::new(frames)),
                    None => DecodeState::Failed,
                };
                *lock(&shared.state) = state;
            });
        match worker {
            Ok(handle) => {
                let mut state = lock(&image.shared.state);
                if matches!(*state, DecodeState::Pending(_)) {
                    *state = DecodeState::Pending(Some(handle));
                }
            }
            Err(_) => *lock(&image.shared.state) = DecodeState::Failed,
        }
        image
    }

    /// An image whose frames are already decoded.
    pub fn from_frames(frames: ImageFrames) -> Self {
        if frames.is_empty() {
            return Self::with_state(DecodeState::Failed);
        }
        Self::with_state(DecodeState::Ready(Arc::new(frames)))
    }

    fn with_state(state: DecodeState) -> Self {
        Self {
            shared: Arc::new(AnimatedShared {
                state: std::sync::Mutex::new(state),
                started_ms: std::sync::atomic::AtomicU64::new(u64::MAX),
            }),
        }
    }

    /// The decoded frames, once ready.
    pub fn frames(&self) -> Option<Arc<ImageFrames>> {
        match &*lock(&self.shared.state) {
            DecodeState::Ready(frames) => Some(Arc::clone(frames)),
            DecodeState::Pending(_) | DecodeState::Failed => None,
        }
    }

    pub fn is_pending(&self) -> bool {
        matches!(*lock(&self.shared.state), DecodeState::Pending(_))
    }

    /// Block until decoding finishes, for callers (and tests) that need the
    /// frames now.
    pub fn wait(&self) {
        let handle = match &mut *lock(&self.shared.state) {
            DecodeState::Pending(handle) => handle.take(),
            _ => None,
        };
        if let Some(handle) = handle {
            let _ = handle.join();
        }
    }

    /// Milliseconds into the animation at `now_ms`, starting the clock on
    /// the first call.
    fn elapsed(&self, now_ms: u64) -> u64 {
        use std::sync::atomic::Ordering;
        let started = match self.shared.started_ms.compare_exchange(
            u64::MAX,
            now_ms,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => now_ms,
            Err(started) => started,
        };
        now_ms.saturating_sub(started)
    }
}

/// A panic while decoding can only leave the state unset, so a poisoned
/// lock is still usable.
fn lock(state: &std::sync::Mutex<DecodeState>) -> std::sync::MutexGuard<'_, DecodeState> {
    state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Shows an [`AnimatedImage`], sized `width` x `height` points (its pixel
/// size until set).
pub struct AnimatedImageElement {
    image: AnimatedImage,
    size: Option<(f32, f32)>,
}

pub fn animated_image(image: &AnimatedImage) -> AnimatedImageElement {
    AnimatedImageElement {
        image: image.clone(),
        size: None,
    }
}

impl AnimatedImageElement {
    pub fn size(mut self, width: f32, height: f32) -> Self {
        self.size = Some((width, height));
        self
    }
}

impl Element for AnimatedImageElement {
    type LayoutState = ();
    /// Whether the image is inside its clip, which only prepaint knows.
    type PrepaintState = bool;

    fn request_layout(
        &mut self,
        engine: &mut LayoutEngine,
        _cx: &mut ElementContext,
    ) -> (LayoutId, ()) {
        let (width, height) = self.size.unwrap_or_else(|| {
            self.image
                .frames()
                .map_or((0.0, 0.0), |f| (f.width as f32, f.height as f32))
        });
        let id = engine.request_layout(
            taffy::Style {
                size: taffy::Size {
                    width: taffy::Dimension::length(width),
                    height: taffy::Dimension::length(height),
                },
                flex_shrink: 0.0,
                ..Default::default()
            },
            &[],
        );
        (id, ())
    }

    fn prepaint(
        &mut self,
        bounds: Bounds,
        _layout_state: &mut (),
        _engine: &LayoutEngine,
        cx: &mut ElementContext,
    ) -> bool {
        bounds.intersection(cx.current_clip()).is_some()
    }

    fn paint(
        &mut self,
        bounds: Bounds,
        _layout_state: &mut (),
        visible: &mut bool,
        _engine: &LayoutEngine,
        scene: &mut Scene,
        cx: &mut ElementContext,
    ) {
        let visible = *visible;
        let Some(frames) = self.image.frames() else {
            if visible && self.image.is_pending() {
                cx.request_frame_at_ms(cx.clock_ms + DECODE_POLL_MS);
            }
            return;
        };
        let index = if cx.theme.reduced_motion {
            0
        } else {
            let (index, next_in) = frames.frame_at(self.image.elapsed(cx.clock_ms));
            if visible && next_in != u64::MAX {
                cx.request_frame_at_ms(cx.clock_ms.saturating_add(next_in));
            }
            index
        };
        let Some(frame) = frames.frame(index) else {
            return;
        };
        scene.image(quark_render::ImagePrimitive {
            rect: bounds,
            width: frames.width,
            height: frames.height,
            rgba: frame.rgba.clone(),
            cache_key: frame.cache_key,
        });
    }
}

impl IntoAnyElement for AnimatedImageElement {
    fn into_any(self) -> AnyElement {
        element_into_any(self)
    }
}

#[cfg(test)]
mod animated_tests {
    use super::*;
    use crate::theme::Theme;
    use quark::reactive::SignalStore;

    const RED: [u8; 4] = [255, 0, 0, 255];
    const BLUE: [u8; 4] = [0, 0, 255, 255];

    /// A 4x4 GIF: red for 100 ms, then blue for 200 ms.
    fn two_frame_gif() -> Vec<u8> {
        use ::image::codecs::gif::{GifEncoder, Repeat};
        use ::image::{Delay, Frame, RgbaImage};
        let mut bytes = Vec::new();
        {
            let mut encoder = GifEncoder::new(&mut bytes);
            encoder.set_repeat(Repeat::Infinite).expect("repeat");
            for (color, ms) in [(RED, 100), (BLUE, 200)] {
                let pixels = RgbaImage::from_pixel(4, 4, ::image::Rgba(color));
                let delay = Delay::from_numer_denom_ms(ms, 1);
                encoder
                    .encode_frame(Frame::from_parts(pixels, 0, 0, delay))
                    .expect("encode frame");
            }
        }
        bytes
    }

    struct Painted {
        /// First pixel of the painted frame, if one was painted.
        pixel: Option<[u8; 4]>,
        next_frame_ms: Option<u64>,
    }

    /// Paint `image` at `now`, inside a 20 px clip scrolled by `scroll`.
    fn paint(image: &AnimatedImage, now: u64, scroll: f32, reduced_motion: bool) -> Painted {
        let mut text = quark_text::TextSystem::vendored_only(&Default::default());
        let mut layouts = quark_text::LayoutCache::default();
        let store = SignalStore::new();
        let mut theme = Theme::default_dark();
        theme.reduced_motion = reduced_motion;
        let mut cx =
            ElementContext::new(&theme, 1.0, &mut text, &mut layouts, None, &store).with_clock(now);
        let mut scene = Scene::default();
        let mut root = div()
            .w(20.0)
            .h(20.0)
            .scroll_y(scroll)
            .child(animated_image(image).size(8.0, 8.0))
            .into_any();
        render_element(&mut root, &mut scene, &mut cx, 100.0, 100.0);
        let pixel = scene.primitives.iter().find_map(|p| match p {
            quark_render::Primitive::Image(image) => {
                image.rgba.get(..4).map(|p| [p[0], p[1], p[2], p[3]])
            }
            _ => None,
        });
        Painted {
            pixel,
            next_frame_ms: cx.next_frame_ms(),
        }
    }

    fn decoded() -> AnimatedImage {
        let image = AnimatedImage::decode(two_frame_gif(), AnimationLimits::default());
        image.wait();
        image
    }

    // Catches an animation that ignores the frame clock or keeps asking
    // for frames while scrolled out of its clip.
    #[test]
    fn gif_advances_with_the_clock_and_stops_requesting_frames_when_hidden() {
        let image = decoded();
        let first = paint(&image, 1_000, 0.0, false);
        assert_eq!(first.pixel, Some(RED));
        assert_eq!(first.next_frame_ms, Some(1_100), "due when red ends");
        let second = paint(&image, 1_150, 0.0, false);
        assert_eq!(second.pixel, Some(BLUE));
        assert_eq!(second.next_frame_ms, Some(1_300));
        assert_eq!(paint(&image, 1_310, 0.0, false).pixel, Some(RED), "loops");

        let hidden = paint(&image, 1_320, 100.0, false);
        assert_eq!(hidden.next_frame_ms, None, "hidden image asked for frames");
    }

    // Reduced motion shows the first frame and never schedules another.
    #[test]
    fn gif_with_reduced_motion_holds_the_first_frame() {
        let image = decoded();
        for now in [0, 150, 400] {
            let painted = paint(&image, now, 0.0, true);
            assert_eq!(painted.pixel, Some(RED), "at {now}");
            assert_eq!(painted.next_frame_ms, None, "at {now}");
        }
    }

    // The byte cap keeps the frames before it: a cap of one 4x4 frame
    // keeps the first frame and marks the animation truncated.
    #[test]
    fn decode_stops_at_the_memory_cap() {
        let limits = AnimationLimits {
            max_bytes: 4 * 4 * 4,
            ..AnimationLimits::default()
        };
        let frames = decode_animation(&two_frame_gif(), limits).expect("decodes");
        assert_eq!((frames.len(), frames.truncated()), (1, true));
    }
}

#[cfg(test)]
mod svg_icon_tests {
    use super::*;
    use crate::theme::Theme;
    use quark::reactive::SignalStore;

    const SQUARE: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><rect width="24" height="24" fill="currentColor"/></svg>"#;

    /// Paint a 16 pt icon `inset` points from the window's top left at
    /// `scale`, and return its bitmap size and where it lands, in pixels.
    fn painted_icon(scale: f32, inset: f32) -> ((u32, u32), quark_render::Rect) {
        let mut text = quark_text::TextSystem::vendored_only(&Default::default());
        let mut layouts = quark_text::LayoutCache::default();
        let store = SignalStore::new();
        let theme = Theme::default_dark();
        let mut cx = ElementContext::new(&theme, scale, &mut text, &mut layouts, None, &store);
        let mut scene = Scene::default();
        let mut root = div()
            .w(100.0)
            .h(100.0)
            .pl(inset)
            .pt(inset)
            .child(svg_icon(SQUARE, 16.0))
            .into_any();
        render_element(&mut root, &mut scene, &mut cx, 100.0, 100.0);
        scene
            .primitives
            .into_iter()
            .find_map(|mut p| {
                p.to_physical(scale);
                match p {
                    quark_render::Primitive::Image(image) => {
                        Some(((image.width, image.height), image.rect))
                    }
                    _ => None,
                }
            })
            .expect("the icon paints an image")
    }

    // Catches icons rasterized at their size in points, which the scale
    // factor then stretches into a blurry bitmap: the bitmap must cover the
    // pixels it lands on one to one.
    #[test]
    fn icon_bitmap_matches_its_device_pixels() {
        for (scale, inset, pixels) in [
            (1.0, 0.0, 16),
            (1.25, 0.0, 20),
            (1.5, 0.0, 24),
            (1.5, 3.3, 24),
            (2.0, 0.0, 32),
            (2.0, 0.25, 32),
        ] {
            let ((w, h), rect) = painted_icon(scale, inset);
            assert_eq!(
                (w, h),
                (pixels, pixels),
                "bitmap at {scale}x, inset {inset}"
            );
            assert_eq!(
                (rect.width, rect.height),
                (pixels as f32, pixels as f32),
                "drawn size at {scale}x, inset {inset}"
            );
            assert_eq!(
                (rect.x.fract(), rect.y.fract()),
                (0.0, 0.0),
                "on the pixel grid at {scale}x, inset {inset}"
            );
        }
    }
}
