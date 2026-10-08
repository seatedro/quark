//! Chunks flattened once and drawn again in later frames.
//!
//! A [`ChunkPrimitive`] draws a recorded [`SceneChunk`](quark::scene::SceneChunk)
//! in its place. The first frame that draws a chunk converts its primitives
//! (moved to where the chunk lands, in physical pixels) into [`Drawn`]
//! items, each with the chunk's own clip, if any, and z-index around it.
//! Later frames draw those items again while the chunk's generation and
//! scale match and it has moved by whole pixels: each item is moved by the
//! difference, clipped by the clips around the chunk, faded by the layers
//! drawn in place around it, and placed like any primitive, so draw order
//! and batching are those of the chunk's primitives drawn one by one. A
//! nested chunk is an item that draws the inner chunk's own entry, so a
//! changed outer chunk converts only its own primitives again.
//!
//! Chunks that start a layer, or that a layer groups, are not drawn here:
//! layer planning needs their primitives, so the renderer expands them
//! first.

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

use quark::scene::ChunkPrimitive;

use super::{
    ActiveClip, BandScratch, Draw, Drawn, FlattenedScene, Flattener, PathInstance, Primitive, Rect,
    convert, emit, path_parts,
};

/// Frames a chunk may go undrawn before its entry is dropped. A recording
/// alternates between two chunks while frames still hold the last one, so
/// both stay warm.
const KEEP_UNUSED_CHUNK_FRAMES: u64 = 30;

/// Chunk ids are sequential; spread them over the hash.
#[derive(Default)]
struct IdHasher(u64);

impl Hasher for IdHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 = self.0.rotate_left(8) ^ u64::from(*byte);
        }
    }

    fn write_u64(&mut self, id: u64) {
        self.0 = id.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    }
}

/// Entries by chunk id, and the working storage for drawing them.
#[derive(Debug, Default)]
pub(super) struct ChunkCache {
    entries: HashMap<u64, ChunkEntry, BuildHasherDefault<IdHasher>>,
    frame: u64,
    /// Clips of the items of the chunks being drawn, outermost first:
    /// each chunk's local clips under the clips around it, `None` when
    /// nothing inside can show.
    clips: Vec<Option<ActiveClip>>,
    clip_stack: Vec<u32>,
    z_stack: Vec<Option<i32>>,
}

impl ChunkCache {
    /// Start a frame: drop entries no recent frame drew.
    pub(super) fn begin_frame(&mut self) {
        self.frame += 1;
        let frame = self.frame;
        self.entries
            .retain(|_, entry| frame - entry.last_used <= KEEP_UNUSED_CHUNK_FRAMES);
    }
}

/// One chunk's primitives as converted for some frame.
#[derive(Debug, Default)]
struct ChunkEntry {
    /// The chunk generation and scale it was converted from; `None` until
    /// converted.
    generation: Option<u64>,
    scale: Option<f32>,
    /// The chunk offset it was converted at.
    offset: [f32; 2],
    last_used: u64,
    items: Vec<Item>,
    /// Clips inside the chunk; `clips[0]` is none.
    clips: Vec<LocalClip>,
    /// Band instances of the chunk's paths; their segment starts index
    /// `segments`.
    bands: Vec<PathInstance>,
    segments: Vec<[f32; 4]>,
}

#[derive(Debug, Clone)]
struct Item {
    draw: ItemDraw,
    /// Index into the entry's clips.
    clip: u32,
    /// The z-index pushed inside the chunk, if any.
    z: Option<i32>,
}

#[derive(Debug, Clone)]
enum ItemDraw {
    Drawn(Drawn),
    /// The nested chunk at this index of the chunk's primitives.
    Chunk(u32),
}

/// The clips pushed inside a chunk around an item, combined.
#[derive(Debug, Clone, Copy)]
struct LocalClip {
    /// Intersection of the rects; empty when they share no pixel. `None`
    /// at the chunk's top level.
    scissor: Option<Rect>,
    /// The innermost rounded clip.
    rounded: Option<(Rect, [f32; 4])>,
}

impl LocalClip {
    const NONE: Self = Self {
        scissor: None,
        rounded: None,
    };

    /// This clip with `rect` pushed inside it, as [`ActiveClip::push`]
    /// combines them.
    fn push(&self, rect: Rect, corner_radii: [f32; 4]) -> Self {
        let scissor = match self.scissor {
            None => rect,
            Some(scissor) => scissor.intersection(rect).unwrap_or_default(),
        };
        let rounded = corner_radii.iter().any(|&r| r > 0.0);
        Self {
            scissor: Some(scissor),
            rounded: if rounded {
                Some((rect, corner_radii))
            } else {
                self.rounded
            },
        }
    }

    /// The clip of an item under `outer`, the clip around the chunk, with
    /// the chunk moved by `(dx, dy)`.
    fn under(&self, outer: ActiveClip, (dx, dy): (f32, f32)) -> Option<ActiveClip> {
        let scissor = match self.scissor {
            None => outer.scissor,
            Some(scissor) => outer.scissor.intersection(scissor.offset(dx, dy))?,
        };
        let (rounded_rect, corner_radii) = match self.rounded {
            Some((rect, radii)) => (rect.offset(dx, dy), radii),
            None => (outer.rounded_rect, outer.corner_radii),
        };
        Some(ActiveClip {
            scissor,
            rounded_rect,
            corner_radii,
        })
    }
}

impl ChunkEntry {
    /// How far the items moved since they were converted, when they can
    /// be drawn moved: the same content at the same scale, moved by whole
    /// physical pixels, so every snapped edge moves by the same amount.
    fn moved(&self, chunk: &ChunkPrimitive) -> Option<(f32, f32)> {
        if self.generation != Some(chunk.chunk.generation()) || self.scale != chunk.scale {
            return None;
        }
        let s = self.scale.unwrap_or(1.0);
        let dx = (chunk.offset[0] - self.offset[0]) * s;
        let dy = (chunk.offset[1] - self.offset[1]) * s;
        let whole = dx.fract() == 0.0 && dy.fract() == 0.0;
        (whole || self.scale.is_none()).then_some((dx, dy))
    }

    /// Convert `chunk`'s primitives where it lands now.
    fn convert(
        &mut self,
        chunk: &ChunkPrimitive,
        cache: &mut ChunkCache,
        scratch: &mut BandScratch,
    ) {
        self.generation = Some(chunk.chunk.generation());
        self.scale = chunk.scale;
        self.offset = chunk.offset;
        self.items.clear();
        self.clips.clear();
        self.clips.push(LocalClip::NONE);
        self.bands.clear();
        self.segments.clear();
        let clip_stack = &mut cache.clip_stack;
        let z_stack = &mut cache.z_stack;
        clip_stack.clear();
        clip_stack.push(0);
        z_stack.clear();
        z_stack.push(None);
        for (index, primitive) in chunk.chunk.primitives().iter().enumerate() {
            let clip = *clip_stack.last().expect("root clip");
            let z = *z_stack.last().expect("root z");
            let item = |draw| Item { draw, clip, z };
            match primitive {
                Primitive::Chunk(_) => self.items.push(item(ItemDraw::Chunk(index as u32))),
                Primitive::ClipStart(_) => {
                    let Primitive::ClipStart(placed) = chunk.place(primitive) else {
                        unreachable!("placing keeps the kind");
                    };
                    let local = self.clips[clip as usize].push(placed.rect, placed.corner_radii);
                    clip_stack.push(self.clips.len() as u32);
                    self.clips.push(local);
                }
                Primitive::ClipEnd => {
                    if clip_stack.len() > 1 {
                        clip_stack.pop();
                    }
                }
                Primitive::ZIndexPush(z) => z_stack.push(Some(*z)),
                Primitive::ZIndexPop => {
                    if z_stack.len() > 1 {
                        z_stack.pop();
                    }
                }
                // Chunks with layers are expanded before flattening.
                Primitive::LayerStart(_) | Primitive::LayerEnd | Primitive::LayerBoundary => {}
                Primitive::Path(_) => {
                    let Primitive::Path(path) = chunk.place(primitive) else {
                        unreachable!("placing keeps the kind");
                    };
                    let mut parts = [None, None];
                    path_parts(
                        &path,
                        &mut self.segments,
                        scratch,
                        &mut self.bands,
                        &mut parts,
                    );
                    for part in parts.into_iter().flatten() {
                        self.items.push(item(ItemDraw::Drawn(part)));
                    }
                }
                _ => {
                    // An entry outlives the GPU copy of its icons, so it
                    // keeps their pixels.
                    if let Some(drawn) = convert(&chunk.place(primitive), |_| true) {
                        self.items.push(item(ItemDraw::Drawn(drawn)));
                    }
                }
            }
        }
    }
}

/// Draw `chunk` under the flattener's current clip, z-index, and inline
/// layers.
pub(super) fn draw_chunk(chunk: &ChunkPrimitive, fl: &mut Flattener, out: &mut FlattenedScene) {
    let (clip, z, alpha) = (fl.clip(), fl.z(), fl.alpha);
    draw(chunk, clip, z, alpha, fl, out);
}

fn draw(
    chunk: &ChunkPrimitive,
    outer: ActiveClip,
    z: i32,
    alpha: f32,
    fl: &mut Flattener,
    out: &mut FlattenedScene,
) {
    let id = chunk.chunk.id();
    let mut entry = fl.chunks.entries.remove(&id).unwrap_or_default();
    let (dx, dy) = match entry.moved(chunk) {
        Some(moved) => moved,
        None => {
            entry.convert(chunk, &mut fl.chunks, &mut fl.band_scratch);
            (0.0, 0.0)
        }
    };
    entry.last_used = fl.chunks.frame;
    let shift = (dx - fl.origin.0, dy - fl.origin.1);
    let segment_base = fl.segments.len() as u32;
    fl.segments.extend_from_slice(&entry.segments);
    let first_clip = fl.chunks.clips.len();
    for local in &entry.clips {
        fl.chunks.clips.push(local.under(outer, shift));
    }
    for item in &entry.items {
        let Some(clip) = fl.chunks.clips[first_clip + item.clip as usize] else {
            continue;
        };
        let z = item.z.unwrap_or(z);
        match &item.draw {
            ItemDraw::Drawn(drawn) => {
                let draw = Draw {
                    shift,
                    clip,
                    z,
                    alpha,
                    bands: &entry.bands,
                    segment_base,
                    run: id,
                };
                emit(drawn.clone(), &draw, fl, out);
            }
            ItemDraw::Chunk(index) => {
                let Primitive::Chunk(inner) =
                    chunk.place(&chunk.chunk.primitives()[*index as usize])
                else {
                    unreachable!("items index chunks");
                };
                self::draw(&inner, clip, z, alpha, fl, out);
            }
        }
    }
    fl.chunks.clips.truncate(first_clip);
    fl.chunks.entries.insert(id, entry);
}
