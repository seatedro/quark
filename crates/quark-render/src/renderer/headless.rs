//! Windowless renderers, which draw into textures of their own and read
//! the pixels back: fixture renders, screenshots, and pixel tests.

use winit::dpi::PhysicalSize;

use super::gpu_context::GpuContext;
use super::surface::Surface;
use super::{RenderError, Renderer, RendererOptions};
use crate::scene::Scene;
use quark_text::TextSystem;

impl Renderer {
    /// Build a windowless renderer that targets `OffscreenTarget`s only, on a
    /// new headless [`GpuContext`]. No swapchain is created.
    pub fn new_headless(width: u32, height: u32, scale_factor: f64) -> Result<Self, RenderError> {
        Ok(Self::headless_with_gpu(
            &GpuContext::headless()?,
            width,
            height,
            scale_factor,
        ))
    }

    /// A windowless renderer on an existing context.
    pub fn headless_with_gpu(gpu: &GpuContext, width: u32, height: u32, scale_factor: f64) -> Self {
        let size = PhysicalSize::new(width.max(1), height.max(1));
        let direct_encoded = gpu
            .inner
            .adapter
            .get_downlevel_capabilities()
            .flags
            .contains(wgpu::DownlevelFlags::VIEW_FORMATS);
        Self::assemble(
            gpu.clone(),
            Surface::Headless { size },
            scale_factor,
            direct_encoded,
        )
    }

    /// [`Self::headless_with_gpu`] drawing as `options` say.
    pub fn headless_with_options(
        gpu: &GpuContext,
        width: u32,
        height: u32,
        scale_factor: f64,
        options: RendererOptions,
    ) -> Self {
        let mut renderer = Self::headless_with_gpu(gpu, width, height, scale_factor);
        renderer.set_options(options);
        renderer
    }

    /// Render `scene` into an offscreen sRGB texture at the given physical
    /// `width`/`height`, read the pixels back, and write them as a PNG to `path`.
    /// `text` must be the system that shaped the scene's text layouts.
    ///
    /// This is a self-contained, no-swapchain draw flow used by the dev/test
    /// "screenshot" leg.
    pub fn render_to_png(
        &mut self,
        scene: &Scene,
        text: &mut TextSystem,
        width: u32,
        height: u32,
        path: &std::path::Path,
    ) -> Result<(), RenderError> {
        let (w, h) = (width.max(1), height.max(1));
        let pixels = self.render_to_rgba(scene, text, w, h)?;
        let buffer = image::RgbaImage::from_raw(w, h, pixels)
            .expect("readback pixel buffer matches dimensions");
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        buffer
            .save(path)
            .map_err(|e| RenderError::PngWrite(e.to_string()))
    }

    /// Render `scene` offscreen and return tightly packed sRGB RGBA8 rows.
    /// Pixel readback honours the wgpu 256-byte `bytes_per_row` alignment by
    /// padding rows on copy and unpadding on read.
    pub fn render_to_rgba(
        &mut self,
        scene: &Scene,
        text: &mut TextSystem,
        width: u32,
        height: u32,
    ) -> Result<Vec<u8>, RenderError> {
        let w = width.max(1);
        let h = height.max(1);
        self.begin_frame(0.0);
        self.flattener.adopt_svg_fonts(text);
        self.flatten(scene, w, h);

        // Owned target texture (COPY_SRC so we can read it back). Format matches
        // the surface format the pipelines were built against.
        let encoded_format = self.gpu.target_format(true);
        let target = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("quark_png_target"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.gpu.target_format(false),
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: if self.options.encoded() && self.direct_encoded {
                std::slice::from_ref(&encoded_format)
            } else {
                &[]
            },
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("quark_png_encoder"),
            });
        self.record_root(&mut encoder, &target, (w, h), text)?;

        let bytes_per_pixel = 4u32;
        let unpadded_bytes_per_row = w * bytes_per_pixel;
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let padded_bytes_per_row = unpadded_bytes_per_row.div_ceil(align) * align;
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("quark_png_readback"),
            size: (padded_bytes_per_row * h) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &target,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_bytes_per_row),
                    rows_per_image: Some(h),
                },
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit(Some(encoder.finish()));

        let slice = readback.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |res| {
            let _ = tx.send(res);
        });
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
        rx.recv()
            .map_err(|_| RenderError::BufferMap)?
            .map_err(|_| RenderError::BufferMap)?;

        let mut pixels = Vec::with_capacity((unpadded_bytes_per_row * h) as usize);
        {
            let data = slice.get_mapped_range();
            for row in 0..h {
                let start = (row * padded_bytes_per_row) as usize;
                let end = start + unpadded_bytes_per_row as usize;
                pixels.extend_from_slice(&data[start..end]);
            }
        }
        readback.unmap();

        self.end_frame();
        Ok(pixels)
    }
}
