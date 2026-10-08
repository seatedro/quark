//! The image a drag out shows under the pointer, validated when built so
//! nothing about it can fail once the platform's drag has begun.

use std::sync::Arc;

/// Pixels to drag with: straight (not premultiplied) sRGB RGBA, row by row
/// from the top, with the display scale they were drawn for and the pixel
/// that sits under the pointer.
#[derive(Clone, PartialEq)]
pub struct DragImage {
    rgba: Arc<[u8]>,
    width: u32,
    height: u32,
    scale: f64,
    hotspot: (u32, u32),
}

/// Why a [`DragImage`] was refused.
#[derive(Debug, Clone, PartialEq)]
pub enum DragImageError {
    /// A side is zero.
    Empty,
    /// A side is over [`DragImage::MAX_SIDE`] pixels.
    TooLarge { width: u32, height: u32 },
    /// The pixels are not `width * height * 4` bytes.
    WrongLength { expected: usize, actual: usize },
    /// The scale is outside [`DragImage::SCALES`].
    Scale(f64),
    /// The hotspot is outside the image.
    HotspotOutside { x: u32, y: u32 },
}

impl std::fmt::Display for DragImageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => f.write_str("the drag image is empty"),
            Self::TooLarge { width, height } => write!(
                f,
                "the drag image is {width}x{height}, over {} pixels a side",
                DragImage::MAX_SIDE
            ),
            Self::WrongLength { expected, actual } => {
                write!(f, "the drag image has {actual} bytes, not {expected}")
            }
            Self::Scale(scale) => write!(f, "the drag image scale {scale} is out of range"),
            Self::HotspotOutside { x, y } => {
                write!(f, "the drag image hotspot ({x}, {y}) is outside it")
            }
        }
    }
}

impl std::error::Error for DragImageError {}

impl std::fmt::Debug for DragImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DragImage")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("scale", &self.scale)
            .field("hotspot", &self.hotspot)
            .finish_non_exhaustive()
    }
}

impl DragImage {
    /// The longest side accepted, in pixels.
    pub const MAX_SIDE: u32 = 1024;
    /// The scales accepted, in pixels per logical point.
    pub const SCALES: std::ops::RangeInclusive<f64> = 0.25..=16.0;

    /// `width` by `height` pixels of straight RGBA, at scale 1, with the
    /// hotspot in the middle.
    pub fn from_rgba(
        width: u32,
        height: u32,
        rgba: impl Into<Arc<[u8]>>,
    ) -> Result<Self, DragImageError> {
        let rgba = rgba.into();
        if width == 0 || height == 0 {
            return Err(DragImageError::Empty);
        }
        if width > Self::MAX_SIDE || height > Self::MAX_SIDE {
            return Err(DragImageError::TooLarge { width, height });
        }
        let expected = width as usize * height as usize * 4;
        if rgba.len() != expected {
            return Err(DragImageError::WrongLength {
                expected,
                actual: rgba.len(),
            });
        }
        Ok(Self {
            rgba,
            width,
            height,
            scale: 1.0,
            hotspot: (width / 2, height / 2),
        })
    }

    /// Drawn for `scale` pixels per logical point: shown `width / scale`
    /// points wide where the platform scales drag images (macOS, Wayland,
    /// which rounds it to a whole number). Windows and X11 show the pixels
    /// as they are.
    pub fn with_scale(mut self, scale: f64) -> Result<Self, DragImageError> {
        if !Self::SCALES.contains(&scale) {
            return Err(DragImageError::Scale(scale));
        }
        self.scale = scale;
        Ok(self)
    }

    /// Put pixel (`x`, `y`) under the pointer; the far edges count, so
    /// (`width`, `height`) hangs the image up and left of the pointer.
    pub fn with_hotspot(mut self, x: u32, y: u32) -> Result<Self, DragImageError> {
        if x > self.width || y > self.height {
            return Err(DragImageError::HotspotOutside { x, y });
        }
        self.hotspot = (x, y);
        Ok(self)
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn scale(&self) -> f64 {
        self.scale
    }

    pub fn hotspot(&self) -> (u32, u32) {
        self.hotspot
    }

    pub fn rgba(&self) -> &[u8] {
        &self.rgba
    }

    /// Premultiplied BGRA bytes: `ARGB8888` in little-endian words, as
    /// Wayland's shared memory buffers and X11's 32-bit visuals take them.
    #[cfg(any(target_os = "linux", test))]
    pub(crate) fn premultiplied_bgra(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.rgba.len());
        for &[r, g, b, a] in self.rgba.as_chunks::<4>().0 {
            out.extend_from_slice(&[mul(b, a), mul(g, a), mul(r, a), a]);
        }
        out
    }

    /// Straight BGRA bytes, as a 32-bit DIB for Windows' drag image helper,
    /// which takes alpha unpremultiplied.
    #[cfg(any(windows, test))]
    pub(crate) fn straight_bgra(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.rgba.len());
        for &[r, g, b, a] in self.rgba.as_chunks::<4>().0 {
            out.extend_from_slice(&[b, g, r, a]);
        }
        out
    }

    /// Opaque BGRX bytes, blended over `background` (RGB), for X11 without
    /// a compositor to blend the alpha.
    #[cfg(any(target_os = "linux", test))]
    pub(crate) fn opaque_bgrx(&self, background: [u8; 3]) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.rgba.len());
        for &[r, g, b, a] in self.rgba.as_chunks::<4>().0 {
            let over = |c: u8, bg: u8| mul(c, a) + mul(bg, 255 - a);
            out.extend_from_slice(&[
                over(b, background[2]),
                over(g, background[1]),
                over(r, background[0]),
                0xff,
            ]);
        }
        out
    }
}

/// `c * a / 255`, rounded.
#[cfg(any(target_os = "linux", test))]
fn mul(c: u8, a: u8) -> u8 {
    let x = u32::from(c) * u32::from(a) + 128;
    ((x + (x >> 8)) >> 8) as u8
}

/// The image Linux drags files with when the app gives none, the same on
/// every desktop: a page with a folded corner, or two stacked for several
/// files, 32 points square at `scale`.
#[cfg(any(target_os = "linux", test))]
pub(crate) fn file_icon(count: usize, scale: f64) -> DragImage {
    const SIDE: f64 = 32.0;
    // Pages are 18 by 24 points with a 6 point fold.
    const PAGE: (f64, f64) = (18.0, 24.0);
    const FOLD: f64 = 6.0;
    const OUTLINE: [u8; 4] = [84, 84, 84, 255];
    const PAPER: [u8; 4] = [250, 250, 250, 255];
    const CREASE: [u8; 4] = [214, 214, 214, 255];
    let scale = scale.clamp(1.0, 4.0);
    let side = (SIDE * scale).round() as u32;
    let pages: &[(f64, f64)] = if count > 1 {
        &[(10.0, 2.0), (5.0, 6.0)]
    } else {
        &[(7.0, 4.0)]
    };
    // Where (x, y), in points, falls on a page at `origin`: outside, on the
    // outline, the fold, or the paper.
    let shade = |x: f64, y: f64, (ox, oy): (f64, f64)| -> Option<[u8; 4]> {
        let (u, v) = (x - ox, y - oy);
        if u < 0.0 || v < 0.0 || u >= PAGE.0 || v >= PAGE.1 {
            return None;
        }
        // Distance in from the corner's diagonal: negative beyond the fold.
        let diagonal = (PAGE.0 - u) + v - FOLD;
        if diagonal < 0.0 {
            return None;
        }
        let fold = u >= PAGE.0 - FOLD && v < FOLD;
        let edge = 1.0;
        if u < edge || v < edge || PAGE.0 - u <= edge || PAGE.1 - v <= edge || diagonal < edge {
            Some(OUTLINE)
        } else if fold {
            // The flap, outlined along the crease.
            let crease = (u - (PAGE.0 - FOLD)).min(FOLD - v);
            Some(if crease < edge { OUTLINE } else { CREASE })
        } else {
            Some(PAPER)
        }
    };
    let mut rgba = Vec::with_capacity((side * side * 4) as usize);
    for py in 0..side {
        for px in 0..side {
            let (x, y) = ((f64::from(px) + 0.5) / scale, (f64::from(py) + 0.5) / scale);
            // The last page is in front.
            let color = pages
                .iter()
                .rev()
                .find_map(|&origin| shade(x, y, origin))
                .unwrap_or([0; 4]);
            rgba.extend_from_slice(&color);
        }
    }
    DragImage::from_rgba(side, side, rgba)
        .and_then(|image| image.with_scale(scale))
        .expect("the file icon is a valid drag image")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drag_images_refuse_pixels_that_do_not_fit_their_size() {
        let build = |width: u32, height: u32, len: usize, scale: f64, hotspot: (u32, u32)| {
            match DragImage::from_rgba(width, height, vec![0u8; len])
                .and_then(|image| image.with_scale(scale))
                .and_then(|image| image.with_hotspot(hotspot.0, hotspot.1))
            {
                Ok(image) => format!(
                    "{}x{} at {} hotspot {:?}",
                    image.width(),
                    image.height(),
                    image.scale(),
                    image.hotspot()
                ),
                Err(error) => error.to_string(),
            }
        };
        let cases = [
            (build(2, 3, 24, 2.0, (2, 3)), "2x3 at 2 hotspot (2, 3)"),
            (build(0, 3, 0, 1.0, (0, 0)), "the drag image is empty"),
            (
                build(1025, 1, 4100, 1.0, (0, 0)),
                "the drag image is 1025x1, over 1024 pixels a side",
            ),
            (
                build(2, 3, 23, 1.0, (0, 0)),
                "the drag image has 23 bytes, not 24",
            ),
            (
                build(2, 3, 24, 0.0, (0, 0)),
                "the drag image scale 0 is out of range",
            ),
            (
                build(2, 3, 24, f64::NAN, (0, 0)),
                "the drag image scale NaN is out of range",
            ),
            (
                build(2, 3, 24, 1.0, (3, 0)),
                "the drag image hotspot (3, 0) is outside it",
            ),
        ];
        for (got, expected) in cases {
            assert_eq!(got, expected);
        }
    }

    #[test]
    fn platform_pixel_formats_swap_channels_and_treat_alpha_as_each_expects() {
        // Opaque red, half transparent white, and fully transparent green.
        let image =
            DragImage::from_rgba(3, 1, vec![255, 0, 0, 255, 255, 255, 255, 128, 0, 255, 0, 0])
                .unwrap();
        assert_eq!(
            image.premultiplied_bgra(),
            [0, 0, 255, 255, 128, 128, 128, 128, 0, 0, 0, 0]
        );
        assert_eq!(
            image.straight_bgra(),
            [0, 0, 255, 255, 255, 255, 255, 128, 0, 255, 0, 0]
        );
        assert_eq!(
            image.opaque_bgrx([0, 0, 0]),
            [0, 0, 255, 255, 128, 128, 128, 255, 0, 0, 0, 255]
        );
    }

    #[test]
    fn the_fallback_file_icon_stacks_a_second_page_for_several_files() {
        let pixel = |image: &DragImage, x: u32, y: u32| {
            let i = ((y * image.width() + x) * 4) as usize;
            image.rgba()[i..i + 4].to_vec()
        };
        let one = file_icon(1, 2.0);
        let two = file_icon(2, 2.0);
        assert_eq!(
            (one.width(), one.scale(), one.hotspot()),
            (64, 2.0, (32, 32))
        );
        let paper = vec![250, 250, 250, 255];
        let clear = vec![0; 4];
        // The middle of the page, a corner of the image, the folded corner
        // cut off the page, and a spot right of the front page where only
        // the page behind shows.
        let probes = [
            ((32, 32), &paper, &paper),
            ((1, 1), &clear, &clear),
            ((49, 8), &clear, &clear),
            ((51, 30), &clear, &paper),
        ];
        for ((x, y), single, stacked) in probes {
            assert_eq!(&pixel(&one, x, y), single, "one file at ({x}, {y})");
            assert_eq!(&pixel(&two, x, y), stacked, "two files at ({x}, {y})");
        }
    }
}
