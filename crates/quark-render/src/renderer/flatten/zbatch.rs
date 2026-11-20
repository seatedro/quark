//! Spatial batching within z-layers: which segment each primitive joins.

use super::{
    DrawKey, FlattenedBlurRegion, Flattener, KIND_COUNT, PrimKind, rect_union, rects_overlap,
};
use crate::scene::Rect;

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

/// One segment of a z-layer: a batch of one kind, or a blur barrier.
#[derive(Debug, Clone, Copy)]
pub(super) enum SegmentSlot {
    Batch(PrimKind),
    Blur(FlattenedBlurRegion),
}

/// Segment assignment state for one z-layer.
#[derive(Debug, Default)]
pub(super) struct ZBuilder {
    pub(super) z: i32,
    pub(super) segments: Vec<SegmentSlot>,
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
        self.block_others(k, segment, std::slice::from_ref(&bounds));
        segment
    }

    /// Choose the segments of consecutive primitives of `kind` covering
    /// `bounds` (whose union is `union`), as [`Self::place`] would one by
    /// one, calling `assign` with each segment and how many of them, in
    /// order, take it. Primitives of one kind never block each other, so
    /// once one of them opens a segment (or none of them overlaps what
    /// blocks the latest) the rest join it in bulk.
    fn place_span(
        &mut self,
        kind: PrimKind,
        bounds: &[Rect],
        union: Rect,
        mut assign: impl FnMut(u32, usize),
    ) {
        let k = kind as usize;
        let mut rest = bounds;
        if self.latest[k].is_none() || self.blockers[k].overlaps(union) {
            while let Some((&first, tail)) = rest.split_first() {
                let opened = self.segments.len();
                assign(self.place(kind, first), 1);
                rest = tail;
                if self.segments.len() > opened {
                    break;
                }
            }
        }
        let Some(segment) = self.latest[k].filter(|_| !rest.is_empty()) else {
            return;
        };
        self.block_others(k, segment, rest);
        assign(segment, rest.len());
    }

    /// Record `bounds`, placed in `segment` of kind `k`, as blocking every
    /// other kind whose latest segment is below it.
    fn block_others(&mut self, k: usize, segment: u32, bounds: &[Rect]) {
        for j in 0..KIND_COUNT {
            if j != k
                && let Some(latest) = self.latest[j]
                && segment > latest
            {
                for &rect in bounds {
                    self.blockers[j].push(rect);
                }
            }
        }
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

impl Flattener {
    /// The segment builder of z-layer `z`, started on first use this target.
    fn builder(&mut self, z: i32) -> &mut ZBuilder {
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

    /// The draw key of a primitive of `kind` covering `bounds` at `z`.
    pub(super) fn place(&mut self, z: i32, kind: PrimKind, bounds: Rect) -> DrawKey {
        self.chunks.log_place(kind, z, bounds);
        self.seq += 1;
        let seq = self.seq;
        let builder = self.builder(z);
        DrawKey {
            z,
            segment: builder.place(kind, bounds),
            seq,
        }
    }

    /// Place consecutive primitives of `kind` covering `bounds` (whose
    /// union is `union`) as [`Self::place`] would one by one, appending
    /// their keys to `keys`.
    pub(super) fn place_span(
        &mut self,
        z: i32,
        kind: PrimKind,
        bounds: &[Rect],
        union: Rect,
        keys: &mut Vec<DrawKey>,
    ) {
        for &rect in bounds {
            self.chunks.log_place(kind, z, rect);
        }
        let mut seq = self.seq;
        self.builder(z)
            .place_span(kind, bounds, union, |segment, count| {
                keys.extend((0..count).map(|_| {
                    seq += 1;
                    DrawKey { z, segment, seq }
                }));
            });
        self.seq = seq;
    }

    /// End every segment of z-layer `z` at a blur barrier.
    pub(super) fn barrier(&mut self, z: i32, blur: FlattenedBlurRegion) {
        self.chunks.log_barrier(z, blur);
        self.builder(z).barrier(blur);
    }
}
