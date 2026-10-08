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
//!
//! A run that moved by whole pixels (a scrolled row) draws a renderer's
//! vertices as they are, moved on the GPU by the renderer's draw offset,
//! when every glyph it would prepare is one that renderer prepared, moved
//! by exactly as much: the same glyph, subpixel position, and color, and
//! a clip moved alike inside the target. Nothing is prepared or uploaded
//! but the offsets. Anything else (a fractional move, a clip that now
//! cuts the text differently, a glyph evicted since) prepares it again.
//!
//! A vertical move proves that per text instead of per glyph: with the
//! same left edge every column and subpixel bin is the same, and the
//! row offsets kept from preparing (each line top and glyph offset added
//! to the text's top) say exactly which lines pass the clip and where
//! each glyph's row lands. Other moves compare every glyph.

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};
use std::ops::ControlFlow;

use glyphon::{
    MAX_DRAW_OFFSETS, PositionedGlyph, PrepareError, Resolution, TextBounds, TextRenderer, Viewport,
};
use quark_text::{TextLayout, TextSystem};

use super::{ClippedRichText, ClippedText, Renderer, TargetFrame, TextRunItems};
use crate::text::{
    RowOffsets, TextPath, prepare_text_areas, push_positioned_glyphs, visit_positioned_glyphs,
};

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
    /// Per renderer: how far its glyphs moved since they were prepared,
    /// its draw offset.
    offsets: Vec<[i32; 2]>,
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
    /// This frame's run hashes, and last frame's renderers, without the
    /// texts' places.
    moved_hashes: Vec<u64>,
    moved_lookup: HashMap<u64, usize, BuildHasherDefault<Passthrough>>,
}

/// The texts a renderer's vertices were prepared from.
#[derive(Default)]
struct RunMemo {
    valid: bool,
    target: usize,
    resolution: Option<Resolution>,
    epoch: u64,
    texts: Vec<ClippedText>,
    rich_texts: Vec<ClippedRichText>,
    /// The glyphs prepared, on the positioned path, where they were
    /// prepared: the renderer's draw offset has moved them since.
    glyphs: Vec<PositionedGlyph>,
    /// The row offsets of the texts prepared.
    rows: RowOffsets,
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

    /// How far `texts` and `rich_texts` moved, in whole pixels, when they
    /// are the texts prepared, each moved with its clip by that much, in
    /// a target of the same size. Their glyphs may still differ.
    fn moved_by(
        &self,
        target: usize,
        resolution: Resolution,
        epoch: u64,
        texts: &[ClippedText],
        rich_texts: &[ClippedRichText],
    ) -> Option<(i32, i32)> {
        if !(self.valid
            && self.target == target
            && self.resolution == Some(resolution)
            && self.epoch == epoch
            && self.texts.len() == texts.len()
            && self.rich_texts.len() == rich_texts.len())
        {
            return None;
        }
        let first = |texts: &[ClippedText], rich_texts: &[ClippedRichText]| {
            texts
                .first()
                .map(|text| text.primitive.rect)
                .or_else(|| rich_texts.first().map(|text| text.primitive.rect))
        };
        let (was, now) = (
            first(&self.texts, &self.rich_texts)?,
            first(texts, rich_texts)?,
        );
        let (dx, dy) = (now.x - was.x, now.y - was.y);
        // Whole pixels that fit an i32 exactly.
        let whole = |d: f32| d.fract() == 0.0 && d.abs() < 16_777_216.0;
        if !(whole(dx) && whole(dy)) {
            return None;
        }
        let moved = |a: crate::scene::Rect, b: crate::scene::Rect| a.offset(dx, dy) == b;
        let plain = self.texts.iter().zip(texts).all(|(a, b)| {
            a.primitive.layout == b.primitive.layout
                && a.primitive.color == b.primitive.color
                && moved(a.primitive.rect, b.primitive.rect)
                && moved(a.clip, b.clip)
        });
        let rich = self.rich_texts.iter().zip(rich_texts).all(|(a, b)| {
            a.primitive.layout == b.primitive.layout
                && a.primitive.default_color == b.primitive.default_color
                && a.primitive.span_colors == b.primitive.span_colors
                && a.alpha == b.alpha
                && moved(a.primitive.rect, b.primitive.rect)
                && moved(a.clip, b.clip)
        });
        (plain && rich).then_some((dx as i32, dy as i32))
    }

    fn forget(&mut self) {
        self.valid = false;
        self.texts.clear();
        self.rich_texts.clear();
        self.glyphs.clear();
        self.rows.clear();
    }

    /// Whether `texts` and `rich_texts`, the texts prepared moved by
    /// `(0, dy)` as [`Self::moved_by`] found, place every glyph prepared
    /// exactly `dy` pixels lower, decided per text from its row offsets.
    fn moved_down(
        &self,
        texts: &[ClippedText],
        rich_texts: &[ClippedRichText],
        dy: i32,
        resolution: Resolution,
    ) -> bool {
        let size = (resolution.width, resolution.height);
        let plain = self
            .texts
            .iter()
            .zip(texts)
            .map(|(a, b)| ((a.primitive.rect, a.clip), (b.primitive.rect, b.clip)));
        let rich = self
            .rich_texts
            .iter()
            .zip(rich_texts)
            .map(|(a, b)| ((a.primitive.rect, a.clip), (b.primitive.rect, b.clip)));
        plain
            .chain(rich)
            .enumerate()
            .all(|(index, (was, now))| self.rows.moved_down(index, was, now, dy, size))
    }
}

/// Whether `texts` and `rich_texts` place exactly the glyphs `prepared`
/// moved by `to`, clips included, with every clip inside `resolution`
/// both times (`prepared` moved by `from`, and by `to`), so glyphon's
/// clamp to the viewport changes none.
fn glyphs_moved(
    prepared: &[PositionedGlyph],
    (texts, rich_texts): (&[ClippedText], &[ClippedRichText]),
    (from, to): ([i32; 2], [i32; 2]),
    resolution: Resolution,
) -> bool {
    let (width, height) = (resolution.width as i32, resolution.height as i32);
    let inside =
        |b: TextBounds| b.left >= 0 && b.top >= 0 && b.right <= width && b.bottom <= height;
    let mut prepared = prepared.iter();
    let same = visit_positioned_glyphs(texts, rich_texts, |glyph| {
        let moved = prepared.next().is_some_and(|was| {
            glyph
                == PositionedGlyph {
                    x: was.x + to[0],
                    y: was.y + to[1],
                    bounds: moved_bounds(was.bounds, (to[0], to[1])),
                    ..*was
                }
                && inside(glyph.bounds)
                && inside(moved_bounds(was.bounds, (from[0], from[1])))
        });
        if moved {
            ControlFlow::Continue(())
        } else {
            ControlFlow::Break(())
        }
    });
    same.is_continue() && prepared.next().is_none()
}

fn moved_bounds(bounds: TextBounds, (dx, dy): (i32, i32)) -> TextBounds {
    TextBounds {
        left: bounds.left + dx,
        top: bounds.top + dy,
        right: bounds.right + dx,
        bottom: bounds.bottom + dy,
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

/// Mix of what decides a run's vertices apart from where its texts are;
/// runs that differ only by a move hash equal.
fn moved_hash(target: usize, texts: &[ClippedText], rich_texts: &[ClippedRichText]) -> u64 {
    let mut h = target as u64 ^ 0x2545_f491_4f6c_dd1d;
    let mut mix = |value: u64| {
        h = (h ^ value)
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            .rotate_left(29)
    };
    let layout = |text: &quark::scene::ShapedText| {
        text.downcast_ref::<TextLayout>()
            .map_or(0, |layout| std::ptr::from_ref(layout) as u64)
    };
    let size =
        |r: crate::scene::Rect| u64::from(r.width.to_bits()) << 32 | u64::from(r.height.to_bits());
    for text in texts {
        let c = text.primitive.color;
        mix(layout(&text.primitive.layout) ^ size(text.clip).rotate_left(7));
        mix(u64::from(u32::from_be_bytes([c.r, c.g, c.b, c.a])));
    }
    for text in rich_texts {
        mix(layout(&text.primitive.layout) ^ size(text.clip).rotate_left(7) ^ 1);
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
    /// The renderer that draws run `run` of this frame, and its draw
    /// offset slot.
    pub(super) fn renderer(&self, run: usize) -> (&TextRenderer, u32) {
        let slot = self.slots[run];
        (&self.renderers[slot], draw_slot(slot))
    }

    /// Upload the vertices prepared since the last upload, and the draw
    /// offsets to the `viewports` that draw them.
    pub(super) fn upload<'a>(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        viewports: impl IntoIterator<Item = &'a mut Viewport>,
    ) {
        for (renderer, pending) in self.renderers.iter_mut().zip(&mut self.pending) {
            if std::mem::take(pending) {
                renderer.upload(device, queue);
            }
        }
        for viewport in viewports {
            viewport.set_draw_offsets(queue, &self.offsets);
        }
    }

    /// Prepare everything again next frame.
    pub(super) fn forget(&mut self) {
        self.memos.iter_mut().for_each(RunMemo::forget);
        self.lookup.clear();
        self.moved_lookup.clear();
    }

    /// Draw `slot`'s vertices moved, when they are the run's texts
    /// prepared and moved by whole pixels.
    fn translate(
        &mut self,
        slot: usize,
        (target, resolution, epoch): (usize, Resolution, u64),
        texts: &[ClippedText],
        rich_texts: &[ClippedRichText],
    ) -> bool {
        let memo = &mut self.memos[slot];
        let offset = &mut self.offsets[slot];
        let Some(moved) = memo.moved_by(target, resolution, epoch, texts, rich_texts) else {
            return false;
        };
        let to = [offset[0] + moved.0, offset[1] + moved.1];
        let exact = (moved.0 == 0 && memo.moved_down(texts, rich_texts, moved.1, resolution))
            || glyphs_moved(&memo.glyphs, (texts, rich_texts), (*offset, to), resolution);
        if !exact {
            return false;
        }
        memo.texts.clear();
        memo.texts.extend_from_slice(texts);
        memo.rich_texts.clear();
        memo.rich_texts.extend_from_slice(rich_texts);
        *offset = to;
        true
    }
}

/// The draw offset slot of renderer `index`. Renderers past the offsets
/// glyphon has share the last slot, which never moves.
fn draw_slot(index: usize) -> u32 {
    index.min(MAX_DRAW_OFFSETS - 1) as u32
}

/// Whether renderer `index` has a draw offset of its own.
fn can_move(index: usize) -> bool {
    index < MAX_DRAW_OFFSETS - 1
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
            runs.offsets.truncate(total.max(1));
            runs.pending.truncate(total.max(1));
        }
        let count = runs.renderers.len();
        runs.claimed.clear();
        runs.claimed.resize(count, false);
        runs.prepared.clear();
        runs.prepared.resize(count, false);
        runs.slots.clear();
        runs.hashes.clear();
        runs.moved_hashes.clear();
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
                runs.moved_hashes
                    .push(moved_hash(target, texts, rich_texts));
            }
        }

        // Runs that moved, once every unchanged run has its renderer.
        if reuse {
            let mut index = 0;
            for (target, frame) in frames.iter().enumerate() {
                let resolution = match target {
                    0 => self.viewport.resolution(),
                    i => self.layer_viewports[i - 1].resolution(),
                };
                for run in &frame.batches.text_runs {
                    let runs = &mut self.text_runs;
                    let candidate = (runs.slots[index] == usize::MAX)
                        .then(|| runs.moved_lookup.get(&runs.moved_hashes[index]).copied())
                        .flatten()
                        .filter(|&slot| slot < count && !runs.claimed[slot] && can_move(slot));
                    if let Some(slot) = candidate {
                        let (texts, rich_texts) = run_items(frame, run);
                        if runs.translate(slot, (target, resolution, epoch), texts, rich_texts) {
                            runs.claimed[slot] = true;
                            runs.slots[index] = slot;
                        }
                    }
                    index += 1;
                }
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
                        runs.offsets.push([0, 0]);
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
        runs.moved_lookup.clear();
        for ((&slot, &hash), &moved) in runs.slots.iter().zip(&runs.hashes).zip(&runs.moved_hashes)
        {
            runs.memos[slot].epoch = epoch;
            runs.lookup.insert(hash, slot);
            runs.moved_lookup.insert(moved, slot);
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
            TextPath::Positioned => {
                // Kept to check a later move of the run against.
                push_positioned_glyphs(texts, rich_texts, &mut memo.glyphs, &mut memo.rows);
                renderer.prepare_glyphs(
                    &self.device,
                    &self.queue,
                    text.raster_font_system(),
                    &mut self.atlas,
                    viewport,
                    memo.glyphs.iter().copied(),
                    &mut self.swash_cache,
                )?
            }
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
        runs.offsets[slot] = [0, 0];
        memo.valid = true;
        memo.target = target;
        memo.resolution = Some(viewport.resolution());
        memo.texts.extend_from_slice(texts);
        memo.rich_texts.extend_from_slice(rich_texts);
        Ok(())
    }
}
