//! Prepared glyphs kept across frames.
//!
//! Each text step draws as one or more runs: consecutive texts drawn from
//! the same chunk (or from outside any chunk) share a run, so one changed
//! row of a cached grid is a run of its own. Every run has its own glyphon
//! renderer, and a run whose texts are exactly those a renderer prepared
//! last frame, for the same target, draws that renderer's vertices again
//! without preparing or uploading anything. Vertices point into the glyph
//! atlas, so they hold only while no cached glyph has been evicted or
//! cleared since: the atlas epoch.

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

use glyphon::{PrepareError, TextRenderer};
use quark_text::TextSystem;

use super::{ClippedRichText, ClippedText, Renderer, TargetFrame, TextRunItems};
use crate::text::{TextPath, positioned_glyphs, prepare_text_areas};

/// Run hashes are already mixed.
#[derive(Default)]
struct Passthrough(u64);

impl Hasher for Passthrough {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 = self.0.rotate_left(8) ^ u64::from(*byte);
        }
    }

    fn write_u64(&mut self, value: u64) {
        self.0 = value;
    }
}

/// The renderers of a frame's text runs and what each last prepared.
#[derive(Default)]
pub(super) struct TextRuns {
    renderers: Vec<TextRenderer>,
    memos: Vec<RunMemo>,
    /// This frame's renderer of each run.
    slots: Vec<usize>,
    hashes: Vec<u64>,
    /// Per renderer: holds a run this frame, was prepared this frame, and
    /// has vertices not uploaded yet.
    claimed: Vec<bool>,
    prepared: Vec<bool>,
    pending: Vec<bool>,
    /// Last frame's renderer by run hash.
    lookup: HashMap<u64, usize, BuildHasherDefault<Passthrough>>,
}

/// The texts a renderer's vertices were prepared from.
#[derive(Default)]
struct RunMemo {
    valid: bool,
    target: usize,
    epoch: u64,
    texts: Vec<ClippedText>,
    rich_texts: Vec<ClippedRichText>,
}

impl RunMemo {
    fn matches(
        &self,
        target: usize,
        epoch: u64,
        texts: &[ClippedText],
        rich_texts: &[ClippedRichText],
    ) -> bool {
        self.valid
            && self.target == target
            && self.epoch == epoch
            && self.texts.len() == texts.len()
            && self.rich_texts.len() == rich_texts.len()
            && self
                .texts
                .iter()
                .zip(texts)
                .all(|(a, b)| a.primitive == b.primitive && a.clip == b.clip)
            && self
                .rich_texts
                .iter()
                .zip(rich_texts)
                .all(|(a, b)| a.primitive == b.primitive && a.clip == b.clip && a.alpha == b.alpha)
    }

    fn forget(&mut self) {
        self.valid = false;
        self.texts.clear();
        self.rich_texts.clear();
    }
}

/// Mix of what decides a run's vertices; equal runs hash equal.
fn run_hash(target: usize, texts: &[ClippedText], rich_texts: &[ClippedRichText]) -> u64 {
    let mut h = target as u64 ^ 0x517c_c1b7_2722_0a95;
    let mut mix = |value: u64| {
        h = (h ^ value)
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            .rotate_left(29)
    };
    let rect = |r: crate::scene::Rect| {
        u64::from(r.x.to_bits()) << 32 | u64::from(r.y.to_bits()) ^ u64::from(r.width.to_bits())
    };
    // Layouts compare by identity in `RunMemo::matches`; their place and
    // color spread the hash well enough.
    for text in texts {
        let c = text.primitive.color;
        mix(rect(text.primitive.rect) ^ rect(text.clip).rotate_left(17));
        mix(u64::from(u32::from_be_bytes([c.r, c.g, c.b, c.a])));
    }
    for text in rich_texts {
        mix(rect(text.primitive.rect) ^ rect(text.clip).rotate_left(17) ^ 1);
        mix(u64::from(text.alpha.to_bits()));
    }
    h
}

fn run_items<'a>(
    frame: &'a TargetFrame,
    run: &TextRunItems,
) -> (&'a [ClippedText], &'a [ClippedRichText]) {
    let flat = &frame.flat;
    (
        &flat.texts[run.plain.start as usize..run.plain.end as usize],
        &flat.rich_texts[run.rich.start as usize..run.rich.end as usize],
    )
}

impl TextRuns {
    /// The renderer that draws run `run` of this frame.
    pub(super) fn renderer(&self, run: usize) -> &TextRenderer {
        &self.renderers[self.slots[run]]
    }

    /// Upload the vertices prepared since the last upload.
    pub(super) fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        for (renderer, pending) in self.renderers.iter_mut().zip(&mut self.pending) {
            if std::mem::take(pending) {
                renderer.upload(device, queue);
            }
        }
    }

    /// Prepare everything again next frame.
    pub(super) fn forget(&mut self) {
        self.memos.iter_mut().for_each(RunMemo::forget);
        self.lookup.clear();
    }
}

impl Renderer {
    /// Prepare every text run of `frames`, drawing the vertices a renderer
    /// already holds for an unchanged run when `reuse` allows it.
    pub(super) fn prepare_text_runs(
        &mut self,
        frames: &[TargetFrame],
        text: &mut TextSystem,
        reuse: bool,
    ) -> Result<(), PrepareError> {
        let total: usize = frames.iter().map(|f| f.batches.text_runs.len()).sum();
        let runs = &mut self.text_runs;
        // A busy frame's spare renderers would hold their vertex buffers
        // forever.
        if runs.renderers.len() > total.max(1) * 2 {
            runs.renderers.truncate(total.max(1));
            runs.memos.truncate(total.max(1));
            runs.pending.truncate(total.max(1));
        }
        let count = runs.renderers.len();
        runs.claimed.clear();
        runs.claimed.resize(count, false);
        runs.prepared.clear();
        runs.prepared.resize(count, false);
        runs.slots.clear();
        runs.hashes.clear();
        // Only the positioned path draws exactly what the texts say; the
        // buffer path reads recolored buffers that change underneath.
        let reuse = reuse && self.text_path == TextPath::Positioned;
        let epoch = self.atlas.epoch();

        for (target, frame) in frames.iter().enumerate() {
            for run in &frame.batches.text_runs {
                let (texts, rich_texts) = run_items(frame, run);
                let hash = run_hash(target, texts, rich_texts);
                let kept = reuse
                    .then(|| runs.lookup.get(&hash).copied())
                    .flatten()
                    .filter(|&slot| {
                        slot < count
                            && !runs.claimed[slot]
                            && runs.memos[slot].matches(target, epoch, texts, rich_texts)
                    });
                if let Some(slot) = kept {
                    runs.claimed[slot] = true;
                }
                runs.slots.push(kept.unwrap_or(usize::MAX));
                runs.hashes.push(hash);
            }
        }

        let mut free = 0;
        let mut index = 0;
        for (target, frame) in frames.iter().enumerate() {
            for run in &frame.batches.text_runs {
                if self.text_runs.slots[index] == usize::MAX {
                    let runs = &mut self.text_runs;
                    while free < runs.renderers.len() && runs.claimed[free] {
                        free += 1;
                    }
                    if free == runs.renderers.len() {
                        runs.renderers.push(TextRenderer::new(
                            &mut self.atlas,
                            &self.device,
                            wgpu::MultisampleState::default(),
                            None,
                        ));
                        runs.memos.push(RunMemo::default());
                        runs.claimed.push(false);
                        runs.prepared.push(false);
                        runs.pending.push(false);
                    }
                    runs.claimed[free] = true;
                    runs.slots[index] = free;
                    self.prepare_run(free, target, frame, run, text)?;
                }
                index += 1;
            }
        }

        // Preparing evicted glyphs that kept vertices may point at.
        if self.atlas.epoch() != epoch {
            let mut index = 0;
            for (target, frame) in frames.iter().enumerate() {
                for run in &frame.batches.text_runs {
                    let slot = self.text_runs.slots[index];
                    if !self.text_runs.prepared[slot] {
                        self.prepare_run(slot, target, frame, run, text)?;
                    }
                    index += 1;
                }
            }
        }

        let runs = &mut self.text_runs;
        let epoch = self.atlas.epoch();
        runs.lookup.clear();
        for (&slot, &hash) in runs.slots.iter().zip(&runs.hashes) {
            runs.memos[slot].epoch = epoch;
            runs.lookup.insert(hash, slot);
        }
        // Unclaimed renderers keep no texts alive.
        for (memo, &claimed) in runs.memos.iter_mut().zip(&runs.claimed) {
            if !claimed {
                memo.forget();
            }
        }
        Ok(())
    }

    fn prepare_run(
        &mut self,
        slot: usize,
        target: usize,
        frame: &TargetFrame,
        run: &TextRunItems,
        text: &mut TextSystem,
    ) -> Result<(), PrepareError> {
        let (texts, rich_texts) = run_items(frame, run);
        let viewport = match target {
            0 => &self.viewport,
            i => &self.layer_viewports[i - 1],
        };
        let runs = &mut self.text_runs;
        let memo = &mut runs.memos[slot];
        memo.forget();
        let renderer = &mut runs.renderers[slot];
        match self.text_path {
            TextPath::Positioned => renderer.prepare_glyphs(
                &self.device,
                &self.queue,
                text.raster_font_system(),
                &mut self.atlas,
                viewport,
                positioned_glyphs(texts, rich_texts),
                &mut self.swash_cache,
            )?,
            TextPath::Buffer => renderer.prepare(
                &self.device,
                &self.queue,
                text.raster_font_system(),
                &mut self.atlas,
                viewport,
                prepare_text_areas(texts, rich_texts, &self.recolored),
                &mut self.swash_cache,
            )?,
        }
        runs.prepared[slot] = true;
        runs.pending[slot] = true;
        memo.valid = true;
        memo.target = target;
        memo.texts.extend_from_slice(texts);
        memo.rich_texts.extend_from_slice(rich_texts);
        Ok(())
    }
}
