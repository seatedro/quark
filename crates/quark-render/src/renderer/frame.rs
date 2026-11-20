//! One frame from scene to commands: plan layers and acquire their
//! targets, flatten and batch every target, then upload, prepare text, and
//! record each target's passes into one encoder.

use super::batches::FrameBatches;
use super::blur::BlurTargets;
use super::encode::{EncodeTarget, FrameBuffers, clear_color};
use super::flatten::{FlattenTarget, FlattenedScene};
use super::gpu_context::GpuShared;
use super::images::SharedImages;
use super::instances::ViewportUniform;
use super::layers::{LayerMode, expand, needs_expansion, plan_layers};
use super::pipelines::viewport_uniform;
use super::pools::OffscreenTarget;
use super::text_runs::FrameText;
use super::{RenderError, Renderer};
use crate::scene::{Rect, Scene};
use crate::text_engine::{Resolution, Viewport};
use quark_text::TextSystem;

/// Texels per row of the path segment texture; the path shader's
/// `SEGMENT_ROW` must match.
const SEGMENT_TEXTURE_WIDTH: u32 = 1024;

/// One render target of a frame: the window, or an offscreen layer. Kept on
/// the renderer so its vectors keep their capacity between frames.
#[derive(Default)]
pub(super) struct TargetFrame {
    pub(super) flat: FlattenedScene,
    pub(super) batches: FrameBatches,
    pub(super) width: u32,
    pub(super) height: u32,
    /// The target holds encoded sRGB values (web-compatible compositing).
    pub(super) encoded: bool,
    /// The pooled texture an offscreen layer renders into this frame.
    pub(super) layer: Option<OffscreenTarget>,
    /// Index of this target's first text run among the frame's.
    pub(super) text_base: usize,
    // Held only while the frame is recorded:
    pub(super) buffers: FrameBuffers,
    pub(super) blur: Option<BlurTargets>,
    /// Bind group sampling `layer`, for its composite in the parent.
    pub(super) layer_bind: Option<wgpu::BindGroup>,
}

/// The viewport uniform and glyph viewport of every target: the window's
/// first, then each offscreen layer's by target number. Slots are created
/// as frames first need them, and kept.
pub(super) struct Viewports {
    targets: Vec<TargetViewport>,
}

struct TargetViewport {
    uniform: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    glyphs: Viewport,
}

impl TargetViewport {
    fn new(gpu: &GpuShared, label: &'static str) -> Self {
        let (uniform, bind_group) = viewport_uniform(&gpu.device, &gpu.layouts.viewport, label);
        Self {
            uniform,
            bind_group,
            glyphs: Viewport::new(&gpu.device, &gpu.glyph_cache),
        }
    }
}

impl Viewports {
    pub(super) fn new(gpu: &GpuShared) -> Self {
        Self {
            targets: vec![TargetViewport::new(gpu, "quark_viewport_uniform")],
        }
    }

    /// Size every target's viewports to its frame in `frames`, at
    /// animation time `time`.
    fn update(&mut self, gpu: &GpuShared, frames: &[TargetFrame], time: f32) {
        while self.targets.len() < frames.len() {
            self.targets
                .push(TargetViewport::new(gpu, "quark_layer_viewport_uniform"));
        }
        for (viewport, frame) in self.targets.iter_mut().zip(frames) {
            let uniform = ViewportUniform::new(frame.width, frame.height, time, frame.encoded);
            gpu.queue
                .write_buffer(&viewport.uniform, 0, bytemuck::bytes_of(&uniform));
            viewport.glyphs.set_encoded(&gpu.queue, frame.encoded);
            viewport.glyphs.update(
                &gpu.queue,
                Resolution {
                    width: frame.width,
                    height: frame.height,
                },
            );
        }
    }

    /// The viewport uniform bind group of target `target`.
    pub(super) fn uniform(&self, target: usize) -> &wgpu::BindGroup {
        &self.targets[target].bind_group
    }

    /// The glyph viewport of target `target`.
    pub(super) fn glyphs(&self, target: usize) -> &Viewport {
        &self.targets[target].glyphs
    }

    /// The glyph viewports of the first `count` targets.
    pub(super) fn glyphs_mut(&mut self, count: usize) -> impl Iterator<Item = &mut Viewport> {
        self.targets[..count]
            .iter_mut()
            .map(|target| &mut target.glyphs)
    }
}

/// Every path segment of a frame, one `[x0, y0, x1, y1]` texel each.
pub(super) struct SegmentTexture {
    texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
    rows: u32,
}

impl Renderer {
    /// Start a frame drawn at animation time `time`, in seconds.
    pub(super) fn begin_frame(&mut self, time: f32) {
        self.texture_pool.begin_frame();
        self.instance_buffer_pool.begin_frame();
        self.time = time;
    }

    /// End a frame once its commands are submitted: unpin its glyphs and
    /// trim what no recent frame used.
    pub(super) fn end_frame(&mut self) {
        self.atlas.end_frame();
        self.texture_pool.trim_unused();
    }

    /// Plan the frame's layers, acquire their textures, and flatten the
    /// window and every offscreen layer into their targets.
    pub(super) fn flatten(&mut self, scene: &Scene, width: u32, height: u32) {
        let viewport = Rect {
            x: 0.0,
            y: 0.0,
            width: width as f32,
            height: height as f32,
        };
        // Layer planning reads every primitive a layer groups, so chunks a
        // layer groups, or that start one, draw from their primitives.
        let mut expanded = std::mem::take(&mut self.expanded);
        let prims = if needs_expansion(&scene.primitives) {
            expanded.clear();
            let mut depth = 0;
            for primitive in &scene.primitives {
                expand(primitive.clone(), &mut depth, &mut expanded);
            }
            &expanded[..]
        } else {
            &scene.primitives[..]
        };
        let encoded = self.options.encoded();
        let offscreen = plan_layers(
            prims,
            viewport,
            encoded,
            &mut self.plans,
            &mut self.layer_scratch,
        );
        self.active_frames = 1 + offscreen;
        if self.frames.len() < self.active_frames {
            self.frames
                .resize_with(self.active_frames, TargetFrame::default);
        }
        let root = &mut self.frames[0];
        (root.width, root.height, root.encoded) = (width, height, encoded);
        // Textures first, so composites know their texture sizes.
        for plan in &mut self.plans {
            let LayerMode::Offscreen(layer) = &mut plan.mode else {
                continue;
            };
            let texture = self
                .texture_pool
                .acquire_in(layer.size.0, layer.size.1, plan.encoded);
            layer.texture = (texture.width, texture.height);
            let frame = &mut self.frames[layer.target];
            (frame.width, frame.height) = layer.size;
            frame.encoded = plan.encoded;
            frame.layer = Some(texture);
        }

        let images = SharedImages::lock(&self.images);
        self.flattener.begin_frame(self.options.text_rendering);
        let root = FlattenTarget {
            prims,
            range: 0..prims.len(),
            origin: (0.0, 0.0),
            viewport,
            encoded,
        };
        self.flattener
            .flatten(&root, &self.plans, &images.cache, &mut self.frames[0].flat);
        for plan in &self.plans {
            let LayerMode::Offscreen(layer) = plan.mode else {
                continue;
            };
            let frame = &mut self.frames[layer.target];
            let region = Rect {
                x: 0.0,
                y: 0.0,
                width: frame.width as f32,
                height: frame.height as f32,
            };
            // An isolated group shows nothing outside its bounds.
            let viewport = match plan.isolate {
                Some(bounds) => bounds
                    .offset(-layer.origin.0, -layer.origin.1)
                    .intersection(region)
                    .unwrap_or_default(),
                None => region,
            };
            let target = FlattenTarget {
                prims,
                range: plan.start + 1..plan.end,
                origin: layer.origin,
                viewport,
                encoded: plan.encoded,
            };
            self.flattener
                .flatten(&target, &self.plans, &images.cache, &mut frame.flat);
        }
        drop(images);
        // Let go of the chunks and layouts; the flattened frame holds what
        // it draws.
        expanded.clear();
        self.expanded = expanded;
        let mut runs = 0;
        for frame in &mut self.frames[..self.active_frames] {
            frame.batches.build(&frame.flat);
            frame.text_base = runs;
            runs += frame.batches.text_runs.len();
        }
    }

    /// Upload, prepare, and encode the flattened frame into `encoder`,
    /// targeting `target`. Everything goes into one encoder: each text run
    /// has its own `TextRenderer`, so preparing one cannot overwrite the
    /// vertices another run's pass reads.
    pub(super) fn record_frame(
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
        SharedImages::lock(&self.images).begin_frame();
        for frame in frames.iter() {
            for image in &frame.flat.images {
                self.ensure_image_uploaded(&image.primitive);
            }
        }
        let segments = self.upload_segments();
        for frame in frames.iter_mut() {
            frame.buffers = FrameBuffers::upload(&mut self.instance_buffer_pool, &frame.batches);
        }
        self.viewports.update(&self.gpu.inner, frames, self.time);

        // Prepare every text run before recording any pass.
        let mut frame_text = self.prepare_frame_text(frames, text);
        if frame_text != FrameText::Skipped {
            let viewports = self.viewports.glyphs_mut(frames.len());
            self.text_runs.upload(&self.device, &self.queue, viewports);
        }
        // Glyph uploads land ahead of every pass of the frame.
        self.atlas.record_uploads(&self.queue, encoder);

        for frame in frames.iter_mut() {
            frame.blur = self.prepare_blur(&frame.flat, (frame.width, frame.height), frame.encoded);
            frame.layer_bind = frame
                .layer
                .as_ref()
                .map(|texture| self.texture_pool.bind_group(texture));
        }

        // A layer's target number is above its parent's, so encoding from
        // the last target down draws every layer before its composite.
        let mut result = Ok(());
        for index in (0..frames.len()).rev() {
            let frame = &frames[index];
            let (view, clear) = match &frame.layer {
                Some(texture) => (self.texture_pool.view(texture), wgpu::Color::TRANSPARENT),
                None => (output, clear_color(self.background, frame.encoded)),
            };
            let target = EncodeTarget {
                frame,
                pipelines: self.gpu.pipelines(frame.encoded),
                uniform: self.viewports.uniform(index),
                glyph_viewport: self.viewports.glyphs(index),
                frames,
                segments: segments.as_ref(),
                clear,
            };
            result = self.encode_steps(encoder, view, &target, &mut frame_text);
            if result.is_err() {
                break;
            }
        }
        for frame in frames.iter_mut() {
            frame.buffers = FrameBuffers::default();
            frame.layer_bind = None;
            if let Some(blur) = frame.blur.take() {
                blur.release(&mut self.texture_pool);
            }
        }
        SharedImages::lock(&self.images).trim();
        result
    }

    /// Copy this frame's path segments into the segment texture, growing
    /// it as needed. `None` when no path draws.
    fn upload_segments(&mut self) -> Option<wgpu::BindGroup> {
        let segments = self.flattener.segments_mut();
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
                layout: &self.gpu.inner.layouts.segment,
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
}
