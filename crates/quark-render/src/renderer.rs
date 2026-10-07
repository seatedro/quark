use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use bytemuck::{Pod, Zeroable};
use glyphon::{Cache, Resolution, SwashCache, TextAtlas, TextRenderer, Viewport};
use quark_text::TextSystem;
use thiserror::Error;
use wgpu::util::DeviceExt;
use winit::{dpi::PhysicalSize, window::Window};

use crate::path::{Band, push_bands, rule_code, stroke_outline, to_kurbo};
use crate::scene::{
    ClipPrimitive, Primitive, Rect, RichTextPrimitive, Scene, TextPrimitive, Transform2D,
};

use crate::shaders::{
    BLIT_SHADER, BLUR_SHADER, EFFECT_SHADER, LAYER_SHADER, PATH_SHADER, QUAD_SHADER, SHADOW_SHADER,
};
use crate::text::{RecoloredBuffers, color_to_linear, measure_mono_char_width, prepare_text_areas};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextMetrics {
    pub ui_font_size_px: f32,
    pub ui_line_height_px: f32,
    pub mono_font_size_px: f32,
    pub mono_line_height_px: f32,
    pub mono_char_width_px: f32,
}

impl Default for TextMetrics {
    fn default() -> Self {
        Self {
            ui_font_size_px: 14.0,
            ui_line_height_px: 18.0,
            mono_font_size_px: 13.0,
            mono_line_height_px: 24.0,
            mono_char_width_px: 8.0,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FrameStats {
    pub primitive_count: usize,
    pub viewport_width: u32,
    pub viewport_height: u32,
    pub cpu_us: u64,
    pub acquire_us: u64,
    pub present_us: u64,
}

#[derive(Debug, Error)]
pub enum RenderError {
    #[error("no compatible GPU adapter found")]
    NoAdapter,
    #[error("failed to create surface: {0}")]
    CreateSurface(#[from] wgpu::CreateSurfaceError),
    #[error("device request failed: {0}")]
    RequestDevice(#[from] wgpu::RequestDeviceError),
    #[error("failed to prepare text: {0}")]
    PrepareText(#[from] glyphon::PrepareError),
    #[error("failed to render text: {0}")]
    RenderText(#[from] glyphon::RenderError),
    #[error("surface acquisition failed")]
    SurfaceAcquire,
    /// The surface was lost or outdated and has been reconfigured; nothing
    /// was drawn, so draw the frame again.
    #[error("the surface was lost or outdated and has been reconfigured")]
    SurfaceReconfigured,
    /// The next surface texture did not arrive in time; skip this frame.
    #[error("timed out acquiring the next surface texture")]
    SurfaceTimeout,
    /// The GPU ran out of memory for the next frame. Not recoverable.
    #[error("the GPU is out of memory")]
    OutOfMemory,
    #[error("the renderer has no window surface; use the headless render path")]
    NoSurface,
    #[error("no surface format matches the GPU's pipelines")]
    IncompatibleSurface,
    #[error("buffer map failed")]
    BufferMap,
    #[error("png encode/write failed: {0}")]
    PngWrite(String),
}

/// GPU-resident image (icon, avatar). The bind group keeps the view alive.
struct CachedImage {
    _texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
    last_used_frame: u64,
}

/// Frames an uploaded image may go undrawn before its texture is dropped.
/// Matches the text buffer cache horizon; re-uploading is cheap because the
/// pixels stay in the primitive (raster images) or the CPU icon cache.
const KEEP_UNUSED_IMAGE_FRAMES: u64 = 240;

/// GPU-resident images keyed by content hash.
type ImageCache = HashMap<u64, CachedImage>;

// ---------------------------------------------------------------------------
// TexturePool — reusable offscreen render targets
// ---------------------------------------------------------------------------

struct PooledTexture {
    view: wgpu::TextureView,
    /// Lazily created bind group for sampling this texture. The view is
    /// immutable for the entry's lifetime and the layout/sampler never change,
    /// so the bind group is created once and reused across frames. It is
    /// dropped together with the entry when `trim_unused` evicts it.
    bind_group: Option<wgpu::BindGroup>,
    width: u32,
    height: u32,
    /// Shared with the [`OffscreenTarget`] handed out for this texture, so
    /// the entry is in use exactly while that handle is alive. Dropping a
    /// target without releasing it frees the entry instead of pinning the
    /// pool forever.
    lease: Arc<()>,
    last_used_frame: u64,
}

impl PooledTexture {
    fn in_use(&self) -> bool {
        Arc::strong_count(&self.lease) > 1
    }
}

struct TexturePool {
    textures: Vec<PooledTexture>,
    format: wgpu::TextureFormat,
    frame: u64,
}

const KEEP_UNUSED_OFFSCREEN_FRAMES: u64 = 2;

#[derive(Debug)]
struct ReusableBuffer {
    buffer: wgpu::Buffer,
    capacity_bytes: usize,
}

#[derive(Debug, Default)]
struct TransientBufferPool {
    buffers: Vec<ReusableBuffer>,
    next_buffer: usize,
}

impl TransientBufferPool {
    fn begin_frame(&mut self) {
        self.next_buffer = 0;
    }

    fn upload<T: Pod>(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        label: &'static str,
        data: &[T],
    ) -> Option<wgpu::Buffer> {
        if data.is_empty() {
            return None;
        }

        let bytes = bytemuck::cast_slice(data);
        let required_bytes = bytes.len().max(std::mem::size_of::<T>());
        let capacity_bytes = required_bytes.next_power_of_two().max(256);
        let buffer = if let Some(entry) = self.buffers.get_mut(self.next_buffer) {
            if entry.capacity_bytes < required_bytes {
                entry.buffer = create_transient_buffer(device, label, capacity_bytes as u64);
                entry.capacity_bytes = capacity_bytes;
            }
            entry.buffer.clone()
        } else {
            let buffer = create_transient_buffer(device, label, capacity_bytes as u64);
            self.buffers.push(ReusableBuffer {
                buffer: buffer.clone(),
                capacity_bytes,
            });
            buffer
        };
        self.next_buffer += 1;
        queue.write_buffer(&buffer, 0, bytes);
        Some(buffer)
    }
}

fn create_transient_buffer(device: &wgpu::Device, label: &'static str, size: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

/// Handle to an offscreen render target allocated from the pool. The
/// texture returns to the pool when the handle is released or dropped.
pub struct OffscreenTarget {
    lease: Arc<()>,
    pub width: u32,
    pub height: u32,
}

impl TexturePool {
    fn new(format: wgpu::TextureFormat) -> Self {
        Self {
            textures: Vec::new(),
            format,
            frame: 0,
        }
    }

    fn begin_frame(&mut self) {
        self.frame = self.frame.saturating_add(1);
    }

    /// Acquire a texture of at least the given dimensions. It stays out of
    /// the pool until the target is released or dropped.
    fn acquire(&mut self, device: &wgpu::Device, width: u32, height: u32) -> OffscreenTarget {
        let w = width.max(1);
        let h = height.max(1);

        // Look for an existing unused texture that's big enough.
        for entry in &mut self.textures {
            if !entry.in_use() && entry.width >= w && entry.height >= h {
                entry.last_used_frame = self.frame;
                return OffscreenTarget {
                    lease: Arc::clone(&entry.lease),
                    width: entry.width,
                    height: entry.height,
                };
            }
        }

        // Allocate a new texture.
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("quark_offscreen"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let lease = Arc::new(());
        self.textures.push(PooledTexture {
            view,
            bind_group: None,
            width: w,
            height: h,
            lease: Arc::clone(&lease),
            last_used_frame: self.frame,
        });
        OffscreenTarget {
            lease,
            width: w,
            height: h,
        }
    }

    /// The entry `target` leases. A target from another renderer's pool is a
    /// caller bug.
    fn entry_mut(&mut self, target: &OffscreenTarget) -> &mut PooledTexture {
        self.textures
            .iter_mut()
            .find(|entry| Arc::ptr_eq(&entry.lease, &target.lease))
            .expect("offscreen target belongs to this renderer's pool")
    }

    fn view(&self, target: &OffscreenTarget) -> &wgpu::TextureView {
        &self
            .textures
            .iter()
            .find(|entry| Arc::ptr_eq(&entry.lease, &target.lease))
            .expect("offscreen target belongs to this renderer's pool")
            .view
    }

    /// Get the cached bind group for sampling `target`, creating it on first
    /// use. Returns an owned handle (`wgpu::BindGroup` is internally
    /// refcounted) so multiple targets can be bound in the same pass.
    fn bind_group(
        &mut self,
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        sampler: &wgpu::Sampler,
        target: &OffscreenTarget,
    ) -> wgpu::BindGroup {
        let entry = self.entry_mut(target);
        let view = &entry.view;
        entry
            .bind_group
            .get_or_insert_with(|| create_texture_bind_group(device, layout, view, sampler))
            .clone()
    }

    /// Return `target` to the pool, counting this frame as its last use.
    fn release(&mut self, target: OffscreenTarget) {
        let frame = self.frame;
        self.entry_mut(&target).last_used_frame = frame;
    }

    fn trim_unused(&mut self) {
        let frame = self.frame;
        self.textures.retain(|entry| {
            entry.in_use()
                || frame.saturating_sub(entry.last_used_frame) <= KEEP_UNUSED_OFFSCREEN_FRAMES
        });
    }
}

/// GPU images shared by every renderer on one [`GpuContext`], keyed by
/// content hash, with the frame counter that ages them out.
#[derive(Default)]
struct SharedImages {
    cache: ImageCache,
    frame: u64,
}

/// The GPU state every window shares: one instance, adapter, device, and
/// queue, the pipelines built for one surface format, glyphon's pipeline
/// cache, and the uploaded-image cache. Cloning is cheap and shares it.
///
/// Each [`Renderer`] keeps only per-window state on top: its surface, frame
/// buffers, offscreen targets, and glyph atlas. The atlas stays per window
/// because glyphon trims it after every frame, which would evict glyphs
/// another window still draws and re-upload them every frame.
#[derive(Clone)]
pub struct GpuContext {
    inner: Arc<GpuShared>,
}

struct GpuShared {
    instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    /// Every pipeline targets this format, so every surface uses it.
    format: wgpu::TextureFormat,
    quad_pipeline: wgpu::RenderPipeline,
    shadow_pipeline: wgpu::RenderPipeline,
    effect_quad_pipeline: wgpu::RenderPipeline,
    blit_pipeline: wgpu::RenderPipeline,
    blur_pipeline: wgpu::RenderPipeline,
    layer_pipeline: wgpu::RenderPipeline,
    path_pipeline: wgpu::RenderPipeline,
    viewport_bind_group_layout: wgpu::BindGroupLayout,
    texture_bind_group_layout: wgpu::BindGroupLayout,
    segment_bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    glyph_cache: Cache,
    images: Arc<Mutex<SharedImages>>,
}

impl GpuContext {
    /// Context for a window: the adapter is chosen to present to `surface`,
    /// which must come from `instance`.
    async fn for_surface(
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
    fn headless_with_limits(limits: wgpu::Limits) -> Result<Self, RenderError> {
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
    fn supports(&self, surface: &wgpu::Surface<'_>) -> bool {
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
        let viewport_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("quark_viewport_bind_group_layout"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("quark_quad_shader"),
            source: wgpu::ShaderSource::Wgsl(QUAD_SHADER.into()),
        });
        let quad_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("quark_quad_pipeline_layout"),
            bind_group_layouts: &[&viewport_bind_group_layout],
            immediate_size: 0,
        });
        let quad_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("quark_quad_pipeline"),
            layout: Some(&quad_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_quad"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[QuadInstance::layout()],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_quad"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                strip_index_format: None,
                ..wgpu::PrimitiveState::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let shadow_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("quark_shadow_shader"),
            source: wgpu::ShaderSource::Wgsl(SHADOW_SHADER.into()),
        });
        let shadow_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("quark_shadow_pipeline_layout"),
                bind_group_layouts: &[&viewport_bind_group_layout],
                immediate_size: 0,
            });
        let shadow_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("quark_shadow_pipeline"),
            layout: Some(&shadow_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shadow_shader,
                entry_point: Some("vs_shadow"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[ShadowInstance::layout()],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shadow_shader,
                entry_point: Some("fs_shadow"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                strip_index_format: None,
                ..wgpu::PrimitiveState::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let effect_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("quark_effect_shader"),
            source: wgpu::ShaderSource::Wgsl(EFFECT_SHADER.into()),
        });
        let effect_quad_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("quark_effect_quad_pipeline_layout"),
                bind_group_layouts: &[&viewport_bind_group_layout],
                immediate_size: 0,
            });
        let effect_quad_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("quark_effect_quad_pipeline"),
            layout: Some(&effect_quad_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &effect_shader,
                entry_point: Some("vs_effect"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[EffectQuadInstance::layout()],
            },
            fragment: Some(wgpu::FragmentState {
                module: &effect_shader,
                entry_point: Some("fs_effect"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                strip_index_format: None,
                ..wgpu::PrimitiveState::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        // --- Blit pipeline (composites offscreen textures back to screen) ---

        let texture_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("quark_texture_bind_group_layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            multisampled: false,
                            view_dimension: wgpu::TextureViewDimension::D2,
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("quark_blit_sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let blit_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("quark_blit_shader"),
            source: wgpu::ShaderSource::Wgsl(BLIT_SHADER.into()),
        });
        let blit_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("quark_blit_pipeline_layout"),
            bind_group_layouts: &[&viewport_bind_group_layout, &texture_bind_group_layout],
            immediate_size: 0,
        });
        let blit_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("quark_blit_pipeline"),
            layout: Some(&blit_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &blit_shader,
                entry_point: Some("vs_blit"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[BlitInstance::layout()],
            },
            fragment: Some(wgpu::FragmentState {
                module: &blit_shader,
                entry_point: Some("fs_blit"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                strip_index_format: None,
                ..wgpu::PrimitiveState::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let blur_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("quark_blur_shader"),
            source: wgpu::ShaderSource::Wgsl(BLUR_SHADER.into()),
        });
        let blur_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("quark_blur_pipeline_layout"),
            bind_group_layouts: &[&viewport_bind_group_layout, &texture_bind_group_layout],
            immediate_size: 0,
        });
        let blur_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("quark_blur_pipeline"),
            layout: Some(&blur_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &blur_shader,
                entry_point: Some("vs_blur"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[BlurInstance::layout()],
            },
            fragment: Some(wgpu::FragmentState {
                module: &blur_shader,
                entry_point: Some("fs_blur"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: None, // blur fully overwrites
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                strip_index_format: None,
                ..wgpu::PrimitiveState::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let layer_pipeline = instanced_pipeline(
            &device,
            "quark_layer",
            LAYER_SHADER,
            ("vs_layer", "fs_layer"),
            &[&viewport_bind_group_layout, &texture_bind_group_layout],
            LayerInstance::layout(),
            surface_format,
        );
        let segment_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("quark_segment_bind_group_layout"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    },
                    count: None,
                }],
            });
        let path_pipeline = instanced_pipeline(
            &device,
            "quark_path",
            PATH_SHADER,
            ("vs_path", "fs_path"),
            &[&viewport_bind_group_layout, &segment_bind_group_layout],
            PathInstance::layout(),
            surface_format,
        );

        let glyph_cache = Cache::new(&device);
        Ok(Self {
            inner: Arc::new(GpuShared {
                instance,
                adapter,
                device,
                queue,
                format: surface_format,
                quad_pipeline,
                shadow_pipeline,
                effect_quad_pipeline,
                blit_pipeline,
                blur_pipeline,
                layer_pipeline,
                path_pipeline,
                viewport_bind_group_layout,
                texture_bind_group_layout,
                segment_bind_group_layout,
                sampler,
                glyph_cache,
                images: Arc::default(),
            }),
        })
    }
}

/// An instanced triangle-strip pipeline drawing premultiplied color.
fn instanced_pipeline(
    device: &wgpu::Device,
    label: &'static str,
    source: &'static str,
    (vertex, fragment): (&'static str, &'static str),
    bind_group_layouts: &[&wgpu::BindGroupLayout],
    instance: wgpu::VertexBufferLayout<'static>,
    format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts,
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some(vertex),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[instance],
        },
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some(fragment),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleStrip,
            strip_index_format: None,
            ..wgpu::PrimitiveState::default()
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
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

/// Locks the shared image cache. A panic while holding it can only leave
/// stale cache entries, so a poisoned lock is still usable.
fn lock_images(images: &Mutex<SharedImages>) -> MutexGuard<'_, SharedImages> {
    images
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Draws scenes into one window's surface (or offscreen targets) using a
/// [`GpuContext`] that may be shared with other windows.
pub struct Renderer {
    gpu: GpuContext,
    // Handles cloned out of `gpu` (wgpu handles are reference counted) so
    // the drawing code reads them as plain fields.
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: Option<wgpu::Surface<'static>>,
    surface_config: wgpu::SurfaceConfiguration,
    size: PhysicalSize<u32>,
    scale_factor: f64,
    quad_pipeline: wgpu::RenderPipeline,
    shadow_pipeline: wgpu::RenderPipeline,
    effect_quad_pipeline: wgpu::RenderPipeline,
    blit_pipeline: wgpu::RenderPipeline,
    blur_pipeline: wgpu::RenderPipeline,
    layer_pipeline: wgpu::RenderPipeline,
    path_pipeline: wgpu::RenderPipeline,
    texture_bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    texture_pool: TexturePool,
    instance_buffer_pool: TransientBufferPool,
    images: Arc<Mutex<SharedImages>>,
    viewport_buffer: wgpu::Buffer,
    viewport_bind_group: wgpu::BindGroup,
    swash_cache: SwashCache,
    viewport: Viewport,
    atlas: TextAtlas,
    /// One text renderer per text segment of the frame.
    text_renderers: Vec<TextRenderer>,
    /// False when this frame's glyphs did not fit the atlas; its text
    /// segments are skipped.
    text_ready: bool,
    recolored: RecoloredBuffers,
    /// `(font size, TextSystem generation, width)` of the last measurement.
    cached_mono_char_width: Option<(f32, u64, f32)>,
    flattener: Flattener,
    /// The window's target, then each offscreen layer's by target number.
    frames: Vec<TargetFrame>,
    /// Targets in use this frame.
    active_frames: usize,
    plans: Vec<LayerPlan>,
    /// Viewport uniform and glyphon viewport of layer target `i + 1`.
    layer_uniforms: Vec<(wgpu::Buffer, wgpu::BindGroup)>,
    layer_viewports: Vec<Viewport>,
    segment_texture: Option<SegmentTexture>,
    /// Effect animation time of the frame being drawn, in seconds.
    time: f32,
}

impl Renderer {
    /// A renderer for `window` with a new [`GpuContext`] chosen for it.
    pub fn new(window: Arc<Window>) -> Result<Self, RenderError> {
        pollster::block_on(async {
            let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::from_env_or_default());
            let surface = instance.create_surface(window.clone())?;
            let gpu = GpuContext::for_surface(instance, &surface).await?;
            Self::for_surface(gpu, surface, &window)
        })
    }

    /// A renderer for another window on an existing context.
    ///
    /// The context's adapter was picked for the first window's surface. If it
    /// cannot present to this window's surface (another GPU's display, or no
    /// surface format matching the shared pipelines), the window gets a new
    /// context chosen for its own surface instead; that window then shares
    /// nothing with the others. Compare with [`GpuContext::same_device`].
    pub fn with_gpu(gpu: &GpuContext, window: Arc<Window>) -> Result<Self, RenderError> {
        let surface = gpu.inner.instance.create_surface(window.clone())?;
        if gpu.supports(&surface) {
            return Self::for_surface(gpu.clone(), surface, &window);
        }
        tracing::warn!("the shared GPU adapter cannot present to this window; using a new device");
        pollster::block_on(async {
            let fresh = GpuContext::for_surface(gpu.inner.instance.clone(), &surface).await?;
            Self::for_surface(fresh, surface, &window)
        })
    }

    /// The context this renderer draws with; pass it to [`Self::with_gpu`]
    /// to share it with another window.
    pub fn gpu(&self) -> &GpuContext {
        &self.gpu
    }

    fn for_surface(
        gpu: GpuContext,
        surface: wgpu::Surface<'static>,
        window: &Window,
    ) -> Result<Self, RenderError> {
        let size = window.inner_size();
        let shared = &*gpu.inner;
        let surface_config = surface
            .get_default_config(&shared.adapter, size.width.max(1), size.height.max(1))
            .map(|config| wgpu::SurfaceConfiguration {
                format: shared.format,
                ..config
            })
            .unwrap_or(wgpu::SurfaceConfiguration {
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                format: shared.format,
                width: size.width.max(1),
                height: size.height.max(1),
                desired_maximum_frame_latency: 2,
                present_mode: wgpu::PresentMode::Fifo,
                alpha_mode: wgpu::CompositeAlphaMode::Opaque,
                view_formats: vec![],
            });
        surface.configure(&shared.device, &surface_config);
        Ok(Self::assemble(
            gpu,
            surface_config,
            size,
            window.scale_factor(),
            Some(surface),
        ))
    }

    /// Build a windowless renderer that targets `OffscreenTarget`s only, on a
    /// new headless [`GpuContext`]. No swapchain is created.
    #[cfg(any(test, feature = "headless-render"))]
    pub fn new_headless(width: u32, height: u32, scale_factor: f64) -> Result<Self, RenderError> {
        Ok(Self::headless_with_gpu(
            &GpuContext::headless()?,
            width,
            height,
            scale_factor,
        ))
    }

    /// A windowless renderer on an existing context.
    #[cfg(any(test, feature = "headless-render"))]
    pub fn headless_with_gpu(gpu: &GpuContext, width: u32, height: u32, scale_factor: f64) -> Self {
        let size = PhysicalSize::new(width.max(1), height.max(1));
        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: gpu.inner.format,
            width: size.width,
            height: size.height,
            desired_maximum_frame_latency: 2,
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: wgpu::CompositeAlphaMode::Opaque,
            view_formats: vec![],
        };
        Self::assemble(gpu.clone(), surface_config, size, scale_factor, None)
    }

    /// Per-window state on top of the shared context.
    fn assemble(
        gpu: GpuContext,
        surface_config: wgpu::SurfaceConfiguration,
        size: PhysicalSize<u32>,
        scale_factor: f64,
        surface: Option<wgpu::Surface<'static>>,
    ) -> Self {
        let shared = &*gpu.inner;
        let device = shared.device.clone();
        let queue = shared.queue.clone();
        let viewport_uniform = ViewportUniform::new(surface_config.width, surface_config.height);
        let viewport_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("quark_viewport_uniform"),
            contents: bytemuck::bytes_of(&viewport_uniform),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let viewport_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("quark_viewport_bind_group"),
            layout: &shared.viewport_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: viewport_buffer.as_entire_binding(),
            }],
        });

        let texture_pool = TexturePool::new(shared.format);
        let swash_cache = SwashCache::new();
        let viewport = Viewport::new(&device, &shared.glyph_cache);
        let mut atlas = TextAtlas::new(&device, &queue, &shared.glyph_cache, shared.format);
        let text_renderer =
            TextRenderer::new(&mut atlas, &device, wgpu::MultisampleState::default(), None);

        Self {
            device,
            queue,
            surface,
            surface_config,
            size,
            scale_factor,
            quad_pipeline: shared.quad_pipeline.clone(),
            shadow_pipeline: shared.shadow_pipeline.clone(),
            effect_quad_pipeline: shared.effect_quad_pipeline.clone(),
            blit_pipeline: shared.blit_pipeline.clone(),
            blur_pipeline: shared.blur_pipeline.clone(),
            layer_pipeline: shared.layer_pipeline.clone(),
            path_pipeline: shared.path_pipeline.clone(),
            texture_bind_group_layout: shared.texture_bind_group_layout.clone(),
            sampler: shared.sampler.clone(),
            texture_pool,
            instance_buffer_pool: TransientBufferPool::default(),
            images: Arc::clone(&shared.images),
            viewport_buffer,
            viewport_bind_group,
            swash_cache,
            viewport,
            atlas,
            text_renderers: vec![text_renderer],
            text_ready: true,
            recolored: RecoloredBuffers::default(),
            cached_mono_char_width: None,
            flattener: Flattener::default(),
            frames: Vec::new(),
            active_frames: 0,
            plans: Vec::new(),
            layer_uniforms: Vec::new(),
            layer_viewports: Vec::new(),
            segment_texture: None,
            time: 0.0,
            gpu,
        }
    }

    /// Adopt a new window size. A zero dimension (a minimized window) leaves
    /// the surface configured at its last size and makes [`Self::render`] a
    /// no-op until a real size arrives.
    pub fn resize(&mut self, width: u32, height: u32, scale_factor: f64) {
        self.size = PhysicalSize::new(width, height);
        self.scale_factor = scale_factor;
        if width == 0 || height == 0 {
            return;
        }
        self.surface_config.width = width;
        self.surface_config.height = height;
        if let Some(surface) = &self.surface {
            surface.configure(&self.device, &self.surface_config);
        }
        self.queue.write_buffer(
            &self.viewport_buffer,
            0,
            bytemuck::bytes_of(&ViewportUniform::new(width, height)),
        );
    }

    pub fn scale_factor(&self) -> f64 {
        self.scale_factor
    }

    pub fn text_metrics(&mut self, text: &mut TextSystem) -> TextMetrics {
        let scale = self.scale_factor as f32;
        let mono_font_size = 13.0 * scale;
        let generation = text.generation();
        let char_w = match self.cached_mono_char_width {
            Some((cached_size, cached_generation, cached_w))
                if (cached_size - mono_font_size).abs() < 0.001
                    && cached_generation == generation =>
            {
                cached_w
            }
            _ => {
                let w = measure_mono_char_width(text, mono_font_size);
                self.cached_mono_char_width = Some((mono_font_size, generation, w));
                w
            }
        };
        TextMetrics {
            ui_font_size_px: 14.0 * scale,
            ui_line_height_px: 18.0 * scale,
            mono_font_size_px: mono_font_size,
            mono_line_height_px: 24.0 * scale,
            mono_char_width_px: char_w,
        }
    }

    // -- Offscreen render target management --

    /// Acquire an offscreen texture from the pool. The returned target can be
    /// used as a render attachment and later sampled via `create_texture_bind_group`.
    pub fn acquire_offscreen(&mut self, width: u32, height: u32) -> OffscreenTarget {
        self.texture_pool.acquire(&self.device, width, height)
    }

    /// Get the texture view for an offscreen target (for use as a render attachment).
    pub fn offscreen_view(&self, target: &OffscreenTarget) -> &wgpu::TextureView {
        self.texture_pool.view(target)
    }

    /// Create a bind group for sampling an offscreen target in a shader.
    pub fn create_texture_bind_group(&self, target: &OffscreenTarget) -> wgpu::BindGroup {
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("quark_offscreen_bind_group"),
            layout: &self.texture_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(self.texture_pool.view(target)),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        })
    }

    /// Return an offscreen target to the pool for reuse. Dropping the target
    /// also returns it.
    pub fn release_offscreen(&mut self, target: OffscreenTarget) {
        self.texture_pool.release(target);
    }

    /// Render `scene` into an offscreen sRGB texture at the given physical
    /// `width`/`height`, read the pixels back, and write them as a PNG to `path`.
    /// `text` must be the system that shaped the scene's text layouts.
    ///
    /// This is a self-contained, no-swapchain draw flow used by the dev/test
    /// "screenshot" leg.
    #[cfg(any(test, feature = "headless-render"))]
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
    #[cfg(any(test, feature = "headless-render"))]
    pub fn render_to_rgba(
        &mut self,
        scene: &Scene,
        text: &mut TextSystem,
        width: u32,
        height: u32,
    ) -> Result<Vec<u8>, RenderError> {
        let w = width.max(1);
        let h = height.max(1);

        self.texture_pool.begin_frame();
        self.instance_buffer_pool.begin_frame();
        self.time = 0.0;
        self.queue.write_buffer(
            &self.viewport_buffer,
            0,
            bytemuck::bytes_of(&ViewportUniform::new(w, h)),
        );
        self.viewport.update(
            &self.queue,
            Resolution {
                width: w,
                height: h,
            },
        );
        self.flatten(scene, w, h);

        // Owned target texture (COPY_SRC so we can read it back). Format matches
        // the surface format the pipelines were built against.
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
            format: self.texture_pool.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("quark_png_encoder"),
            });
        self.record_frame(&mut encoder, &view, text)?;

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

        self.atlas.trim();
        self.texture_pool.trim_unused();
        Ok(pixels)
    }

    /// `text` must be the system that shaped the scene's text layouts.
    pub fn render(
        &mut self,
        scene: &Scene,
        text: &mut TextSystem,
        time_seconds: f32,
    ) -> Result<FrameStats, RenderError> {
        // A minimized window reports a zero size while `surface_config` keeps
        // the last real one. Acquiring then fails as outdated on some
        // platforms, and reconfiguring and asking for another redraw would
        // spin the event loop for as long as the window stays minimized.
        if self.size.width == 0 || self.size.height == 0 {
            return Ok(FrameStats::default());
        }
        let render_started_at = Instant::now();
        self.texture_pool.begin_frame();
        self.instance_buffer_pool.begin_frame();
        let sw = self.surface_config.width;
        let sh = self.surface_config.height;

        // Update time in the viewport uniform buffer.
        self.time = time_seconds;
        let viewport_uniform = ViewportUniform {
            resolution: [sw as f32, sh as f32],
            time: time_seconds,
            _padding: 0.0,
        };
        self.queue.write_buffer(
            &self.viewport_buffer,
            0,
            bytemuck::bytes_of(&viewport_uniform),
        );

        self.flatten(scene, sw, sh);

        let surface = self.surface.as_ref().ok_or(RenderError::NoSurface)?;
        let acquire_started_at = Instant::now();
        let frame = match surface.get_current_texture() {
            Ok(frame) => frame,
            Err(wgpu::SurfaceError::Outdated | wgpu::SurfaceError::Lost) => {
                surface.configure(&self.device, &self.surface_config);
                return Err(RenderError::SurfaceReconfigured);
            }
            Err(wgpu::SurfaceError::Timeout) => return Err(RenderError::SurfaceTimeout),
            Err(wgpu::SurfaceError::OutOfMemory) => return Err(RenderError::OutOfMemory),
            Err(wgpu::SurfaceError::Other) => return Err(RenderError::SurfaceAcquire),
        };
        let acquire_us = acquire_started_at.elapsed().as_micros() as u64;

        self.viewport.update(
            &self.queue,
            Resolution {
                width: sw,
                height: sh,
            },
        );

        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("quark_frame_encoder"),
            });
        self.record_frame(&mut encoder, &view, text)?;

        let present_started_at = Instant::now();
        self.queue.submit(Some(encoder.finish()));
        frame.present();
        let present_us = present_started_at.elapsed().as_micros() as u64;
        self.atlas.trim();
        self.texture_pool.trim_unused();

        let cpu_us = (render_started_at.elapsed().as_micros() as u64)
            .saturating_sub(acquire_us)
            .saturating_sub(present_us);
        Ok(FrameStats {
            primitive_count: scene.len(),
            viewport_width: sw,
            viewport_height: sh,
            cpu_us,
            acquire_us,
            present_us,
        })
    }

    /// Plan the frame's layers, acquire their textures, and flatten the
    /// window and every offscreen layer into their targets.
    fn flatten(&mut self, scene: &Scene, width: u32, height: u32) {
        let viewport = Rect {
            x: 0.0,
            y: 0.0,
            width: width as f32,
            height: height as f32,
        };
        let offscreen = plan_layers(scene, viewport, &mut self.plans);
        self.active_frames = 1 + offscreen;
        if self.frames.len() < self.active_frames {
            self.frames
                .resize_with(self.active_frames, TargetFrame::default);
        }
        self.frames[0].width = width;
        self.frames[0].height = height;
        // Textures first, so composites know their texture sizes.
        for plan in &mut self.plans {
            if plan.mode != LayerMode::Offscreen {
                continue;
            }
            let texture = self
                .texture_pool
                .acquire(&self.device, plan.size.0, plan.size.1);
            plan.texture = (texture.width, texture.height);
            let frame = &mut self.frames[plan.target];
            (frame.width, frame.height) = plan.size;
            frame.layer = Some(texture);
        }

        let images = lock_images(&self.images);
        self.flattener.segments.clear();
        let prims = &scene.primitives;
        flatten_scene_into(
            prims,
            0..prims.len(),
            (0.0, 0.0),
            viewport,
            &self.plans,
            &images.cache,
            &mut self.flattener,
            &mut self.frames[0].flat,
        );
        for plan in &self.plans {
            if plan.mode != LayerMode::Offscreen {
                continue;
            }
            let frame = &mut self.frames[plan.target];
            flatten_scene_into(
                prims,
                plan.start + 1..plan.end,
                plan.origin,
                Rect {
                    x: 0.0,
                    y: 0.0,
                    width: frame.width as f32,
                    height: frame.height as f32,
                },
                &self.plans,
                &images.cache,
                &mut self.flattener,
                &mut frame.flat,
            );
        }
    }

    /// Upload, prepare, and encode the flattened frame into `encoder`,
    /// targeting `target`. Everything goes into one encoder: each text segment
    /// has its own `TextRenderer`, so preparing one cannot overwrite the
    /// vertices another segment's pass reads.
    fn record_frame(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        text: &mut TextSystem,
    ) -> Result<(), RenderError> {
        let mut frames = std::mem::take(&mut self.frames);
        let active = self.active_frames.min(frames.len());
        let result = self.record_targets(encoder, target, &mut frames[..active], text);
        // The commands are recorded; later frames may reuse the textures.
        for frame in &mut frames {
            if let Some(texture) = frame.layer.take() {
                self.texture_pool.release(texture);
            }
        }
        self.frames = frames;
        result
    }

    fn record_targets(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        output: &wgpu::TextureView,
        frames: &mut [TargetFrame],
        text: &mut TextSystem,
    ) -> Result<(), RenderError> {
        lock_images(&self.images).frame += 1;
        for frame in frames.iter() {
            for image in &frame.flat.images {
                self.ensure_image_uploaded(&image.primitive);
            }
        }
        let segments = self.upload_segments();

        let mut text_steps = 0;
        let mut buffers = Vec::with_capacity(frames.len());
        for frame in frames.iter_mut() {
            build_batches(&frame.flat, &mut frame.batches);
            frame.text_base = text_steps;
            text_steps += frame.batches.text_steps;
            let (device, queue) = (&self.device, &self.queue);
            let pool = &mut self.instance_buffer_pool;
            let batches = &frame.batches;
            buffers.push(FrameBuffers {
                shadow: pool.upload(device, queue, "quark_shadow_instances", &batches.shadows),
                effect: pool.upload(
                    device,
                    queue,
                    "quark_effect_quad_instances",
                    &batches.effects,
                ),
                quad: pool.upload(device, queue, "quark_quad_instances", &batches.quads),
                image: pool.upload(device, queue, "quark_image_blit", &batches.images),
                path: pool.upload(device, queue, "quark_path_instances", &batches.paths),
                layer: pool.upload(device, queue, "quark_layer_instances", &batches.layers),
                ..FrameBuffers::default()
            });
        }
        self.prepare_layer_viewports(frames);

        // Prepare every text segment before recording any pass.
        self.fit_text_renderers(text_steps);
        self.recolored
            .prepare(frames.iter().map(|frame| &frame.flat.rich_texts[..]), text);
        self.text_ready = match self.prepare_text(frames, text) {
            Ok(()) => true,
            Err(_) => {
                // The atlas is full of glyphs pinned by this frame. Unpin
                // them and prepare every segment again, since the retry may
                // evict glyphs that earlier segments' vertices point at.
                self.atlas.trim();
                match self.prepare_text(frames, text) {
                    Ok(()) => true,
                    Err(error) => {
                        // This frame's glyphs do not fit even alone. Draw
                        // everything else rather than failing the frame, and
                        // unpin so the next frame starts from a clean atlas.
                        tracing::warn!("skipping text for one frame: {error}");
                        self.atlas.trim();
                        false
                    }
                }
            }
        };

        let blurs: Vec<Option<BlurTargets>> = frames
            .iter()
            .zip(buffers.iter_mut())
            .map(|(frame, buffers)| {
                self.prepare_blur(&frame.flat, buffers, frame.width, frame.height)
            })
            .collect();
        let mut layer_binds = Vec::with_capacity(frames.len());
        let mut layer_views = Vec::with_capacity(frames.len());
        for frame in frames.iter() {
            let Some(texture) = &frame.layer else {
                layer_binds.push(None);
                layer_views.push(None);
                continue;
            };
            layer_binds.push(Some(self.texture_pool.bind_group(
                &self.device,
                &self.texture_bind_group_layout,
                &self.sampler,
                texture,
            )));
            layer_views.push(Some(self.texture_pool.view(texture).clone()));
        }

        // A layer's target number is above its parent's, so encoding from
        // the last target down draws every layer before its composite.
        let mut result = Ok(());
        for index in (0..frames.len()).rev() {
            let (view, clear) = match &layer_views[index] {
                Some(view) => (view, wgpu::Color::TRANSPARENT),
                None => (output, wgpu::Color::BLACK),
            };
            let target = EncodeTarget {
                frame: &frames[index],
                buffers: &buffers[index],
                blur: blurs[index].as_ref(),
                uniform: match index {
                    0 => &self.viewport_bind_group,
                    i => &self.layer_uniforms[i - 1].1,
                },
                glyph_viewport: match index {
                    0 => &self.viewport,
                    i => &self.layer_viewports[i - 1],
                },
                layer_binds: &layer_binds,
                segments: segments.as_ref(),
                clear,
            };
            result = self.encode_steps(encoder, view, &target);
            if result.is_err() {
                break;
            }
        }
        for targets in blurs.into_iter().flatten() {
            self.texture_pool.release(targets.scene);
            self.texture_pool.release(targets.h);
            self.texture_pool.release(targets.v);
        }
        let mut images = lock_images(&self.images);
        let frame = images.frame;
        images
            .cache
            .retain(|_, image| frame - image.last_used_frame <= KEEP_UNUSED_IMAGE_FRAMES);
        result
    }

    /// Size the viewport uniform and glyphon viewport of every layer
    /// target to its texture region.
    fn prepare_layer_viewports(&mut self, frames: &[TargetFrame]) {
        for (slot, frame) in frames.iter().skip(1).enumerate() {
            if slot == self.layer_uniforms.len() {
                let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("quark_layer_viewport_uniform"),
                    size: std::mem::size_of::<ViewportUniform>() as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("quark_layer_viewport_bind_group"),
                    layout: &self.gpu.inner.viewport_bind_group_layout,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: buffer.as_entire_binding(),
                    }],
                });
                self.layer_uniforms.push((buffer, bind_group));
                self.layer_viewports
                    .push(Viewport::new(&self.device, &self.gpu.inner.glyph_cache));
            }
            let uniform = ViewportUniform {
                time: self.time,
                ..ViewportUniform::new(frame.width, frame.height)
            };
            self.queue.write_buffer(
                &self.layer_uniforms[slot].0,
                0,
                bytemuck::bytes_of(&uniform),
            );
            self.layer_viewports[slot].update(
                &self.queue,
                Resolution {
                    width: frame.width,
                    height: frame.height,
                },
            );
        }
    }

    /// Copy this frame's path segments into the segment texture, growing
    /// it as needed. `None` when no path draws.
    fn upload_segments(&mut self) -> Option<wgpu::BindGroup> {
        let segments = &mut self.flattener.segments;
        if segments.is_empty() {
            return None;
        }
        let max_rows = self.device.limits().max_texture_dimension_2d;
        let rows = (segments.len() as u32)
            .div_ceil(SEGMENT_TEXTURE_WIDTH)
            .min(max_rows);
        if self
            .segment_texture
            .as_ref()
            .is_none_or(|texture| texture.rows < rows)
        {
            let allocated = rows.next_power_of_two().min(max_rows);
            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("quark_path_segments"),
                size: wgpu::Extent3d {
                    width: SEGMENT_TEXTURE_WIDTH,
                    height: allocated,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba32Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("quark_path_segments"),
                layout: &self.gpu.inner.segment_bind_group_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                }],
            });
            self.segment_texture = Some(SegmentTexture {
                texture,
                bind_group,
                rows: allocated,
            });
        }
        let texture = self.segment_texture.as_ref()?;
        // Whole rows; segments past the largest texture are dropped.
        segments.resize((rows * SEGMENT_TEXTURE_WIDTH) as usize, [0.0; 4]);
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytemuck::cast_slice(segments),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(SEGMENT_TEXTURE_WIDTH * 16),
                rows_per_image: Some(rows),
            },
            wgpu::Extent3d {
                width: SEGMENT_TEXTURE_WIDTH,
                height: rows,
                depth_or_array_layers: 1,
            },
        );
        Some(texture.bind_group.clone())
    }

    /// Keep one text renderer per text segment of this frame. Renderers left
    /// over from a frame with many more segments are dropped, so one busy
    /// frame does not hold their vertex buffers forever.
    fn fit_text_renderers(&mut self, segments: usize) {
        let wanted = segments.max(1);
        if self.text_renderers.len() > wanted * 2 {
            self.text_renderers.truncate(wanted);
        }
        while self.text_renderers.len() < wanted {
            self.text_renderers.push(TextRenderer::new(
                &mut self.atlas,
                &self.device,
                wgpu::MultisampleState::default(),
                None,
            ));
        }
    }

    /// Prepare each text segment of every target into its own renderer.
    fn prepare_text(
        &mut self,
        frames: &[TargetFrame],
        text: &mut TextSystem,
    ) -> Result<(), glyphon::PrepareError> {
        for (index, frame) in frames.iter().enumerate() {
            let flat = &frame.flat;
            let viewport = match index {
                0 => &self.viewport,
                i => &self.layer_viewports[i - 1],
            };
            let mut text_index = frame.text_base;
            for step in &flat.steps {
                let DrawStep::Batch {
                    kind: PrimKind::Text,
                    items,
                    rich,
                } = step
                else {
                    continue;
                };
                let text_areas = prepare_text_areas(
                    &flat.texts[items.start as usize..items.end as usize],
                    &flat.rich_texts[rich.start as usize..rich.end as usize],
                    &self.recolored,
                );
                self.text_renderers[text_index].prepare(
                    &self.device,
                    &self.queue,
                    text.font_system_mut(),
                    &mut self.atlas,
                    viewport,
                    text_areas,
                    &mut self.swash_cache,
                )?;
                text_index += 1;
            }
        }
        Ok(())
    }

    /// Acquire offscreen targets and upload blur instances when the frame has
    /// blur regions.
    fn prepare_blur(
        &mut self,
        flat: &FlattenedScene,
        buffers: &mut FrameBuffers,
        width: u32,
        height: u32,
    ) -> Option<BlurTargets> {
        if !flat.steps.iter().any(|s| matches!(s, DrawStep::Blur(_))) {
            return None;
        }
        let scene = self.texture_pool.acquire(&self.device, width, height);
        let h = self.texture_pool.acquire(&self.device, width, height);
        let v = self.texture_pool.acquire(&self.device, width, height);
        // Pooled textures can be larger than the frame; passes on them set a
        // viewport of the frame size, so UVs divide by the texture size.
        let (tw, th) = (scene.width as f32, scene.height as f32);

        let mut blur_instances = Vec::new();
        let mut blit_instances = Vec::new();
        for step in &flat.steps {
            let DrawStep::Blur(region) = step else {
                continue;
            };
            let sigma = (region.blur_radius * 0.5).max(0.5);
            let br = region.rect;
            let uv = [br.x / tw, br.y / th, br.right() / tw, br.bottom() / th];
            let bounds = [br.x, br.y, br.width, br.height];
            // The vertical pass samples up to six (scaled) texels above and
            // below the region, so the horizontal pass covers that margin
            // too; otherwise the region's top and bottom edges blend with
            // the cleared, transparent texels around it.
            let reach = 6.0 * (sigma / 6.0).max(1.0) + 1.0;
            let top = (br.y - reach).max(0.0);
            let bottom = (br.bottom() + reach).min(height as f32);
            blur_instances.push(BlurInstance {
                bounds: [br.x, top, br.width, bottom - top],
                uv_rect: [br.x / tw, top / th, br.right() / tw, bottom / th],
                blur_params: [1.0, 0.0, sigma, 0.0],
            });
            blur_instances.push(BlurInstance {
                bounds,
                uv_rect: uv,
                blur_params: [0.0, 1.0, sigma, 0.0],
            });
            blit_instances.push(BlitInstance {
                bounds,
                uv_rect: uv,
                tint: [1.0; 4],
                radii: region.corner_radii,
            });
        }
        blit_instances.push(BlitInstance {
            bounds: [0.0, 0.0, width as f32, height as f32],
            uv_rect: [0.0, 0.0, width as f32 / tw, height as f32 / th],
            tint: [1.0; 4],
            radii: [0.0; 4],
        });
        let pool = &mut self.instance_buffer_pool;
        buffers.blur = pool.upload(
            &self.device,
            &self.queue,
            "quark_blur_instances",
            &blur_instances,
        );
        buffers.blur_blit = pool.upload(
            &self.device,
            &self.queue,
            "quark_blur_blit",
            &blit_instances,
        );

        let layout = &self.texture_bind_group_layout;
        let sampler = &self.sampler;
        let scene_bind = self
            .texture_pool
            .bind_group(&self.device, layout, sampler, &scene);
        let h_bind = self
            .texture_pool
            .bind_group(&self.device, layout, sampler, &h);
        let v_bind = self
            .texture_pool
            .bind_group(&self.device, layout, sampler, &v);
        Some(BlurTargets {
            scene_view: self.texture_pool.view(&scene).clone(),
            h_view: self.texture_pool.view(&h).clone(),
            v_view: self.texture_pool.view(&v).clone(),
            scene,
            h,
            v,
            scene_bind,
            h_bind,
            v_bind,
        })
    }

    /// Record one target's passes. Without blur this is one pass on `view`.
    /// With blur, steps draw into an offscreen scene texture; at each blur step
    /// the region is blurred (horizontal then vertical pass) and composited
    /// back before later steps draw, and the scene is finally copied to
    /// `view`.
    fn encode_steps(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        t: &EncodeTarget<'_>,
    ) -> Result<(), RenderError> {
        let flat = &t.frame.flat;
        let (width, height) = (t.frame.width, t.frame.height);
        let blur = t.blur;
        let buffers = t.buffers;
        let draw_view = blur.map_or(view, |b| &b.scene_view);
        // Pooled textures (layers, blur scratch) can be larger than the
        // target; draw into its corner.
        let set_viewport = |pass: &mut wgpu::RenderPass<'_>| {
            pass.set_viewport(0.0, 0.0, width as f32, height as f32, 0.0, 1.0);
        };
        let mut load = wgpu::LoadOp::Clear(t.clear);
        let mut blurs_done = 0u32;
        let mut start = 0;
        loop {
            let end = flat.steps[start..]
                .iter()
                .position(|s| matches!(s, DrawStep::Blur(_)))
                .map_or(flat.steps.len(), |p| start + p);
            {
                let mut pass = begin_pass(encoder, "quark_frame_pass", draw_view, load);
                set_viewport(&mut pass);
                if let (Some(b), Some(buf)) = (blur, &buffers.blur_blit)
                    && blurs_done > 0
                {
                    pass.set_pipeline(&self.blit_pipeline);
                    pass.set_bind_group(0, t.uniform, &[]);
                    pass.set_bind_group(1, &b.v_bind, &[]);
                    pass.set_vertex_buffer(0, buf.slice(..));
                    pass.set_scissor_rect(0, 0, width, height);
                    pass.draw(0..4, blurs_done - 1..blurs_done);
                }
                for i in start..end {
                    self.draw_step(
                        &mut pass,
                        &flat.steps[i],
                        t.frame.batches.step_cmds[i].clone(),
                        t,
                    )?;
                }
            }
            load = wgpu::LoadOp::Load;
            if end == flat.steps.len() {
                break;
            }
            let (Some(b), Some(buf)) = (blur, &buffers.blur) else {
                break;
            };
            for (dir, source, dest) in [(0, &b.scene_bind, &b.h_view), (1, &b.h_bind, &b.v_view)] {
                let mut pass = begin_pass(
                    encoder,
                    "quark_blur_pass",
                    dest,
                    wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                );
                set_viewport(&mut pass);
                pass.set_pipeline(&self.blur_pipeline);
                pass.set_bind_group(0, t.uniform, &[]);
                pass.set_bind_group(1, source, &[]);
                pass.set_vertex_buffer(0, buf.slice(..));
                let instance = blurs_done * 2 + dir;
                pass.draw(0..4, instance..instance + 1);
            }
            blurs_done += 1;
            start = end + 1;
        }

        if let (Some(b), Some(buf)) = (blur, &buffers.blur_blit) {
            let mut pass = begin_pass(
                encoder,
                "quark_composite_pass",
                view,
                wgpu::LoadOp::Clear(t.clear),
            );
            set_viewport(&mut pass);
            pass.set_pipeline(&self.blit_pipeline);
            pass.set_bind_group(0, t.uniform, &[]);
            pass.set_bind_group(1, &b.scene_bind, &[]);
            pass.set_vertex_buffer(0, buf.slice(..));
            pass.draw(0..4, blurs_done..blurs_done + 1);
        }
        Ok(())
    }

    fn draw_step(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        step: &DrawStep,
        cmds: std::ops::Range<u32>,
        t: &EncodeTarget<'_>,
    ) -> Result<(), RenderError> {
        let DrawStep::Batch { kind, .. } = step else {
            return Ok(());
        };
        let (width, height) = (t.frame.width, t.frame.height);
        let batches = &t.frame.batches;
        let buffers = t.buffers;
        let cmds = cmds.start as usize..cmds.end as usize;
        let (pipeline, buffer) = match kind {
            PrimKind::Shadow => (&self.shadow_pipeline, &buffers.shadow),
            PrimKind::Effect => (&self.effect_quad_pipeline, &buffers.effect),
            PrimKind::Quad => (&self.quad_pipeline, &buffers.quad),
            PrimKind::Path => (&self.path_pipeline, &buffers.path),
            PrimKind::Layer => {
                let Some(buffer) = &buffers.layer else {
                    return Ok(());
                };
                pass.set_pipeline(&self.layer_pipeline);
                pass.set_bind_group(0, t.uniform, &[]);
                pass.set_vertex_buffer(0, buffer.slice(..));
                for command in &batches.layer_cmds[cmds] {
                    let Some(bind) = t.layer_binds.get(command.target).and_then(Option::as_ref)
                    else {
                        continue;
                    };
                    let Some((sx, sy, sw, sh)) = scissor_rect(command.clip, width, height) else {
                        continue;
                    };
                    pass.set_bind_group(1, bind, &[]);
                    pass.set_scissor_rect(sx, sy, sw, sh);
                    pass.draw(0..4, command.instance..command.instance + 1);
                }
                return Ok(());
            }
            PrimKind::Image => {
                let Some(buffer) = &buffers.image else {
                    return Ok(());
                };
                let images = lock_images(&self.images);
                pass.set_pipeline(&self.blit_pipeline);
                pass.set_bind_group(0, t.uniform, &[]);
                pass.set_vertex_buffer(0, buffer.slice(..));
                for command in &batches.image_cmds[cmds] {
                    // The cache lookup is the gate: icons resolved from a prior
                    // frame carry an empty `rgba` but still draw via their
                    // uploaded texture. Images that were never uploadable miss.
                    let Some(entry) = images.cache.get(&command.cache_key) else {
                        continue;
                    };
                    let Some((sx, sy, sw, sh)) = scissor_rect(command.clip, width, height) else {
                        continue;
                    };
                    pass.set_bind_group(1, &entry.bind_group, &[]);
                    pass.set_scissor_rect(sx, sy, sw, sh);
                    pass.draw(0..4, command.instance_start..command.instance_end);
                }
                return Ok(());
            }
            PrimKind::Text => {
                if !self.text_ready {
                    return Ok(());
                }
                pass.set_scissor_rect(0, 0, width, height);
                self.text_renderers[t.frame.text_base + cmds.start].render(
                    &self.atlas,
                    t.glyph_viewport,
                    pass,
                )?;
                return Ok(());
            }
        };
        let Some(buffer) = buffer else {
            return Ok(());
        };
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, t.uniform, &[]);
        if *kind == PrimKind::Path {
            let Some(segments) = t.segments else {
                return Ok(());
            };
            pass.set_bind_group(1, segments, &[]);
        }
        pass.set_vertex_buffer(0, buffer.slice(..));
        for command in &batches.quad_cmds[cmds] {
            let Some((sx, sy, sw, sh)) = scissor_rect(command.clip, width, height) else {
                continue;
            };
            pass.set_scissor_rect(sx, sy, sw, sh);
            pass.draw(0..4, command.instance_range());
        }
        Ok(())
    }

    /// Upload `image` to the GPU cache under its key if it is not there yet,
    /// and mark the entry used this frame.
    fn ensure_image_uploaded(&mut self, image: &crate::scene::ImagePrimitive) {
        let key = image.cache_key;
        let mut images = lock_images(&self.images);
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
        let texture = self.device.create_texture_with_data(
            &self.queue,
            &wgpu::TextureDescriptor {
                label: Some("quark_cached_image"),
                size: wgpu::Extent3d {
                    width: image.width,
                    height: image.height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &image.rgba,
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = create_texture_bind_group(
            &self.device,
            &self.texture_bind_group_layout,
            &view,
            &self.sampler,
        );
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

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Instance buffers for one frame, one per pipeline.
#[derive(Default)]
struct FrameBuffers {
    shadow: Option<wgpu::Buffer>,
    effect: Option<wgpu::Buffer>,
    quad: Option<wgpu::Buffer>,
    image: Option<wgpu::Buffer>,
    path: Option<wgpu::Buffer>,
    layer: Option<wgpu::Buffer>,
    /// Two instances per blur region: horizontal then vertical.
    blur: Option<wgpu::Buffer>,
    /// One instance per blur region (composite the blurred region), then a
    /// final full-target instance (copy the offscreen scene to the output).
    blur_blit: Option<wgpu::Buffer>,
}

/// Texels per row of the path segment texture; the path shader's
/// `SEGMENT_ROW` must match.
const SEGMENT_TEXTURE_WIDTH: u32 = 1024;

/// Every path segment of a frame, one `[x0, y0, x1, y1]` texel each.
struct SegmentTexture {
    texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
    rows: u32,
}

/// One render target of a frame: the window, or an offscreen layer. Kept on
/// the renderer so its vectors keep their capacity between frames.
#[derive(Default)]
struct TargetFrame {
    flat: FlattenedScene,
    batches: FrameBatches,
    width: u32,
    height: u32,
    /// The pooled texture an offscreen layer renders into this frame.
    layer: Option<OffscreenTarget>,
    /// Text renderer of this target's first text step.
    text_base: usize,
}

/// What encoding one target needs.
struct EncodeTarget<'a> {
    frame: &'a TargetFrame,
    buffers: &'a FrameBuffers,
    blur: Option<&'a BlurTargets>,
    uniform: &'a wgpu::BindGroup,
    glyph_viewport: &'a Viewport,
    /// Texture bind groups of the frame's layer targets, by target number.
    layer_binds: &'a [Option<wgpu::BindGroup>],
    segments: Option<&'a wgpu::BindGroup>,
    clear: wgpu::Color,
}

/// Offscreen targets used when a frame contains blur regions.
struct BlurTargets {
    scene: OffscreenTarget,
    h: OffscreenTarget,
    v: OffscreenTarget,
    scene_view: wgpu::TextureView,
    h_view: wgpu::TextureView,
    v_view: wgpu::TextureView,
    scene_bind: wgpu::BindGroup,
    h_bind: wgpu::BindGroup,
    v_bind: wgpu::BindGroup,
}

fn begin_pass<'e>(
    encoder: &'e mut wgpu::CommandEncoder,
    label: &'static str,
    view: &wgpu::TextureView,
    load: wgpu::LoadOp<wgpu::Color>,
) -> wgpu::RenderPass<'e> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load,
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    })
}

fn scissor_rect(clip: Rect, viewport_w: u32, viewport_h: u32) -> Option<(u32, u32, u32, u32)> {
    if viewport_w == 0
        || viewport_h == 0
        || clip.width <= 0.0
        || clip.height <= 0.0
        || !clip.x.is_finite()
        || !clip.y.is_finite()
        || !clip.width.is_finite()
        || !clip.height.is_finite()
    {
        return None;
    }

    let viewport_w = viewport_w as f32;
    let viewport_h = viewport_h as f32;
    let left = clip.x.max(0.0).floor().min(viewport_w);
    let top = clip.y.max(0.0).floor().min(viewport_h);
    let right = (clip.x + clip.width).ceil().clamp(0.0, viewport_w);
    let bottom = (clip.y + clip.height).ceil().clamp(0.0, viewport_h);

    if right <= left || bottom <= top {
        return None;
    }

    let sx = left as u32;
    let sy = top as u32;
    let sw = (right as u32).saturating_sub(sx);
    let sh = (bottom as u32).saturating_sub(sy);

    (sw > 0 && sh > 0).then_some((sx, sy, sw, sh))
}

fn create_texture_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("quark_texture_bind_group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    })
}

// ---------------------------------------------------------------------------
// GPU types
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct QuadInstance {
    bounds: [f32; 4],
    background: [f32; 4],
    border_color: [f32; 4],
    corner_radii: [f32; 4],
    border_widths: [f32; 4],
    /// Rounded-clip rect [x, y, w, h] — the rect whose radii apply to the SDF clip test.
    clip_bounds: [f32; 4],
    /// Rounded-clip corner radii [tl, tr, br, bl]. All zero = no rounded clip.
    clip_radii: [f32; 4],
}

impl QuadInstance {
    fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &[
                wgpu::VertexAttribute {
                    offset: 0,
                    shader_location: 0,
                    format: wgpu::VertexFormat::Float32x4,
                },
                wgpu::VertexAttribute {
                    offset: 16,
                    shader_location: 1,
                    format: wgpu::VertexFormat::Float32x4,
                },
                wgpu::VertexAttribute {
                    offset: 32,
                    shader_location: 2,
                    format: wgpu::VertexFormat::Float32x4,
                },
                wgpu::VertexAttribute {
                    offset: 48,
                    shader_location: 3,
                    format: wgpu::VertexFormat::Float32x4,
                },
                wgpu::VertexAttribute {
                    offset: 64,
                    shader_location: 4,
                    format: wgpu::VertexFormat::Float32x4,
                },
                wgpu::VertexAttribute {
                    offset: 80,
                    shader_location: 5,
                    format: wgpu::VertexFormat::Float32x4,
                },
                wgpu::VertexAttribute {
                    offset: 96,
                    shader_location: 6,
                    format: wgpu::VertexFormat::Float32x4,
                },
            ],
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct ShadowInstance {
    /// Expanded quad bounds (x, y, w, h) — covers the full blur extent.
    draw_bounds: [f32; 4],
    /// Original shadow-casting rect (x, y, w, h) before expansion.
    shadow_bounds: [f32; 4],
    /// Shadow color (linear RGBA, premultiplied).
    color: [f32; 4],
    /// [blur_sigma, corner_radius, 0, 0]
    params: [f32; 4],
    /// Rounded-clip rect [x, y, w, h] — the rect whose radii apply for the SDF clip test.
    clip_bounds: [f32; 4],
    /// Rounded-clip corner radii [tl, tr, br, bl]. All zero = no rounded clip.
    clip_radii: [f32; 4],
}

impl ShadowInstance {
    fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &[
                wgpu::VertexAttribute {
                    offset: 0,
                    shader_location: 0,
                    format: wgpu::VertexFormat::Float32x4,
                },
                wgpu::VertexAttribute {
                    offset: 16,
                    shader_location: 1,
                    format: wgpu::VertexFormat::Float32x4,
                },
                wgpu::VertexAttribute {
                    offset: 32,
                    shader_location: 2,
                    format: wgpu::VertexFormat::Float32x4,
                },
                wgpu::VertexAttribute {
                    offset: 48,
                    shader_location: 3,
                    format: wgpu::VertexFormat::Float32x4,
                },
                wgpu::VertexAttribute {
                    offset: 64,
                    shader_location: 4,
                    format: wgpu::VertexFormat::Float32x4,
                },
                wgpu::VertexAttribute {
                    offset: 80,
                    shader_location: 5,
                    format: wgpu::VertexFormat::Float32x4,
                },
            ],
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct EffectQuadInstance {
    /// Element bounds: [x, y, width, height].
    bounds: [f32; 4],
    /// First color (linear RGBA, premultiplied).
    color_a: [f32; 4],
    /// Second color (linear RGBA, premultiplied).
    color_b: [f32; 4],
    /// [effect_type, param1, param2, corner_radius].
    params: [f32; 4],
    /// Rounded-clip rect [x, y, w, h] — the rect whose radii apply for the SDF clip test.
    clip_bounds: [f32; 4],
    /// Rounded-clip corner radii [tl, tr, br, bl]. All zero = no rounded clip.
    clip_radii: [f32; 4],
}

impl EffectQuadInstance {
    fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &[
                wgpu::VertexAttribute {
                    offset: 0,
                    shader_location: 0,
                    format: wgpu::VertexFormat::Float32x4,
                },
                wgpu::VertexAttribute {
                    offset: 16,
                    shader_location: 1,
                    format: wgpu::VertexFormat::Float32x4,
                },
                wgpu::VertexAttribute {
                    offset: 32,
                    shader_location: 2,
                    format: wgpu::VertexFormat::Float32x4,
                },
                wgpu::VertexAttribute {
                    offset: 48,
                    shader_location: 3,
                    format: wgpu::VertexFormat::Float32x4,
                },
                wgpu::VertexAttribute {
                    offset: 64,
                    shader_location: 4,
                    format: wgpu::VertexFormat::Float32x4,
                },
                wgpu::VertexAttribute {
                    offset: 80,
                    shader_location: 5,
                    format: wgpu::VertexFormat::Float32x4,
                },
            ],
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct BlitInstance {
    /// Screen-space destination bounds: [x, y, width, height].
    bounds: [f32; 4],
    /// Source texture UV rect: [u_min, v_min, u_max, v_max].
    uv_rect: [f32; 4],
    /// Premultiplied tint/opacity multiplier (usually [alpha; 4]).
    tint: [f32; 4],
    /// Rounded mask over `bounds`: [tl, tr, br, bl]. All zero = none.
    radii: [f32; 4],
}

impl BlitInstance {
    fn layout() -> wgpu::VertexBufferLayout<'static> {
        const ATTRIBUTES: [wgpu::VertexAttribute; 4] = wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4];
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &ATTRIBUTES,
        }
    }
}

/// An offscreen layer composited through an affine map.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct LayerInstance {
    /// [a, b, c, d] of the map from layer texels to target pixels.
    linear: [f32; 4],
    /// [tx, ty, layer width, layer height].
    offset_size: [f32; 4],
    /// [1 / texture width, 1 / texture height, opacity, 0].
    params: [f32; 4],
    clip_bounds: [f32; 4],
    clip_radii: [f32; 4],
}

impl LayerInstance {
    fn layout() -> wgpu::VertexBufferLayout<'static> {
        const ATTRIBUTES: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![
            0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4, 4 => Float32x4
        ];
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &ATTRIBUTES,
        }
    }
}

/// One band of a path shape; see `path.rs`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct PathInstance {
    /// Band quad in target pixels: [x, y, w, h].
    bounds: [f32; 4],
    /// [origin x, origin y, fill rule (0 nonzero, 1 even-odd), 0].
    origin_rule: [f32; 4],
    /// Linear RGBA, straight alpha.
    color: [f32; 4],
    /// [first segment, segment count, 0, 0].
    segments: [u32; 4],
    clip_bounds: [f32; 4],
    clip_radii: [f32; 4],
}

impl PathInstance {
    fn layout() -> wgpu::VertexBufferLayout<'static> {
        const ATTRIBUTES: [wgpu::VertexAttribute; 6] = wgpu::vertex_attr_array![
            0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Uint32x4,
            4 => Float32x4, 5 => Float32x4
        ];
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &ATTRIBUTES,
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct BlurInstance {
    /// Destination bounds in the target texture: [x, y, width, height].
    bounds: [f32; 4],
    /// Source UV rect: [u_min, v_min, u_max, v_max].
    uv_rect: [f32; 4],
    /// [direction_x, direction_y, blur_sigma, 0.0]
    /// direction = (1,0) for horizontal, (0,1) for vertical.
    blur_params: [f32; 4],
}

impl BlurInstance {
    fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &[
                wgpu::VertexAttribute {
                    offset: 0,
                    shader_location: 0,
                    format: wgpu::VertexFormat::Float32x4,
                },
                wgpu::VertexAttribute {
                    offset: 16,
                    shader_location: 1,
                    format: wgpu::VertexFormat::Float32x4,
                },
                wgpu::VertexAttribute {
                    offset: 32,
                    shader_location: 2,
                    format: wgpu::VertexFormat::Float32x4,
                },
            ],
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct ViewportUniform {
    resolution: [f32; 2],
    time: f32,
    _padding: f32,
}

impl ViewportUniform {
    fn new(width: u32, height: u32) -> Self {
        Self {
            resolution: [width as f32, height as f32],
            time: 0.0,
            _padding: 0.0,
        }
    }
}

// ---------------------------------------------------------------------------
// Scene flattening
// ---------------------------------------------------------------------------
//
// Draw order follows paint order across primitive kinds. Each z-layer is split
// into ordered segments, one primitive kind per segment, and each segment is
// one batched draw (per scissor clip). A primitive joins the most recent
// segment of its kind unless something of another kind painted after that
// segment overlaps it, in which case it opens a new segment. Disjoint content
// (list rows: background, text, background, text) therefore stays in two
// batches, while a selection quad painted over text gets its own segment above
// the text. Overlap tests use the clipped bounds of each primitive, with a
// chunked bounding-box prefilter so long frames stay near linear.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PrimKind {
    Shadow = 0,
    Effect = 1,
    Quad = 2,
    Image = 3,
    Text = 4,
    Path = 5,
    /// An offscreen layer composited into its parent.
    Layer = 6,
}

const KIND_COUNT: usize = 7;

/// Sort key that puts every primitive in its final draw position: z-layer,
/// then segment within the layer, then paint order within the segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub(super) struct DrawKey {
    z: i32,
    segment: u32,
    seq: u32,
}

#[derive(Debug, Clone, Copy)]
struct FlattenedBlurRegion {
    /// Screen-space bounds of the blur region.
    rect: Rect,
    blur_radius: f32,
    /// Rounded mask of the blurred result: [tl, tr, br, bl].
    corner_radii: [f32; 4],
}

#[derive(Debug, Clone)]
enum DrawStep {
    /// One segment. `items` indexes the kind's sorted array in
    /// `FlattenedScene`; for text, `items` covers plain runs and `rich` covers
    /// rich runs (plain runs of a segment draw before its rich runs).
    Batch {
        kind: PrimKind,
        items: std::ops::Range<u32>,
        rich: std::ops::Range<u32>,
    },
    /// Blur everything drawn so far inside the region, then keep drawing on
    /// top of the result. "So far" means final draw order: lower z-layers and
    /// earlier segments of this layer, whatever their scene order. Outside
    /// the region's rounded corners the backdrop stays sharp.
    Blur(FlattenedBlurRegion),
}

/// Flattened scene in draw order. Kept on the renderer and refilled every
/// frame so its vectors keep their capacity.
#[derive(Debug, Default)]
struct FlattenedScene {
    shadows: Vec<ClippedShadow>,
    effect_quads: Vec<ClippedEffectQuad>,
    quads: Vec<ClippedQuad>,
    images: Vec<ClippedImage>,
    texts: Vec<ClippedText>,
    rich_texts: Vec<ClippedRichText>,
    paths: Vec<ClippedPath>,
    layers: Vec<ClippedLayer>,
    steps: Vec<DrawStep>,
}

impl FlattenedScene {
    fn clear(&mut self) {
        self.shadows.clear();
        self.effect_quads.clear();
        self.quads.clear();
        self.images.clear();
        self.texts.clear();
        self.rich_texts.clear();
        self.paths.clear();
        self.layers.clear();
        self.steps.clear();
    }
}

#[derive(Debug, Clone, Copy)]
struct ClippedShadow {
    key: DrawKey,
    instance: ShadowInstance,
    clip: Rect,
}

#[derive(Debug, Clone, Copy)]
struct ClippedQuad {
    key: DrawKey,
    instance: QuadInstance,
    clip: Rect,
}

#[derive(Debug, Clone, Copy)]
struct ClippedEffectQuad {
    key: DrawKey,
    instance: EffectQuadInstance,
    clip: Rect,
}

#[derive(Debug, Clone)]
struct ClippedImage {
    key: DrawKey,
    primitive: crate::scene::ImagePrimitive,
    clip: Rect,
    /// Opacity of an enclosing layer drawn in place.
    alpha: f32,
}

#[derive(Debug, Clone, Copy)]
struct ClippedPath {
    key: DrawKey,
    instance: PathInstance,
    clip: Rect,
}

#[derive(Debug, Clone, Copy)]
struct ClippedLayer {
    key: DrawKey,
    instance: LayerInstance,
    clip: Rect,
    /// Frame index of the layer's own target.
    target: usize,
}

#[derive(Debug, Clone)]
pub(super) struct ClippedText {
    pub(super) key: DrawKey,
    pub(super) primitive: TextPrimitive,
    pub(super) clip: Rect,
}

#[derive(Debug, Clone)]
pub(super) struct ClippedRichText {
    pub(super) key: DrawKey,
    pub(super) primitive: RichTextPrimitive,
    pub(super) clip: Rect,
}

#[derive(Clone, Copy)]
struct QuadDrawCommand {
    instance_start: u32,
    instance_end: u32,
    clip: Rect,
}

impl QuadDrawCommand {
    fn instance_range(&self) -> std::ops::Range<u32> {
        self.instance_start..self.instance_end
    }
}

#[derive(Clone, Copy)]
struct LayerDrawCommand {
    instance: u32,
    clip: Rect,
    target: usize,
}

#[derive(Clone, Copy)]
struct ImageDrawCommand {
    instance_start: u32,
    instance_end: u32,
    cache_key: u64,
    clip: Rect,
}

/// Bounds of everything drawn after some segment, for overlap queries. Rects
/// are grouped in chunks of consecutive pushes; paint order is spatially
/// coherent, so chunk bounds reject most queries without visiting the rects.
#[derive(Debug, Default)]
struct BlockerList {
    rects: Vec<Rect>,
    chunks: Vec<Rect>,
}

const BLOCKER_CHUNK: usize = 32;

impl BlockerList {
    fn clear(&mut self) {
        self.rects.clear();
        self.chunks.clear();
    }

    fn push(&mut self, rect: Rect) {
        if self.rects.len().is_multiple_of(BLOCKER_CHUNK) {
            self.chunks.push(rect);
        } else if let Some(chunk) = self.chunks.last_mut() {
            *chunk = rect_union(*chunk, rect);
        }
        self.rects.push(rect);
    }

    fn overlaps(&self, rect: Rect) -> bool {
        self.chunks.iter().enumerate().any(|(i, chunk)| {
            if !rects_overlap(*chunk, rect) {
                return false;
            }
            let start = i * BLOCKER_CHUNK;
            let end = (start + BLOCKER_CHUNK).min(self.rects.len());
            self.rects[start..end]
                .iter()
                .any(|blocker| rects_overlap(*blocker, rect))
        })
    }
}

#[derive(Debug, Clone, Copy)]
enum SegmentSlot {
    Batch(PrimKind),
    Blur(FlattenedBlurRegion),
}

/// Segment assignment state for one z-layer.
#[derive(Debug, Default)]
struct ZBuilder {
    z: i32,
    segments: Vec<SegmentSlot>,
    /// Most recent segment of each kind that can still take primitives.
    latest: [Option<u32>; KIND_COUNT],
    /// Per kind: bounds of primitives drawn after that kind's latest segment.
    blockers: [BlockerList; KIND_COUNT],
}

impl ZBuilder {
    fn reset(&mut self, z: i32) {
        self.z = z;
        self.segments.clear();
        self.latest = [None; KIND_COUNT];
        for blockers in &mut self.blockers {
            blockers.clear();
        }
    }

    /// Choose the segment for a primitive of `kind` covering `bounds`.
    fn place(&mut self, kind: PrimKind, bounds: Rect) -> u32 {
        let k = kind as usize;
        let segment = match self.latest[k] {
            Some(segment) if !self.blockers[k].overlaps(bounds) => segment,
            _ => {
                let segment = self.segments.len() as u32;
                self.segments.push(SegmentSlot::Batch(kind));
                self.latest[k] = Some(segment);
                self.blockers[k].clear();
                segment
            }
        };
        for j in 0..KIND_COUNT {
            if j != k
                && let Some(latest) = self.latest[j]
                && segment > latest
            {
                self.blockers[j].push(bounds);
            }
        }
        segment
    }

    /// A blur samples everything before it, so nothing after it may merge
    /// into an earlier segment.
    fn barrier(&mut self, blur: FlattenedBlurRegion) {
        self.segments.push(SegmentSlot::Blur(blur));
        self.latest = [None; KIND_COUNT];
        for blockers in &mut self.blockers {
            blockers.clear();
        }
    }
}

/// Reusable scratch state for `flatten_scene_into`.
#[derive(Debug, Default)]
struct Flattener {
    builders: Vec<ZBuilder>,
    active: usize,
    clips: Vec<ActiveClip>,
    z_stack: Vec<i32>,
    seq: u32,
    /// Subtracted from scene coordinates: the target's texture origin, moved
    /// by the layers drawn in place around the current primitive.
    origin: (f32, f32),
    /// Opacity of the layers drawn in place around the current primitive.
    alpha: f32,
    /// `origin` and `alpha` outside each layer drawn in place.
    inline: Vec<((f32, f32), f32)>,
    /// Path segments of every target this frame, in segment texture order.
    segments: Vec<[f32; 4]>,
    bands: Vec<Band>,
    scratch: Vec<[f32; 4]>,
}

impl Flattener {
    fn shift(&self, rect: Rect) -> Rect {
        rect.offset(-self.origin.0, -self.origin.1)
    }

    fn clip(&self) -> ActiveClip {
        *self.clips.last().expect("root clip")
    }

    fn builder(&mut self) -> &mut ZBuilder {
        let z = self.z_stack.last().copied().unwrap_or(0);
        let index = match self.builders[..self.active].iter().position(|b| b.z == z) {
            Some(index) => index,
            None => {
                if self.active == self.builders.len() {
                    self.builders.push(ZBuilder::default());
                }
                self.builders[self.active].reset(z);
                self.active += 1;
                self.active - 1
            }
        };
        &mut self.builders[index]
    }

    fn place(&mut self, kind: PrimKind, bounds: Rect) -> DrawKey {
        self.seq += 1;
        let seq = self.seq;
        let builder = self.builder();
        DrawKey {
            z: builder.z,
            segment: builder.place(kind, bounds),
            seq,
        }
    }

    fn barrier(&mut self, blur: FlattenedBlurRegion) {
        self.builder().barrier(blur);
    }
}

/// Active clip state carried on the flatten_scene stack.
///
/// `scissor` is the intersection of every rect clip active at this point — it
/// maps directly to `set_scissor_rect` for rectangular culling. The rounded
/// clip is tracked separately: `rounded_rect` and `corner_radii` describe the
/// innermost rounded clip ancestor (if any). When a non-rounded clip is
/// pushed, we inherit the parent's rounded clip so that descendants of a
/// rounded container are still clipped to its rounded boundary. This drops
/// information for nested rounded clips (inner wins) — good enough in
/// practice since nested rounded clips are rare.
#[derive(Debug, Clone, Copy)]
struct ActiveClip {
    scissor: Rect,
    rounded_rect: Rect,
    corner_radii: [f32; 4],
}

impl ActiveClip {
    fn root(viewport: Rect) -> Self {
        Self {
            scissor: viewport,
            rounded_rect: viewport,
            corner_radii: [0.0; 4],
        }
    }

    fn has_rounded(&self) -> bool {
        self.corner_radii[0] > 0.0
            || self.corner_radii[1] > 0.0
            || self.corner_radii[2] > 0.0
            || self.corner_radii[3] > 0.0
    }

    fn push(&self, rect: Rect, corner_radii: [f32; 4]) -> Option<Self> {
        let scissor = self.scissor.intersection(rect)?;
        let is_rounded = corner_radii[0] > 0.0
            || corner_radii[1] > 0.0
            || corner_radii[2] > 0.0
            || corner_radii[3] > 0.0;
        let (rounded_rect, radii) = if is_rounded {
            (rect, corner_radii)
        } else {
            (self.rounded_rect, self.corner_radii)
        };
        Some(Self {
            scissor,
            rounded_rect,
            corner_radii: radii,
        })
    }

    fn clip_bounds_attr(&self) -> [f32; 4] {
        if self.has_rounded() {
            [
                self.rounded_rect.x,
                self.rounded_rect.y,
                self.rounded_rect.width,
                self.rounded_rect.height,
            ]
        } else {
            [0.0; 4]
        }
    }

    fn clip_radii_attr(&self) -> [f32; 4] {
        self.corner_radii
    }
}

#[cfg(test)]
fn flatten_scene(scene: &Scene, viewport: Rect, image_cache: &ImageCache) -> FlattenedScene {
    let mut out = FlattenedScene::default();
    let mut plans = Vec::new();
    plan_layers(scene, viewport, &mut plans);
    flatten_scene_into(
        &scene.primitives,
        0..scene.primitives.len(),
        (0.0, 0.0),
        viewport,
        &plans,
        image_cache,
        &mut Flattener::default(),
        &mut out,
    );
    out
}

/// Fade a linear straight-alpha color by `alpha`.
fn fade(mut color: [f32; 4], alpha: f32) -> [f32; 4] {
    color[3] *= alpha;
    color
}

fn fade_color(color: quark::Color, alpha: f32) -> quark::Color {
    if alpha >= 1.0 {
        return color;
    }
    quark::Color {
        a: (f32::from(color.a) * alpha).round() as u8,
        ..color
    }
}

/// Flatten `prims[range]` into one target's draw steps. Coordinates have
/// `origin` subtracted, so an offscreen layer draws its content at its own
/// texture origin. Layers draw per their plan: skipped, inline, or as one
/// composite of their texture (their content is a target of its own).
#[allow(clippy::too_many_arguments)]
fn flatten_scene_into(
    prims: &[Primitive],
    range: std::ops::Range<usize>,
    origin: (f32, f32),
    viewport: Rect,
    plans: &[LayerPlan],
    image_cache: &ImageCache,
    fl: &mut Flattener,
    out: &mut FlattenedScene,
) {
    out.clear();
    fl.active = 0;
    fl.seq = 0;
    fl.clips.clear();
    fl.clips.push(ActiveClip::root(viewport));
    fl.z_stack.clear();
    fl.z_stack.push(0);
    fl.origin = origin;
    fl.alpha = 1.0;
    fl.inline.clear();

    let mut index = range.start;
    while index < range.end {
        let primitive = &prims[index];
        index += 1;
        match primitive {
            Primitive::Rect(rect) => push_quad(
                fl.shift(rect.rect),
                fade(color_to_linear(rect.color), fl.alpha),
                [0.0; 4],
                [0.0; 4],
                [0.0; 4],
                fl,
                &mut out.quads,
            ),
            Primitive::RoundedRect(rect) => push_quad(
                fl.shift(rect.rect),
                fade(color_to_linear(rect.color), fl.alpha),
                [0.0; 4],
                rect.corner_radii,
                [0.0; 4],
                fl,
                &mut out.quads,
            ),
            Primitive::Border(border) => push_quad(
                fl.shift(border.rect),
                [0.0; 4],
                fade(color_to_linear(border.color), fl.alpha),
                border.corner_radii,
                border.widths,
                fl,
                &mut out.quads,
            ),
            Primitive::Shadow(shadow) => {
                let sigma = (shadow.blur_radius * 0.5).max(0.5);
                let expansion = sigma * 3.0;
                let rect = fl.shift(shadow.rect);
                let offset_x = shadow.offset[0];
                let offset_y = shadow.offset[1];
                let expanded = Rect {
                    x: rect.x + offset_x - expansion,
                    y: rect.y + offset_y - expansion,
                    width: rect.width + expansion * 2.0,
                    height: rect.height + expansion * 2.0,
                };
                let clip = fl.clip();
                if let Some(bounds) = expanded.intersection(clip.scissor) {
                    let key = fl.place(PrimKind::Shadow, bounds);
                    out.shadows.push(ClippedShadow {
                        key,
                        instance: ShadowInstance {
                            draw_bounds: [expanded.x, expanded.y, expanded.width, expanded.height],
                            shadow_bounds: [
                                rect.x + offset_x,
                                rect.y + offset_y,
                                rect.width,
                                rect.height,
                            ],
                            color: fade(color_to_linear(shadow.color), fl.alpha),
                            params: [sigma, shadow.corner_radius, 0.0, 0.0],
                            clip_bounds: clip.clip_bounds_attr(),
                            clip_radii: clip.clip_radii_attr(),
                        },
                        clip: clip.scissor,
                    });
                }
            }
            Primitive::TextRun(text) => {
                let clip = fl.clip();
                let rect = fl.shift(text.rect);
                if let Some(intersection) = rect.intersection(clip.scissor) {
                    let key = fl.place(PrimKind::Text, intersection);
                    out.texts.push(ClippedText {
                        key,
                        primitive: TextPrimitive {
                            rect,
                            layout: text.layout.clone(),
                            color: fade_color(text.color, fl.alpha),
                        },
                        clip: intersection,
                    });
                }
            }
            Primitive::RichTextRun(text) => {
                let clip = fl.clip();
                let rect = fl.shift(text.rect);
                if let Some(intersection) = rect.intersection(clip.scissor) {
                    let key = fl.place(PrimKind::Text, intersection);
                    let mut primitive = RichTextPrimitive {
                        rect,
                        ..text.clone()
                    };
                    if fl.alpha < 1.0 {
                        let alpha = fl.alpha;
                        primitive.default_color = fade_color(text.default_color, alpha);
                        primitive.span_colors = text
                            .span_colors
                            .iter()
                            .map(|c| fade_color(*c, alpha))
                            .collect();
                    }
                    out.rich_texts.push(ClippedRichText {
                        key,
                        primitive,
                        clip: intersection,
                    });
                }
            }
            Primitive::BlurRegion(blur) => {
                let clip = fl.clip();
                if let Some(rect) = fl.shift(blur.rect).intersection(clip.scissor) {
                    fl.barrier(FlattenedBlurRegion {
                        rect,
                        blur_radius: blur.blur_radius,
                        corner_radii: blur.corner_radii,
                    });
                }
            }
            Primitive::EffectQuad(effect) => {
                let clip = fl.clip();
                let rect = fl.shift(effect.rect);
                if let Some(bounds) = rect.intersection(clip.scissor) {
                    let key = fl.place(PrimKind::Effect, bounds);
                    out.effect_quads.push(ClippedEffectQuad {
                        key,
                        instance: EffectQuadInstance {
                            bounds: [rect.x, rect.y, rect.width, rect.height],
                            color_a: fade(color_to_linear(effect.color_a), fl.alpha),
                            color_b: fade(color_to_linear(effect.color_b), fl.alpha),
                            params: [
                                effect.effect_type as u32 as f32,
                                effect.params[0],
                                effect.params[1],
                                effect.corner_radius,
                            ],
                            clip_bounds: clip.clip_bounds_attr(),
                            clip_radii: clip.clip_radii_attr(),
                        },
                        clip: clip.scissor,
                    });
                }
            }
            Primitive::Image(img) => {
                let clip = fl.clip();
                let rect = fl.shift(img.rect);
                if let Some(bounds) = rect.intersection(clip.scissor) {
                    let key = fl.place(PrimKind::Image, bounds);
                    out.images.push(ClippedImage {
                        key,
                        primitive: crate::scene::ImagePrimitive {
                            rect,
                            ..img.clone()
                        },
                        clip: clip.scissor,
                        alpha: fl.alpha,
                    });
                }
            }
            Primitive::Icon(icon) => {
                let clip = fl.clip();
                let shifted = fl.shift(icon.rect);
                let rect = crate::Rect {
                    x: shifted.x.round(),
                    y: shifted.y.round(),
                    width: shifted.width.round(),
                    height: shifted.height.round(),
                };
                if let Some(bounds) = rect.intersection(clip.scissor) {
                    let px_size = icon.rect.width.max(icon.rect.height).ceil() as u32;
                    let cache_key = crate::icons::cache_key(&icon.name, px_size, icon.color);
                    // Only rasterize (and copy RGBA out of the icon cache)
                    // when the texture is not on the GPU yet; once
                    // uploaded, the cache key alone is enough to draw.
                    let (rgba, w, h) = if image_cache.contains_key(&cache_key) {
                        (empty_rgba(), 0, 0)
                    } else {
                        crate::icons::rasterize_svg(&icon.name, px_size, icon.color)
                    };
                    let key = fl.place(PrimKind::Image, bounds);
                    out.images.push(ClippedImage {
                        key,
                        primitive: crate::scene::ImagePrimitive {
                            rect,
                            width: w,
                            height: h,
                            rgba,
                            cache_key,
                        },
                        clip: clip.scissor,
                        alpha: fl.alpha,
                    });
                }
            }
            Primitive::Path(path) => push_path(path, fl, &mut out.paths),
            Primitive::ClipStart(ClipPrimitive { rect, corner_radii }) => {
                let rect = fl.shift(*rect);
                let next = fl
                    .clips
                    .last()
                    .and_then(|clip| clip.push(rect, *corner_radii))
                    .unwrap_or(ActiveClip {
                        scissor: Rect::default(),
                        rounded_rect: Rect::default(),
                        corner_radii: [0.0; 4],
                    });
                fl.clips.push(next);
            }
            Primitive::ClipEnd => {
                if fl.clips.len() > 1 {
                    fl.clips.pop();
                }
            }
            Primitive::ZIndexPush(z) => fl.z_stack.push(*z),
            Primitive::ZIndexPop => {
                if fl.z_stack.len() > 1 {
                    fl.z_stack.pop();
                }
            }
            Primitive::LayerStart(_) => {
                let Ok(at) = plans.binary_search_by_key(&(index - 1), |plan| plan.start) else {
                    continue;
                };
                let plan = &plans[at];
                match plan.mode {
                    LayerMode::Skip => index = plan.end + 1,
                    LayerMode::Inline { dx, dy, alpha } => {
                        fl.inline.push((fl.origin, fl.alpha));
                        fl.origin = (fl.origin.0 - dx, fl.origin.1 - dy);
                        fl.alpha *= alpha;
                    }
                    LayerMode::Offscreen => {
                        push_layer(plan, fl, &mut out.layers);
                        index = plan.end + 1;
                    }
                }
            }
            Primitive::LayerEnd => {
                if let Some((origin, alpha)) = fl.inline.pop() {
                    fl.origin = origin;
                    fl.alpha = alpha;
                }
            }
            Primitive::LayerBoundary => {}
        }
    }

    // Put every kind's array into draw order, then walk the z-layers'
    // segment lists to slice those arrays into steps.
    out.shadows.sort_unstable_by_key(|item| item.key);
    out.effect_quads.sort_unstable_by_key(|item| item.key);
    out.quads.sort_unstable_by_key(|item| item.key);
    out.images.sort_unstable_by_key(|item| item.key);
    out.texts.sort_unstable_by_key(|item| item.key);
    out.rich_texts.sort_unstable_by_key(|item| item.key);
    out.paths.sort_by_key(|item| item.key);
    out.layers.sort_unstable_by_key(|item| item.key);

    let builders = &mut fl.builders[..fl.active];
    builders.sort_unstable_by_key(|builder| builder.z);
    let mut cursors = [0usize; KIND_COUNT];
    let mut rich_cursor = 0usize;
    for builder in builders.iter() {
        for (segment, slot) in builder.segments.iter().enumerate() {
            let (z, segment) = (builder.z, segment as u32);
            match *slot {
                SegmentSlot::Blur(region) => out.steps.push(DrawStep::Blur(region)),
                SegmentSlot::Batch(kind) => {
                    let cursor = &mut cursors[kind as usize];
                    let items = match kind {
                        PrimKind::Shadow => take_run(&out.shadows, cursor, z, segment, |i| i.key),
                        PrimKind::Effect => {
                            take_run(&out.effect_quads, cursor, z, segment, |i| i.key)
                        }
                        PrimKind::Quad => take_run(&out.quads, cursor, z, segment, |i| i.key),
                        PrimKind::Image => take_run(&out.images, cursor, z, segment, |i| i.key),
                        PrimKind::Text => take_run(&out.texts, cursor, z, segment, |i| i.key),
                        PrimKind::Path => take_run(&out.paths, cursor, z, segment, |i| i.key),
                        PrimKind::Layer => take_run(&out.layers, cursor, z, segment, |i| i.key),
                    };
                    let rich = if kind == PrimKind::Text {
                        take_run(&out.rich_texts, &mut rich_cursor, z, segment, |i| i.key)
                    } else {
                        0..0
                    };
                    if !items.is_empty() || !rich.is_empty() {
                        out.steps.push(DrawStep::Batch { kind, items, rich });
                    }
                }
            }
        }
    }
}

/// Append the bands of `path`'s fill, then its stroke.
fn push_path(path: &crate::scene::PathPrimitive, fl: &mut Flattener, out: &mut Vec<ClippedPath>) {
    let clip = fl.clip();
    let Some(visible) = fl.shift(path.bounds()).intersection(clip.scissor) else {
        return;
    };
    if path.path.is_empty() || !(path.scale.is_finite() && path.scale > 0.0) {
        return;
    }
    let origin = [path.origin[0] - fl.origin.0, path.origin[1] - fl.origin.1];
    let geometry = to_kurbo(&path.path, path.scale);
    let fill = path
        .fill
        .map(|fill| (geometry.clone(), fill.color, rule_code(fill.rule)));
    let stroke = path
        .stroke
        .filter(|stroke| stroke.style.width > 0.0)
        .map(|stroke| {
            let outline = stroke_outline(&geometry, &stroke.style, path.scale);
            // Stroke outlines wind consistently and are meant for nonzero.
            (outline, stroke.color, 0.0)
        });
    for (outline, color, rule) in [fill, stroke].into_iter().flatten() {
        fl.bands.clear();
        let Flattener {
            segments,
            bands,
            scratch,
            ..
        } = &mut *fl;
        if !push_bands(&outline, segments, bands, scratch) {
            continue;
        }
        // Fill and stroke each take their own key, so the stroke draws on
        // top; the bands of one shape never overlap.
        let key = fl.place(PrimKind::Path, visible);
        let color = fade(color_to_linear(color), fl.alpha);
        for band in &fl.bands {
            let rect = band.rect.offset(origin[0], origin[1]);
            if rect.intersection(clip.scissor).is_none() {
                continue;
            }
            out.push(ClippedPath {
                key,
                instance: PathInstance {
                    bounds: [rect.x, rect.y, rect.width, rect.height],
                    origin_rule: [origin[0], origin[1], rule, 0.0],
                    color,
                    segments: [band.start, band.count, 0, 0],
                    clip_bounds: clip.clip_bounds_attr(),
                    clip_radii: clip.clip_radii_attr(),
                },
                clip: clip.scissor,
            });
        }
    }
}

/// Append the composite of offscreen layer `plan` into the current target.
fn push_layer(plan: &LayerPlan, fl: &mut Flattener, out: &mut Vec<ClippedLayer>) {
    let map = plan
        .composite()
        .then(Transform2D::translate(-fl.origin.0, -fl.origin.1));
    let (w, h) = (plan.size.0 as f32, plan.size.1 as f32);
    let clip = fl.clip();
    let footprint = map.map_rect_bounds(Rect {
        x: 0.0,
        y: 0.0,
        width: w,
        height: h,
    });
    let Some(bounds) = footprint.intersection(clip.scissor) else {
        return;
    };
    let key = fl.place(PrimKind::Layer, bounds);
    out.push(ClippedLayer {
        key,
        instance: LayerInstance {
            linear: [map.a, map.b, map.c, map.d],
            offset_size: [map.tx, map.ty, w, h],
            params: [
                1.0 / plan.texture.0.max(1) as f32,
                1.0 / plan.texture.1.max(1) as f32,
                plan.opacity * fl.alpha,
                0.0,
            ],
            clip_bounds: clip.clip_bounds_attr(),
            clip_radii: clip.clip_radii_attr(),
        },
        clip: clip.scissor,
        target: plan.target,
    });
}

// ---------------------------------------------------------------------------
// Layer planning
// ---------------------------------------------------------------------------
//
// A layer fades and transforms its content as one group. Most layers never
// touch a texture: one at zero opacity (or with a degenerate transform) is
// skipped, an opaque one that only translates draws in place at whole
// pixels, and a translated one with a single primitive draws in place with
// that primitive's alpha scaled (one primitive cannot show through itself).
// Everything else renders into a pooled offscreen texture sized to its
// visible content plus a transparent pixel, then composites as one textured
// quad through the layer's transform with its opacity.

/// Opacity below this skips a layer; above one minus it, a layer is opaque.
const ALPHA_EPSILON: f32 = 1.0 / 512.0;

/// Largest side of a layer texture; larger content is cut off.
const MAX_LAYER_SIDE: f32 = 8192.0;

#[derive(Debug, Clone, Copy, PartialEq)]
enum LayerMode {
    Skip,
    /// Draw the content in place, moved by whole pixels and faded.
    Inline {
        dx: f32,
        dy: f32,
        alpha: f32,
    },
    Offscreen,
}

#[derive(Debug, Clone, Copy)]
struct LayerPlan {
    /// Index of the `LayerStart` in the scene.
    start: usize,
    /// Index of the matching `LayerEnd` (the scene length when missing).
    end: usize,
    mode: LayerMode,
    opacity: f32,
    transform: Transform2D,
    /// Whole-pixel origin, in the layer's content coordinates, and size of
    /// the region an offscreen layer renders.
    origin: (f32, f32),
    size: (u32, u32),
    /// Size of the pooled texture it renders into (at least `size`).
    texture: (u32, u32),
    /// Frame index of an offscreen layer's target.
    target: usize,
}

impl LayerPlan {
    /// Map from layer texels to the parent's content coordinates.
    fn composite(&self) -> Transform2D {
        Transform2D::translate(self.origin.0, self.origin.1).then(self.transform)
    }
}

struct OpenLayer {
    plan: usize,
    clip_depth: usize,
    /// Visible content bounds so far, in the layer's content coordinates.
    bounds: Option<Rect>,
    /// Primitives drawn in the layer, with nested layers counting as many.
    drawables: u32,
}

/// Plan every layer of `scene` (in scene order) and number the offscreen
/// ones' targets from 1. Returns how many render offscreen.
fn plan_layers(scene: &Scene, viewport: Rect, plans: &mut Vec<LayerPlan>) -> usize {
    plans.clear();
    let prims = &scene.primitives;
    if !prims.iter().any(|p| matches!(p, Primitive::LayerStart(_))) {
        return 0;
    }
    let mut open: Vec<OpenLayer> = Vec::new();
    // Clip rects in the coordinates of the innermost open layer's content.
    let mut clips = vec![viewport];
    for (index, primitive) in prims.iter().enumerate() {
        match primitive {
            Primitive::LayerStart(layer) => {
                let outer = *clips.last().expect("root clip");
                let content_clip = layer
                    .transform
                    .invert()
                    .map_or(Rect::default(), |inverse| inverse.map_rect_bounds(outer));
                let opacity = if layer.opacity.is_nan() {
                    0.0
                } else {
                    layer.opacity.clamp(0.0, 1.0)
                };
                plans.push(LayerPlan {
                    start: index,
                    end: prims.len(),
                    mode: LayerMode::Skip,
                    opacity,
                    transform: layer.transform,
                    origin: (0.0, 0.0),
                    size: (0, 0),
                    texture: (0, 0),
                    target: 0,
                });
                open.push(OpenLayer {
                    plan: plans.len() - 1,
                    clip_depth: clips.len(),
                    bounds: None,
                    drawables: 0,
                });
                clips.push(content_clip);
            }
            Primitive::LayerEnd => {
                if let Some(layer) = open.pop() {
                    close_layer(layer, index, plans, &mut open, &mut clips);
                }
            }
            Primitive::ClipStart(clip) => {
                let top = *clips.last().expect("root clip");
                clips.push(top.intersection(clip.rect).unwrap_or_default());
            }
            Primitive::ClipEnd => {
                let floor = open.last().map_or(1, |layer| layer.clip_depth + 1);
                if clips.len() > floor {
                    clips.pop();
                }
            }
            _ => {
                if let Some(layer) = open.last_mut()
                    && let Some(bounds) = paint_bounds(primitive)
                {
                    layer.drawables += 1;
                    if let Some(visible) = bounds.intersection(*clips.last().expect("root clip")) {
                        layer.bounds =
                            Some(layer.bounds.map_or(visible, |b| rect_union(b, visible)));
                    }
                }
            }
        }
    }
    while let Some(layer) = open.pop() {
        close_layer(layer, prims.len(), plans, &mut open, &mut clips);
    }
    let mut targets = 0;
    for plan in plans.iter_mut() {
        if plan.mode == LayerMode::Offscreen {
            targets += 1;
            plan.target = targets;
        }
    }
    targets
}

fn close_layer(
    layer: OpenLayer,
    end: usize,
    plans: &mut [LayerPlan],
    open: &mut [OpenLayer],
    clips: &mut Vec<Rect>,
) {
    clips.truncate(layer.clip_depth);
    let plan = &mut plans[layer.plan];
    plan.end = end;
    plan.mode = layer_mode(plan, layer.bounds, layer.drawables);
    let landed = match plan.mode {
        LayerMode::Skip => {
            // Layers nested in a skipped one are never reached.
            for nested in &mut plans[layer.plan + 1..] {
                nested.mode = LayerMode::Skip;
            }
            None
        }
        LayerMode::Inline { dx, dy, .. } => layer.bounds.map(|b| b.offset(dx, dy)),
        LayerMode::Offscreen => Some(plan.composite().map_rect_bounds(Rect {
            x: 0.0,
            y: 0.0,
            width: plan.size.0 as f32,
            height: plan.size.1 as f32,
        })),
    };
    if let Some(parent) = open.last_mut()
        && let Some(landed) = landed
    {
        // A nested group can overlap itself, so its parent cannot fade it
        // per primitive.
        parent.drawables += 2;
        if let Some(visible) = landed.intersection(*clips.last().expect("root clip")) {
            parent.bounds = Some(parent.bounds.map_or(visible, |b| rect_union(b, visible)));
        }
    }
}

fn layer_mode(plan: &mut LayerPlan, bounds: Option<Rect>, drawables: u32) -> LayerMode {
    let Some(bounds) = bounds.filter(|b| {
        b.x.is_finite() && b.y.is_finite() && b.width.is_finite() && b.height.is_finite()
    }) else {
        return LayerMode::Skip;
    };
    if plan.opacity <= ALPHA_EPSILON || plan.transform.invert().is_none() {
        return LayerMode::Skip;
    }
    let transform = plan.transform;
    if transform.is_translation() {
        // Whole-pixel moves keep text and edges as sharp as unmoved ones.
        let (dx, dy) = (transform.tx.round(), transform.ty.round());
        plan.transform = Transform2D::translate(dx, dy);
        if plan.opacity >= 1.0 - ALPHA_EPSILON {
            return LayerMode::Inline { dx, dy, alpha: 1.0 };
        }
        if drawables <= 1 {
            return LayerMode::Inline {
                dx,
                dy,
                alpha: plan.opacity,
            };
        }
    }
    // A transparent pixel around the content lets bilinear sampling
    // antialias the edges of a rotated or scaled layer.
    let x0 = bounds.x.floor() - 1.0;
    let y0 = bounds.y.floor() - 1.0;
    let width = (bounds.right().ceil() + 1.0 - x0).clamp(1.0, MAX_LAYER_SIDE);
    let height = (bounds.bottom().ceil() + 1.0 - y0).clamp(1.0, MAX_LAYER_SIDE);
    plan.origin = (x0, y0);
    plan.size = (width as u32, height as u32);
    LayerMode::Offscreen
}

/// Scene bounds a primitive can paint, or `None` for state changes.
fn paint_bounds(primitive: &Primitive) -> Option<Rect> {
    // Glyphs can overhang their layout box (italics, accents).
    const GLYPH_OVERHANG: f32 = 4.0;
    match primitive {
        Primitive::Rect(p) => Some(p.rect),
        Primitive::RoundedRect(p) => Some(p.rect),
        Primitive::Border(p) => Some(p.rect),
        Primitive::Shadow(p) => {
            let reach = (p.blur_radius * 0.5).max(0.5) * 3.0;
            Some(Rect {
                x: p.rect.x + p.offset[0] - reach,
                y: p.rect.y + p.offset[1] - reach,
                width: p.rect.width + reach * 2.0,
                height: p.rect.height + reach * 2.0,
            })
        }
        Primitive::TextRun(p) => Some(p.rect.inset(-GLYPH_OVERHANG)),
        Primitive::RichTextRun(p) => Some(p.rect.inset(-GLYPH_OVERHANG)),
        Primitive::Icon(p) => Some(p.rect),
        Primitive::Image(p) => Some(p.rect),
        Primitive::EffectQuad(p) => Some(p.rect),
        Primitive::BlurRegion(p) => Some(p.rect),
        Primitive::Path(p) => Some(p.bounds()),
        Primitive::ClipStart(_)
        | Primitive::ClipEnd
        | Primitive::ZIndexPush(_)
        | Primitive::ZIndexPop
        | Primitive::LayerStart(_)
        | Primitive::LayerEnd
        | Primitive::LayerBoundary => None,
    }
}

/// Shared empty pixel buffer for icons whose texture is already uploaded.
fn empty_rgba() -> Arc<[u8]> {
    static EMPTY: std::sync::LazyLock<Arc<[u8]>> = std::sync::LazyLock::new(|| Arc::from([]));
    EMPTY.clone()
}

/// Advance `cursor` over the items keyed to (`z`, `segment`) and return them.
fn take_run<T>(
    items: &[T],
    cursor: &mut usize,
    z: i32,
    segment: u32,
    key: impl Fn(&T) -> DrawKey,
) -> std::ops::Range<u32> {
    let start = *cursor;
    while *cursor < items.len() {
        let k = key(&items[*cursor]);
        if k.z != z || k.segment != segment {
            break;
        }
        *cursor += 1;
    }
    start as u32..*cursor as u32
}

fn push_quad(
    rect: Rect,
    background: [f32; 4],
    border_color: [f32; 4],
    corner_radii: [f32; 4],
    border_widths: [f32; 4],
    fl: &mut Flattener,
    out: &mut Vec<ClippedQuad>,
) {
    let clip = *fl.clips.last().expect("root clip");
    if let Some(bounds) = rect.intersection(clip.scissor) {
        let key = fl.place(PrimKind::Quad, bounds);
        out.push(ClippedQuad {
            key,
            instance: QuadInstance {
                bounds: [rect.x, rect.y, rect.width, rect.height],
                background,
                border_color,
                corner_radii,
                border_widths,
                clip_bounds: clip.clip_bounds_attr(),
                clip_radii: clip.clip_radii_attr(),
            },
            clip: clip.scissor,
        });
    }
}

// ---------------------------------------------------------------------------
// GPU batches
// ---------------------------------------------------------------------------

/// Per-frame instance data in draw order, one array per pipeline. Kept on the
/// renderer so the vectors keep their capacity between frames.
#[derive(Default)]
struct FrameBatches {
    shadows: Vec<ShadowInstance>,
    effects: Vec<EffectQuadInstance>,
    quads: Vec<QuadInstance>,
    images: Vec<BlitInstance>,
    paths: Vec<PathInstance>,
    layers: Vec<LayerInstance>,
    /// Scissor batches for shadow, effect, quad, and path steps. Each step's
    /// range indexes the instance array of its own kind.
    quad_cmds: Vec<QuadDrawCommand>,
    image_cmds: Vec<ImageDrawCommand>,
    layer_cmds: Vec<LayerDrawCommand>,
    /// Per step: a range into `quad_cmds` or `image_cmds`, or for text the
    /// index of its text renderer (`start`).
    step_cmds: Vec<std::ops::Range<u32>>,
    text_steps: usize,
}

fn build_batches(flat: &FlattenedScene, out: &mut FrameBatches) {
    out.shadows.clear();
    out.shadows
        .extend(flat.shadows.iter().map(|item| item.instance));
    out.effects.clear();
    out.effects
        .extend(flat.effect_quads.iter().map(|item| item.instance));
    out.quads.clear();
    out.quads
        .extend(flat.quads.iter().map(|item| item.instance));
    out.images.clear();
    out.images.extend(flat.images.iter().map(|item| {
        let r = item.primitive.rect;
        BlitInstance {
            bounds: [r.x, r.y, r.width, r.height],
            uv_rect: [0.0, 0.0, 1.0, 1.0],
            tint: [item.alpha; 4],
            radii: [0.0; 4],
        }
    }));
    out.paths.clear();
    out.paths
        .extend(flat.paths.iter().map(|item| item.instance));
    out.layers.clear();
    out.layers
        .extend(flat.layers.iter().map(|item| item.instance));
    out.quad_cmds.clear();
    out.image_cmds.clear();
    out.layer_cmds.clear();
    out.step_cmds.clear();
    out.text_steps = 0;

    for step in &flat.steps {
        let range = match step {
            DrawStep::Blur(_) => 0..0,
            DrawStep::Batch { kind, items, .. } => {
                let (start, end) = (items.start as usize, items.end as usize);
                match kind {
                    PrimKind::Shadow => push_clip_batches(
                        flat.shadows[start..end].iter().map(|i| i.clip),
                        items.start,
                        &mut out.quad_cmds,
                    ),
                    PrimKind::Effect => push_clip_batches(
                        flat.effect_quads[start..end].iter().map(|i| i.clip),
                        items.start,
                        &mut out.quad_cmds,
                    ),
                    PrimKind::Quad => push_clip_batches(
                        flat.quads[start..end].iter().map(|i| i.clip),
                        items.start,
                        &mut out.quad_cmds,
                    ),
                    PrimKind::Path => push_clip_batches(
                        flat.paths[start..end].iter().map(|i| i.clip),
                        items.start,
                        &mut out.quad_cmds,
                    ),
                    PrimKind::Layer => {
                        let first = out.layer_cmds.len() as u32;
                        for (offset, layer) in flat.layers[start..end].iter().enumerate() {
                            out.layer_cmds.push(LayerDrawCommand {
                                instance: items.start + offset as u32,
                                clip: layer.clip,
                                target: layer.target,
                            });
                        }
                        first..out.layer_cmds.len() as u32
                    }
                    PrimKind::Image => {
                        let first = out.image_cmds.len();
                        for (offset, image) in flat.images[start..end].iter().enumerate() {
                            let index = items.start + offset as u32;
                            let key = image.primitive.cache_key;
                            let own = out.image_cmds.len() > first;
                            match out.image_cmds.last_mut() {
                                Some(last)
                                    if own
                                        && last.cache_key == key
                                        && rects_equal(last.clip, image.clip) =>
                                {
                                    last.instance_end = index + 1;
                                }
                                _ => out.image_cmds.push(ImageDrawCommand {
                                    instance_start: index,
                                    instance_end: index + 1,
                                    cache_key: key,
                                    clip: image.clip,
                                }),
                            }
                        }
                        first as u32..out.image_cmds.len() as u32
                    }
                    PrimKind::Text => {
                        out.text_steps += 1;
                        (out.text_steps - 1) as u32..out.text_steps as u32
                    }
                }
            }
        };
        out.step_cmds.push(range);
    }
}

/// Append scissor batches for consecutive instances that share a clip and
/// return the range of commands added.
fn push_clip_batches(
    clips: impl Iterator<Item = Rect>,
    first_instance: u32,
    out: &mut Vec<QuadDrawCommand>,
) -> std::ops::Range<u32> {
    let first = out.len();
    for (index, clip) in (first_instance..).zip(clips) {
        let own = out.len() > first;
        match out.last_mut() {
            Some(last) if own && rects_equal(last.clip, clip) => {
                last.instance_end = index + 1;
            }
            _ => out.push(QuadDrawCommand {
                instance_start: index,
                instance_end: index + 1,
                clip,
            }),
        }
    }
    first as u32..out.len() as u32
}

fn rects_equal(a: Rect, b: Rect) -> bool {
    a.x == b.x && a.y == b.y && a.width == b.width && a.height == b.height
}

fn rects_overlap(a: Rect, b: Rect) -> bool {
    a.x < b.right() && b.x < a.right() && a.y < b.bottom() && b.y < a.bottom()
}

fn rect_union(a: Rect, b: Rect) -> Rect {
    let x = a.x.min(b.x);
    let y = a.y.min(b.y);
    Rect {
        x,
        y,
        width: a.right().max(b.right()) - x,
        height: a.bottom().max(b.bottom()) - y,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::ShapedText;
    use crate::text::test_text;
    use quark_text::{TextParams, TextStyle};

    #[test]
    fn scissor_rect_clamps_to_target_and_rejects_degenerate_clips() {
        let cases = [
            (rect(807.0, 118.0, 844.0, 865.0), Some((807, 118, 843, 865))),
            (rect(-10.0, -5.0, 20.0, 10.0), Some((0, 0, 10, 5))),
            (rect(1650.0, 118.0, 10.0, 865.0), None),
            (rect(10.0, 10.0, -5.0, 5.0), None),
            (rect(f32::NAN, 0.0, 10.0, 10.0), None),
            (rect(0.0, 0.0, f32::INFINITY, 10.0), None),
        ];
        for (clip, expected) in cases {
            assert_eq!(scissor_rect(clip, 1650, 1050), expected, "{clip:?}");
        }
    }

    // A rectangular clip inside a rounded one must keep the rounded corners
    // for its children, or a list inside a rounded card paints square
    // corners over the card's.
    #[test]
    fn render_child_of_rect_clip_inside_rounded_clip_keeps_rounded_corners() {
        let area = rect(0.0, 0.0, 40.0, 40.0);
        let mut scene = Scene::default();
        scene.push(Primitive::ClipStart(ClipPrimitive {
            rect: area,
            corner_radii: [12.0; 4],
        }));
        scene.push(Primitive::ClipStart(ClipPrimitive {
            rect: area,
            corner_radii: [0.0; 4],
        }));
        scene.push(solid(area, 255, 255, 255));
        scene.push(Primitive::ClipEnd);
        scene.push(Primitive::ClipEnd);
        let Some(image) = render_pixels(&scene, 40, 40) else {
            return;
        };
        assert_eq!(
            image.get_pixel(1, 1).0,
            [0, 0, 0, 255],
            "corner not clipped"
        );
        assert_eq!(image.get_pixel(20, 20).0, [255, 255, 255, 255]);
    }

    // -- Headless GPU helpers ------------------------------------------------

    /// Headless renderer, or `None` when no adapter exists (a failure when
    /// `QUARK_REQUIRE_GPU` is set).
    fn gpu_renderer(width: u32, height: u32) -> Option<Renderer> {
        match Renderer::new_headless(width, height, 1.0) {
            Ok(renderer) => Some(renderer),
            Err(RenderError::NoAdapter) => {
                assert!(
                    std::env::var_os("QUARK_REQUIRE_GPU").is_none(),
                    "QUARK_REQUIRE_GPU is set but no wgpu adapter is available"
                );
                None
            }
            Err(error) => panic!("headless renderer failed: {error}"),
        }
    }

    fn render_pixels(scene: &Scene, width: u32, height: u32) -> Option<image::RgbaImage> {
        render_pixels_with(scene, &mut test_text(), width, height)
    }

    fn render_pixels_with(
        scene: &Scene,
        text: &mut TextSystem,
        width: u32,
        height: u32,
    ) -> Option<image::RgbaImage> {
        let mut renderer = gpu_renderer(width, height)?;
        let pixels = renderer
            .render_to_rgba(scene, text, width, height)
            .expect("offscreen render");
        Some(image::RgbaImage::from_raw(width, height, pixels).expect("pixel buffer size"))
    }

    fn rect(x: f32, y: f32, width: f32, height: f32) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    fn white_text(rect: Rect, text: &str) -> Primitive {
        let params = TextParams::new(text, TextStyle::new(16.0));
        let layout = test_text().layout(&params).expect("layout");
        Primitive::TextRun(TextPrimitive {
            rect,
            layout: ShapedText::new(Arc::new(layout)),
            color: quark::Color::rgba(255, 255, 255, 255),
        })
    }

    fn solid(rect: Rect, r: u8, g: u8, b: u8) -> Primitive {
        Primitive::Rect(crate::scene::RectPrimitive {
            rect,
            color: quark::Color::rgba(r, g, b, 255),
        })
    }

    fn any_light_pixel(image: &image::RgbaImage, area: Rect) -> bool {
        let (x0, y0) = (area.x as u32, area.y as u32);
        let (x1, y1) = (area.right() as u32, area.bottom() as u32);
        (y0..y1).any(|y| {
            (x0..x1).any(|x| {
                let p = image.get_pixel(x, y).0;
                p[0] > 128 && p[1] > 128 && p[2] > 128
            })
        })
    }

    // -- Draw order ------------------------------------------------------------

    // Regression: each z-layer drew all quads, then images, then text, so a
    // quad painted after text (selection highlight, overlay) sat under it.
    #[test]
    fn render_quad_painted_after_text_covers_text() {
        let band = rect(4.0, 4.0, 120.0, 24.0);
        let mut scene = Scene::default();
        scene.push(white_text(band, "WWWWWWWW"));
        scene.push(solid(band, 200, 0, 0));
        let Some(image) = render_pixels(&scene, 128, 32) else {
            return;
        };
        assert!(
            !any_light_pixel(&image, band),
            "text drew over the quad painted after it"
        );
    }

    // Guards the single-encoder path: every text segment has its own text
    // renderer, so preparing a later z-layer's text must not erase earlier text.
    #[test]
    fn render_text_in_two_z_layers_draws_both() {
        let low = rect(4.0, 4.0, 120.0, 24.0);
        let high = rect(4.0, 36.0, 120.0, 24.0);
        let mut scene = Scene::default();
        scene.push(white_text(low, "WWWWWWWW"));
        scene.push(Primitive::ZIndexPush(1));
        scene.push(white_text(high, "WWWWWWWW"));
        scene.push(Primitive::ZIndexPop);
        let Some(image) = render_pixels(&scene, 128, 64) else {
            return;
        };
        assert!(any_light_pixel(&image, low), "z=0 text is missing");
        assert!(any_light_pixel(&image, high), "z=1 text is missing");
    }

    fn count_pixels(image: &image::RgbaImage, area: Rect, pred: impl Fn([u8; 4]) -> bool) -> usize {
        let (x0, y0) = (area.x as u32, area.y as u32);
        let (x1, y1) = (area.right() as u32, area.bottom() as u32);
        (y0..y1)
            .flat_map(|y| (x0..x1).map(move |x| (x, y)))
            .filter(|&(x, y)| pred(image.get_pixel(x, y).0))
            .count()
    }

    // The renderer bakes span colors into a copy of the layout's buffer; each
    // span must keep its own color and the other span's glyphs must not
    // bleed over.
    #[test]
    fn render_rich_text_paints_each_span_in_its_color() {
        let text = "WWWWWWMMMMMM";
        let params = TextParams::new(text, TextStyle::new(20.0)).spans(vec![
            quark_text::TextSpan {
                range: 0..6,
                weight: None,
                style: None,
                kind: None,
            },
            quark_text::TextSpan {
                range: 6..12,
                weight: None,
                style: None,
                kind: None,
            },
        ]);
        let layout = test_text().layout(&params).expect("layout");
        let split = layout.caret(6).x;
        let mut scene = Scene::default();
        scene.rich_text(RichTextPrimitive {
            rect: rect(0.0, 0.0, 200.0, 32.0),
            layout: ShapedText::new(Arc::new(layout)),
            default_color: quark::Color::rgba(255, 255, 255, 255),
            span_colors: Arc::from([
                quark::Color::rgba(255, 0, 0, 255),
                quark::Color::rgba(0, 255, 0, 255),
            ]),
        });
        let Some(image) = render_pixels(&scene, 200, 32) else {
            return;
        };
        let red = |p: [u8; 4]| p[0] > 120 && p[1] < 40;
        let green = |p: [u8; 4]| p[1] > 120 && p[0] < 40;
        // One pixel of slack each side: area bounds round to whole pixels.
        let left = rect(0.0, 0.0, split.floor() - 1.0, 32.0);
        let right = rect(split.ceil() + 1.0, 0.0, 198.0 - split.ceil(), 32.0);
        assert!(count_pixels(&image, left, red) > 20, "first span not red");
        assert!(
            count_pixels(&image, right, green) > 20,
            "second span not green"
        );
        assert_eq!(count_pixels(&image, left, green), 0, "green left of split");
        assert_eq!(count_pixels(&image, right, red), 0, "red right of split");
    }

    // Regression: rich text drew one glyphon area per same-colored stretch of
    // each line, and glyphon walks the buffer's lines for every area, so
    // highlighted code cost grew with the square of its line count.
    #[test]
    fn multi_line_rich_text_prepares_one_text_area() {
        let source = "let a = 1;\nlet b = 2;\nlet c = 3;\nlet d = 4;";
        let spans = source
            .match_indices("let")
            .map(|(start, _)| quark_text::TextSpan {
                range: start..start + 3,
                weight: None,
                style: None,
                kind: None,
            })
            .collect::<Vec<_>>();
        let mut text = test_text();
        let params = TextParams::new(source, TextStyle::new(14.0)).spans(spans);
        let layout = text.layout(&params).expect("layout");
        assert_eq!(layout.line_count(), 4);
        let mut scene = Scene::default();
        scene.rich_text(RichTextPrimitive {
            rect: rect(0.0, 0.0, 200.0, 100.0),
            layout: ShapedText::new(Arc::new(layout)),
            default_color: quark::Color::rgba(255, 255, 255, 255),
            span_colors: Arc::from([quark::Color::rgba(255, 0, 0, 255); 4]),
        });
        let flat = flatten_scene(&scene, rect(0.0, 0.0, 200.0, 100.0), &ImageCache::new());
        let mut recolored = RecoloredBuffers::default();
        recolored.prepare([&flat.rich_texts[..]], &mut text);
        let areas = prepare_text_areas(&flat.texts, &flat.rich_texts, &recolored);
        assert_eq!(areas.len(), 1);
    }

    // Splitting at every kind transition would cost one draw and one text
    // renderer per list row; disjoint rows must stay in a quad and a text batch.
    #[test]
    fn flatten_disjoint_rows_keep_one_quad_and_one_text_batch() {
        let mut scene = Scene::default();
        scene.push(solid(rect(0.0, 0.0, 200.0, 400.0), 10, 10, 10));
        for row in 0..20 {
            let row_rect = rect(0.0, row as f32 * 20.0, 200.0, 20.0);
            scene.push(solid(row_rect, 30, 30, 30));
            scene.push(white_text(row_rect, "row"));
        }
        let flat = flatten_scene(&scene, rect(0.0, 0.0, 200.0, 400.0), &ImageCache::new());
        let kinds: Vec<PrimKind> = flat
            .steps
            .iter()
            .filter_map(|step| match step {
                DrawStep::Batch { kind, .. } => Some(*kind),
                DrawStep::Blur(_) => None,
            })
            .collect();
        assert_eq!(kinds, [PrimKind::Quad, PrimKind::Text]);
    }

    // -- Borders -----------------------------------------------------------------

    // Regression: the border shader used the widest side on every side, so a
    // bottom-only border drew a full outline.
    #[test]
    fn render_bottom_only_border_leaves_other_sides_empty() {
        let mut scene = Scene::default();
        scene.push(Primitive::Border(crate::scene::BorderPrimitive {
            rect: rect(4.0, 4.0, 40.0, 40.0),
            widths: [0.0, 0.0, 4.0, 0.0],
            corner_radii: [6.0; 4],
            color: quark::Color::rgba(255, 255, 255, 255),
        }));
        let Some(image) = render_pixels(&scene, 48, 48) else {
            return;
        };
        let lit = |x: u32, y: u32| image.get_pixel(x, y).0[0] > 128;
        assert!(lit(24, 42), "bottom border missing");
        assert!(!lit(24, 5), "top side drew a border");
        assert!(!lit(5, 24), "left side drew a border");
        assert!(!lit(42, 24), "right side drew a border");
    }

    // -- Blur --------------------------------------------------------------------

    fn blur(rect: Rect, blur_radius: f32) -> Primitive {
        Primitive::BlurRegion(crate::scene::BlurRegionPrimitive {
            rect,
            blur_radius,
            corner_radii: [0.0; 4],
        })
    }

    /// Black left half, white right half, so a blur turns the seam gray.
    fn push_seam(scene: &mut Scene, area: Rect) {
        let half = area.width / 2.0;
        scene.push(solid(rect(area.x, area.y, half, area.height), 0, 0, 0));
        scene.push(solid(
            rect(area.x + half, area.y, half, area.height),
            255,
            255,
            255,
        ));
    }

    fn is_gray(pixel: [u8; 4]) -> bool {
        (40..=215).contains(&pixel[0])
    }

    // Regression: only blur_regions[0] was honored; later regions stayed sharp.
    #[test]
    fn render_second_blur_region_blurs_its_content() {
        let mut scene = Scene::default();
        push_seam(&mut scene, rect(0.0, 0.0, 64.0, 32.0));
        push_seam(&mut scene, rect(0.0, 32.0, 64.0, 32.0));
        scene.push(blur(rect(0.0, 0.0, 64.0, 32.0), 12.0));
        scene.push(blur(rect(0.0, 32.0, 64.0, 32.0), 12.0));
        let Some(image) = render_pixels(&scene, 64, 64) else {
            return;
        };
        assert!(
            is_gray(image.get_pixel(33, 16).0),
            "first region not blurred"
        );
        assert!(
            is_gray(image.get_pixel(33, 48).0),
            "second region not blurred"
        );
    }

    // Regression: the blur path drew all text last, sharp over the blur, even
    // text painted before the blur region.
    #[test]
    fn render_text_painted_before_blur_region_is_blurred() {
        let band = rect(4.0, 4.0, 120.0, 24.0);
        let mut scene = Scene::default();
        scene.push(white_text(band, "WWWWWWWW"));
        scene.push(blur(rect(0.0, 0.0, 128.0, 32.0), 24.0));
        let Some(image) = render_pixels(&scene, 128, 32) else {
            return;
        };
        let brightest = image.pixels().map(|p| p.0[0]).max().unwrap_or(0);
        assert!(brightest < 200, "sharp text survived the blur: {brightest}");
    }

    // Regression: the windowed blur path never drew images. render() and
    // render_to_rgba now share one encoder path, so this probe covers it.
    #[test]
    fn render_image_after_blur_region_draws() {
        let mut scene = Scene::default();
        scene.push(blur(rect(0.0, 0.0, 32.0, 32.0), 8.0));
        scene.push(Primitive::Image(crate::scene::ImagePrimitive {
            rect: rect(8.0, 8.0, 16.0, 16.0),
            width: 2,
            height: 2,
            rgba: Arc::from(vec![255u8; 16]),
            cache_key: 7,
        }));
        let Some(image) = render_pixels(&scene, 32, 32) else {
            return;
        };
        assert!(image.get_pixel(16, 16).0[0] > 200, "image missing");
    }

    // Rounded blur regions: the corner the radius cuts off keeps the sharp
    // backdrop, a square corner is blurred.
    #[test]
    fn render_blur_region_leaves_its_rounded_corner_sharp() {
        let mut scene = Scene::default();
        for x in (0..64).step_by(2) {
            let shade = if x % 4 == 0 { 255 } else { 0 };
            scene.push(solid(rect(x as f32, 0.0, 2.0, 64.0), shade, shade, shade));
        }
        scene.push(Primitive::BlurRegion(crate::scene::BlurRegionPrimitive {
            rect: rect(0.0, 0.0, 64.0, 64.0),
            blur_radius: 8.0,
            corner_radii: [20.0, 0.0, 0.0, 0.0],
        }));
        let Some(image) = render_pixels(&scene, 64, 64) else {
            return;
        };
        let top_left = image.get_pixel(1, 1).0[0];
        assert!(
            top_left == 0 || top_left == 255,
            "rounded corner blurred: {top_left}"
        );
        assert!(is_gray(image.get_pixel(62, 1).0), "square corner sharp");
        assert!(is_gray(image.get_pixel(32, 32).0), "center sharp");
    }

    // -- Layers ------------------------------------------------------------------

    fn layer(opacity: f32, transform: Transform2D) -> Primitive {
        Primitive::LayerStart(crate::scene::LayerPrimitive { opacity, transform })
    }

    // Group opacity: a faded group shows nothing of a child its other child
    // covers (per-primitive alpha would let the green show through), and
    // its text dims with it.
    #[test]
    fn render_opacity_layer_fades_children_and_text_as_one_group() {
        let mut scene = Scene::default();
        scene.push(layer(0.5, Transform2D::IDENTITY));
        scene.push(solid(rect(0.0, 0.0, 40.0, 40.0), 0, 255, 0));
        scene.push(solid(rect(0.0, 0.0, 40.0, 40.0), 0, 0, 255));
        let band = rect(44.0, 4.0, 120.0, 24.0);
        scene.push(white_text(band, "WWWWWWWW"));
        scene.push(Primitive::LayerEnd);
        let Some(image) = render_pixels(&scene, 168, 40) else {
            return;
        };
        let [r, g, b, _] = image.get_pixel(20, 20).0;
        // Half of linear blue over black, in sRGB.
        assert!(g < 4 && r < 4, "green showed through: {:?}", [r, g, b]);
        assert!((180..=196).contains(&b), "blue not half faded: {b}");
        let brightest = (44..164)
            .flat_map(|x| (4..28).map(move |y| (x, y)))
            .map(|(x, y)| image.get_pixel(x, y).0[0])
            .max()
            .unwrap_or(0);
        assert!(
            (150..=200).contains(&brightest),
            "text not faded: {brightest}"
        );
    }

    // A rotated layer moves its corners: a square turned 45 degrees about
    // its center leaves its old corner empty and reaches past its old side.
    #[test]
    fn render_rotated_layer_moves_the_corners() {
        let mut scene = Scene::default();
        let rotate = Transform2D::rotate(std::f32::consts::FRAC_PI_4).around(50.0, 50.0);
        scene.push(layer(1.0, rotate));
        scene.push(solid(rect(30.0, 30.0, 40.0, 40.0), 255, 255, 255));
        scene.push(Primitive::LayerEnd);
        let Some(image) = render_pixels(&scene, 100, 100) else {
            return;
        };
        let lit = |x: u32, y: u32| image.get_pixel(x, y).0[0];
        assert_eq!(lit(31, 31), 0, "old corner still drawn");
        assert_eq!(lit(50, 50), 255, "center missing");
        // The diagonal now points along the axes: 28 px from the center.
        assert_eq!(lit(50 + 26, 50), 255, "tip missing");
        assert_eq!(lit(50, 50 - 26), 255, "tip missing");
        assert_eq!(lit(50 + 30, 50), 0, "drawn past the tip");
    }

    // -- Paths -------------------------------------------------------------------

    fn path(build: impl FnOnce(&mut crate::scene::PathBuilder)) -> Arc<crate::scene::Path> {
        let mut builder = crate::scene::Path::builder();
        build(&mut builder);
        Arc::new(builder.build())
    }

    // A filled circle covers its inside, leaves its outside, and blends its
    // edge pixels.
    #[test]
    fn render_path_fill_covers_inside_with_antialiased_edge() {
        let circle = path(|b| {
            b.arc(32.0, 32.0, 20.0, 0.0, std::f32::consts::TAU).close();
        });
        let mut scene = Scene::default();
        scene.path(
            crate::scene::PathPrimitive::new(circle, [0.0, 0.0])
                .fill(quark::Color::rgba(255, 255, 255, 255)),
        );
        let Some(image) = render_pixels(&scene, 64, 64) else {
            return;
        };
        let lit = |x: u32, y: u32| image.get_pixel(x, y).0[0];
        assert_eq!(lit(32, 32), 255, "center");
        assert_eq!(lit(32, 14), 255, "inside near the top");
        assert_eq!(lit(32, 10), 0, "outside above");
        assert_eq!(lit(5, 5), 0, "outside corner");
        // An aliased circle has no partly covered pixels; this one has a
        // ring of them around its 126 px perimeter.
        let partial = image
            .pixels()
            .filter(|p| (20..235).contains(&p.0[0]))
            .count();
        assert!(
            partial > 40,
            "edge not antialiased: {partial} partial pixels"
        );
    }

    // A stroke covers its width around the line and its round cap past the
    // end, and nothing beyond.
    #[test]
    fn render_path_stroke_covers_its_width_and_caps() {
        let line = path(|b| {
            b.move_to(10.0, 20.0).line_to(50.0, 20.0);
        });
        let mut scene = Scene::default();
        scene.path(crate::scene::PathPrimitive::new(line, [0.0, 0.0]).stroke(
            quark::Color::rgba(255, 255, 255, 255),
            crate::scene::StrokeStyle::new(8.0).cap(crate::scene::LineCap::Round),
        ));
        let Some(image) = render_pixels(&scene, 64, 40) else {
            return;
        };
        let lit = |x: u32, y: u32| image.get_pixel(x, y).0[0];
        assert_eq!(lit(30, 17), 255, "inside the width");
        assert_eq!(lit(30, 22), 255, "inside the width");
        assert_eq!(lit(30, 26), 0, "past the width");
        assert_eq!(lit(52, 19), 255, "round cap missing");
        assert_eq!(lit(56, 19), 0, "drawn past the cap");
    }

    // Regression: a target dropped without `release_offscreen` stayed in use
    // forever, which also stopped the pool from trimming any texture.
    #[test]
    fn dropped_offscreen_target_returns_to_the_pool() {
        let Some(mut renderer) = gpu_renderer(16, 16) else {
            return;
        };
        drop(renderer.acquire_offscreen(64, 64));
        let reused = renderer.acquire_offscreen(32, 32);
        assert_eq!((reused.width, reused.height), (64, 64));
    }

    // Regression: a minimized window (size 0) kept acquiring and
    // reconfiguring its stale surface, and the runner redrew on every
    // `SurfaceReconfigured`, spinning a core while minimized.
    #[test]
    fn render_at_zero_size_skips_the_surface() {
        let Some(mut renderer) = gpu_renderer(16, 16) else {
            return;
        };
        let scene = Scene::default();
        renderer.resize(0, 0, 1.0);
        let skipped = renderer.render(&scene, &mut test_text(), 0.0);
        assert_eq!(skipped.expect("zero-size frame"), FrameStats::default());
        // At a real size the same renderer reaches for its (absent) surface.
        renderer.resize(16, 16, 1.0);
        let drawn = renderer.render(&scene, &mut test_text(), 0.0);
        assert!(matches!(drawn, Err(RenderError::NoSurface)), "{drawn:?}");
    }

    // Regression: the upload trusted `rgba.len()`, so a buffer shorter than
    // width * height * 4 panicked inside wgpu.
    #[test]
    fn render_image_with_short_pixel_buffer_draws_nothing() {
        let mut scene = Scene::default();
        scene.push(solid(rect(0.0, 0.0, 16.0, 16.0), 0, 0, 255));
        scene.push(Primitive::Image(crate::scene::ImagePrimitive {
            rect: rect(0.0, 0.0, 16.0, 16.0),
            width: 4,
            height: 4,
            rgba: Arc::from(vec![255u8; 16]),
            cache_key: 9,
        }));
        let Some(image) = render_pixels(&scene, 16, 16) else {
            return;
        };
        assert_eq!(image.get_pixel(8, 8).0, [0, 0, 255, 255]);
    }

    // Regression: a full glyph atlas failed the frame before the end-of-frame
    // trim, so its glyphs stayed pinned and every later frame failed too. Now
    // that frame draws everything but text and the next frame draws text.
    #[test]
    fn render_recovers_from_a_full_glyph_atlas() {
        // glyphon caps its atlas at the device's texture size limit: at 256px
        // a few dozen large glyphs cannot fit in one frame.
        let limits = wgpu::Limits {
            max_texture_dimension_2d: 256,
            ..wgpu::Limits::default()
        };
        let gpu = match GpuContext::headless_with_limits(limits) {
            Ok(gpu) => gpu,
            Err(error) => {
                assert!(
                    std::env::var_os("QUARK_REQUIRE_GPU").is_none(),
                    "QUARK_REQUIRE_GPU is set but no capped device: {error}"
                );
                return;
            }
        };
        let mut renderer = Renderer::headless_with_gpu(&gpu, 256, 128, 1.0);
        let mut text = test_text();
        let shaped = |text: &mut TextSystem, s: &str, size: f32| {
            let layout = text.layout(&TextParams::new(s, TextStyle::new(size)));
            ShapedText::new(Arc::new(layout.expect("layout")))
        };
        let white = quark::Color::rgba(255, 255, 255, 255);

        let mut crowded = Scene::default();
        crowded.push(solid(rect(0.0, 0.0, 256.0, 128.0), 0, 0, 255));
        crowded.text(TextPrimitive {
            rect: rect(0.0, 0.0, 256.0, 128.0),
            layout: shaped(
                &mut text,
                "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789",
                120.0,
            ),
            color: white,
        });
        let pixels = renderer
            .render_to_rgba(&crowded, &mut text, 256, 128)
            .expect("a full atlas still renders the frame");
        let crowded = image::RgbaImage::from_raw(256, 128, pixels).expect("pixel buffer size");
        assert_eq!(
            crowded.get_pixel(250, 120).0,
            [0, 0, 255, 255],
            "quad missing"
        );

        let mut sparse = Scene::default();
        sparse.text(TextPrimitive {
            rect: rect(0.0, 0.0, 256.0, 128.0),
            layout: shaped(&mut text, "W", 48.0),
            color: white,
        });
        let pixels = renderer
            .render_to_rgba(&sparse, &mut text, 256, 128)
            .expect("next frame renders");
        let sparse = image::RgbaImage::from_raw(256, 128, pixels).expect("pixel buffer size");
        assert!(
            any_light_pixel(&sparse, rect(0.0, 0.0, 64.0, 64.0)),
            "text did not recover after the atlas filled"
        );
    }

    // Two windows share one device: an image uploaded while drawing one
    // window draws in another from its cache key alone, which only works
    // when both use the same device and texture cache.
    #[test]
    fn image_uploaded_by_one_renderer_draws_in_another_on_the_same_gpu() {
        let gpu = match GpuContext::headless() {
            Ok(gpu) => gpu,
            Err(_) => {
                assert!(
                    std::env::var_os("QUARK_REQUIRE_GPU").is_none(),
                    "QUARK_REQUIRE_GPU is set but no wgpu adapter is available"
                );
                return;
            }
        };
        let image = |rgba: Arc<[u8]>| {
            let mut scene = Scene::default();
            scene.push(Primitive::Image(crate::scene::ImagePrimitive {
                rect: rect(0.0, 0.0, 16.0, 16.0),
                width: 1,
                height: 1,
                rgba,
                cache_key: 42,
            }));
            scene
        };
        let mut first = Renderer::headless_with_gpu(&gpu, 16, 16, 1.0);
        let mut second = Renderer::headless_with_gpu(&gpu, 16, 16, 1.0);
        let mut text = test_text();
        first
            .render_to_rgba(&image(Arc::from(vec![0, 255, 0, 255])), &mut text, 16, 16)
            .expect("first render");
        let pixels = second
            .render_to_rgba(&image(empty_rgba()), &mut text, 16, 16)
            .expect("second render");
        assert_eq!(&pixels[..4], &[0, 255, 0, 255]);
    }
}

// Text preparation, caching, and color helpers are in text.rs
// Shader source constants are in shaders.rs
