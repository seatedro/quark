//! A flattened scene as GPU batches: instance arrays in draw order, one
//! per pipeline, and the scissor-batched draw commands of each step.

use std::ops::Range;

use super::flatten::{DrawStep, FlattenedScene, PrimKind};
use super::instances::{
    BlitInstance, EffectQuadInstance, LayerInstance, PathInstance, QuadInstance, ShadowInstance,
    SourceSpace,
};
use crate::scene::Rect;

/// Per-frame instance data in draw order, one array per pipeline. Kept on the
/// renderer so the vectors keep their capacity between frames.
#[derive(Default)]
pub(super) struct FrameBatches {
    pub(super) shadows: Vec<ShadowInstance>,
    pub(super) effects: Vec<EffectQuadInstance>,
    pub(super) quads: Vec<QuadInstance>,
    pub(super) images: Vec<BlitInstance>,
    pub(super) paths: Vec<PathInstance>,
    pub(super) layers: Vec<LayerInstance>,
    /// Scissor batches for shadow, effect, quad, and path steps. Each step's
    /// range indexes the instance array of its own kind.
    pub(super) quad_cmds: Vec<QuadDrawCommand>,
    pub(super) image_cmds: Vec<ImageDrawCommand>,
    pub(super) layer_cmds: Vec<LayerDrawCommand>,
    /// Per step: a range into `quad_cmds`, `image_cmds`, `layer_cmds`, or
    /// `text_runs`.
    pub(super) step_cmds: Vec<Range<u32>>,
    pub(super) text_runs: Vec<TextRunItems>,
}

/// Consecutive instances sharing one scissor clip.
#[derive(Clone)]
pub(super) struct QuadDrawCommand {
    pub(super) instances: Range<u32>,
    pub(super) clip: Rect,
}

/// The composite of one offscreen layer.
#[derive(Clone, Copy)]
pub(super) struct LayerDrawCommand {
    pub(super) instance: u32,
    pub(super) clip: Rect,
    /// Frame index of the layer's target.
    pub(super) target: usize,
}

/// Consecutive instances of one cached image sharing one scissor clip.
#[derive(Clone)]
pub(super) struct ImageDrawCommand {
    pub(super) instances: Range<u32>,
    pub(super) cache_key: u64,
    pub(super) clip: Rect,
}

/// Texts of a step that prepare and draw together: plain runs, then rich
/// ones, as the step orders them. A run holds texts of one chunk only.
#[derive(Debug, Clone)]
pub(super) struct TextRunItems {
    pub(super) plain: Range<u32>,
    pub(super) rich: Range<u32>,
}

impl FrameBatches {
    /// Refill from `flat`.
    pub(super) fn build(&mut self, flat: &FlattenedScene) {
        self.shadows.clear();
        self.shadows
            .extend(flat.shadows.iter().map(|item| item.instance));
        self.effects.clear();
        self.effects
            .extend(flat.effect_quads.iter().map(|item| item.instance));
        self.quads.clear();
        self.quads
            .extend(flat.quads.iter().map(|item| item.instance));
        self.images.clear();
        self.images.extend(flat.images.iter().map(|item| {
            let r = item.primitive.rect;
            BlitInstance {
                bounds: [r.x, r.y, r.width, r.height],
                uv_rect: [0.0, 0.0, 1.0, 1.0],
                tint: [item.alpha; 4],
                radii: [0.0; 4],
                // Images live in sRGB textures, which sample as linear light.
                space: SourceSpace::Linear.attribute(),
            }
        }));
        self.paths.clear();
        self.paths
            .extend(flat.paths.iter().map(|item| item.instance));
        self.layers.clear();
        self.layers
            .extend(flat.layers.iter().map(|item| item.instance));
        self.quad_cmds.clear();
        self.image_cmds.clear();
        self.layer_cmds.clear();
        self.step_cmds.clear();
        self.text_runs.clear();

        for step in &flat.steps {
            let range = match step {
                DrawStep::Blur(_) => 0..0,
                DrawStep::Batch { kind, items, rich } => {
                    let (start, end) = (items.start as usize, items.end as usize);
                    let quad_cmds = &mut self.quad_cmds;
                    match kind {
                        PrimKind::Shadow => push_clip_batches(
                            flat.shadows[start..end].iter().map(|i| i.clip),
                            items.start,
                            quad_cmds,
                        ),
                        PrimKind::Effect => push_clip_batches(
                            flat.effect_quads[start..end].iter().map(|i| i.clip),
                            items.start,
                            quad_cmds,
                        ),
                        PrimKind::Quad => push_clip_batches(
                            flat.quads[start..end].iter().map(|i| i.clip),
                            items.start,
                            quad_cmds,
                        ),
                        PrimKind::Path => push_clip_batches(
                            flat.paths[start..end].iter().map(|i| i.clip),
                            items.start,
                            quad_cmds,
                        ),
                        PrimKind::Layer => self.push_layers(flat, items),
                        PrimKind::Image => self.push_images(flat, items),
                        PrimKind::Text => push_text_runs(flat, items, rich, &mut self.text_runs),
                    }
                }
            };
            self.step_cmds.push(range);
        }
    }

    /// Append one command per layer composite of a step.
    fn push_layers(&mut self, flat: &FlattenedScene, items: &Range<u32>) -> Range<u32> {
        let first = self.layer_cmds.len() as u32;
        let layers = &flat.layers[items.start as usize..items.end as usize];
        for (instance, layer) in (items.start..).zip(layers) {
            self.layer_cmds.push(LayerDrawCommand {
                instance,
                clip: layer.clip,
                target: layer.target,
            });
        }
        first..self.layer_cmds.len() as u32
    }

    /// Append the commands of an image step, batching consecutive
    /// instances of one image under one clip.
    fn push_images(&mut self, flat: &FlattenedScene, items: &Range<u32>) -> Range<u32> {
        let first = self.image_cmds.len();
        let images = &flat.images[items.start as usize..items.end as usize];
        for (index, image) in (items.start..).zip(images) {
            let key = image.primitive.cache_key;
            let own = self.image_cmds.len() > first;
            match self.image_cmds.last_mut() {
                Some(last) if own && last.cache_key == key && last.clip == image.clip => {
                    last.instances.end = index + 1;
                }
                _ => self.image_cmds.push(ImageDrawCommand {
                    instances: index..index + 1,
                    cache_key: key,
                    clip: image.clip,
                }),
            }
        }
        first as u32..self.image_cmds.len() as u32
    }
}

/// Append the runs of a text step, split where the chunk its texts come
/// from changes, and return the range of runs added.
fn push_text_runs(
    flat: &FlattenedScene,
    items: &Range<u32>,
    rich: &Range<u32>,
    out: &mut Vec<TextRunItems>,
) -> Range<u32> {
    let first = out.len();
    let mut current = None;
    let plain = flat.texts[items.start as usize..items.end as usize].iter();
    for (index, text) in (items.start..).zip(plain) {
        match out.last_mut() {
            Some(run) if current == Some(text.run) => run.plain.end = index + 1,
            _ => out.push(TextRunItems {
                plain: index..index + 1,
                rich: rich.start..rich.start,
            }),
        }
        current = Some(text.run);
    }
    let rich_texts = flat.rich_texts[rich.start as usize..rich.end as usize].iter();
    for (index, text) in (rich.start..).zip(rich_texts) {
        match out.last_mut() {
            Some(run) if current == Some(text.run) => run.rich.end = index + 1,
            _ => out.push(TextRunItems {
                plain: items.end..items.end,
                rich: index..index + 1,
            }),
        }
        current = Some(text.run);
    }
    first as u32..out.len() as u32
}

/// Append scissor batches for consecutive instances that share a clip and
/// return the range of commands added.
fn push_clip_batches(
    clips: impl Iterator<Item = Rect>,
    first_instance: u32,
    out: &mut Vec<QuadDrawCommand>,
) -> Range<u32> {
    let first = out.len();
    for (index, clip) in (first_instance..).zip(clips) {
        let own = out.len() > first;
        match out.last_mut() {
            Some(last) if own && last.clip == clip => last.instances.end = index + 1,
            _ => out.push(QuadDrawCommand {
                instances: index..index + 1,
                clip,
            }),
        }
    }
    first as u32..out.len() as u32
}
