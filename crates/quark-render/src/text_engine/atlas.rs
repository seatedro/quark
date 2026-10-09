//! The glyph atlas: rasterized glyphs resident in fixed-size texture pages.
//!
//! Masks (one byte of coverage) and color glyphs (sRGB RGBA, straight
//! alpha) live in separate pools of lazily created square pages, each its
//! own texture with a stable slot and a unique id, so a new page never
//! moves a cached glyph. Every bitmap keeps a transparent pixel of gutter
//! on each side. A glyph too large for a page gets a dedicated texture of
//! its own size, charged to the same budget.
//!
//! Entries are keyed by what decides their pixels ([`GlyphKey`]: the
//! rasterizer, the exact font instance, and the request), so a font
//! database replacement or a paint change (color, backdrop, linear
//! correction) reuses them. A front map from cosmic-text cache keys skips
//! resolving the font instance on repeat glyphs of one font epoch.
//!
//! Residency: a glyph used this frame is pinned (its `used` stamp is the
//! current frame) and moves to the front of its pool's LRU list. Misses
//! take space from existing pages, then a new page while under the soft
//! target, then the least recently used unpinned glyphs of their kind,
//! then the headroom up to the hard limit (less one overflow page per
//! kind, kept in reserve). Evicting a glyph increments only its own
//! generation: [`GlyphHandle`]s held by retained draw runs go stale one by
//! one, and runs check theirs before drawing again.
//!
//! When even that cannot hold the frame's glyphs, the frame draws in
//! overflow mode: resident glyphs still draw from their pages, and every
//! other glyph goes into a reusable overflow page per kind, filled and
//! drawn one ordinal at a time; the renderer splits its passes so each
//! ordinal's uploads land after the previous ordinal's draws.
//!
//! Uploads pack into CPU shelves whose rows match wgpu's copy pitch, one
//! write per kind into a reused transfer buffer, then one buffer-to-texture
//! copy per glyph in the frame's own command encoder before any text draws.

use std::collections::HashMap;
use std::hash::BuildHasherDefault;

use etagere::{AllocId, AtlasAllocator, size2};
use quark_text::cosmic_text::{CacheKey, CacheKeyFlags, FontSystem, SwashCache, SwashContent};
use quark_text::fonts::{FontRegistry, PreparedFont};
use quark_text::{FontEpoch, TextSystem};
use rustc_hash::FxHasher;
use wgpu::{
    BindGroup, Buffer, BufferDescriptor, BufferUsages, CommandEncoder, Device, Extent3d, Origin3d,
    Queue, TexelCopyBufferInfo, TexelCopyBufferLayout, TexelCopyTextureInfo, Texture,
    TextureAspect, TextureDescriptor, TextureDimension, TextureFormat, TextureUsages, TextureView,
    TextureViewDescriptor,
};

use super::backend::{Native, NativeOutcome, TextRasterizer};
use super::raster::FontInstanceId;
use super::raster::swash::SwashRasterizer;
use super::raster::{
    AlphaMode, BitmapContent, GlyphRasterizer, MAX_BITMAP_BYTES, RasterError, RasterKey,
    RasterOutcome, RasterRequest, RasterScratch,
};
use super::{Cache, ContentType, PrepareError};

type Fx = BuildHasherDefault<FxHasher>;

/// Side of an atlas page, before clamping to the device's limit.
pub(crate) const DEFAULT_PAGE_SIZE: u32 = 1024;
/// End of an LRU list, and an entry in none.
const NIL: u32 = u32::MAX;
/// Transparent pixels kept around every bitmap.
const GUTTER: u32 = 1;
/// The most bytes one frame stages for uploads; past it uploads go one by
/// one through the queue (and overflow glyphs are dropped).
const MAX_STAGING_BYTES: usize = 16 << 20;
/// The most pixel bytes one frame's evacuation copies.
const EVACUATION_BYTES: u64 = 4 << 20;
/// A page whose allocated area is at most this fraction is sparse enough
/// to evacuate.
const SPARSE_PAGE: f32 = 0.25;
/// Blank and failed glyphs kept (about 4 MiB of metadata with resident
/// entries); the least recently used go first.
const MAX_METADATA_ENTRIES: usize = 65_536;
/// Frames before a glyph whose platform rasterizer failed is tried again.
const TRANSIENT_RETRY_FRAMES: u64 = 30;
/// The front map is cleared past this many keys, since entries of evicted
/// glyphs only go stale in it.
const MAX_FRONT_KEYS: usize = 1 << 16;
/// Frames without overflow after which the overflow pages are released.
const OVERFLOW_KEEP_FRAMES: u64 = 120;

/// Memory limits of one renderer's glyph atlas. Pages are created lazily:
/// these are limits, not reservations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AtlasLimits {
    /// Side of a page in pixels (clamped to the device's texture limit).
    /// A mask page holds one byte per pixel and a color page four.
    pub page_size: u32,
    /// Resident bytes the atlas trims back to: new pages are created
    /// freely below it, and unused glyphs are evicted above it.
    pub target_bytes: u64,
    /// Bytes the atlas never exceeds, overflow pages and pages released
    /// this frame included. A frame whose glyphs need more draws in
    /// overflow mode.
    pub hard_limit_bytes: u64,
}

impl Default for AtlasLimits {
    fn default() -> Self {
        Self {
            page_size: DEFAULT_PAGE_SIZE,
            target_bytes: 48 << 20,
            hard_limit_bytes: 64 << 20,
        }
    }
}

/// Counts of the work a glyph atlas has done since it was created.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AtlasStats {
    /// Glyphs admitted because the cache did not hold them, blank ones
    /// included (not every raster attempt: failures count apart).
    pub misses: u64,
    /// Cached glyphs dropped to make room for others or to trim.
    pub evictions: u64,
    /// Atlas pages created (each a new texture; no cached glyph moves).
    pub growths: u64,
    /// Glyphs rasterized again because the atlas moved them. Always zero:
    /// pages never grow, and evacuation copies on the GPU.
    pub rerasterized: u64,
    /// Bytes of glyph pixels uploaded (gutters excluded).
    pub upload_bytes: u64,
    /// Bytes staged for uploads, gutters and row padding included.
    pub padded_upload_bytes: u64,
    /// Buffer-to-texture copy commands recorded for glyph uploads.
    pub copy_commands: u64,
    /// Writes of staged pixels into transfer buffers.
    pub transfer_chunks: u64,
    /// Glyphs uploaded one by one through the queue (oversized, or past
    /// the staging limit).
    pub unbatched_uploads: u64,
    /// Raster attempts that failed (unsupported, invalid, too large, or a
    /// platform error).
    pub raster_failures: u64,
    /// Glyphs drawn by the font system path because their font instance
    /// could not be prepared for the rasterizer.
    pub fallbacks: u64,
    /// Glyphs the native rasterizer could not draw, drawn by swash.
    pub native_fallbacks: u64,
    /// Glyphs not drawn at all (too large even alone, or overflow staging
    /// full).
    pub dropped_glyphs: u64,
    /// Pages released (emptied by eviction, trimming, or evacuation).
    pub pages_released: u64,
    /// Frames drawn in overflow mode, and the ordinals they drew in.
    pub overflow_frames: u64,
    pub overflow_segments: u64,
    /// Misses that found no room although a page had enough free area.
    pub fragmented_misses: u64,
    /// Sparse pages emptied by moving their glyphs, and the bytes copied.
    pub evacuated_pages: u64,
    pub evacuated_bytes: u64,
}

impl std::ops::Add for AtlasStats {
    type Output = Self;

    fn add(self, o: Self) -> Self {
        Self {
            misses: self.misses + o.misses,
            evictions: self.evictions + o.evictions,
            growths: self.growths + o.growths,
            rerasterized: self.rerasterized + o.rerasterized,
            upload_bytes: self.upload_bytes + o.upload_bytes,
            padded_upload_bytes: self.padded_upload_bytes + o.padded_upload_bytes,
            copy_commands: self.copy_commands + o.copy_commands,
            transfer_chunks: self.transfer_chunks + o.transfer_chunks,
            unbatched_uploads: self.unbatched_uploads + o.unbatched_uploads,
            raster_failures: self.raster_failures + o.raster_failures,
            fallbacks: self.fallbacks + o.fallbacks,
            native_fallbacks: self.native_fallbacks + o.native_fallbacks,
            dropped_glyphs: self.dropped_glyphs + o.dropped_glyphs,
            pages_released: self.pages_released + o.pages_released,
            overflow_frames: self.overflow_frames + o.overflow_frames,
            overflow_segments: self.overflow_segments + o.overflow_segments,
            fragmented_misses: self.fragmented_misses + o.fragmented_misses,
            evacuated_pages: self.evacuated_pages + o.evacuated_pages,
            evacuated_bytes: self.evacuated_bytes + o.evacuated_bytes,
        }
    }
}

/// What a glyph atlas holds now.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AtlasMemory {
    /// Shared pages by kind, dedicated (oversized glyph) textures, and
    /// overflow pages.
    pub mask_pages: u32,
    pub color_pages: u32,
    pub dedicated_pages: u32,
    pub overflow_pages: u32,
    /// Texture bytes of each.
    pub mask_bytes: u64,
    pub color_bytes: u64,
    pub dedicated_bytes: u64,
    pub overflow_bytes: u64,
    /// Bytes of pages released this frame, still counted against the hard
    /// limit until the frame is submitted.
    pub retired_bytes: u64,
    /// Everything above.
    pub total_bytes: u64,
    /// The most `total_bytes` has been.
    pub peak_bytes: u64,
    pub target_bytes: u64,
    pub hard_limit_bytes: u64,
    /// Glyph entries (resident, blank, and failed) and resident ones.
    pub entries: u32,
    pub resident: u32,
    /// Bytes the upload staging holds for reuse, CPU shelves and transfer
    /// buffer.
    pub staging_bytes: u64,
}

/// What decides a glyph's pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum GlyphKey {
    /// Drawn by a rasterizer from an exact font instance; valid across
    /// text systems and font epochs.
    Raster(RasterKey),
    /// Drawn through the font system that shaped it, for faces the
    /// registry cannot prepare; valid for one font epoch.
    System { fonts: FontEpoch, key: CacheKey },
}

/// A glyph entry as a retained draw run remembers it. Stale once the entry
/// is evicted or moved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct GlyphHandle {
    index: u32,
    generation: u32,
}

/// Where a resident bitmap is: its page, its pixels inside the gutter,
/// and its bearings.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Placed {
    pub page: u32,
    alloc: AllocId,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub left: i32,
    pub top: i32,
    pub kind: ContentType,
    /// Coverage planes stacked below each other, a gutter apart (five for
    /// a smoothing bundle, one otherwise).
    pub planes: u8,
}

impl Placed {
    fn rows(&self) -> u32 {
        u32::from(self.planes) * (self.height + 2 * GUTTER)
    }

    /// Texels from one coverage plane to the next; zero for one plane.
    pub(crate) fn plane_step(&self) -> u32 {
        if self.planes > 1 {
            self.height + 2 * GUTTER
        } else {
            0
        }
    }
}

/// Plane steps the vertex format can carry (14 bits).
const MAX_PLANE_STEP: u32 = 1 << 14;

/// A glyph ready to draw.
#[derive(Debug, Clone, Copy)]
pub(crate) struct AtlasGlyph {
    /// `None` for a glyph in an overflow page, which no run may keep.
    pub handle: Option<GlyphHandle>,
    pub placed: Placed,
    /// The overflow ordinal whose uploads it needs (zero outside overflow).
    pub ordinal: u32,
}

#[derive(Debug, Clone, Copy)]
enum State {
    Free,
    /// Nothing to draw.
    Blank,
    /// The rasterizer failed; tried again from frame `retry` (never for a
    /// permanent failure).
    Failed {
        retry: u64,
    },
    Resident(Placed),
}

#[derive(Debug)]
struct Entry {
    key: GlyphKey,
    generation: u32,
    state: State,
    /// The frame that last used it; the current frame pins it.
    used: u64,
    prev: u32,
    next: u32,
    list: u8,
}

/// LRU lists: resident masks, resident color glyphs, and metadata.
const MASK_LIST: u8 = 0;
const COLOR_LIST: u8 = 1;
const META_LIST: u8 = 2;
const NO_LIST: u8 = 3;

fn list_of(kind: ContentType) -> u8 {
    match kind {
        ContentType::Mask => MASK_LIST,
        ContentType::Color => COLOR_LIST,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    Shared,
    Dedicated,
    Overflow,
}

struct Page {
    uid: u32,
    kind: ContentType,
    role: Role,
    texture: Texture,
    view: TextureView,
    width: u32,
    height: u32,
    allocator: AtlasAllocator,
    live: u32,
    bytes: u64,
}

fn bytes_per_pixel(kind: ContentType) -> u32 {
    match kind {
        ContentType::Mask => 1,
        ContentType::Color => 4,
    }
}

fn texture_format(kind: ContentType) -> TextureFormat {
    match kind {
        ContentType::Mask => TextureFormat::R8Unorm,
        ContentType::Color => TextureFormat::Rgba8UnormSrgb,
    }
}

/// A CPU image whose rows are a valid buffer copy pitch, packed with
/// glyph rectangles in shelves.
#[derive(Default)]
struct Shelf {
    pitch: usize,
    bpp: usize,
    data: Vec<u8>,
    x: usize,
    y: usize,
    height: usize,
}

impl Shelf {
    fn new(width: u32, bpp: u32) -> Self {
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as usize;
        Self {
            pitch: (width as usize * bpp as usize).div_ceil(align) * align,
            bpp: bpp as usize,
            ..Self::default()
        }
    }

    fn reset(&mut self) {
        self.data.clear();
        self.x = 0;
        self.y = 0;
        self.height = 0;
    }

    /// Room for a `width` by `height` rectangle, zeroed, at the returned
    /// byte offset; `None` past the pitch or `budget` bytes in all.
    fn place(&mut self, width: usize, height: usize, budget: usize) -> Option<usize> {
        if width * self.bpp > self.pitch {
            return None;
        }
        if (self.x + width) * self.bpp > self.pitch {
            self.y += self.height;
            self.x = 0;
            self.height = 0;
        }
        let end = (self.y + height.max(self.height)) * self.pitch;
        if end > budget {
            return None;
        }
        if self.data.len() < end {
            self.data.resize(end, 0);
        }
        let offset = self.y * self.pitch + self.x * self.bpp;
        self.x += width;
        self.height = self.height.max(height);
        Some(offset)
    }
}

/// A staged glyph: its gutter-wrapped rectangle in a shelf and in a page.
struct PendingCopy {
    kind: ContentType,
    offset: usize,
    page: u32,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    ordinal: u32,
}

/// A glyph moved by evacuation, copied page to page.
struct Move {
    from: Texture,
    from_origin: (u32, u32),
    to: u32,
    to_origin: (u32, u32),
    size: (u32, u32),
}

/// The overflow pages of the frame being prepared.
#[derive(Default)]
struct Overflow {
    active: bool,
    ordinal: u32,
    /// Pages by kind (mask, color), kept between overflow frames.
    pages: [Option<u32>; 2],
    /// Glyphs of the current ordinal.
    placed: HashMap<GlyphKey, Placed, Fx>,
    /// Kinds (mask, color) whose resident pages are full this frame.
    full: [bool; 2],
    last_frame: u64,
}

pub(crate) struct GlyphAtlas {
    device: Device,
    queue: Queue,
    pub(crate) cache: Cache,
    pub(crate) format: TextureFormat,
    limits: AtlasLimits,
    page_size: u32,
    max_dimension: u32,
    pages: Vec<Option<Page>>,
    free_pages: Vec<u32>,
    next_uid: u32,
    entries: Vec<Entry>,
    free_entries: Vec<u32>,
    heads: [u32; 3],
    tails: [u32; 3],
    lens: [usize; 3],
    map: HashMap<GlyphKey, u32, Fx>,
    front: HashMap<(CacheKey, u32), GlyphHandle, Fx>,
    owner: Option<FontEpoch>,
    frame: u64,
    registry: Option<FontRegistry>,
    swash: SwashRasterizer,
    /// The native rasterizer drawing glyphs before swash, if any, and the
    /// font instances it cannot draw (swash draws them).
    native: Option<Native>,
    native_unsupported: std::collections::HashSet<FontInstanceId, Fx>,
    legacy: SwashCache,
    scratch: RasterScratch,
    shelves: [Shelf; 2],
    pending: Vec<PendingCopy>,
    moves: Vec<Move>,
    transfer: Option<Buffer>,
    /// Where each shelf starts in the transfer buffer.
    shelf_base: [u64; 2],
    /// Pages released this frame: kept until its commands are recorded.
    retired: Vec<Texture>,
    retired_bytes: u64,
    overflow: Overflow,
    bind_groups: Vec<(u32, u32, BindGroup)>,
    dummy: [TextureView; 2],
    fragmented: bool,
    peak_bytes: u64,
    stats: AtlasStats,
}

/// A rasterized glyph, its pixels in the atlas's scratch.
struct Drawn {
    kind: ContentType,
    premultiplied: bool,
    left: i32,
    top: i32,
    width: u32,
    height: u32,
    stride: usize,
    /// The first plane's bytes; a bundle's later planes follow at
    /// `plane_len` steps.
    bytes: std::ops::Range<usize>,
    planes: u32,
    plane_len: usize,
}

impl Drawn {
    /// Rows of the glyph's rectangle in the atlas: every plane, each in a
    /// gutter of its own.
    fn rows(&self) -> u32 {
        self.planes * (self.height + 2 * GUTTER)
    }
}

enum Raster {
    Blank,
    Failed { transient: bool },
    Drawn(Drawn),
}

impl GlyphAtlas {
    pub(crate) fn new(
        device: &Device,
        queue: &Queue,
        cache: &Cache,
        format: TextureFormat,
    ) -> Self {
        let max_dimension = device.limits().max_texture_dimension_2d;
        let dummy = [ContentType::Mask, ContentType::Color].map(|kind| {
            device
                .create_texture(&TextureDescriptor {
                    label: Some("quark text atlas placeholder"),
                    size: Extent3d {
                        width: 1,
                        height: 1,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: TextureDimension::D2,
                    format: texture_format(kind),
                    usage: TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&TextureViewDescriptor::default())
        });
        let limits = AtlasLimits::default();
        let page_size = limits.page_size.clamp(1, max_dimension);
        Self {
            device: device.clone(),
            queue: queue.clone(),
            cache: cache.clone(),
            format,
            limits,
            page_size,
            max_dimension,
            pages: Vec::new(),
            free_pages: Vec::new(),
            next_uid: 1,
            entries: Vec::new(),
            free_entries: Vec::new(),
            heads: [NIL; 3],
            tails: [NIL; 3],
            lens: [0; 3],
            map: HashMap::default(),
            front: HashMap::default(),
            owner: None,
            frame: 1,
            registry: None,
            swash: SwashRasterizer::default(),
            native: None,
            native_unsupported: Default::default(),
            legacy: SwashCache::new(),
            scratch: RasterScratch::default(),
            shelves: [Shelf::new(page_size, 1), Shelf::new(page_size, 4)],
            pending: Vec::new(),
            moves: Vec::new(),
            transfer: None,
            shelf_base: [0, 0],
            retired: Vec::new(),
            retired_bytes: 0,
            overflow: Overflow::default(),
            bind_groups: Vec::new(),
            dummy,
            fragmented: false,
            peak_bytes: 0,
            stats: AtlasStats::default(),
        }
    }

    /// Uses `limits` from now on. A new page size applies to new pages;
    /// pages already made keep theirs until released.
    pub(crate) fn set_limits(&mut self, limits: AtlasLimits) {
        self.limits = limits;
        let page_size = limits.page_size.clamp(16, self.max_dimension);
        if page_size != self.page_size {
            self.clear();
            self.page_size = page_size;
            self.shelves = [Shelf::new(page_size, 1), Shelf::new(page_size, 4)];
        }
    }

    /// Rasterizes glyphs with `choice` from now on (swash where it cannot
    /// draw a font or glyph). Glyphs of the previous rasterizer are
    /// forgotten. Fails, changing nothing, when this build or platform
    /// lacks the rasterizer.
    pub(crate) fn set_rasterizer(&mut self, choice: TextRasterizer) -> Result<(), RasterError> {
        self.native = Native::new(choice)?;
        self.native_unsupported.clear();
        self.clear();
        Ok(())
    }

    /// The rasterizer drawing glyphs before swash: `None` for swash alone.
    pub(crate) fn native_profile(&self) -> Option<super::raster::RasterProfile> {
        self.native.as_ref().map(Native::profile)
    }

    pub(crate) fn stats(&self) -> AtlasStats {
        self.stats
    }

    pub(crate) fn memory(&self) -> AtlasMemory {
        let mut memory = AtlasMemory {
            retired_bytes: self.retired_bytes,
            peak_bytes: self.peak_bytes,
            target_bytes: self.limits.target_bytes,
            hard_limit_bytes: self.limits.hard_limit_bytes,
            staging_bytes: self
                .shelves
                .iter()
                .map(|s| s.data.capacity() as u64)
                .sum::<u64>()
                + self.transfer.as_ref().map_or(0, Buffer::size),
            ..AtlasMemory::default()
        };
        for page in self.pages.iter().flatten() {
            match (page.role, page.kind) {
                (Role::Shared, ContentType::Mask) => {
                    memory.mask_pages += 1;
                    memory.mask_bytes += page.bytes;
                }
                (Role::Shared, ContentType::Color) => {
                    memory.color_pages += 1;
                    memory.color_bytes += page.bytes;
                }
                (Role::Dedicated, _) => {
                    memory.dedicated_pages += 1;
                    memory.dedicated_bytes += page.bytes;
                }
                (Role::Overflow, _) => {
                    memory.overflow_pages += 1;
                    memory.overflow_bytes += page.bytes;
                }
            }
        }
        memory.total_bytes = self.total_bytes();
        for entry in &self.entries {
            match entry.state {
                State::Free => {}
                State::Resident(_) => {
                    memory.entries += 1;
                    memory.resident += 1;
                }
                _ => memory.entries += 1,
            }
        }
        memory
    }

    fn total_bytes(&self) -> u64 {
        self.pages.iter().flatten().map(|p| p.bytes).sum::<u64>() + self.retired_bytes
    }

    /// Bytes kept free below the hard limit for one overflow page per
    /// kind, unless those pages already exist.
    fn overflow_reserve(&self) -> u64 {
        let side = u64::from(self.page_size) * u64::from(self.page_size);
        [ContentType::Mask, ContentType::Color]
            .iter()
            .enumerate()
            .filter(|(i, _)| self.overflow.pages[*i].is_none())
            .map(|(_, &kind)| side * u64::from(bytes_per_pixel(kind)))
            .sum()
    }

    /// Starts preparing a frame drawn with `text`: unpins last frame's
    /// glyphs, follows the text system's fonts, and moves the glyphs of
    /// one sparse page when misses found pages fragmented.
    pub(crate) fn begin_frame(&mut self, text: &mut TextSystem) {
        self.frame += 1;
        self.retired_bytes = 0;
        let fonts = text.font_epoch();
        if self.owner != Some(fonts) {
            self.owner = Some(fonts);
            self.front.clear();
            let snapshot = text.font_snapshot();
            match &mut self.registry {
                Some(registry) => registry.set_snapshot(snapshot),
                None => self.registry = Some(FontRegistry::new(snapshot)),
            }
        }
        if std::mem::take(&mut self.fragmented) {
            self.evacuate();
            debug_assert!(
                self.verify_integrity().is_ok(),
                "{:?}",
                self.verify_integrity()
            );
        }
    }

    /// Forgets every glyph and releases every page.
    pub(crate) fn clear(&mut self) {
        for index in 0..self.entries.len() as u32 {
            if !matches!(self.entries[index as usize].state, State::Free) {
                self.remove(index);
            }
        }
        for slot in 0..self.pages.len() as u32 {
            if self.pages[slot as usize].is_some() {
                self.release_page(slot);
            }
        }
        self.overflow.pages = [None, None];
        self.front.clear();
        self.pending.clear();
        self.moves.clear();
        for shelf in &mut self.shelves {
            shelf.reset();
        }
    }

    /// Whether every handle still names its glyph; if so pins them all for
    /// this frame, so no miss evicts them.
    pub(crate) fn pin(&mut self, handles: &[GlyphHandle]) -> bool {
        let valid = handles.iter().all(|h| {
            self.entries.get(h.index as usize).is_some_and(|e| {
                e.generation == h.generation && matches!(e.state, State::Resident(_))
            })
        });
        if valid {
            for h in handles {
                self.touch(h.index);
            }
        }
        valid
    }

    /// The glyph cosmic-text keyed as `key`, shaped at device scale
    /// `scale`: resident (pinned for this frame), or rasterized and placed
    /// now. `None` for a glyph with nothing to draw or that cannot be
    /// drawn. Fails only when the frame's pinned glyphs fill the hard
    /// limit outside overflow mode.
    pub(crate) fn glyph(
        &mut self,
        font_system: &mut FontSystem,
        key: CacheKey,
        scale: f32,
    ) -> Result<Option<AtlasGlyph>, PrepareError> {
        // Linear correction and its backdrop are paint, not pixels.
        let key = CacheKey {
            flags: key.flags & !(CacheKeyFlags::LINEAR_CORRECTED | CacheKeyFlags::BLEND_BACKGROUND),
            ..key
        };
        let front = (key, scale.to_bits());
        let index = match self.front.get(&front) {
            Some(&handle) if self.current(handle) => Some(handle.index),
            _ => None,
        };
        let index = match index {
            Some(index) => index,
            None => {
                let (glyph_key, font) = self.resolve(key, scale);
                let index = match self.map.get(&glyph_key) {
                    Some(&index) => index,
                    None => match self.admit(font_system, glyph_key, key, font.as_ref())? {
                        Admitted::Entry(index) => index,
                        Admitted::Transient(placed) => {
                            return Ok(Some(AtlasGlyph {
                                handle: None,
                                placed,
                                ordinal: self.overflow.ordinal,
                            }));
                        }
                        Admitted::Dropped => return Ok(None),
                    },
                };
                if self.front.len() >= MAX_FRONT_KEYS {
                    self.front.clear();
                }
                let generation = self.entries[index as usize].generation;
                self.front.insert(front, GlyphHandle { index, generation });
                index
            }
        };
        let entry = &self.entries[index as usize];
        let generation = entry.generation;
        match entry.state {
            State::Resident(placed) => {
                self.touch(index);
                Ok(Some(AtlasGlyph {
                    handle: Some(GlyphHandle { index, generation }),
                    placed,
                    ordinal: 0,
                }))
            }
            State::Failed { retry } if retry <= self.frame => {
                // A transient failure: forget it and try again.
                self.remove(index);
                self.glyph(font_system, key, scale)
            }
            _ => {
                self.touch(index);
                Ok(None)
            }
        }
    }

    fn current(&self, handle: GlyphHandle) -> bool {
        self.entries
            .get(handle.index as usize)
            .is_some_and(|e| e.generation == handle.generation && !matches!(e.state, State::Free))
    }

    fn resolve(&mut self, key: CacheKey, scale: f32) -> (GlyphKey, Option<PreparedFont>) {
        if let Some(registry) = &mut self.registry
            && let Ok(font) = registry.prepare(key.font_id, key.font_weight, key.flags)
            && let Ok(request) = RasterRequest::from_cache_key(&key, scale)
        {
            let profile = match &self.native {
                Some(native) if !self.native_unsupported.contains(&font.instance()) => {
                    native.profile()
                }
                _ => self.swash.profile(),
            };
            let key = RasterKey {
                profile,
                instance: font.instance(),
                request,
            };
            return (GlyphKey::Raster(key), Some(font.clone()));
        }
        let fonts = self.owner.expect("begin_frame names the fonts");
        (GlyphKey::System { fonts, key }, None)
    }

    fn rasterize(
        &mut self,
        font_system: &mut FontSystem,
        glyph_key: &GlyphKey,
        key: CacheKey,
        font: Option<&PreparedFont>,
    ) -> Raster {
        if let (GlyphKey::Raster(raster), Some(font)) = (glyph_key, font) {
            if let Some(native) = &mut self.native
                && raster.profile == native.profile()
            {
                match native.rasterize(font, &raster.request, &mut self.scratch) {
                    Ok(NativeOutcome::Single(outcome)) => return self.raster_outcome(Ok(outcome)),
                    Ok(NativeOutcome::Bundle {
                        placement,
                        stride,
                        planes,
                    }) => {
                        return Raster::Drawn(Drawn {
                            kind: ContentType::Mask,
                            premultiplied: false,
                            left: placement.left,
                            top: placement.top,
                            width: placement.width,
                            height: placement.height,
                            stride: stride as usize,
                            plane_len: planes[0].len(),
                            bytes: planes[0].clone(),
                            planes: planes.len() as u32,
                        });
                    }
                    // A font the backend cannot use: swash draws all of it.
                    Err(RasterError::UnsupportedFont) => {
                        self.native_unsupported.insert(font.instance());
                        self.stats.native_fallbacks += 1;
                    }
                    // A glyph or option it cannot draw: swash draws this
                    // glyph, kept under the native key.
                    Err(RasterError::UnsupportedFormat | RasterError::UnsupportedOptions) => {
                        self.stats.native_fallbacks += 1;
                    }
                    Err(error) => return self.raster_outcome(Err(error)),
                }
            }
            let outcome = self
                .swash
                .rasterize(font, &raster.request, &mut self.scratch);
            return self.raster_outcome(outcome);
        }
        self.legacy_raster(font_system, key)
    }

    /// A single-bitmap outcome as the atlas admits it.
    fn raster_outcome(&self, outcome: Result<RasterOutcome, RasterError>) -> Raster {
        match outcome {
            Ok(RasterOutcome::Empty) => Raster::Blank,
            Ok(RasterOutcome::Bitmap(bitmap)) => {
                let (kind, premultiplied) = match bitmap.content {
                    BitmapContent::Mask => (ContentType::Mask, false),
                    BitmapContent::Color { alpha } => {
                        (ContentType::Color, alpha == AlphaMode::Premultiplied)
                    }
                };
                Raster::Drawn(Drawn {
                    kind,
                    premultiplied,
                    left: bitmap.placement.left,
                    top: bitmap.placement.top,
                    width: bitmap.placement.width,
                    height: bitmap.placement.height,
                    stride: bitmap.stride as usize,
                    plane_len: bitmap.bytes.len(),
                    bytes: bitmap.bytes,
                    planes: 1,
                })
            }
            Err(error) => Raster::Failed {
                transient: error == RasterError::Platform,
            },
        }
    }

    /// Draws a glyph through the font system that shaped it, for a face
    /// the registry could not prepare.
    fn legacy_raster(&mut self, font_system: &mut FontSystem, key: CacheKey) -> Raster {
        self.stats.fallbacks += 1;
        let Some(image) = self.legacy.get_image_uncached(font_system, key) else {
            return Raster::Blank;
        };
        let (kind, premultiplied) = match image.content {
            SwashContent::Mask => (ContentType::Mask, false),
            SwashContent::Color => (
                ContentType::Color,
                matches!(
                    image.source,
                    quark_text::cosmic_text::SwashSource::ColorOutline(_)
                ),
            ),
            SwashContent::SubpixelMask => return Raster::Failed { transient: false },
        };
        let placement = image.placement;
        if placement.width == 0 || placement.height == 0 {
            return Raster::Blank;
        }
        let stride = placement.width as usize * bytes_per_pixel(kind) as usize;
        let len = stride * placement.height as usize;
        if len > image.data.len() {
            return Raster::Failed { transient: false };
        }
        self.scratch.pixels = image.data;
        Raster::Drawn(Drawn {
            kind,
            premultiplied,
            left: placement.left,
            top: placement.top,
            width: placement.width,
            height: placement.height,
            stride,
            bytes: 0..len,
            planes: 1,
            plane_len: len,
        })
    }

    fn admit(
        &mut self,
        font_system: &mut FontSystem,
        glyph_key: GlyphKey,
        key: CacheKey,
        font: Option<&PreparedFont>,
    ) -> Result<Admitted, PrepareError> {
        if self.overflow.active
            && let Some(&placed) = self.overflow.placed.get(&glyph_key)
        {
            return Ok(Admitted::Transient(placed));
        }
        let drawn = match self.rasterize(font_system, &glyph_key, key, font) {
            Raster::Blank => {
                self.stats.misses += 1;
                return Ok(Admitted::Entry(self.insert(glyph_key, State::Blank)));
            }
            Raster::Failed { transient } => {
                self.stats.raster_failures += 1;
                let retry = if transient {
                    self.frame + TRANSIENT_RETRY_FRAMES
                } else {
                    u64::MAX
                };
                return Ok(Admitted::Entry(
                    self.insert(glyph_key, State::Failed { retry }),
                ));
            }
            Raster::Drawn(drawn) => drawn,
        };
        let bpp = bytes_per_pixel(drawn.kind);
        let (w, h) = (drawn.width + 2 * GUTTER, drawn.rows());
        let too_large = (drawn.planes > 1 && drawn.height + 2 * GUTTER >= MAX_PLANE_STEP)
            || w > self.max_dimension
            || h > self.max_dimension
            || u64::from(w) * u64::from(h) * u64::from(bpp) > MAX_BITMAP_BYTES as u64;
        if too_large {
            self.stats.raster_failures += 1;
            self.stats.dropped_glyphs += 1;
            return Ok(Admitted::Entry(
                self.insert(glyph_key, State::Failed { retry: u64::MAX }),
            ));
        }
        let kind_index = (drawn.kind == ContentType::Color) as usize;
        let resident = if self.overflow.active {
            // Overflowing, glyphs still become resident while the atlas has
            // room; past that, only the overflow pages take them.
            if self.overflow.full[kind_index] {
                None
            } else {
                match self.allocate(drawn.kind, w, h) {
                    Ok(found) => Some(found),
                    Err(PrepareError::AtlasFull) => {
                        self.overflow.full[kind_index] = true;
                        None
                    }
                }
            }
        } else {
            Some(self.allocate(drawn.kind, w, h)?)
        };
        let Some((page, alloc, x, y)) = resident else {
            let Some((page, alloc, x, y)) = self.overflow_allocate(drawn.kind, w, h) else {
                self.stats.dropped_glyphs += 1;
                return Ok(Admitted::Dropped);
            };
            let placed = self.placed(&drawn, page, alloc, x, y);
            if !self.stage(&drawn, &placed, self.overflow.ordinal, false) {
                self.stats.dropped_glyphs += 1;
                return Ok(Admitted::Dropped);
            }
            self.stats.misses += 1;
            self.overflow.placed.insert(glyph_key, placed);
            return Ok(Admitted::Transient(placed));
        };
        let placed = self.placed(&drawn, page, alloc, x, y);
        self.stage(&drawn, &placed, 0, true);
        self.stats.misses += 1;
        if let Some(page) = self.pages[page as usize].as_mut() {
            page.live += 1;
        }
        Ok(Admitted::Entry(
            self.insert(glyph_key, State::Resident(placed)),
        ))
    }

    fn placed(&self, drawn: &Drawn, page: u32, alloc: AllocId, x: u32, y: u32) -> Placed {
        Placed {
            page,
            alloc,
            x: x + GUTTER,
            y: y + GUTTER,
            width: drawn.width,
            height: drawn.height,
            left: drawn.left,
            top: drawn.top,
            kind: drawn.kind,
            planes: drawn.planes as u8,
        }
    }

    /// Copies `drawn`'s pixels, gutter around them, into the staging
    /// shelves for `placed`'s page. With `fallback`, a glyph that does not
    /// fit the shelves is written through the queue instead; without it,
    /// false.
    fn stage(&mut self, drawn: &Drawn, placed: &Placed, ordinal: u32, fallback: bool) -> bool {
        let bpp = bytes_per_pixel(drawn.kind) as usize;
        let (w, h) = (
            drawn.width as usize + 2 * GUTTER as usize,
            drawn.rows() as usize,
        );
        let shelf_index = (drawn.kind == ContentType::Color) as usize;
        let used = self.shelves.iter().map(|s| s.data.len()).sum::<usize>()
            - self.shelves[shelf_index].data.len();
        let budget = MAX_STAGING_BYTES.saturating_sub(used);
        let pixels = &self.scratch.pixels;
        let row_bytes = drawn.width as usize * bpp;
        let plane_rows = drawn.height as usize + 2 * GUTTER as usize;
        let write_rows = |out: &mut [u8], pitch: usize| {
            for plane in 0..drawn.planes as usize {
                let start = drawn.bytes.start + plane * drawn.plane_len;
                let source = &pixels[start..start + drawn.plane_len];
                for row in 0..drawn.height as usize {
                    let src = &source[row * drawn.stride..row * drawn.stride + row_bytes];
                    let at = (plane * plane_rows + row + GUTTER as usize) * pitch
                        + GUTTER as usize * bpp;
                    let dst = &mut out[at..at + row_bytes];
                    dst.copy_from_slice(src);
                    if drawn.premultiplied {
                        unpremultiply(dst);
                    }
                }
            }
        };
        self.stats.upload_bytes +=
            (row_bytes * drawn.height as usize * drawn.planes as usize) as u64;
        let shelf = &mut self.shelves[shelf_index];
        if let Some(offset) = shelf.place(w, h, budget) {
            let pitch = shelf.pitch;
            write_rows(&mut shelf.data[offset..], pitch);
            self.pending.push(PendingCopy {
                kind: drawn.kind,
                offset,
                page: placed.page,
                x: placed.x - GUTTER,
                y: placed.y - GUTTER,
                width: w as u32,
                height: h as u32,
                ordinal,
            });
            return true;
        }
        if !fallback {
            return false;
        }
        // Oversized for the shelves or past the staging limit: one write
        // through the queue, which lands before the frame's commands.
        let mut image = vec![0u8; w * h * bpp];
        write_rows(&mut image, w * bpp);
        self.stats.unbatched_uploads += 1;
        self.stats.padded_upload_bytes += image.len() as u64;
        if let Some(page) = self.pages[placed.page as usize].as_ref() {
            self.queue.write_texture(
                TexelCopyTextureInfo {
                    texture: &page.texture,
                    mip_level: 0,
                    origin: Origin3d {
                        x: placed.x - GUTTER,
                        y: placed.y - GUTTER,
                        z: 0,
                    },
                    aspect: TextureAspect::All,
                },
                &image,
                TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some((w * bpp) as u32),
                    rows_per_image: None,
                },
                Extent3d {
                    width: w as u32,
                    height: h as u32,
                    depth_or_array_layers: 1,
                },
            );
        }
        true
    }

    fn insert(&mut self, key: GlyphKey, state: State) -> u32 {
        let list = match state {
            State::Resident(placed) => list_of(placed.kind),
            _ => META_LIST,
        };
        if list == META_LIST && self.lens[META_LIST as usize] >= MAX_METADATA_ENTRIES {
            let tail = self.tails[META_LIST as usize];
            if tail != NIL && self.entries[tail as usize].used != self.frame {
                self.remove(tail);
            }
        }
        let index = match self.free_entries.pop() {
            Some(index) => {
                let entry = &mut self.entries[index as usize];
                entry.key = key;
                entry.state = state;
                index
            }
            None => {
                self.entries.push(Entry {
                    key,
                    generation: 0,
                    state,
                    used: 0,
                    prev: NIL,
                    next: NIL,
                    list: NO_LIST,
                });
                self.entries.len() as u32 - 1
            }
        };
        self.map.insert(key, index);
        self.link(index, list);
        self.entries[index as usize].used = self.frame;
        index
    }

    /// Drops entry `index`: frees its rectangle, stales its handles.
    fn remove(&mut self, index: u32) {
        self.unlink(index);
        let entry = &mut self.entries[index as usize];
        let state = std::mem::replace(&mut entry.state, State::Free);
        entry.generation = entry.generation.wrapping_add(1);
        let key = entry.key;
        self.map.remove(&key);
        self.free_entries.push(index);
        self.stats.evictions += 1;
        if let State::Resident(placed) = state {
            let (empty, dedicated) = match self.pages[placed.page as usize].as_mut() {
                Some(page) => {
                    page.allocator.deallocate(placed.alloc);
                    page.live -= 1;
                    (page.live == 0, page.role == Role::Dedicated)
                }
                None => (false, false),
            };
            // An empty page goes when it is oversized, the atlas is over
            // its target, or another page of its kind remains.
            if empty
                && (dedicated
                    || self.total_bytes_without_retired() > self.limits.target_bytes
                    || self.shared_pages(placed.kind) > 1)
            {
                self.release_page(placed.page);
            }
        }
    }

    fn total_bytes_without_retired(&self) -> u64 {
        self.pages.iter().flatten().map(|p| p.bytes).sum()
    }

    fn shared_pages(&self, kind: ContentType) -> usize {
        self.pages
            .iter()
            .flatten()
            .filter(|p| p.role == Role::Shared && p.kind == kind)
            .count()
    }

    fn touch(&mut self, index: u32) {
        let list = self.entries[index as usize].list;
        self.entries[index as usize].used = self.frame;
        if self.heads[list as usize] != index {
            self.unlink(index);
            self.link(index, list);
        }
    }

    fn link(&mut self, index: u32, list: u8) {
        let l = list as usize;
        let head = self.heads[l];
        {
            let entry = &mut self.entries[index as usize];
            entry.prev = NIL;
            entry.next = head;
            entry.list = list;
        }
        if head != NIL {
            self.entries[head as usize].prev = index;
        } else {
            self.tails[l] = index;
        }
        self.heads[l] = index;
        self.lens[l] += 1;
    }

    fn unlink(&mut self, index: u32) {
        let (prev, next, list) = {
            let e = &self.entries[index as usize];
            (e.prev, e.next, e.list)
        };
        if list == NO_LIST {
            return;
        }
        let l = list as usize;
        if prev != NIL {
            self.entries[prev as usize].next = next;
        } else {
            self.heads[l] = next;
        }
        if next != NIL {
            self.entries[next as usize].prev = prev;
        } else {
            self.tails[l] = prev;
        }
        let entry = &mut self.entries[index as usize];
        entry.prev = NIL;
        entry.next = NIL;
        entry.list = NO_LIST;
        self.lens[l] -= 1;
    }

    /// Evicts the least recently used unpinned glyph of `kind`; false when
    /// every one is pinned.
    fn evict_one(&mut self, kind: ContentType) -> bool {
        let tail = self.tails[list_of(kind) as usize];
        if tail == NIL || self.entries[tail as usize].used == self.frame {
            return false;
        }
        self.remove(tail);
        true
    }

    /// Releases the shared page with the fewest glyphs none of which is
    /// pinned, evicting them; false when every page holds a pinned glyph.
    fn release_unpinned_page(&mut self) -> bool {
        let mut pinned = vec![false; self.pages.len()];
        for entry in &self.entries {
            if let State::Resident(placed) = entry.state
                && entry.used == self.frame
            {
                pinned[placed.page as usize] = true;
            }
        }
        let candidate = self
            .pages
            .iter()
            .enumerate()
            .filter_map(|(slot, page)| Some((slot, page.as_ref()?)))
            .filter(|(slot, page)| page.role != Role::Overflow && !pinned[*slot])
            .min_by_key(|(_, page)| page.live)
            .map(|(slot, _)| slot as u32);
        let Some(slot) = candidate else {
            return false;
        };
        self.empty_page(slot);
        if self.pages[slot as usize].is_some() {
            self.release_page(slot);
        }
        true
    }

    /// Evicts every glyph on page `slot`.
    fn empty_page(&mut self, slot: u32) {
        for index in 0..self.entries.len() as u32 {
            if let State::Resident(placed) = self.entries[index as usize].state
                && placed.page == slot
            {
                self.remove(index);
            }
        }
    }

    /// Space for a `w` by `h` rectangle (gutter included) of `kind`.
    fn allocate(
        &mut self,
        kind: ContentType,
        w: u32,
        h: u32,
    ) -> Result<(u32, AllocId, u32, u32), PrepareError> {
        let side = self.page_size;
        if w > side || h > side {
            return self.dedicated(kind, w, h);
        }
        let page_bytes = u64::from(side) * u64::from(side) * u64::from(bytes_per_pixel(kind));
        let mut fragmented = false;
        loop {
            if let Some(found) = self.try_pages(kind, w, h, &mut fragmented) {
                return Ok(found);
            }
            if self.total_bytes() + page_bytes <= self.limits.target_bytes {
                return Ok(self.allocate_in_new_page(kind, w, h));
            }
            if self.evict_one(kind) {
                continue;
            }
            if self.total_bytes() + page_bytes + self.overflow_reserve()
                <= self.limits.hard_limit_bytes
            {
                return Ok(self.allocate_in_new_page(kind, w, h));
            }
            if self.release_unpinned_page() {
                continue;
            }
            if fragmented {
                self.stats.fragmented_misses += 1;
                self.fragmented = true;
            }
            return Err(PrepareError::AtlasFull);
        }
    }

    fn try_pages(
        &mut self,
        kind: ContentType,
        w: u32,
        h: u32,
        fragmented: &mut bool,
    ) -> Option<(u32, AllocId, u32, u32)> {
        let need = (w * h) as i32;
        for slot in (0..self.pages.len()).rev() {
            let Some(page) = self.pages[slot].as_mut() else {
                continue;
            };
            if page.role != Role::Shared || page.kind != kind {
                continue;
            }
            if let Some(allocation) = page.allocator.allocate(size2(w as i32, h as i32)) {
                let min = allocation.rectangle.min;
                return Some((slot as u32, allocation.id, min.x as u32, min.y as u32));
            }
            if page.allocator.free_space() >= need {
                *fragmented = true;
            }
        }
        if *fragmented {
            self.stats.fragmented_misses += 1;
            self.fragmented = true;
        }
        None
    }

    fn allocate_in_new_page(
        &mut self,
        kind: ContentType,
        w: u32,
        h: u32,
    ) -> (u32, AllocId, u32, u32) {
        let side = self.page_size;
        let slot = self.new_page(kind, Role::Shared, side, side);
        let page = self.pages[slot as usize].as_mut().expect("new page");
        let allocation = page
            .allocator
            .allocate(size2(w as i32, h as i32))
            .expect("a glyph no larger than an empty page fits it");
        let min = allocation.rectangle.min;
        (slot, allocation.id, min.x as u32, min.y as u32)
    }

    fn dedicated(
        &mut self,
        kind: ContentType,
        w: u32,
        h: u32,
    ) -> Result<(u32, AllocId, u32, u32), PrepareError> {
        let bytes = u64::from(w) * u64::from(h) * u64::from(bytes_per_pixel(kind));
        while self.total_bytes() + bytes + self.overflow_reserve() > self.limits.hard_limit_bytes {
            if !self.release_unpinned_page() {
                return Err(PrepareError::AtlasFull);
            }
        }
        let slot = self.new_page(kind, Role::Dedicated, w, h);
        let page = self.pages[slot as usize].as_mut().expect("new page");
        let allocation = page
            .allocator
            .allocate(size2(w as i32, h as i32))
            .expect("a dedicated page fits its glyph");
        Ok((slot, allocation.id, 0, 0))
    }

    fn new_page(&mut self, kind: ContentType, role: Role, width: u32, height: u32) -> u32 {
        let texture = self.device.create_texture(&TextureDescriptor {
            label: Some(match kind {
                ContentType::Mask => "quark text mask page",
                ContentType::Color => "quark text color page",
            }),
            size: Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: texture_format(kind),
            usage: TextureUsages::TEXTURE_BINDING
                | TextureUsages::COPY_DST
                | TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&TextureViewDescriptor::default());
        let page = Page {
            uid: self.next_uid,
            kind,
            role,
            texture,
            view,
            width,
            height,
            allocator: AtlasAllocator::new(size2(width as i32, height as i32)),
            live: 0,
            bytes: u64::from(width) * u64::from(height) * u64::from(bytes_per_pixel(kind)),
        };
        self.next_uid += 1;
        self.stats.growths += 1;
        let slot = match self.free_pages.pop() {
            Some(slot) => {
                self.pages[slot as usize] = Some(page);
                slot
            }
            None => {
                self.pages.push(Some(page));
                self.pages.len() as u32 - 1
            }
        };
        self.peak_bytes = self.peak_bytes.max(self.total_bytes());
        slot
    }

    fn release_page(&mut self, slot: u32) {
        let Some(page) = self.pages[slot as usize].take() else {
            return;
        };
        self.free_pages.push(slot);
        self.retired_bytes += page.bytes;
        self.bind_groups
            .retain(|&(color, mask, _)| color != page.uid && mask != page.uid);
        self.pending.retain(|copy| copy.page != slot);
        self.retired.push(page.texture);
        self.stats.pages_released += 1;
    }

    /// Room in the current overflow ordinal's page of `kind`, starting the
    /// next ordinal when it is full; `None` for a glyph larger than an
    /// empty overflow page.
    fn overflow_allocate(
        &mut self,
        kind: ContentType,
        w: u32,
        h: u32,
    ) -> Option<(u32, AllocId, u32, u32)> {
        let i = (kind == ContentType::Color) as usize;
        let slot = match self.overflow.pages[i] {
            Some(slot) => slot,
            None => {
                let side = self.page_size;
                let slot = self.new_page(kind, Role::Overflow, side, side);
                self.overflow.pages[i] = Some(slot);
                slot
            }
        };
        let page = self.pages[slot as usize].as_mut()?;
        if let Some(allocation) = page.allocator.allocate(size2(w as i32, h as i32)) {
            let min = allocation.rectangle.min;
            return Some((slot, allocation.id, min.x as u32, min.y as u32));
        }
        if page.allocator.is_empty() {
            return None;
        }
        // The next ordinal reuses both pages from empty.
        self.overflow.ordinal += 1;
        self.stats.overflow_segments += 1;
        self.overflow.placed.clear();
        for slot in self.overflow.pages.into_iter().flatten() {
            if let Some(page) = self.pages[slot as usize].as_mut() {
                page.allocator.clear();
            }
        }
        let page = self.pages[slot as usize].as_mut()?;
        let allocation = page.allocator.allocate(size2(w as i32, h as i32))?;
        let min = allocation.rectangle.min;
        Some((slot, allocation.id, min.x as u32, min.y as u32))
    }

    /// Switches the frame being prepared to overflow mode: glyphs not
    /// resident go through the overflow pages, ordinal by ordinal.
    pub(crate) fn begin_overflow(&mut self) {
        self.overflow.active = true;
        self.overflow.ordinal = 0;
        self.overflow.full = [false; 2];
        self.overflow.placed.clear();
        self.overflow.last_frame = self.frame;
        for slot in self.overflow.pages.into_iter().flatten() {
            if let Some(page) = self.pages[slot as usize].as_mut() {
                page.allocator.clear();
            }
        }
        // Copies staged for overflow pages by an earlier attempt are void.
        let overflow = self.overflow.pages;
        self.pending
            .retain(|copy| !overflow.contains(&Some(copy.page)));
        self.stats.overflow_frames += 1;
        self.stats.overflow_segments += 1;
    }

    pub(crate) fn overflowing(&self) -> bool {
        self.overflow.active
    }

    /// The bind group drawing glyphs from color page `color` and mask page
    /// `mask` (either may be absent).
    pub(crate) fn bind_group(&mut self, color: Option<u32>, mask: Option<u32>) -> BindGroup {
        let page = |slot: Option<u32>| slot.and_then(|s| self.pages[s as usize].as_ref());
        let (color_page, mask_page) = (page(color), page(mask));
        let key = (
            color_page.map_or(0, |p| p.uid),
            mask_page.map_or(0, |p| p.uid),
        );
        if let Some((_, _, group)) = self.bind_groups.iter().find(|(c, m, _)| (*c, *m) == key) {
            return group.clone();
        }
        let group = self.cache.create_atlas_bind_group(
            &self.device,
            color_page.map_or(&self.dummy[1], |p| &p.view),
            mask_page.map_or(&self.dummy[0], |p| &p.view),
        );
        self.bind_groups.push((key.0, key.1, group.clone()));
        group
    }

    /// Records this frame's evacuation moves and staged uploads (all but
    /// later overflow ordinals') into `encoder`, ahead of its draws.
    pub(crate) fn record_uploads(&mut self, queue: &Queue, encoder: &mut CommandEncoder) {
        for m in self.moves.drain(..) {
            let Some(to) = self.pages[m.to as usize].as_ref() else {
                continue;
            };
            encoder.copy_texture_to_texture(
                TexelCopyTextureInfo {
                    texture: &m.from,
                    mip_level: 0,
                    origin: Origin3d {
                        x: m.from_origin.0,
                        y: m.from_origin.1,
                        z: 0,
                    },
                    aspect: TextureAspect::All,
                },
                TexelCopyTextureInfo {
                    texture: &to.texture,
                    mip_level: 0,
                    origin: Origin3d {
                        x: m.to_origin.0,
                        y: m.to_origin.1,
                        z: 0,
                    },
                    aspect: TextureAspect::All,
                },
                Extent3d {
                    width: m.size.0,
                    height: m.size.1,
                    depth_or_array_layers: 1,
                },
            );
        }
        if self.pending.is_empty() {
            return;
        }
        // One write per shelf into the transfer buffer.
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as u64;
        let mask_len = (self.shelves[0].data.len() as u64).div_ceil(align) * align;
        let color_len = (self.shelves[1].data.len() as u64).div_ceil(4) * 4;
        let needed = (mask_len + color_len).max(align);
        if self.transfer.as_ref().is_none_or(|b| b.size() < needed) {
            self.transfer = Some(self.device.create_buffer(&BufferDescriptor {
                label: Some("quark text uploads"),
                size: needed.next_power_of_two(),
                usage: BufferUsages::COPY_SRC | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        let transfer = self.transfer.as_ref().expect("transfer buffer");
        for (shelf, base) in self.shelves.iter_mut().zip([0, mask_len]) {
            if shelf.data.is_empty() {
                continue;
            }
            let padded = shelf.data.len().div_ceil(4) * 4;
            shelf.data.resize(padded, 0);
            queue.write_buffer(transfer, base, &shelf.data);
            self.stats.transfer_chunks += 1;
            self.stats.padded_upload_bytes += shelf.data.len() as u64;
        }
        self.shelf_base = [0, mask_len];
        self.stats.copy_commands += self.pending.iter().filter(|c| c.ordinal > 0).count() as u64;
        self.stats.copy_commands += self.record_ordinal(encoder, 0);
    }

    /// Records the uploads overflow ordinal `ordinal` draws from, after
    /// the previous ordinal's draws.
    pub(crate) fn record_overflow(&self, encoder: &mut CommandEncoder, ordinal: u32) {
        self.record_ordinal(encoder, ordinal);
    }

    /// Records the copies of `ordinal`'s staged glyphs; returns how many.
    fn record_ordinal(&self, encoder: &mut CommandEncoder, ordinal: u32) -> u64 {
        let Some(transfer) = self.transfer.as_ref() else {
            return 0;
        };
        let mut recorded = 0;
        for copy in self.pending.iter().filter(|c| c.ordinal == ordinal) {
            let Some(page) = self.pages[copy.page as usize].as_ref() else {
                continue;
            };
            let shelf = (copy.kind == ContentType::Color) as usize;
            encoder.copy_buffer_to_texture(
                TexelCopyBufferInfo {
                    buffer: transfer,
                    layout: TexelCopyBufferLayout {
                        offset: self.shelf_base[shelf] + copy.offset as u64,
                        bytes_per_row: Some(self.shelves[shelf].pitch as u32),
                        rows_per_image: None,
                    },
                },
                TexelCopyTextureInfo {
                    texture: &page.texture,
                    mip_level: 0,
                    origin: Origin3d {
                        x: copy.x,
                        y: copy.y,
                        z: 0,
                    },
                    aspect: TextureAspect::All,
                },
                Extent3d {
                    width: copy.width,
                    height: copy.height,
                    depth_or_array_layers: 1,
                },
            );
            recorded += 1;
        }
        recorded
    }

    /// Ends the frame once its commands are recorded: drops its uploads,
    /// lets pages released this frame go, leaves overflow mode, and trims
    /// back to the target.
    pub(crate) fn end_frame(&mut self) {
        self.pending.clear();
        for shelf in &mut self.shelves {
            shelf.reset();
        }
        self.retired.clear();
        self.overflow.active = false;
        self.overflow.placed.clear();
        if self.frame.saturating_sub(self.overflow.last_frame) > OVERFLOW_KEEP_FRAMES {
            for slot in std::mem::take(&mut self.overflow.pages)
                .into_iter()
                .flatten()
            {
                self.release_page(slot);
            }
        }
        // Glyphs this frame used stay; above the target, whole pages of
        // others go, sparsest first.
        while self.total_bytes_without_retired() > self.limits.target_bytes {
            if !self.release_unpinned_page() {
                break;
            }
        }
        self.peak_bytes = self.peak_bytes.max(self.total_bytes());
        debug_assert!(
            self.verify_integrity().is_ok(),
            "{:?}",
            self.verify_integrity()
        );
    }

    /// Moves the glyphs of the sparsest shared page into holes of the
    /// others of its kind, then releases it, copying at most
    /// [`EVACUATION_BYTES`]. Moved glyphs get new generations, so only the
    /// runs that draw them prepare again; nothing is rasterized.
    fn evacuate(&mut self) {
        let candidate = self
            .pages
            .iter()
            .enumerate()
            .filter_map(|(slot, page)| Some((slot as u32, page.as_ref()?)))
            .filter(|(_, page)| {
                page.role == Role::Shared
                    && page.live > 0
                    && (page.allocator.allocated_space() as f32)
                        <= SPARSE_PAGE * (page.width * page.height) as f32
            })
            .filter(|(_, page)| self.shared_pages(page.kind) > 1)
            .min_by_key(|(_, page)| page.allocator.allocated_space())
            .map(|(slot, page)| (slot, page.kind));
        let Some((source, kind)) = candidate else {
            return;
        };
        let bpp = u64::from(bytes_per_pixel(kind));
        let moving: Vec<u32> = (0..self.entries.len() as u32)
            .filter(|&i| {
                matches!(self.entries[i as usize].state, State::Resident(p) if p.page == source)
            })
            .collect();
        let bytes: u64 = moving
            .iter()
            .map(|&i| match self.entries[i as usize].state {
                State::Resident(p) => u64::from(p.width + 2 * GUTTER) * u64::from(p.rows()) * bpp,
                _ => 0,
            })
            .sum();
        if bytes > EVACUATION_BYTES {
            return;
        }
        // Destinations first, so a page without room for all leaves
        // everything where it was.
        let mut targets = Vec::with_capacity(moving.len());
        for &index in &moving {
            let State::Resident(p) = self.entries[index as usize].state else {
                continue;
            };
            let (w, h) = (p.width + 2 * GUTTER, p.rows());
            match self.allocate_elsewhere(kind, w, h, source) {
                Some(found) => targets.push(found),
                None => {
                    for (slot, alloc, _, _) in targets {
                        if let Some(page) = self.pages[slot as usize].as_mut() {
                            page.allocator.deallocate(alloc);
                        }
                    }
                    return;
                }
            }
        }
        let from = self.pages[source as usize]
            .as_ref()
            .expect("source page")
            .texture
            .clone();
        for (&index, (slot, alloc, x, y)) in moving.iter().zip(targets) {
            let entry = &mut self.entries[index as usize];
            let State::Resident(p) = entry.state else {
                continue;
            };
            self.moves.push(Move {
                from: from.clone(),
                from_origin: (p.x - GUTTER, p.y - GUTTER),
                to: slot,
                to_origin: (x, y),
                size: (p.width + 2 * GUTTER, p.rows()),
            });
            entry.state = State::Resident(Placed {
                page: slot,
                alloc,
                x: x + GUTTER,
                y: y + GUTTER,
                ..p
            });
            entry.generation = entry.generation.wrapping_add(1);
            if let Some(page) = self.pages[slot as usize].as_mut() {
                page.live += 1;
            }
        }
        if let Some(page) = self.pages[source as usize].as_mut() {
            page.live = 0;
        }
        self.release_page(source);
        self.stats.evacuated_pages += 1;
        self.stats.evacuated_bytes += bytes;
    }
}

/// A broken invariant of a [`GlyphAtlas`].
#[derive(Debug)]
#[allow(dead_code)]
pub(crate) struct IntegrityError(String);

impl GlyphAtlas {
    /// Checks the entry slab, the LRU lists, the key map, and the page
    /// counts against each other.
    pub(crate) fn verify_integrity(&self) -> Result<(), IntegrityError> {
        let fail = |what: String| Err(IntegrityError(what));
        for list in 0..3u8 {
            let (mut at, mut prev, mut len) = (self.heads[list as usize], NIL, 0);
            while at != NIL {
                let entry = &self.entries[at as usize];
                if entry.list != list || entry.prev != prev {
                    return fail(format!("list {list} broken at entry {at}"));
                }
                let expected = match entry.state {
                    State::Resident(p) => list_of(p.kind),
                    State::Free => NO_LIST,
                    _ => META_LIST,
                };
                if expected != list {
                    return fail(format!("entry {at} in list {list}, belongs in {expected}"));
                }
                (prev, at, len) = (at, entry.next, len + 1);
            }
            if self.tails[list as usize] != prev || self.lens[list as usize] != len {
                return fail(format!("list {list} tail or length wrong"));
            }
        }
        let mut live = vec![0u32; self.pages.len()];
        let mut used = 0;
        for (index, entry) in self.entries.iter().enumerate() {
            if matches!(entry.state, State::Free) {
                if entry.list != NO_LIST {
                    return fail(format!("free entry {index} is listed"));
                }
                continue;
            }
            used += 1;
            if self.map.get(&entry.key) != Some(&(index as u32)) {
                return fail(format!("entry {index} missing from the key map"));
            }
            if let State::Resident(p) = entry.state {
                match self.pages.get(p.page as usize).and_then(Option::as_ref) {
                    Some(page) if page.kind == p.kind => live[p.page as usize] += 1,
                    _ => return fail(format!("entry {index} on a missing page")),
                }
            }
        }
        if used != self.map.len() || used + self.free_entries.len() != self.entries.len() {
            return fail("entry counts disagree".into());
        }
        for (slot, page) in self.pages.iter().enumerate() {
            if let Some(page) = page
                && page.role != Role::Overflow
                && page.live != live[slot]
            {
                return fail(format!(
                    "page {slot} counts {} live, holds {}",
                    page.live, live[slot]
                ));
            }
        }
        Ok(())
    }

    /// Evicts every glyph the current frame has not used.
    #[cfg(test)]
    pub(crate) fn evict_unused(&mut self) {
        for index in 0..self.entries.len() as u32 {
            let entry = &self.entries[index as usize];
            if !matches!(entry.state, State::Free) && entry.used != self.frame {
                self.remove(index);
            }
        }
        debug_assert!(
            self.verify_integrity().is_ok(),
            "{:?}",
            self.verify_integrity()
        );
    }

    /// Evacuates a sparse page at the start of the next frame, as a
    /// fragmented miss would.
    #[cfg(test)]
    pub(crate) fn request_evacuation(&mut self) {
        self.fragmented = true;
    }

    /// Room in a shared page of `kind` other than `source`, without new
    /// pages or evictions.
    fn allocate_elsewhere(
        &mut self,
        kind: ContentType,
        w: u32,
        h: u32,
        source: u32,
    ) -> Option<(u32, AllocId, u32, u32)> {
        for slot in (0..self.pages.len()).rev() {
            if slot as u32 == source {
                continue;
            }
            let Some(page) = self.pages[slot].as_mut() else {
                continue;
            };
            if page.role != Role::Shared || page.kind != kind {
                continue;
            }
            if let Some(allocation) = page.allocator.allocate(size2(w as i32, h as i32)) {
                let min = allocation.rectangle.min;
                return Some((slot as u32, allocation.id, min.x as u32, min.y as u32));
            }
        }
        None
    }
}

enum Admitted {
    Entry(u32),
    /// Placed in an overflow page for this ordinal only.
    Transient(Placed),
    Dropped,
}

/// Straight color from premultiplied, in place: RGB divided by alpha,
/// zero where alpha is.
fn unpremultiply(rgba: &mut [u8]) {
    for pixel in rgba.as_chunks_mut::<4>().0 {
        let a = u32::from(pixel[3]);
        for channel in &mut pixel[..3] {
            *channel = (u32::from(*channel) * 255 + a / 2)
                .checked_div(a)
                .map_or(0, |v| v.min(255) as u8);
        }
    }
}
