//! Chunks flattened once and drawn again in later frames.
//!
//! A [`ChunkPrimitive`] draws a recorded [`SceneChunk`](quark::scene::SceneChunk)
//! in its place. The first frame that draws a chunk converts its primitives
//! (moved to where the chunk lands, in physical pixels) into [`Drawn`]
//! items, each with the chunk's own clip, if any, and z-index around it.
//! Later frames draw those items again while the chunk's generation and
//! scale match and it has moved by whole pixels, keeping the fraction of a
//! pixel its primitives snap around: each item is moved by the
//! difference, clipped by the clips around the chunk, faded by the layers
//! drawn in place around it, and placed like any primitive, so draw order
//! and batching are those of the chunk's primitives drawn one by one. A
//! nested chunk is an item that draws the inner chunk's own entry, so a
//! changed outer chunk converts only its own primitives again.
//!
//! A chunk drawn where it was drawn the frame before, under the same
//! clips, z-index, and fade, also keeps what drawing it appended: every
//! clipped item of its own and of its nested chunks, and the bounds each
//! was placed with. Later frames in that place append those again in bulk
//! and place them span by span (consecutive items of one kind and
//! z-index), assigning the segments that placing them one by one would.
//! A chunk drawn somewhere new each frame (a scrolling row) records
//! nothing. One drawn wholly outside the clip around it (a row scrolled
//! out of view) appends nothing, which its converted bounds tell without
//! visiting its items.
//!
//! Chunks that start a layer, or that a layer groups, are not drawn here:
//! layer planning needs their primitives, so the renderer expands them
//! first.

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

use quark::scene::ChunkPrimitive;

use super::{
    ActiveClip, BandScratch, Draw, DrawKey, Drawn, FlattenedBlurRegion, FlattenedScene, Flattener,
    PathInstance, PrimKind, Primitive, Rect, bounds_rect, convert, emit, path_parts, rect_union,
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
    entries: HashMap<u64, Box<ChunkEntry>, BuildHasherDefault<IdHasher>>,
    frame: u64,
    /// Clips of the items of the chunks being drawn, outermost first:
    /// each chunk's local clips under the clips around it, `None` when
    /// nothing inside can show.
    clips: Vec<Option<ActiveClip>>,
    clip_stack: Vec<u32>,
    z_stack: Vec<Option<i32>>,
    /// How many chunks being drawn record their placement, and what
    /// was placed and which chunks were drawn since the outermost began.
    recording: u32,
    log: Vec<Logged>,
    drawn: Vec<u64>,
    /// Keys of the placements of the chunk being replayed.
    keys: Vec<DrawKey>,
}

impl ChunkCache {
    /// Start a frame: drop entries no recent frame drew.
    pub(super) fn begin_frame(&mut self) {
        self.frame += 1;
        let frame = self.frame;
        self.entries
            .retain(|_, entry| frame - entry.last_used <= KEEP_UNUSED_CHUNK_FRAMES);
    }

    /// Note a placement for the chunks recording theirs.
    pub(super) fn log_place(&mut self, kind: PrimKind, z: i32, bounds: Rect) {
        if self.recording > 0 {
            self.log.push(Logged::Place { kind, z, bounds });
        }
    }

    /// Note a blur barrier for the chunks recording their placement.
    pub(super) fn log_barrier(&mut self, z: i32, blur: FlattenedBlurRegion) {
        if self.recording > 0 {
            self.log.push(Logged::Barrier { z, blur });
        }
    }
}

/// A placement, or a blur barrier, made while chunks record theirs.
#[derive(Debug, Clone, Copy)]
enum Logged {
    Place {
        kind: PrimKind,
        z: i32,
        bounds: Rect,
    },
    Barrier {
        z: i32,
        blur: FlattenedBlurRegion,
    },
}

/// Where a chunk is drawn: what moves and clips its items, all of it.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Context {
    shift: (f32, f32),
    scissor: Rect,
    rounded_rect: Rect,
    corner_radii: [f32; 4],
    z: i32,
    alpha: f32,
}

/// What drawing a chunk in one context appended, kept to append again.
#[derive(Debug, Default)]
struct Placement {
    /// The context the chunk was last drawn in.
    context: Option<Context>,
    /// Whether the rest holds the draw in `context`.
    recorded: bool,
    spans: Vec<Span>,
    /// Bounds of each placement, in order; spans index them.
    bounds: Vec<Rect>,
    /// The clipped items appended, by kind, in order. Each key's `seq` is
    /// the index of its placement, and path segment starts index
    /// `segments`.
    items: FlattenedScene,
    segments: Vec<[f32; 4]>,
    /// The nested chunks drawn, at any depth, kept alive while this one
    /// draws from its record.
    nested: Vec<u64>,
    /// The frame that last kept `nested` alive.
    touched: u64,
}

#[derive(Debug, Clone, Copy)]
enum Span {
    /// Consecutive placements of one kind at one z-index.
    Place {
        kind: PrimKind,
        z: i32,
        start: u32,
        end: u32,
        union: Rect,
    },
    Barrier {
        z: i32,
        blur: FlattenedBlurRegion,
    },
}

/// Where a recording chunk's draw starts in the frame.
struct Mark {
    log: usize,
    drawn: usize,
    seq: u32,
    segments: usize,
    lens: [usize; 7],
}

impl Mark {
    /// Start recording what is drawn into `out` from here.
    fn begin(fl: &mut Flattener, out: &FlattenedScene) -> Self {
        fl.chunks.recording += 1;
        Self {
            log: fl.chunks.log.len(),
            drawn: fl.chunks.drawn.len(),
            seq: fl.seq,
            segments: fl.segments.len(),
            lens: [
                out.shadows.len(),
                out.effect_quads.len(),
                out.quads.len(),
                out.images.len(),
                out.texts.len(),
                out.rich_texts.len(),
                out.paths.len(),
            ],
        }
    }
}

impl Placement {
    /// Keep what was drawn into `out` since `mark`.
    fn record(&mut self, mark: Mark, fl: &mut Flattener, out: &FlattenedScene) {
        let cache = &mut fl.chunks;
        cache.recording -= 1;
        self.spans.clear();
        self.bounds.clear();
        for logged in &cache.log[mark.log..] {
            match *logged {
                Logged::Barrier { z, blur } => self.spans.push(Span::Barrier { z, blur }),
                Logged::Place { kind, z, bounds } => {
                    let index = self.bounds.len() as u32;
                    self.bounds.push(bounds);
                    match self.spans.last_mut() {
                        Some(Span::Place {
                            kind: k,
                            z: span_z,
                            end,
                            union,
                            ..
                        }) if *k == kind && *span_z == z => {
                            *end = index + 1;
                            *union = rect_union(*union, bounds);
                        }
                        _ => self.spans.push(Span::Place {
                            kind,
                            z,
                            start: index,
                            end: index + 1,
                            union: bounds,
                        }),
                    }
                }
            }
        }
        debug_assert_eq!(self.bounds.len() as u32, fl.seq - mark.seq);
        let seq = mark.seq + 1;
        let key = |key: DrawKey| DrawKey {
            seq: key.seq - seq,
            ..key
        };
        let base = mark.segments as u32;
        let [shadows, effects, quads, images, texts, rich_texts, paths] = mark.lens;
        let items = &mut self.items;
        items.clear();
        items
            .shadows
            .extend(out.shadows[shadows..].iter().map(|item| {
                let mut item = *item;
                item.key = key(item.key);
                item
            }));
        items
            .effect_quads
            .extend(out.effect_quads[effects..].iter().map(|item| {
                let mut item = *item;
                item.key = key(item.key);
                item
            }));
        items.quads.extend(out.quads[quads..].iter().map(|item| {
            let mut item = *item;
            item.key = key(item.key);
            item
        }));
        items.images.extend(out.images[images..].iter().map(|item| {
            let mut item = item.clone();
            item.key = key(item.key);
            item
        }));
        items.texts.extend(out.texts[texts..].iter().map(|item| {
            let mut item = item.clone();
            item.key = key(item.key);
            item
        }));
        items
            .rich_texts
            .extend(out.rich_texts[rich_texts..].iter().map(|item| {
                let mut item = item.clone();
                item.key = key(item.key);
                item
            }));
        items.paths.extend(out.paths[paths..].iter().map(|item| {
            let mut item = *item;
            item.key = key(item.key);
            item.instance.segments[0] -= base;
            item
        }));
        self.segments.clear();
        self.segments
            .extend_from_slice(&fl.segments[mark.segments..]);
        self.nested.clear();
        self.nested.extend_from_slice(&cache.drawn[mark.drawn..]);
        if cache.recording == 0 {
            cache.log.clear();
            cache.drawn.clear();
        }
        self.recorded = true;
        self.touched = cache.frame;
    }

    /// Append the recorded draw again: place each span and give every
    /// item its placement's key.
    fn replay(&mut self, fl: &mut Flattener, out: &mut FlattenedScene) {
        let cache = &mut fl.chunks;
        if cache.recording > 0 {
            cache.drawn.extend_from_slice(&self.nested);
        }
        if cache.frame - self.touched >= KEEP_UNUSED_CHUNK_FRAMES / 2 {
            for id in &self.nested {
                if let Some(entry) = cache.entries.get_mut(id) {
                    entry.last_used = cache.frame;
                }
            }
            self.touched = cache.frame;
        }
        let mut keys = std::mem::take(&mut cache.keys);
        keys.clear();
        for span in &self.spans {
            match *span {
                Span::Barrier { z, blur } => fl.barrier(z, blur),
                Span::Place {
                    kind,
                    z,
                    start,
                    end,
                    union,
                } => {
                    let bounds = &self.bounds[start as usize..end as usize];
                    fl.place_span(z, kind, bounds, union, &mut keys);
                }
            }
        }
        let key = |key: DrawKey| keys[key.seq as usize];
        let base = fl.segments.len() as u32;
        fl.segments.extend_from_slice(&self.segments);
        let items = &self.items;
        out.shadows.extend(items.shadows.iter().map(|item| {
            let mut item = *item;
            item.key = key(item.key);
            item
        }));
        out.effect_quads
            .extend(items.effect_quads.iter().map(|item| {
                let mut item = *item;
                item.key = key(item.key);
                item
            }));
        out.quads.extend(items.quads.iter().map(|item| {
            let mut item = *item;
            item.key = key(item.key);
            item
        }));
        out.images.extend(items.images.iter().map(|item| {
            let mut item = item.clone();
            item.key = key(item.key);
            item
        }));
        out.texts.extend(items.texts.iter().map(|item| {
            let mut item = item.clone();
            item.key = key(item.key);
            item
        }));
        out.rich_texts.extend(items.rich_texts.iter().map(|item| {
            let mut item = item.clone();
            item.key = key(item.key);
            item
        }));
        out.paths.extend(items.paths.iter().map(|item| {
            let mut item = *item;
            item.key = key(item.key);
            item.instance.segments[0] += base;
            item
        }));
        fl.chunks.keys = keys;
    }
}

/// One chunk's primitives as converted for some frame.
#[derive(Debug, Default)]
struct ChunkEntry {
    /// The chunk generation and scale it was converted from; `None` until
    /// converted.
    generation: Option<u64>,
    scale: Option<f32>,
    /// The chunk offset it was converted at, and its pixel origin there.
    offset: [f32; 2],
    origin: Option<([f32; 2], [f32; 2])>,
    last_used: u64,
    items: Vec<Item>,
    /// Union of every item's extent where it was converted, when the
    /// chunk holds no nested chunk and every extent is finite.
    bounds: Option<Rect>,
    /// Clips inside the chunk; `clips[0]` is none.
    clips: Vec<LocalClip>,
    /// Band instances of the chunk's paths; their segment starts index
    /// `segments`.
    bands: Vec<PathInstance>,
    segments: Vec<[f32; 4]>,
    placement: Placement,
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
    /// be drawn moved: the same content at the same scale, with the same
    /// fraction of a pixel in its origin, so every edge snaps as it did
    /// and moves by the difference of whole pixels. In logical points
    /// nothing snaps, so any move will do.
    fn moved(&self, chunk: &ChunkPrimitive) -> Option<(f32, f32)> {
        if self.generation != Some(chunk.chunk.generation()) || self.scale != chunk.scale {
            return None;
        }
        match (self.origin, chunk.pixel_origin()) {
            (Some((whole, fraction)), Some((now, fraction_now))) => {
                (fraction == fraction_now).then_some((now[0] - whole[0], now[1] - whole[1]))
            }
            _ => Some((
                chunk.offset[0] - self.offset[0],
                chunk.offset[1] - self.offset[1],
            )),
        }
    }

    /// Convert `chunk`'s primitives where it lands now.
    fn convert(
        &mut self,
        chunk: &ChunkPrimitive,
        cache: &mut ChunkCache,
        scratch: &mut BandScratch,
        fonts: Option<&crate::icons::SvgFonts>,
    ) {
        self.generation = Some(chunk.chunk.generation());
        self.scale = chunk.scale;
        self.offset = chunk.offset;
        self.origin = chunk.pixel_origin();
        self.placement.context = None;
        self.placement.recorded = false;
        self.items.clear();
        self.bounds = None;
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
                Primitive::LayerStart(_)
                | Primitive::LayerEnd
                | Primitive::IsolateStart(_)
                | Primitive::IsolateEnd
                | Primitive::LayerBoundary => {}
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
                    if let Some(drawn) = convert(&chunk.place(primitive), |_| true, fonts) {
                        self.items.push(item(ItemDraw::Drawn(drawn)));
                    }
                }
            }
        }
        self.bounds = items_bounds(&self.items);
    }

    /// Whether no item can show under `scissor` with the chunk moved by
    /// `shift`. Each item is clipped by `scissor` or a clip inside it, so
    /// bounds a pixel short of `scissor` (more than the rounding of the
    /// items' own moves and clips) show nothing.
    fn hidden(&self, (dx, dy): (f32, f32), scissor: Rect) -> bool {
        let Some(bounds) = self.bounds else {
            return false;
        };
        let bounds = bounds.offset(dx, dy);
        let edges = [
            bounds.x - 1.0,
            bounds.y - 1.0,
            bounds.right() + 1.0,
            bounds.bottom() + 1.0,
        ];
        let [left, top, right, bottom] = edges;
        edges.iter().all(|edge| edge.abs() < 1_048_576.0)
            && (right <= scissor.x
                || left >= scissor.right()
                || bottom <= scissor.y
                || top >= scissor.bottom())
    }
}

/// The union of what `items` can draw, as [`ChunkEntry::bounds`] says.
fn items_bounds(items: &[Item]) -> Option<Rect> {
    let mut union: Option<Rect> = None;
    for item in items {
        let extent = match &item.draw {
            ItemDraw::Chunk(_) => return None,
            ItemDraw::Drawn(drawn) => match drawn {
                Drawn::Quad(instance) => bounds_rect(instance.bounds),
                Drawn::Shadow(instance) => bounds_rect(instance.draw_bounds),
                Drawn::Effect(instance) => bounds_rect(instance.bounds),
                Drawn::Image(image) => image.rect,
                Drawn::Text(text) => text.rect,
                Drawn::RichText(text) => text.rect,
                Drawn::StyledText(text) => text.rect,
                Drawn::Path { area, .. } => *area,
                Drawn::Blur(blur) => blur.rect,
            },
        };
        let finite = [extent.x, extent.y, extent.right(), extent.bottom()]
            .iter()
            .all(|v| v.is_finite());
        if !finite {
            return None;
        }
        union = Some(union.map_or(extent, |union| rect_union(union, extent)));
    }
    union
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
            entry.convert(
                chunk,
                &mut fl.chunks,
                &mut fl.band_scratch,
                fl.svg_fonts.as_ref(),
            );
            (0.0, 0.0)
        }
    };
    entry.last_used = fl.chunks.frame;
    let shift = (dx - fl.origin.0, dy - fl.origin.1);
    if fl.chunks.recording > 0 {
        fl.chunks.drawn.push(id);
    }
    let context = Some(Context {
        shift,
        scissor: outer.scissor,
        rounded_rect: outer.rounded_rect,
        corner_radii: outer.corner_radii,
        z,
        alpha,
    });
    let hidden = entry.hidden(shift, outer.scissor);
    let placement = &mut entry.placement;
    if placement.recorded && placement.context == context {
        placement.replay(fl, out);
        fl.chunks.entries.insert(id, entry);
        return;
    }
    if hidden {
        placement.context = context;
        placement.recorded = false;
        fl.chunks.entries.insert(id, entry);
        return;
    }
    // Record the second frame in one place, so a chunk that moves every
    // frame copies nothing.
    let mark = (placement.context == context).then(|| Mark::begin(fl, out));
    placement.context = context;
    placement.recorded = false;
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
    if let Some(mark) = mark {
        entry.placement.record(mark, fl, out);
    }
    fl.chunks.entries.insert(id, entry);
}
