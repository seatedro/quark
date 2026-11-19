//! The GPU state every window shares.

use std::sync::{Arc, Mutex, OnceLock};

use super::RenderError;
use super::images::SharedImages;
use super::pipelines::{Layouts, Pipelines};
use crate::text_engine::Cache;

/// The GPU state every window shares: one instance, adapter, device, and
/// queue, the pipelines built for one surface format, the text engine's
/// pipeline cache, and the uploaded-image cache. Cloning is cheap and
/// shares it.
///
/// Each [`Renderer`](super::Renderer) keeps only per-window state on top:
/// its surface, frame buffers, offscreen targets, and glyph atlas. The
/// atlas stays per window because it is trimmed after every frame, which
/// would evict glyphs another window still draws and re-upload them every
/// frame.
#[derive(Clone)]
pub struct GpuContext {
    pub(super) inner: Arc<GpuShared>,
}

pub(super) struct GpuShared {
    pub(super) instance: wgpu::Instance,
    pub(super) adapter: wgpu::Adapter,
    pub(super) device: wgpu::Device,
    pub(super) queue: wgpu::Queue,
    /// Every surface uses this (sRGB) format; `linear` draws to it.
    pub(super) format: wgpu::TextureFormat,
    pub(super) linear: Pipelines,
    /// Pipelines drawing to encoded targets, built on first use.
    encoded: OnceLock<Pipelines>,
    pub(super) layouts: Layouts,
    pub(super) sampler: wgpu::Sampler,
    pub(super) glyph_cache: Cache,
    pub(super) images: Arc<Mutex<SharedImages>>,
}

impl GpuContext {
    /// Context for a window: the adapter is chosen to present to `surface`,
    /// which must come from `instance`.
    pub(super) async fn for_surface(
        instance: wgpu::Instance,
        surface: &wgpu::Surface<'static>,
    ) -> Result<Self, RenderError> {
        let adapter = request_adapter(&instance, Some(surface)).await?;
        let capabilities = surface.get_capabilities(&adapter);
        let format = capabilities
            .formats
            .iter()
            .copied()
            .find(wgpu::TextureFormat::is_srgb)
            .or_else(|| capabilities.formats.first().copied())
            .ok_or(RenderError::IncompatibleSurface)?;
        Self::build(instance, adapter, format, wgpu::Limits::default()).await
    }

    /// Context with no window, rendering into sRGB offscreen targets.
    #[cfg(any(test, feature = "headless-render"))]
    pub fn headless() -> Result<Self, RenderError> {
        Self::headless_with_limits(wgpu::Limits::default())
    }

    /// [`Self::headless`] on a device capped at `limits`. Tests shrink the
    /// texture size limit to fill the glyph atlas with a few glyphs.
    #[cfg(any(test, feature = "headless-render"))]
    pub(super) fn headless_with_limits(limits: wgpu::Limits) -> Result<Self, RenderError> {
        pollster::block_on(async {
            let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::from_env_or_default());
            let adapter = request_adapter(&instance, None).await?;
            // Match the on-screen path: an sRGB target so colors and PNG bytes agree.
            Self::build(
                instance,
                adapter,
                wgpu::TextureFormat::Rgba8UnormSrgb,
                limits,
            )
            .await
        })
    }

    /// True when both handles share one device (and so every GPU resource).
    pub fn same_device(&self, other: &GpuContext) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }

    /// Whether this context's adapter can present to `surface` in the format
    /// its pipelines were built for.
    pub(super) fn supports(&self, surface: &wgpu::Surface<'_>) -> bool {
        let gpu = &*self.inner;
        gpu.adapter.is_surface_supported(surface)
            && surface
                .get_capabilities(&gpu.adapter)
                .formats
                .contains(&gpu.format)
    }

    async fn build(
        instance: wgpu::Instance,
        adapter: wgpu::Adapter,
        surface_format: wgpu::TextureFormat,
        required_limits: wgpu::Limits,
    ) -> Result<Self, RenderError> {
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                required_limits,
                ..wgpu::DeviceDescriptor::default()
            })
            .await?;
        let layouts = Layouts::new(&device);
        // Trilinear: cached images carry mip levels; render targets have one.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("quark_blit_sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let linear = Pipelines::new(&device, &layouts, surface_format);
        let glyph_cache = Cache::new(&device);
        Ok(Self {
            inner: Arc::new(GpuShared {
                instance,
                adapter,
                device,
                queue,
                format: surface_format,
                linear,
                encoded: OnceLock::new(),
                layouts,
                sampler,
                glyph_cache,
                images: Arc::default(),
            }),
        })
    }

    /// The format of a target holding linear light: the shared sRGB
    /// format. One holding encoded values takes its non-sRGB twin, so a
    /// view of the same texture can write either.
    pub(super) fn target_format(&self, encoded: bool) -> wgpu::TextureFormat {
        if encoded {
            self.inner.format.remove_srgb_suffix()
        } else {
            self.inner.format
        }
    }

    /// The pipelines drawing to an encoded or a linear-light target.
    pub(super) fn pipelines(&self, encoded: bool) -> &Pipelines {
        let gpu = &*self.inner;
        if !encoded {
            return &gpu.linear;
        }
        gpu.encoded
            .get_or_init(|| Pipelines::new(&gpu.device, &gpu.layouts, self.target_format(true)))
    }
}

async fn request_adapter(
    instance: &wgpu::Instance,
    compatible_surface: Option<&wgpu::Surface<'static>>,
) -> Result<wgpu::Adapter, RenderError> {
    let preferred = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            compatible_surface,
            ..wgpu::RequestAdapterOptions::default()
        })
        .await;
    match preferred {
        Ok(adapter) => Ok(adapter),
        Err(_) => instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                force_fallback_adapter: true,
                compatible_surface,
                ..wgpu::RequestAdapterOptions::default()
            })
            .await
            .map_err(|_| RenderError::NoAdapter),
    }
}
