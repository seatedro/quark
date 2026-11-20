//! Recording a target's passes: its draw steps in order, split at blur
//! regions and, when glyphs overflow the atlas, at each overflow ordinal.

use std::ops::Range;

use super::batches::FrameBatches;
use super::flatten::{DrawStep, PrimKind};
use super::frame::TargetFrame;
use super::images::SharedImages;
use super::pipelines::Pipelines;
use super::pools::TransientBufferPool;
use super::text_runs::FrameText;
use super::{RenderError, Renderer};
use crate::scene::{Rect, SurfaceBackground};
use crate::text::color_to_unit;
use crate::text_engine::Viewport;

/// Instance buffers of one target's frame, one per pipeline.
#[derive(Default)]
pub(super) struct FrameBuffers {
    shadow: Option<wgpu::Buffer>,
    effect: Option<wgpu::Buffer>,
    quad: Option<wgpu::Buffer>,
    image: Option<wgpu::Buffer>,
    path: Option<wgpu::Buffer>,
    layer: Option<wgpu::Buffer>,
}

impl FrameBuffers {
    /// Upload `batches`' instances into buffers of `pool`.
    pub(super) fn upload(pool: &mut TransientBufferPool, batches: &FrameBatches) -> Self {
        Self {
            shadow: pool.upload("quark_shadow_instances", &batches.shadows),
            effect: pool.upload("quark_effect_quad_instances", &batches.effects),
            quad: pool.upload("quark_quad_instances", &batches.quads),
            image: pool.upload("quark_image_blit", &batches.images),
            path: pool.upload("quark_path_instances", &batches.paths),
            layer: pool.upload("quark_layer_instances", &batches.layers),
        }
    }
}

/// What encoding one target needs.
pub(super) struct EncodeTarget<'a> {
    pub(super) frame: &'a TargetFrame,
    /// The pipelines of the target's format.
    pub(super) pipelines: &'a Pipelines,
    pub(super) uniform: &'a wgpu::BindGroup,
    pub(super) glyph_viewport: &'a Viewport,
    /// Every target of the frame, for the layers this one composites.
    pub(super) frames: &'a [TargetFrame],
    pub(super) segments: Option<&'a wgpu::BindGroup>,
    pub(super) clear: wgpu::Color,
}

impl EncodeTarget<'_> {
    /// The target's size, which pooled textures may exceed.
    pub(super) fn size(&self) -> (u32, u32) {
        (self.frame.width, self.frame.height)
    }
}

impl Renderer {
    /// Record one target's passes. Without blur this is one pass on `view`.
    /// With blur, steps draw into an offscreen scene texture; at each blur
    /// step the region is blurred and composited back before later steps
    /// draw, and the scene is finally copied to `view`.
    pub(super) fn encode_steps(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        t: &EncodeTarget<'_>,
        text: &mut FrameText,
    ) -> Result<(), RenderError> {
        let steps = &t.frame.flat.steps;
        let (width, height) = t.size();
        let blur = t.frame.blur.as_ref();
        let draw_view = blur.map_or(view, |b| b.scene_view());
        let mut load = wgpu::LoadOp::Clear(t.clear);
        let mut blurs_done = 0u32;
        let mut start = 0;
        loop {
            let end = steps[start..]
                .iter()
                .position(|s| matches!(s, DrawStep::Blur(_)))
                .map_or(steps.len(), |p| start + p);
            {
                let mut pass = begin_pass(encoder, "quark_frame_pass", draw_view, load, t.size());
                if let Some(blur) = blur
                    && blurs_done > 0
                {
                    blur.composite_region(&mut pass, t, blurs_done - 1);
                }
                for (step, cmds) in steps[start..end]
                    .iter()
                    .zip(&t.frame.batches.step_cmds[start..end])
                {
                    let cmds = cmds.clone();
                    // Text of an overflowing frame draws below; every
                    // other step draws in this pass as it is.
                    let (
                        DrawStep::Batch {
                            kind: PrimKind::Text,
                            ..
                        },
                        FrameText::Overflow { reached },
                    ) = (step, &mut *text)
                    else {
                        self.draw_step(&mut pass, step, cmds, t, *text)?;
                        continue;
                    };
                    // Each overflow ordinal's uploads overwrite the overflow
                    // pages the previous one drew from: end the pass, copy,
                    // and go on in a new pass over the same target.
                    for run in cmds.start as usize..cmds.end as usize {
                        let (renderer, slot) = self.text_runs.renderer(t.frame.text_base + run);
                        let mut from = 0;
                        for (k, ordinal) in renderer.segment_ordinals().enumerate() {
                            if ordinal <= *reached {
                                continue;
                            }
                            pass.set_scissor_rect(0, 0, width, height);
                            renderer.render_at(t.glyph_viewport, &mut pass, slot, from..k)?;
                            from = k;
                            drop(pass);
                            for next in *reached + 1..=ordinal {
                                self.atlas.record_overflow(encoder, next);
                            }
                            *reached = ordinal;
                            pass = begin_pass(
                                encoder,
                                "quark_frame_pass",
                                draw_view,
                                wgpu::LoadOp::Load,
                                t.size(),
                            );
                        }
                        pass.set_scissor_rect(0, 0, width, height);
                        renderer.render_at(t.glyph_viewport, &mut pass, slot, from..usize::MAX)?;
                    }
                }
            }
            load = wgpu::LoadOp::Load;
            if end == steps.len() {
                break;
            }
            let Some(blur) = blur else {
                break;
            };
            // The scratch targets hold linear light.
            blur.blur_region(encoder, t, &self.gpu.pipelines(false).blur, blurs_done);
            blurs_done += 1;
            start = end + 1;
        }
        if let Some(blur) = blur {
            blur.composite_scene(encoder, view, t, blurs_done);
        }
        Ok(())
    }

    /// Draw one batch step, its commands `cmds`, into `pass`.
    fn draw_step(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        step: &DrawStep,
        cmds: Range<u32>,
        t: &EncodeTarget<'_>,
        text: FrameText,
    ) -> Result<(), RenderError> {
        let DrawStep::Batch { kind, .. } = step else {
            return Ok(());
        };
        let (width, height) = t.size();
        let batches = &t.frame.batches;
        let buffers = &t.frame.buffers;
        let cmds = cmds.start as usize..cmds.end as usize;
        let pipelines = t.pipelines;
        let (pipeline, buffer) = match kind {
            PrimKind::Shadow => (&pipelines.shadow, &buffers.shadow),
            PrimKind::Effect => (&pipelines.effect, &buffers.effect),
            PrimKind::Quad => (&pipelines.quad, &buffers.quad),
            PrimKind::Path => (&pipelines.path, &buffers.path),
            PrimKind::Layer => {
                let Some(buffer) = &buffers.layer else {
                    return Ok(());
                };
                pass.set_pipeline(&pipelines.layer);
                pass.set_bind_group(0, t.uniform, &[]);
                pass.set_vertex_buffer(0, buffer.slice(..));
                for command in &batches.layer_cmds[cmds] {
                    let Some(bind) = t
                        .frames
                        .get(command.target)
                        .and_then(|frame| frame.layer_bind.as_ref())
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
                let images = SharedImages::lock(&self.images);
                pass.set_pipeline(&pipelines.blit);
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
                    pass.draw(0..4, command.instances.clone());
                }
                return Ok(());
            }
            PrimKind::Text => {
                if text == FrameText::Skipped {
                    return Ok(());
                }
                pass.set_scissor_rect(0, 0, width, height);
                for run in cmds {
                    let (renderer, slot) = self.text_runs.renderer(t.frame.text_base + run);
                    renderer.render_at(t.glyph_viewport, pass, slot, 0..usize::MAX)?;
                }
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
            pass.draw(0..4, command.instances.clone());
        }
        Ok(())
    }
}

/// A pass storing into `view`, drawing into its top-left `width` x
/// `height` corner: pooled textures (layers, blur scratch) can be larger
/// than the target they hold.
pub(super) fn begin_pass<'e>(
    encoder: &'e mut wgpu::CommandEncoder,
    label: &'static str,
    view: &wgpu::TextureView,
    load: wgpu::LoadOp<wgpu::Color>,
    (width, height): (u32, u32),
) -> wgpu::RenderPass<'e> {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
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
    });
    pass.set_viewport(0.0, 0.0, width as f32, height as f32, 0.0, 1.0);
    pass
}

/// The scissor rect of `clip` inside a `viewport_w` x `viewport_h`
/// target, in whole pixels; `None` when nothing of it shows.
pub(super) fn scissor_rect(
    clip: Rect,
    viewport_w: u32,
    viewport_h: u32,
) -> Option<(u32, u32, u32, u32)> {
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

/// The clear color of a window target showing `background`, premultiplied
/// in the target's space.
pub(super) fn clear_color(background: SurfaceBackground, encoded: bool) -> wgpu::Color {
    let SurfaceBackground::Opaque(color) = background else {
        return wgpu::Color::TRANSPARENT;
    };
    let [r, g, b, a] = color_to_unit(color).map(f64::from);
    let decode = |c: f64| {
        if encoded {
            c
        } else if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    wgpu::Color {
        r: decode(r) * a,
        g: decode(g) * a,
        b: decode(b) * a,
        a,
    }
}
