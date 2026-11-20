//! Blur regions: everything drawn before a region is blurred inside it by
//! a separable Gaussian, in scratch targets, and composited back before
//! later steps draw.

use super::Renderer;
use super::encode::{EncodeTarget, begin_pass};
use super::flatten::{DrawStep, FlattenedScene};
use super::instances::{BlitInstance, BlurInstance, SourceSpace};
use super::pools::{OffscreenTarget, TexturePool};
use crate::scene::Rect;

/// The scratch targets and instances of one target's blur regions, held
/// while its frame is recorded. The target draws its steps into `scene`;
/// each region is blurred horizontally into `h`, then vertically into `v`,
/// and composited from there back into `scene`.
pub(super) struct BlurTargets {
    scene: Scratch,
    h: Scratch,
    v: Scratch,
    /// Two instances per region: horizontal then vertical.
    blur_instances: wgpu::Buffer,
    /// One instance per region (composite the blurred region), then a
    /// final full-target instance (copy the scene to the output).
    blit_instances: wgpu::Buffer,
}

/// A pooled target with the view it draws into and the bind group
/// sampling it.
struct Scratch {
    target: OffscreenTarget,
    view: wgpu::TextureView,
    bind: wgpu::BindGroup,
}

impl Scratch {
    fn new(pool: &mut TexturePool, target: OffscreenTarget) -> Self {
        Self {
            view: pool.view(&target).clone(),
            bind: pool.bind_group(&target),
            target,
        }
    }
}

impl BlurTargets {
    /// The texture the target's steps draw into.
    pub(super) fn scene_view(&self) -> &wgpu::TextureView {
        &self.scene.view
    }

    /// Draw blurred region `region` over the scene, in `pass` on it.
    pub(super) fn composite_region(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        t: &EncodeTarget<'_>,
        region: u32,
    ) {
        let (width, height) = t.size();
        pass.set_pipeline(&t.pipelines.blit);
        pass.set_bind_group(0, t.uniform, &[]);
        pass.set_bind_group(1, &self.v.bind, &[]);
        pass.set_vertex_buffer(0, self.blit_instances.slice(..));
        pass.set_scissor_rect(0, 0, width, height);
        pass.draw(0..4, region..region + 1);
    }

    /// Blur region `region` of the scene with `pipeline`: horizontally
    /// into `h`, then vertically into `v`.
    pub(super) fn blur_region(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        t: &EncodeTarget<'_>,
        pipeline: &wgpu::RenderPipeline,
        region: u32,
    ) {
        for (direction, source, dest) in [(0, &self.scene, &self.h), (1, &self.h, &self.v)] {
            let mut pass = begin_pass(
                encoder,
                "quark_blur_pass",
                &dest.view,
                wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                t.size(),
            );
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, t.uniform, &[]);
            pass.set_bind_group(1, &source.bind, &[]);
            pass.set_vertex_buffer(0, self.blur_instances.slice(..));
            let instance = region * 2 + direction;
            pass.draw(0..4, instance..instance + 1);
        }
    }

    /// Copy the finished scene, after its `regions` blur regions, into
    /// `view`.
    pub(super) fn composite_scene(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        t: &EncodeTarget<'_>,
        regions: u32,
    ) {
        let mut pass = begin_pass(
            encoder,
            "quark_composite_pass",
            view,
            wgpu::LoadOp::Clear(t.clear),
            t.size(),
        );
        pass.set_pipeline(&t.pipelines.blit);
        pass.set_bind_group(0, t.uniform, &[]);
        pass.set_bind_group(1, &self.scene.bind, &[]);
        pass.set_vertex_buffer(0, self.blit_instances.slice(..));
        pass.draw(0..4, regions..regions + 1);
    }

    /// Return the scratch targets to `pool`.
    pub(super) fn release(self, pool: &mut TexturePool) {
        for scratch in [self.scene, self.h, self.v] {
            pool.release(scratch.target);
        }
    }
}

impl Renderer {
    /// Acquire scratch targets and upload blur instances when `flat`, a
    /// `width` x `height` target, has blur regions.
    pub(super) fn prepare_blur(
        &mut self,
        flat: &FlattenedScene,
        (width, height): (u32, u32),
        encoded: bool,
    ) -> Option<BlurTargets> {
        if !flat.steps.iter().any(|s| matches!(s, DrawStep::Blur(_))) {
            return None;
        }
        // The scene in the target's own space; the blur filters linear
        // light, so its scratch targets hold that.
        let pool = &mut self.texture_pool;
        let scene = pool.acquire_in(width, height, encoded);
        let h = pool.acquire(width, height);
        let v = pool.acquire(width, height);
        // Pooled textures can be larger than the frame and than each other;
        // passes on them set a viewport of the frame size, so each pass's
        // UVs divide by the size of the texture it reads.
        let uv_in = |target: &OffscreenTarget, r: Rect| {
            let (tw, th) = (target.width as f32, target.height as f32);
            [r.x / tw, r.y / th, r.right() / tw, r.bottom() / th]
        };

        let mut blur_instances = Vec::new();
        let mut blit_instances = Vec::new();
        for step in &flat.steps {
            let DrawStep::Blur(region) = step else {
                continue;
            };
            let sigma = (region.blur_radius * 0.5).max(0.5);
            let br = region.rect;
            let bounds = [br.x, br.y, br.width, br.height];
            // The vertical pass samples up to its kernel's reach above and
            // below the region, so the horizontal pass covers that margin
            // too; otherwise the region's top and bottom edges blend with
            // the cleared, transparent texels around it.
            let reach = blur_reach(sigma) + 1.0;
            let top = (br.y - reach).max(0.0);
            let bottom = (br.bottom() + reach).min(height as f32);
            let band = Rect {
                y: top,
                height: bottom - top,
                ..br
            };
            blur_instances.push(BlurInstance {
                bounds: [br.x, top, br.width, bottom - top],
                uv_rect: uv_in(&scene, band),
                blur_params: [1.0, 0.0, sigma, SourceSpace::between(encoded, false).code()],
            });
            blur_instances.push(BlurInstance {
                bounds,
                uv_rect: uv_in(&h, br),
                blur_params: [0.0, 1.0, sigma, SourceSpace::Same.code()],
            });
            blit_instances.push(BlitInstance {
                bounds,
                uv_rect: uv_in(&v, br),
                tint: [1.0; 4],
                radii: region.corner_radii,
                space: SourceSpace::between(false, encoded).attribute(),
            });
        }
        let frame = Rect {
            x: 0.0,
            y: 0.0,
            width: width as f32,
            height: height as f32,
        };
        blit_instances.push(BlitInstance {
            bounds: [0.0, 0.0, width as f32, height as f32],
            uv_rect: uv_in(&scene, frame),
            tint: [1.0; 4],
            radii: [0.0; 4],
            space: SourceSpace::Same.attribute(),
        });
        let instances = &mut self.instance_buffer_pool;
        let blur_instances = instances.upload("quark_blur_instances", &blur_instances)?;
        let blit_instances = instances.upload("quark_blur_blit", &blit_instances)?;
        Some(BlurTargets {
            scene: Scratch::new(pool, scene),
            h: Scratch::new(pool, h),
            v: Scratch::new(pool, v),
            blur_instances,
            blit_instances,
        })
    }
}

/// How far, in pixels, a blur pass of `sigma` samples from the pixel it
/// writes: up to the shader's `BLUR_TAPS` samples each side, spread so they
/// reach three sigma.
fn blur_reach(sigma: f32) -> f32 {
    const BLUR_TAPS: f32 = 12.0;
    let taps = BLUR_TAPS.min((3.0 * sigma).ceil());
    taps * (3.0 * sigma / BLUR_TAPS).max(1.0)
}
