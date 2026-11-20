//! The glyph atlas: rasterized glyphs resident in fixed-size texture pages.
//!
//! Masks (one byte of coverage) and color glyphs (sRGB RGBA, straight
//! alpha) live in separate pools of lazily created square pages, each its
//! own texture with a stable slot and a unique id, so a new page never
//! moves a cached glyph. Every bitmap keeps a transparent pixel of gutter
//! on each side. A glyph too large for a page gets a dedicated texture of
//! its own size, charged to the same budget ([`pages`]).
//!
//! Entries are keyed by what decides their pixels ([`GlyphKey`]: the
//! rasterizer, the exact font instance, and the request), so a font
//! database replacement or a paint change (color, backdrop, linear
//! correction) reuses them. A front map from cosmic-text cache keys skips
//! resolving the font instance on repeat glyphs of one font epoch.
//!
//! Residency: a glyph used this frame is pinned (its `used` stamp is the
//! current frame) and moves to the front of its pool's LRU list
//! ([`entries`]). Misses take space from existing pages, then a new page
//! while under the soft target, then the least recently used unpinned
//! glyphs of their kind, then the headroom up to the hard limit (less one
//! overflow page per kind, kept in reserve) ([`evict`]). Evicting a glyph
//! increments only its own generation: [`GlyphHandle`]s held by retained
//! draw runs go stale one by one, and runs check theirs before drawing
//! again.
//!
//! When even that cannot hold the frame's glyphs, the frame draws in
//! overflow mode ([`overflow`]): resident glyphs still draw from their
//! pages, and every other glyph goes into a reusable overflow page per
//! kind, filled and drawn one ordinal at a time; the renderer splits its
//! passes so each ordinal's uploads land after the previous ordinal's
//! draws.
//!
//! Uploads pack into CPU shelves whose rows match wgpu's copy pitch, one
//! write per kind into a reused transfer buffer, then one buffer-to-texture
//! copy per glyph in the frame's own command encoder before any text draws
//! ([`upload`]).

mod entries;
mod evict;
mod integrity;
mod overflow;
mod pages;
mod rasterize;
mod stats;
mod upload;

use std::collections::{HashMap, HashSet};
use std::hash::BuildHasherDefault;

use etagere::AllocId;
use quark_text::cosmic_text::{CacheKey, CacheKeyFlags, FontSystem, SwashCache};
use quark_text::fonts::{FontRegistry, PreparedFont};
use quark_text::{FontEpoch, TextSystem};
use rustc_hash::FxHasher;
use wgpu::{
    BindGroup, Buffer, Device, Extent3d, Queue, Texture, TextureDescriptor, TextureDimension,
    TextureFormat, TextureUsages, TextureView, TextureViewDescriptor,
};

use self::entries::{Entry, Lru, LruList, State};
use self::overflow::Overflow;
use self::pages::{Page, texture_format};
use self::rasterize::Raster;
use self::upload::{Move, PendingCopy, Shelf};
use super::backend::{Native, TextRasterizer};
use super::raster::swash::SwashRasterizer;
use super::raster::{
    FontInstanceId, GlyphRasterizer, MAX_BITMAP_BYTES, RasterError, RasterKey, RasterRequest,
    RasterScratch,
};
use super::{Cache, ContentType, PrepareError};

pub use self::stats::{AtlasMemory, AtlasStats};

type Fx = BuildHasherDefault<FxHasher>;

/// Side of an atlas page, before clamping to the device's limit.
const DEFAULT_PAGE_SIZE: u32 = 1024;
/// Transparent pixels kept around every bitmap.
const GUTTER: u32 = 1;
/// Frames before a glyph whose platform rasterizer failed is tried again.
const TRANSIENT_RETRY_FRAMES: u64 = 30;
/// The front map is cleared past this many keys, since entries of evicted
/// glyphs only go stale in it.
const MAX_FRONT_KEYS: usize = 1 << 16;
/// Plane steps the vertex format can carry (14 bits).
const MAX_PLANE_STEP: u32 = 1 << 14;

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
    /// Rows of its rectangle in the page: every plane, each in a gutter.
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

/// A glyph ready to draw.
#[derive(Debug, Clone, Copy)]
pub(crate) struct AtlasGlyph {
    /// `None` for a glyph in an overflow page, which no run may keep.
    pub handle: Option<GlyphHandle>,
    pub placed: Placed,
    /// The overflow ordinal whose uploads it needs (zero outside overflow).
    pub ordinal: u32,
}

/// A rectangle allocated in a page, gutter included.
#[derive(Debug, Clone, Copy)]
struct Allocation {
    page: u32,
    id: AllocId,
    x: u32,
    y: u32,
}

/// The index of `kind` in arrays kept per kind: masks first.
fn kind_slot(kind: ContentType) -> usize {
    match kind {
        ContentType::Mask => 0,
        ContentType::Color => 1,
    }
}

/// The text system's fonts, as the last frame named them.
struct Fonts {
    epoch: FontEpoch,
    registry: FontRegistry,
}

pub(crate) struct GlyphAtlas {
    device: Device,
    queue: Queue,
    pub(crate) cache: Cache,
    pub(crate) format: TextureFormat,
    limits: AtlasLimits,
    page_size: u32,
    max_dimension: u32,
    /// Pages by slot; a released page's slot is reused.
    pages: Vec<Option<Page>>,
    free_pages: Vec<u32>,
    next_uid: u32,
    /// Entries by index; a removed entry's index is reused.
    entries: Vec<Entry>,
    free_entries: Vec<u32>,
    lists: [LruList; Lru::COUNT],
    map: HashMap<GlyphKey, u32, Fx>,
    front: HashMap<(CacheKey, u32), GlyphHandle, Fx>,
    /// `None` until the first frame begins.
    fonts: Option<Fonts>,
    frame: u64,
    swash: SwashRasterizer,
    /// The native rasterizer drawing glyphs before swash, if any, and the
    /// font instances it cannot draw (swash draws them).
    native: Option<Native>,
    native_unsupported: HashSet<FontInstanceId, Fx>,
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
    /// A miss found a page with room in all but its shape: evacuate a
    /// sparse page when the next frame begins.
    fragmented: bool,
    peak_bytes: u64,
    stats: AtlasStats,
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
            lists: [LruList::EMPTY; Lru::COUNT],
            map: HashMap::default(),
            front: HashMap::default(),
            fonts: None,
            frame: 1,
            swash: SwashRasterizer::default(),
            native: None,
            native_unsupported: HashSet::default(),
            legacy: SwashCache::new(),
            scratch: RasterScratch::default(),
            shelves: Shelf::pair(page_size),
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
            self.shelves = Shelf::pair(page_size);
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

    /// Starts preparing a frame drawn with `text`: unpins last frame's
    /// glyphs, follows the text system's fonts, and moves the glyphs of
    /// one sparse page when misses found pages fragmented.
    pub(crate) fn begin_frame(&mut self, text: &mut TextSystem) {
        self.frame += 1;
        self.retired_bytes = 0;
        let epoch = text.font_epoch();
        match &mut self.fonts {
            Some(fonts) if fonts.epoch == epoch => {}
            Some(fonts) => {
                fonts.epoch = epoch;
                fonts.registry.set_snapshot(text.font_snapshot());
                self.front.clear();
            }
            None => {
                self.fonts = Some(Fonts {
                    epoch,
                    registry: FontRegistry::new(text.font_snapshot()),
                });
                self.front.clear();
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
        self.overflow.forget_pages();
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
                        Admitted::Transient { placed, ordinal } => {
                            return Ok(Some(AtlasGlyph {
                                handle: None,
                                placed,
                                ordinal,
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
        let fonts = self.fonts.as_mut().expect("begin_frame names the fonts");
        if let Ok(font) = fonts
            .registry
            .prepare(key.font_id, key.font_weight, key.flags)
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
        let fonts = fonts.epoch;
        (GlyphKey::System { fonts, key }, None)
    }

    /// Rasterize the glyph `glyph_key` names and place it: resident when
    /// the atlas has room, or in this ordinal's overflow pages.
    fn admit(
        &mut self,
        font_system: &mut FontSystem,
        glyph_key: GlyphKey,
        key: CacheKey,
        font: Option<&PreparedFont>,
    ) -> Result<Admitted, PrepareError> {
        if let Some((placed, ordinal)) = self.overflow.placed(&glyph_key) {
            return Ok(Admitted::Transient { placed, ordinal });
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
        let bpp = pages::bytes_per_pixel(drawn.kind);
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
        // Overflowing, glyphs still become resident while the atlas has
        // room; past that, only the overflow pages take them.
        let resident = if self.overflow.kind_full(drawn.kind) {
            None
        } else {
            match self.allocate(drawn.kind, w, h) {
                Ok(found) => Some(found),
                Err(PrepareError::AtlasFull) if self.overflow.is_active() => {
                    self.overflow.mark_full(drawn.kind);
                    None
                }
                Err(error) => return Err(error),
            }
        };
        let Some(allocation) = resident else {
            let Some((allocation, ordinal)) = self.overflow_allocate(drawn.kind, w, h) else {
                self.stats.dropped_glyphs += 1;
                return Ok(Admitted::Dropped);
            };
            let placed = Self::placed(&drawn, allocation);
            if !self.stage(&drawn, &placed, ordinal, false) {
                self.stats.dropped_glyphs += 1;
                return Ok(Admitted::Dropped);
            }
            self.stats.misses += 1;
            self.overflow.remember(glyph_key, placed);
            return Ok(Admitted::Transient { placed, ordinal });
        };
        let placed = Self::placed(&drawn, allocation);
        self.stage(&drawn, &placed, 0, true);
        self.stats.misses += 1;
        if let Some(page) = self.pages[allocation.page as usize].as_mut() {
            page.live += 1;
        }
        Ok(Admitted::Entry(
            self.insert(glyph_key, State::Resident(placed)),
        ))
    }

    /// Where `drawn` lands in `allocation`.
    fn placed(drawn: &rasterize::Drawn, allocation: Allocation) -> Placed {
        Placed {
            page: allocation.page,
            alloc: allocation.id,
            x: allocation.x + GUTTER,
            y: allocation.y + GUTTER,
            width: drawn.width,
            height: drawn.height,
            left: drawn.left,
            top: drawn.top,
            kind: drawn.kind,
            planes: drawn.planes as u8,
        }
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
        self.end_overflow();
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
}

/// How a missed glyph was admitted.
enum Admitted {
    Entry(u32),
    /// Placed in an overflow page for this ordinal only.
    Transient {
        placed: Placed,
        ordinal: u32,
    },
    Dropped,
}
