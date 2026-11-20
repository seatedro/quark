//! Bind group layouts and render pipelines, one set per target format.
//!
//! A frame composites either in linear light or, web-compatible, in
//! encoded sRGB. Linear-light targets use the shared sRGB format, so the
//! hardware decodes and encodes around blending; encoded targets use its
//! non-sRGB twin, so blending sees the encoded values. Every pipeline
//! exists once per format, and each target draws with its own format's.

use super::instances::{
    BlitInstance, BlurInstance, EffectQuadInstance, Instance, LayerInstance, PathInstance,
    QuadInstance, ShadowInstance,
};
use crate::shaders::{
    BLIT_SHADER, BLUR_SHADER, EFFECT_SHADER, LAYER_SHADER, PATH_SHADER, QUAD_SHADER, SHADOW_SHADER,
};

/// Bind group layouts every pipeline set shares.
pub(super) struct Layouts {
    /// The viewport uniform, group 0 of every pipeline.
    pub(super) viewport: wgpu::BindGroupLayout,
    /// A filtered texture and its sampler: images, layers, blur sources.
    pub(super) texture: wgpu::BindGroupLayout,
    /// The path segment texture.
    pub(super) segment: wgpu::BindGroupLayout,
}

impl Layouts {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        let viewport = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
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
        let texture = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
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
        let segment = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
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
        Self {
            viewport,
            texture,
            segment,
        }
    }
}

/// Every quark pipeline, built for one target format.
#[derive(Clone)]
pub(super) struct Pipelines {
    pub(super) quad: wgpu::RenderPipeline,
    pub(super) shadow: wgpu::RenderPipeline,
    pub(super) effect: wgpu::RenderPipeline,
    pub(super) blit: wgpu::RenderPipeline,
    /// Overwrites instead of blending.
    pub(super) blur: wgpu::RenderPipeline,
    pub(super) layer: wgpu::RenderPipeline,
    pub(super) path: wgpu::RenderPipeline,
}

impl Pipelines {
    pub(super) fn new(
        device: &wgpu::Device,
        layouts: &Layouts,
        format: wgpu::TextureFormat,
    ) -> Self {
        let viewport = &layouts.viewport;
        let textured = [viewport, &layouts.texture];
        let blended = PipelineTarget {
            format,
            blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
        };
        Self {
            quad: instanced_pipeline::<QuadInstance>(
                device,
                ("quark_quad", QUAD_SHADER),
                ("vs_quad", "fs_quad"),
                &[viewport],
                blended,
            ),
            shadow: instanced_pipeline::<ShadowInstance>(
                device,
                ("quark_shadow", SHADOW_SHADER),
                ("vs_shadow", "fs_shadow"),
                &[viewport],
                blended,
            ),
            effect: instanced_pipeline::<EffectQuadInstance>(
                device,
                ("quark_effect_quad", EFFECT_SHADER),
                ("vs_effect", "fs_effect"),
                &[viewport],
                blended,
            ),
            blit: instanced_pipeline::<BlitInstance>(
                device,
                ("quark_blit", BLIT_SHADER),
                ("vs_blit", "fs_blit"),
                &textured,
                blended,
            ),
            blur: instanced_pipeline::<BlurInstance>(
                device,
                ("quark_blur", BLUR_SHADER),
                ("vs_blur", "fs_blur"),
                &textured,
                PipelineTarget {
                    blend: None,
                    ..blended
                },
            ),
            layer: instanced_pipeline::<LayerInstance>(
                device,
                ("quark_layer", LAYER_SHADER),
                ("vs_layer", "fs_layer"),
                &textured,
                blended,
            ),
            path: instanced_pipeline::<PathInstance>(
                device,
                ("quark_path", PATH_SHADER),
                ("vs_path", "fs_path"),
                &[viewport, &layouts.segment],
                blended,
            ),
        }
    }
}

/// The color target a pipeline draws premultiplied color into, blending
/// with `blend` (none overwrites).
#[derive(Clone, Copy)]
struct PipelineTarget {
    format: wgpu::TextureFormat,
    blend: Option<wgpu::BlendState>,
}

/// An instanced triangle-strip pipeline drawing one quad per `I`.
fn instanced_pipeline<I: Instance>(
    device: &wgpu::Device,
    (label, source): (&'static str, &'static str),
    (vertex, fragment): (&'static str, &'static str),
    bind_group_layouts: &[&wgpu::BindGroupLayout],
    target: PipelineTarget,
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
            buffers: &[I::layout()],
        },
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some(fragment),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: target.format,
                blend: target.blend,
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

/// A bind group sampling `view` with `sampler`, for [`Layouts::texture`].
pub(super) fn texture_bind_group(
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

/// A uniform buffer for one [`ViewportUniform`](super::instances::ViewportUniform)
/// and its bind group, for [`Layouts::viewport`].
pub(super) fn viewport_uniform(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    label: &'static str,
) -> (wgpu::Buffer, wgpu::BindGroup) {
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: size_of::<super::instances::ViewportUniform>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(label),
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: buffer.as_entire_binding(),
        }],
    });
    (buffer, bind_group)
}
