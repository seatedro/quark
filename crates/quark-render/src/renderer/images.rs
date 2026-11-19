//! GPU-resident images (icons, avatars, pictures) shared by every renderer
//! on one [`GpuContext`](super::GpuContext), keyed by content hash.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, OnceLock};

use wgpu::util::DeviceExt;

use super::Renderer;
use super::pipelines::texture_bind_group;
use crate::scene::ImagePrimitive;

/// Frames an uploaded image may go undrawn before its texture is dropped.
/// Matches the text buffer cache horizon; re-uploading is cheap because the
/// pixels stay in the primitive (raster images) or the CPU icon cache.
const KEEP_UNUSED_IMAGE_FRAMES: u64 = 240;

/// An uploaded image. The bind group keeps the view alive.
pub(super) struct CachedImage {
    _texture: wgpu::Texture,
    pub(super) bind_group: wgpu::BindGroup,
    last_used_frame: u64,
}

/// GPU-resident images keyed by content hash.
pub(super) type ImageCache = HashMap<u64, CachedImage>;

/// The images of one [`GpuContext`](super::GpuContext), with the frame
/// counter that ages them out.
#[derive(Default)]
pub(super) struct SharedImages {
    pub(super) cache: ImageCache,
    frame: u64,
}

impl SharedImages {
    /// Locks the shared images. A panic while holding them can only leave
    /// stale cache entries, so a poisoned lock is still usable.
    pub(super) fn lock(images: &Mutex<Self>) -> MutexGuard<'_, Self> {
        images
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Start a frame drawing images.
    pub(super) fn begin_frame(&mut self) {
        self.frame += 1;
    }

    /// Drop the images no recent frame drew.
    pub(super) fn trim(&mut self) {
        let frame = self.frame;
        self.cache
            .retain(|_, image| frame - image.last_used_frame <= KEEP_UNUSED_IMAGE_FRAMES);
    }
}

impl Renderer {
    /// Upload `image` to the GPU cache under its key if it is not there yet,
    /// and mark the entry used this frame.
    pub(super) fn ensure_image_uploaded(&mut self, image: &ImagePrimitive) {
        let key = image.cache_key;
        let mut images = SharedImages::lock(&self.images);
        let frame = images.frame;
        if let Some(cached) = images.cache.get_mut(&key) {
            cached.last_used_frame = frame;
            return;
        }
        if key == 0 || image.rgba.is_empty() || image.width == 0 || image.height == 0 {
            return;
        }
        // The primitive's fields are public, so a short buffer or a size past
        // the device limit is caller error that must cost one missing image,
        // not a wgpu panic for the whole app.
        let expected = u64::from(image.width) * u64::from(image.height) * 4;
        let max_side = self.device.limits().max_texture_dimension_2d;
        if image.rgba.len() as u64 != expected || image.width > max_side || image.height > max_side
        {
            // Debug level: this runs every frame the image stays in the scene.
            tracing::debug!(
                "skipping image {key:#x}: {}x{} needs {expected} RGBA bytes (max side {max_side}), got {}",
                image.width,
                image.height,
                image.rgba.len()
            );
            return;
        }
        // Images drawn smaller than their pixels (a large picture in a
        // narrow column, a 2x asset on a 1x screen) sample a mip level
        // instead of skipping texels and aliasing. One drawn 1:1 samples
        // level 0 exactly.
        let pixels = premultiply(&image.rgba);
        let (mip_level_count, data) = mip_chain(&pixels, image.width, image.height);
        let texture = self.device.create_texture_with_data(
            &self.queue,
            &wgpu::TextureDescriptor {
                label: Some("quark_cached_image"),
                size: wgpu::Extent3d {
                    width: image.width,
                    height: image.height,
                    depth_or_array_layers: 1,
                },
                mip_level_count,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &data,
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let gpu = &*self.gpu.inner;
        let bind_group =
            texture_bind_group(&self.device, &gpu.layouts.texture, &view, &gpu.sampler);
        images.cache.insert(
            key,
            CachedImage {
                _texture: texture,
                bind_group,
                last_used_frame: frame,
            },
        );
    }
}

/// sRGB-encoded byte to linear light, for averaging texels as the GPU's
/// sRGB sampling would.
fn srgb_to_linear_table() -> &'static [f32; 256] {
    static TABLE: OnceLock<[f32; 256]> = OnceLock::new();
    TABLE.get_or_init(|| {
        std::array::from_fn(|i| {
            let c = i as f32 / 255.0;
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        })
    })
}

/// Linear light to an sRGB-encoded byte.
pub(super) fn linear_to_srgb_byte(l: f32) -> u8 {
    let c = if l <= 0.003_130_8 {
        l * 12.92
    } else {
        1.055 * l.powf(1.0 / 2.4) - 0.055
    };
    (c.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// Image pixels arrive as straight-alpha sRGB, but every pipeline blends
/// premultiplied. Scale color by alpha in linear light, so the sRGB texture
/// decodes to premultiplied linear values; opaque images pass through.
pub(super) fn premultiply(rgba: &[u8]) -> Cow<'_, [u8]> {
    let pixels = rgba.as_chunks::<4>().0;
    if pixels.iter().all(|px| px[3] == u8::MAX) {
        return Cow::Borrowed(rgba);
    }
    let lut = srgb_to_linear_table();
    let mut out = Vec::with_capacity(rgba.len());
    for px in pixels {
        let alpha = f32::from(px[3]) / 255.0;
        for &c in &px[..3] {
            out.push(linear_to_srgb_byte(lut[c as usize] * alpha));
        }
        out.push(px[3]);
    }
    Cow::Owned(out)
}

/// The number of mip levels of a `width` x `height` image and every
/// level's sRGB RGBA8 pixels, level 0 (`rgba` itself) first. Each level
/// halves the last (rounding down, at least 1) and averages 2x2 blocks in
/// linear light; alpha averages linearly.
fn mip_chain(rgba: &[u8], width: u32, height: u32) -> (u32, Cow<'_, [u8]>) {
    let levels = 32 - width.max(height).max(1).leading_zeros();
    if levels == 1 {
        return (1, Cow::Borrowed(rgba));
    }
    let lut = srgb_to_linear_table();
    let mut data = Vec::with_capacity(rgba.len() / 3 * 4 + 16);
    data.extend_from_slice(rgba);
    let (mut w, mut h) = (width as usize, height as usize);
    let mut start = 0;
    for _ in 1..levels {
        let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
        let next = data.len();
        data.reserve(nw * nh * 4);
        for y in 0..nh {
            let rows = [(2 * y).min(h - 1), (2 * y + 1).min(h - 1)];
            for x in 0..nw {
                let cols = [(2 * x).min(w - 1), (2 * x + 1).min(w - 1)];
                let mut sum = [0.0f32; 4];
                for row in rows {
                    for col in cols {
                        let at = start + (row * w + col) * 4;
                        for (c, total) in sum.iter_mut().enumerate().take(3) {
                            *total += lut[data[at + c] as usize];
                        }
                        sum[3] += f32::from(data[at + 3]);
                    }
                }
                for total in &sum[..3] {
                    data.push(linear_to_srgb_byte(total / 4.0));
                }
                data.push((sum[3] / 4.0).round() as u8);
            }
        }
        start = next;
        (w, h) = (nw, nh);
    }
    (levels, Cow::Owned(data))
}
