use std::{collections::HashMap, sync::Arc, time::Instant};

use bytemuck::{Pod, Zeroable};
use glyphon::{
    Buffer, Cache, FontSystem, Resolution, SwashCache, TextAtlas, TextRenderer, Viewport,
};
use thiserror::Error;
use wgpu::util::DeviceExt;
use winit::{dpi::PhysicalSize, window::Window};

use crate::fonts::FontSettings;
use crate::scene::{ClipPrimitive, Primitive, Rect, RichTextPrimitive, Scene, TextPrimitive};

use crate::shaders::{BLIT_SHADER, BLUR_SHADER, EFFECT_SHADER, QUAD_SHADER, SHADOW_SHADER};
use crate::text::{color_to_linear, measure_mono_char_width, prepare_text_areas};

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
    #[error("buffer map failed")]
    BufferMap,
    #[error("png encode/write failed: {0}")]
    PngWrite(String),
}

/// GPU-resident image (icon, avatar). The bind group keeps the view alive.
struct CachedImage {
    _texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
}

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
    in_use: bool,
    last_used_frame: u64,
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

/// Handle to an offscreen render target allocated from the pool.
pub struct OffscreenTarget {
    pool_index: usize,
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

    /// Acquire a texture of at least the given dimensions.
    /// Returns a pool index. The caller must call `release()` when done.
    fn acquire(&mut self, device: &wgpu::Device, width: u32, height: u32) -> OffscreenTarget {
        let w = width.max(1);
        let h = height.max(1);

        // Look for an existing unused texture that's big enough.
        for (i, entry) in self.textures.iter_mut().enumerate() {
            if !entry.in_use && entry.width >= w && entry.height >= h {
                entry.in_use = true;
                entry.last_used_frame = self.frame;
                return OffscreenTarget {
                    pool_index: i,
                    width: entry.width,
                    height: entry.height,
                };
            }
        }

        // Allocate a new texture.
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("diffy_offscreen"),
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
        let index = self.textures.len();
        let _ = texture;
        self.textures.push(PooledTexture {
            view,
            bind_group: None,
            width: w,
            height: h,
            in_use: true,
            last_used_frame: self.frame,
        });
        OffscreenTarget {
            pool_index: index,
            width: w,
            height: h,
        }
    }

    fn view(&self, target: &OffscreenTarget) -> &wgpu::TextureView {
        &self.textures[target.pool_index].view
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
        let entry = &mut self.textures[target.pool_index];
        let view = &entry.view;
        entry
            .bind_group
            .get_or_insert_with(|| create_texture_bind_group(device, layout, view, sampler))
            .clone()
    }

    fn release(&mut self, target: OffscreenTarget) {
        self.textures[target.pool_index].in_use = false;
        self.textures[target.pool_index].last_used_frame = self.frame;
    }

    fn trim_unused(&mut self) {
        if self.textures.iter().any(|entry| entry.in_use) {
            return;
        }
        let frame = self.frame;
        self.textures.retain(|entry| {
            frame.saturating_sub(entry.last_used_frame) <= KEEP_UNUSED_OFFSCREEN_FRAMES
        });
    }
}

pub struct Renderer {
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
    texture_bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    texture_pool: TexturePool,
    instance_buffer_pool: TransientBufferPool,
    image_cache: ImageCache,
    viewport_buffer: wgpu::Buffer,
    viewport_bind_group: wgpu::BindGroup,
    font_system: FontSystem,
    swash_cache: SwashCache,
    viewport: Viewport,
    atlas: TextAtlas,
    /// One text renderer per text segment of the frame, grown on demand.
    text_renderers: Vec<TextRenderer>,
    text_cache: HashMap<u64, CachedTextBuffer>,
    text_cache_frame: u64,
    cached_mono_char_width: Option<(f32, f32)>,
    flattener: Flattener,
    flat: FlattenedScene,
    batches: FrameBatches,
}

impl Renderer {
    pub fn new(window: Arc<Window>, font_settings: &FontSettings) -> Result<Self, RenderError> {
        pollster::block_on(Self::new_async(window, font_settings))
    }

    async fn new_async(
        window: Arc<Window>,
        font_settings: &FontSettings,
    ) -> Result<Self, RenderError> {
        let size = window.inner_size();
        let scale_factor = window.scale_factor();

        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let surface = instance.create_surface(window.clone())?;
        let adapter = match instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                compatible_surface: Some(&surface),
                ..wgpu::RequestAdapterOptions::default()
            })
            .await
        {
            Ok(adapter) => adapter,
            Err(_) => instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    force_fallback_adapter: true,
                    ..wgpu::RequestAdapterOptions::default()
                })
                .await
                .map_err(|_| RenderError::NoAdapter)?,
        };

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default())
            .await?;

        let surface_capabilities = surface.get_capabilities(&adapter);
        let surface_format = surface_capabilities
            .formats
            .iter()
            .copied()
            .find(wgpu::TextureFormat::is_srgb)
            .unwrap_or(surface_capabilities.formats[0]);
        let surface_config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .unwrap_or(wgpu::SurfaceConfiguration {
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                format: surface_format,
                width: size.width.max(1),
                height: size.height.max(1),
                desired_maximum_frame_latency: 2,
                present_mode: wgpu::PresentMode::Fifo,
                alpha_mode: wgpu::CompositeAlphaMode::Opaque,
                view_formats: vec![],
            });
        surface.configure(&device, &surface_config);

        Self::assemble(
            device,
            queue,
            surface_format,
            surface_config,
            size,
            scale_factor,
            Some(surface),
            font_settings,
        )
    }

    /// Build a windowless renderer that targets `OffscreenTarget`s only. Requests
    /// an adapter with no compatible surface, creates a device+queue, and shares
    /// the same pipeline/atlas setup as the windowed path. No swapchain is created.
    #[cfg(any(test, feature = "headless-render"))]
    pub fn new_headless(
        width: u32,
        height: u32,
        scale_factor: f64,
        font_settings: &FontSettings,
    ) -> Result<Self, RenderError> {
        pollster::block_on(Self::new_headless_async(
            width,
            height,
            scale_factor,
            font_settings,
        ))
    }

    #[cfg(any(test, feature = "headless-render"))]
    async fn new_headless_async(
        width: u32,
        height: u32,
        scale_factor: f64,
        font_settings: &FontSettings,
    ) -> Result<Self, RenderError> {
        let size = PhysicalSize::new(width.max(1), height.max(1));

        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let adapter = match instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                compatible_surface: None,
                ..wgpu::RequestAdapterOptions::default()
            })
            .await
        {
            Ok(adapter) => adapter,
            Err(_) => instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    force_fallback_adapter: true,
                    compatible_surface: None,
                    ..wgpu::RequestAdapterOptions::default()
                })
                .await
                .map_err(|_| RenderError::NoAdapter)?,
        };

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default())
            .await?;

        // Match the on-screen path: an sRGB target so colors and PNG bytes agree.
        let surface_format = wgpu::TextureFormat::Rgba8UnormSrgb;
        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: surface_format,
            width: size.width,
            height: size.height,
            desired_maximum_frame_latency: 2,
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: wgpu::CompositeAlphaMode::Opaque,
            view_formats: vec![],
        };

        Self::assemble(
            device,
            queue,
            surface_format,
            surface_config,
            size,
            scale_factor,
            None,
            font_settings,
        )
    }

    /// Shared pipeline/atlas/font setup for both the windowed and headless paths.
    #[allow(clippy::too_many_arguments)]
    fn assemble(
        device: wgpu::Device,
        queue: wgpu::Queue,
        surface_format: wgpu::TextureFormat,
        surface_config: wgpu::SurfaceConfiguration,
        size: PhysicalSize<u32>,
        scale_factor: f64,
        surface: Option<wgpu::Surface<'static>>,
        font_settings: &FontSettings,
    ) -> Result<Self, RenderError> {
        let viewport_uniform = ViewportUniform::new(surface_config.width, surface_config.height);
        let viewport_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("diffy_viewport_uniform"),
            contents: bytemuck::bytes_of(&viewport_uniform),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let viewport_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("diffy_viewport_bind_group_layout"),
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
        let viewport_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("diffy_viewport_bind_group"),
            layout: &viewport_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: viewport_buffer.as_entire_binding(),
            }],
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("diffy_quad_shader"),
            source: wgpu::ShaderSource::Wgsl(QUAD_SHADER.into()),
        });
        let quad_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("diffy_quad_pipeline_layout"),
            bind_group_layouts: &[&viewport_bind_group_layout],
            immediate_size: 0,
        });
        let quad_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("diffy_quad_pipeline"),
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
            label: Some("diffy_shadow_shader"),
            source: wgpu::ShaderSource::Wgsl(SHADOW_SHADER.into()),
        });
        let shadow_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("diffy_shadow_pipeline_layout"),
                bind_group_layouts: &[&viewport_bind_group_layout],
                immediate_size: 0,
            });
        let shadow_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("diffy_shadow_pipeline"),
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
            label: Some("diffy_effect_shader"),
            source: wgpu::ShaderSource::Wgsl(EFFECT_SHADER.into()),
        });
        let effect_quad_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("diffy_effect_quad_pipeline_layout"),
                bind_group_layouts: &[&viewport_bind_group_layout],
                immediate_size: 0,
            });
        let effect_quad_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("diffy_effect_quad_pipeline"),
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
                label: Some("diffy_texture_bind_group_layout"),
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
            label: Some("diffy_blit_sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let blit_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("diffy_blit_shader"),
            source: wgpu::ShaderSource::Wgsl(BLIT_SHADER.into()),
        });
        let blit_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("diffy_blit_pipeline_layout"),
            bind_group_layouts: &[&viewport_bind_group_layout, &texture_bind_group_layout],
            immediate_size: 0,
        });
        let blit_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("diffy_blit_pipeline"),
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
            label: Some("diffy_blur_shader"),
            source: wgpu::ShaderSource::Wgsl(BLUR_SHADER.into()),
        });
        let blur_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("diffy_blur_pipeline_layout"),
            bind_group_layouts: &[&viewport_bind_group_layout, &texture_bind_group_layout],
            immediate_size: 0,
        });
        let blur_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("diffy_blur_pipeline"),
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

        let texture_pool = TexturePool::new(surface_format);

        let font_system = crate::fonts::new_font_system_with_settings(font_settings);
        let swash_cache = SwashCache::new();
        let glyph_cache = Cache::new(&device);
        let viewport = Viewport::new(&device, &glyph_cache);
        let mut atlas = TextAtlas::new(&device, &queue, &glyph_cache, surface_format);
        let text_renderer =
            TextRenderer::new(&mut atlas, &device, wgpu::MultisampleState::default(), None);

        Ok(Self {
            device,
            queue,
            surface,
            surface_config,
            size,
            scale_factor,
            quad_pipeline,
            shadow_pipeline,
            effect_quad_pipeline,
            blit_pipeline,
            blur_pipeline,
            texture_bind_group_layout,
            sampler,
            texture_pool,
            instance_buffer_pool: TransientBufferPool::default(),
            image_cache: HashMap::new(),
            viewport_buffer,
            viewport_bind_group,
            font_system,
            swash_cache,
            viewport,
            atlas,
            text_renderers: vec![text_renderer],
            text_cache: HashMap::new(),
            text_cache_frame: 0,
            cached_mono_char_width: None,
            flattener: Flattener::default(),
            flat: FlattenedScene::default(),
            batches: FrameBatches::default(),
        })
    }

    pub fn resize(&mut self, width: u32, height: u32, scale_factor: f64) {
        if width == 0 || height == 0 {
            self.size = PhysicalSize::new(width, height);
            self.scale_factor = scale_factor;
            return;
        }

        self.size = PhysicalSize::new(width, height);
        self.scale_factor = scale_factor;
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

    pub fn font_system(&mut self) -> &mut FontSystem {
        &mut self.font_system
    }

    pub fn font_system_mut(&mut self) -> &mut FontSystem {
        &mut self.font_system
    }

    pub fn set_font_settings(&mut self, settings: &FontSettings) {
        self.font_system = crate::fonts::new_font_system_with_settings(settings);
        self.swash_cache = SwashCache::new();
        self.text_cache.clear();
        self.cached_mono_char_width = None;
    }

    pub fn scale_factor(&self) -> f64 {
        self.scale_factor
    }

    pub fn text_metrics(&mut self) -> TextMetrics {
        let scale = self.scale_factor as f32;
        let mono_font_size = 13.0 * scale;
        let char_w = match self.cached_mono_char_width {
            Some((cached_size, cached_w)) if (cached_size - mono_font_size).abs() < 0.001 => {
                cached_w
            }
            _ => {
                let w = measure_mono_char_width(&mut self.font_system, mono_font_size);
                self.cached_mono_char_width = Some((mono_font_size, w));
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
            label: Some("diffy_offscreen_bind_group"),
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

    /// Return an offscreen target to the pool for reuse.
    pub fn release_offscreen(&mut self, target: OffscreenTarget) {
        self.texture_pool.release(target);
    }

    /// Render `scene` into an offscreen sRGB texture at the given physical
    /// `width`/`height` and `scale_factor` (which must match the scale used to
    /// build the scene), read the pixels back, and write them as a PNG to `path`.
    ///
    /// This is a self-contained, no-swapchain draw flow used by the dev/test
    /// "screenshot" leg.
    #[cfg(any(test, feature = "headless-render"))]
    pub fn render_to_png(
        &mut self,
        scene: &Scene,
        width: u32,
        height: u32,
        scale_factor: f32,
        path: &std::path::Path,
    ) -> Result<(), RenderError> {
        let (w, h) = (width.max(1), height.max(1));
        let pixels = self.render_to_rgba(scene, w, h, scale_factor)?;
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
        width: u32,
        height: u32,
        scale_factor: f32,
    ) -> Result<Vec<u8>, RenderError> {
        let w = width.max(1);
        let h = height.max(1);

        self.texture_pool.begin_frame();
        self.instance_buffer_pool.begin_frame();
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
            label: Some("diffy_png_target"),
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
                label: Some("diffy_png_encoder"),
            });
        self.record_frame(&mut encoder, &view, w, h, scale_factor as f64)?;

        let bytes_per_pixel = 4u32;
        let unpadded_bytes_per_row = w * bytes_per_pixel;
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let padded_bytes_per_row = unpadded_bytes_per_row.div_ceil(align) * align;
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("diffy_png_readback"),
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

    pub fn render(&mut self, scene: &Scene, time_seconds: f32) -> Result<FrameStats, RenderError> {
        if self.surface_config.width == 0 || self.surface_config.height == 0 {
            return Ok(FrameStats::default());
        }
        let render_started_at = Instant::now();
        self.texture_pool.begin_frame();
        self.instance_buffer_pool.begin_frame();
        let sw = self.surface_config.width;
        let sh = self.surface_config.height;

        // Update time in the viewport uniform buffer.
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

        let surface = self
            .surface
            .as_ref()
            .expect("render() requires a window surface; use render_to_png for headless");
        let acquire_started_at = Instant::now();
        let frame = match surface.get_current_texture() {
            Ok(frame) => frame,
            Err(wgpu::SurfaceError::Outdated | wgpu::SurfaceError::Lost) => {
                surface.configure(&self.device, &self.surface_config);
                return Err(RenderError::SurfaceAcquire);
            }
            Err(_) => return Err(RenderError::SurfaceAcquire),
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
                label: Some("diffy_frame_encoder"),
            });
        self.record_frame(&mut encoder, &view, sw, sh, self.scale_factor)?;

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

    fn flatten(&mut self, scene: &Scene, width: u32, height: u32) {
        let viewport = Rect {
            x: 0.0,
            y: 0.0,
            width: width as f32,
            height: height as f32,
        };
        flatten_scene_into(
            scene,
            viewport,
            &self.image_cache,
            &mut self.flattener,
            &mut self.flat,
        );
    }

    /// Upload, prepare, and encode the flattened frame into `encoder`,
    /// targeting `target`. Everything goes into one encoder: each text segment
    /// has its own `TextRenderer`, so preparing one cannot overwrite the
    /// vertices another segment's pass reads.
    fn record_frame(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        width: u32,
        height: u32,
        scale_factor: f64,
    ) -> Result<(), RenderError> {
        let flat = std::mem::take(&mut self.flat);
        let mut batches = std::mem::take(&mut self.batches);
        let result = self.record_flattened(
            encoder,
            target,
            &flat,
            &mut batches,
            width,
            height,
            scale_factor,
        );
        self.flat = flat;
        self.batches = batches;
        result
    }

    #[allow(clippy::too_many_arguments)]
    fn record_flattened(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        flat: &FlattenedScene,
        batches: &mut FrameBatches,
        width: u32,
        height: u32,
        scale_factor: f64,
    ) -> Result<(), RenderError> {
        for image in &flat.images {
            self.ensure_image_uploaded(&image.primitive);
        }

        build_batches(flat, batches);
        let device = &self.device;
        let queue = &self.queue;
        let pool = &mut self.instance_buffer_pool;
        let mut buffers = FrameBuffers {
            shadow: pool.upload(device, queue, "diffy_shadow_instances", &batches.shadows),
            effect: pool.upload(
                device,
                queue,
                "diffy_effect_quad_instances",
                &batches.effects,
            ),
            quad: pool.upload(device, queue, "diffy_quad_instances", &batches.quads),
            image: pool.upload(device, queue, "diffy_image_blit", &batches.images),
            ..FrameBuffers::default()
        };

        // Prepare every text segment before recording any pass.
        while self.text_renderers.len() < batches.text_steps {
            self.text_renderers.push(TextRenderer::new(
                &mut self.atlas,
                &self.device,
                wgpu::MultisampleState::default(),
                None,
            ));
        }
        let mut text_index = 0;
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
                &mut self.font_system,
                &mut self.text_cache,
                &mut self.text_cache_frame,
                &flat.texts[items.start as usize..items.end as usize],
                &flat.rich_texts[rich.start as usize..rich.end as usize],
                scale_factor,
            );
            self.text_renderers[text_index].prepare(
                &self.device,
                &self.queue,
                &mut self.font_system,
                &mut self.atlas,
                &self.viewport,
                text_areas,
                &mut self.swash_cache,
            )?;
            text_index += 1;
        }

        let blur_targets = self.prepare_blur(flat, &mut buffers, width, height);
        let result = self.encode_steps(
            encoder,
            target,
            flat,
            batches,
            &buffers,
            blur_targets.as_ref(),
            width,
            height,
        );
        if let Some(targets) = blur_targets {
            self.texture_pool.release(targets.scene);
            self.texture_pool.release(targets.h);
            self.texture_pool.release(targets.v);
        }
        result
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
            blur_instances.push(BlurInstance {
                bounds,
                uv_rect: uv,
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
            });
        }
        blit_instances.push(BlitInstance {
            bounds: [0.0, 0.0, width as f32, height as f32],
            uv_rect: [0.0, 0.0, width as f32 / tw, height as f32 / th],
            tint: [1.0; 4],
        });
        let pool = &mut self.instance_buffer_pool;
        buffers.blur = pool.upload(
            &self.device,
            &self.queue,
            "diffy_blur_instances",
            &blur_instances,
        );
        buffers.blur_blit = pool.upload(
            &self.device,
            &self.queue,
            "diffy_blur_blit",
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

    /// Record the frame's passes. Without blur this is one pass on `target`.
    /// With blur, steps draw into an offscreen scene texture; at each blur step
    /// the region is blurred (horizontal then vertical pass) and composited
    /// back before later steps draw, and the scene is finally copied to
    /// `target`.
    #[allow(clippy::too_many_arguments)]
    fn encode_steps(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        flat: &FlattenedScene,
        batches: &FrameBatches,
        buffers: &FrameBuffers,
        blur: Option<&BlurTargets>,
        width: u32,
        height: u32,
    ) -> Result<(), RenderError> {
        let draw_view = blur.map_or(target, |b| &b.scene_view);
        let set_viewport = |pass: &mut wgpu::RenderPass<'_>| {
            if blur.is_some() {
                pass.set_viewport(0.0, 0.0, width as f32, height as f32, 0.0, 1.0);
            }
        };
        let mut load = wgpu::LoadOp::Clear(wgpu::Color::BLACK);
        let mut blurs_done = 0u32;
        let mut start = 0;
        loop {
            let end = flat.steps[start..]
                .iter()
                .position(|s| matches!(s, DrawStep::Blur(_)))
                .map_or(flat.steps.len(), |p| start + p);
            {
                let mut pass = begin_pass(encoder, "diffy_frame_pass", draw_view, load);
                set_viewport(&mut pass);
                if let (Some(b), Some(buf)) = (blur, &buffers.blur_blit)
                    && blurs_done > 0
                {
                    pass.set_pipeline(&self.blit_pipeline);
                    pass.set_bind_group(0, &self.viewport_bind_group, &[]);
                    pass.set_bind_group(1, &b.v_bind, &[]);
                    pass.set_vertex_buffer(0, buf.slice(..));
                    pass.set_scissor_rect(0, 0, width, height);
                    pass.draw(0..4, blurs_done - 1..blurs_done);
                }
                for i in start..end {
                    self.draw_step(
                        &mut pass,
                        &flat.steps[i],
                        batches.step_cmds[i].clone(),
                        batches,
                        buffers,
                        width,
                        height,
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
                    "diffy_blur_pass",
                    dest,
                    wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                );
                set_viewport(&mut pass);
                pass.set_pipeline(&self.blur_pipeline);
                pass.set_bind_group(0, &self.viewport_bind_group, &[]);
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
                "diffy_composite_pass",
                target,
                wgpu::LoadOp::Clear(wgpu::Color::BLACK),
            );
            pass.set_pipeline(&self.blit_pipeline);
            pass.set_bind_group(0, &self.viewport_bind_group, &[]);
            pass.set_bind_group(1, &b.scene_bind, &[]);
            pass.set_vertex_buffer(0, buf.slice(..));
            pass.draw(0..4, blurs_done..blurs_done + 1);
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_step(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        step: &DrawStep,
        cmds: std::ops::Range<u32>,
        batches: &FrameBatches,
        buffers: &FrameBuffers,
        width: u32,
        height: u32,
    ) -> Result<(), RenderError> {
        let DrawStep::Batch { kind, .. } = step else {
            return Ok(());
        };
        let cmds = cmds.start as usize..cmds.end as usize;
        let (pipeline, buffer) = match kind {
            PrimKind::Shadow => (&self.shadow_pipeline, &buffers.shadow),
            PrimKind::Effect => (&self.effect_quad_pipeline, &buffers.effect),
            PrimKind::Quad => (&self.quad_pipeline, &buffers.quad),
            PrimKind::Image => {
                let Some(buffer) = &buffers.image else {
                    return Ok(());
                };
                pass.set_pipeline(&self.blit_pipeline);
                pass.set_bind_group(0, &self.viewport_bind_group, &[]);
                pass.set_vertex_buffer(0, buffer.slice(..));
                for command in &batches.image_cmds[cmds] {
                    // The cache lookup is the gate: icons resolved from a prior
                    // frame carry an empty `rgba` but still draw via their
                    // uploaded texture. Images that were never uploadable miss.
                    let Some(entry) = self.image_cache.get(&command.cache_key) else {
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
                pass.set_scissor_rect(0, 0, width, height);
                self.text_renderers[cmds.start].render(&self.atlas, &self.viewport, pass)?;
                return Ok(());
            }
        };
        let Some(buffer) = buffer else {
            return Ok(());
        };
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &self.viewport_bind_group, &[]);
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

    /// Upload `image` to the GPU cache under its key if it is not there yet.
    fn ensure_image_uploaded(&mut self, image: &crate::scene::ImagePrimitive) {
        let key = image.cache_key;
        if key == 0
            || self.image_cache.contains_key(&key)
            || image.rgba.is_empty()
            || image.width == 0
            || image.height == 0
        {
            return;
        }
        let texture = self.device.create_texture_with_data(
            &self.queue,
            &wgpu::TextureDescriptor {
                label: Some("diffy_cached_image"),
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
        self.image_cache.insert(
            key,
            CachedImage {
                _texture: texture,
                bind_group,
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
    /// Two instances per blur region: horizontal then vertical.
    blur: Option<wgpu::Buffer>,
    /// One instance per blur region (composite the blurred region), then a
    /// final full-target instance (copy the offscreen scene to the output).
    blur_blit: Option<wgpu::Buffer>,
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
        label: Some("diffy_texture_bind_group"),
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
    /// Tint/opacity multiplier (usually [1, 1, 1, alpha]).
    tint: [f32; 4],
}

impl BlitInstance {
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
}

const KIND_COUNT: usize = 5;

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
    /// top of the result.
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
struct ImageDrawCommand {
    instance_start: u32,
    instance_end: u32,
    cache_key: u64,
    clip: Rect,
}

#[derive(Debug)]
pub(super) struct CachedTextBuffer {
    pub(super) buffer: Buffer,
    pub(super) last_used_frame: u64,
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
}

impl Flattener {
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
    flatten_scene_into(
        scene,
        viewport,
        image_cache,
        &mut Flattener::default(),
        &mut out,
    );
    out
}

fn flatten_scene_into(
    scene: &Scene,
    viewport: Rect,
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

    for primitive in &scene.primitives {
        match primitive {
            Primitive::Rect(rect) => push_quad(
                rect.rect,
                color_to_linear(rect.color),
                [0.0; 4],
                [0.0; 4],
                [0.0; 4],
                fl,
                &mut out.quads,
            ),
            Primitive::RoundedRect(rect) => push_quad(
                rect.rect,
                color_to_linear(rect.color),
                [0.0; 4],
                rect.corner_radii,
                [0.0; 4],
                fl,
                &mut out.quads,
            ),
            Primitive::Border(border) => push_quad(
                border.rect,
                [0.0; 4],
                color_to_linear(border.color),
                border.corner_radii,
                border.widths,
                fl,
                &mut out.quads,
            ),
            Primitive::Shadow(shadow) => {
                let sigma = (shadow.blur_radius * 0.5).max(0.5);
                let expansion = sigma * 3.0;
                let offset_x = shadow.offset[0];
                let offset_y = shadow.offset[1];
                let expanded = Rect {
                    x: shadow.rect.x + offset_x - expansion,
                    y: shadow.rect.y + offset_y - expansion,
                    width: shadow.rect.width + expansion * 2.0,
                    height: shadow.rect.height + expansion * 2.0,
                };
                let clip = *fl.clips.last().expect("root clip");
                if let Some(bounds) = expanded.intersection(clip.scissor) {
                    let key = fl.place(PrimKind::Shadow, bounds);
                    out.shadows.push(ClippedShadow {
                        key,
                        instance: ShadowInstance {
                            draw_bounds: [expanded.x, expanded.y, expanded.width, expanded.height],
                            shadow_bounds: [
                                shadow.rect.x + offset_x,
                                shadow.rect.y + offset_y,
                                shadow.rect.width,
                                shadow.rect.height,
                            ],
                            color: color_to_linear(shadow.color),
                            params: [sigma, shadow.corner_radius, 0.0, 0.0],
                            clip_bounds: clip.clip_bounds_attr(),
                            clip_radii: clip.clip_radii_attr(),
                        },
                        clip: clip.scissor,
                    });
                }
            }
            Primitive::TextRun(text) => {
                let clip = *fl.clips.last().expect("root clip");
                if let Some(intersection) = text.rect.intersection(clip.scissor) {
                    let key = fl.place(PrimKind::Text, intersection);
                    out.texts.push(ClippedText {
                        key,
                        primitive: text.clone(),
                        clip: intersection,
                    });
                }
            }
            Primitive::RichTextRun(text) => {
                let clip = *fl.clips.last().expect("root clip");
                if let Some(intersection) = text.rect.intersection(clip.scissor) {
                    let key = fl.place(PrimKind::Text, intersection);
                    out.rich_texts.push(ClippedRichText {
                        key,
                        primitive: text.clone(),
                        clip: intersection,
                    });
                }
            }
            Primitive::BlurRegion(blur) => {
                let clip = *fl.clips.last().expect("root clip");
                if let Some(rect) = blur.rect.intersection(clip.scissor) {
                    fl.barrier(FlattenedBlurRegion {
                        rect,
                        blur_radius: blur.blur_radius,
                    });
                }
            }
            Primitive::EffectQuad(effect) => {
                let clip = *fl.clips.last().expect("root clip");
                if let Some(bounds) = effect.rect.intersection(clip.scissor) {
                    let key = fl.place(PrimKind::Effect, bounds);
                    out.effect_quads.push(ClippedEffectQuad {
                        key,
                        instance: EffectQuadInstance {
                            bounds: [
                                effect.rect.x,
                                effect.rect.y,
                                effect.rect.width,
                                effect.rect.height,
                            ],
                            color_a: color_to_linear(effect.color_a),
                            color_b: color_to_linear(effect.color_b),
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
                let clip = *fl.clips.last().expect("root clip");
                if let Some(bounds) = img.rect.intersection(clip.scissor) {
                    let key = fl.place(PrimKind::Image, bounds);
                    out.images.push(ClippedImage {
                        key,
                        primitive: img.clone(),
                        clip: clip.scissor,
                    });
                }
            }
            Primitive::Icon(icon) => {
                let clip = *fl.clips.last().expect("root clip");
                let rect = crate::Rect {
                    x: icon.rect.x.round(),
                    y: icon.rect.y.round(),
                    width: icon.rect.width.round(),
                    height: icon.rect.height.round(),
                };
                if let Some(bounds) = rect.intersection(clip.scissor) {
                    let px_size = icon.rect.width.max(icon.rect.height).ceil() as u32;
                    let cache_key = crate::icons::cache_key(&icon.name, px_size, icon.color);
                    // Only rasterize (and copy RGBA out of the icon cache)
                    // when the texture is not on the GPU yet; once
                    // uploaded, the cache key alone is enough to draw.
                    let (rgba, w, h) = if image_cache.contains_key(&cache_key) {
                        (Vec::new(), 0, 0)
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
                    });
                }
            }
            Primitive::ClipStart(ClipPrimitive { rect, corner_radii }) => {
                let next = fl
                    .clips
                    .last()
                    .and_then(|clip| clip.push(*rect, *corner_radii))
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
    /// Scissor batches for shadow, effect, and quad steps. Each step's range
    /// indexes the instance array of its own kind.
    quad_cmds: Vec<QuadDrawCommand>,
    image_cmds: Vec<ImageDrawCommand>,
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
            tint: [1.0, 1.0, 1.0, 1.0],
        }
    }));
    out.quad_cmds.clear();
    out.image_cmds.clear();
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
    use crate::fonts::FontSettings;

    #[test]
    fn scissor_rect_clamps_to_render_target() {
        let clip = Rect {
            x: 807.0,
            y: 118.0,
            width: 844.0,
            height: 865.0,
        };

        assert_eq!(scissor_rect(clip, 1650, 1050), Some((807, 118, 843, 865)));
    }

    #[test]
    fn scissor_rect_skips_clips_outside_render_target() {
        let clip = Rect {
            x: 1650.0,
            y: 118.0,
            width: 10.0,
            height: 865.0,
        };

        assert_eq!(scissor_rect(clip, 1650, 1050), None);
    }

    #[test]
    fn child_quad_inherits_rounded_clip_from_parent() {
        use crate::scene::{ClipPrimitive, Primitive, RoundedRectPrimitive, Scene};
        use quark::Color;

        let viewport = Rect {
            x: 0.0,
            y: 0.0,
            width: 400.0,
            height: 36.0,
        };
        let cluster = Rect {
            x: 0.0,
            y: 0.0,
            width: 400.0,
            height: 36.0,
        };
        let child = Rect {
            x: 300.0,
            y: 0.0,
            width: 100.0,
            height: 36.0,
        };

        let mut scene = Scene::default();
        // Parent bg (self-rounded, not clipped by anything)
        scene.push(Primitive::RoundedRect(RoundedRectPrimitive {
            rect: cluster,
            corner_radii: [6.0; 4],
            color: Color::rgba(100, 100, 100, 255),
        }));
        // Parent's rounded clip
        scene.push(Primitive::ClipStart(ClipPrimitive {
            rect: cluster,
            corner_radii: [6.0; 4],
        }));
        // Child bg (square corners, should inherit rounded clip)
        scene.push(Primitive::RoundedRect(RoundedRectPrimitive {
            rect: child,
            corner_radii: [0.0; 4],
            color: Color::rgba(200, 200, 0, 255),
        }));
        scene.push(Primitive::ClipEnd);

        let flat = flatten_scene(&scene, viewport, &ImageCache::new());
        let quads: Vec<&QuadInstance> = flat.quads.iter().map(|q| &q.instance).collect();

        // Expect 2 quads: the parent bg and the child bg.
        assert_eq!(quads.len(), 2, "expected 2 quads, got {}", quads.len());

        // Parent's own rect is drawn before the clip is pushed — should have
        // no rounded-clip attribution (clip_radii all zero).
        let parent_quad = quads[0];
        assert_eq!(
            parent_quad.clip_radii, [0.0; 4],
            "parent bg (before ClipStart) should have no rounded clip: {:?}",
            parent_quad.clip_radii,
        );

        // Child is inside the rounded clip — should inherit clip_bounds =
        // cluster rect and clip_radii = [6; 4].
        let child_quad = quads[1];
        assert_eq!(
            child_quad.clip_radii,
            [6.0, 6.0, 6.0, 6.0],
            "child bg should inherit rounded clip radii from parent",
        );
        assert_eq!(
            child_quad.clip_bounds,
            [cluster.x, cluster.y, cluster.width, cluster.height],
            "child bg should inherit rounded clip bounds from parent",
        );
    }

    // -- Headless GPU helpers ------------------------------------------------

    /// Headless renderer, or `None` when no adapter exists (a failure when
    /// `QUARK_REQUIRE_GPU` is set).
    fn gpu_renderer(width: u32, height: u32) -> Option<Renderer> {
        match Renderer::new_headless(width, height, 1.0, &FontSettings::default()) {
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
        let mut renderer = gpu_renderer(width, height)?;
        let pixels = renderer
            .render_to_rgba(scene, width, height, 1.0)
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
        Primitive::TextRun(TextPrimitive {
            rect,
            text: text.into(),
            color: quark::Color::rgba(255, 255, 255, 255),
            font_size: 16.0,
            font_kind: crate::scene::FontKind::Ui,
            font_weight: crate::scene::FontWeight::Normal,
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
}

// Text preparation, caching, and color helpers are in text.rs
// Shader source constants are in shaders.rs
