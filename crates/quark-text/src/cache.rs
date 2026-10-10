use std::collections::{HashMap, VecDeque};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::mem;
use std::sync::Arc;

use quark::scene::FontStyle;
use quark::{FontKind, FontWeight};

use crate::epoch::FontEpoch;
use crate::layout::{TextError, TextLayout, TextParams, TextQuery};
use crate::system::TextSystem;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LayoutKey {
    pub content: u64,
    pub attrs: u64,
    pub size_bits: u32,
    pub wrap_bits: Option<u32>,
    pub scale_bits: u32,
}

impl LayoutKey {
    pub fn new(params: &TextParams) -> Self {
        Self::with_content(&params.query(), hash_text(&params.text))
    }

    /// The key for `params` whose text hashes to `content`.
    fn with_content(params: &TextQuery, content: u64) -> Self {
        let style = &params.style;
        let mut attrs = DefaultHasher::new();
        attrs.write_u8(kind_tag(style.font_kind));
        attrs.write_u16(weight_tag(Some(style.font_weight)));
        attrs.write_u32(style.line_height.to_bits());
        style.family.hash(&mut attrs);
        attrs.write_u32(style.letter_spacing.to_bits());
        attrs.write_u8(u8::from(style.thicken));
        style.linear_correction.hash(&mut attrs);
        attrs.write_usize(params.spans.len());
        for span in params.spans.iter() {
            attrs.write_usize(span.range.start);
            attrs.write_usize(span.range.end);
            attrs.write_u16(weight_tag(span.weight));
            attrs.write_u8(style_tag(span.style));
            attrs.write_u8(span.kind.map_or(255, kind_tag));
            attrs.write_u32(span.size.map_or(u32::MAX, f32::to_bits));
            attrs.write_u32(span.letter_spacing.map_or(u32::MAX, f32::to_bits));
            attrs.write_u8(u8::from(span.keep_together));
        }

        Self {
            content,
            attrs: attrs.finish(),
            size_bits: style.font_size.to_bits(),
            wrap_bits: params.wrap_width.map(f32::to_bits),
            scale_bits: params.scale_factor.to_bits(),
        }
    }
}

fn hash_text(text: &str) -> u64 {
    let mut content = DefaultHasher::new();
    text.hash(&mut content);
    content.finish()
}

/// Identifies one string allocation by address and length.
fn text_id(text: &str) -> (usize, usize) {
    (text.as_ptr() as usize, text.len())
}

/// A hash map that grows only when its length passes half its capacity,
/// so it never grows during churn. Removals leave tombstones wherever the
/// keys' hashes put them, and a plain `HashMap` that runs out of empty
/// buckets while more than half full grows rather than clearing them. That
/// made whether a miss allocated depend on the process's hash seed (and,
/// for `content_hashes`, on heap addresses). At most half full, running out
/// rehashes in place, which allocates nothing, so allocations follow the
/// length alone.
#[derive(Debug)]
struct HalfLoadMap<K, V> {
    map: HashMap<K, V>,
    /// `map.capacity()` when it was built: tombstones lower `capacity()`.
    built_capacity: usize,
}

impl<K, V> Default for HalfLoadMap<K, V> {
    fn default() -> Self {
        Self {
            map: HashMap::new(),
            built_capacity: 0,
        }
    }
}

impl<K: Hash + Eq, V> HalfLoadMap<K, V> {
    fn get(&self, key: &K) -> Option<&V> {
        self.map.get(key)
    }

    fn get_mut(&mut self, key: &K) -> Option<&mut V> {
        self.map.get_mut(key)
    }

    fn len(&self) -> usize {
        self.map.len()
    }

    fn insert(&mut self, key: K, value: V) {
        if let Some(slot) = self.map.get_mut(&key) {
            // Replacing adds no entry, so it is no reason to grow.
            *slot = value;
            return;
        }
        let len = self.map.len() + 1;
        if len > self.built_capacity / 2 {
            let old = mem::replace(&mut self.map, HashMap::with_capacity(2 * len));
            self.built_capacity = self.map.capacity();
            self.map.extend(old);
        }
        self.map.insert(key, value);
    }

    fn remove(&mut self, key: &K) {
        self.map.remove(key);
    }

    fn clear(&mut self) {
        self.map.clear();
    }
}

/// No slot: the end of the recency list.
const NIL: u32 = u32::MAX;

#[derive(Debug)]
struct Slot {
    key: LayoutKey,
    layout: Arc<TextLayout>,
    /// [`TextLayout::storage_bytes`] when admitted; layouts are immutable.
    bytes: usize,
    /// The scope that used the entry last, and that scope's frame then.
    scope: u64,
    last_used: u64,
    /// Neighbors in recency order, toward the most recently used and away
    /// from it.
    newer: u32,
    older: u32,
}

/// Frame-scoped layout cache. Call [`Self::begin_frame`] once per frame and
/// [`Self::trim`] whenever convenient (e.g. end of frame); entries not used in
/// the last `max_idle_frames` frames are evicted.
///
/// Several windows sharing one cache should each call
/// [`Self::begin_frame_for`] with their own scope. Idle time then counts in
/// the frames of the window that last used an entry, so a window animating
/// at full rate does not evict the layouts of a window that is not drawing.
///
/// Memory is bounded in bytes ([`LayoutCacheLimits`]). Past
/// `history_bytes` of cached layouts, a miss evicts the least recently used
/// layouts nothing else holds; past `max_entries` the least recently used
/// are evicted whatever holds them. Evicted layouts nothing else holds are
/// kept, up to `spare_bytes`, and a miss rebuilds one in place, so fresh
/// text reuses their storage instead of allocating its own. A layout still
/// held when evicted is left untouched until its last holder drops it.
/// [`Self::memory`] reports where the bytes are.
#[derive(Debug)]
pub struct LayoutCache {
    /// Slot of each cached layout.
    index: HalfLoadMap<LayoutKey, u32>,
    slots: Vec<Slot>,
    /// The most and least recently used slots.
    newest: u32,
    oldest: u32,
    /// Content hash of every cached layout's text, by its address. The
    /// entries keep those allocations alive and unchanged, so an address
    /// found here holds the same text, and a lookup of that text (such as
    /// [`TextLayout::query`] at another width) skips hashing it.
    content_hashes: HalfLoadMap<(usize, usize), u64>,
    /// Frame count of each scope that has begun a frame.
    scopes: HashMap<u64, u64>,
    scope: u64,
    max_idle_frames: u64,
    max_entries: usize,
    limits: LayoutCacheLimits,
    /// The slots' bytes.
    cached_bytes: usize,
    /// `cached_bytes` past which a miss evicts. Above the limit while
    /// layouts held elsewhere alone exceed it, so that each miss does not
    /// walk them all again.
    evict_at: usize,
    /// The fonts the entries were shaped with. A lookup with another
    /// system, or after its fonts changed, clears the cache first.
    fonts: Option<FontEpoch>,
    stats: LayoutCacheStats,
    /// Evicted layouts, refilled by later misses.
    pool: LayoutPool,
}

/// Lifetime lookup counters. A miss is a lookup that shaped a new layout.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LayoutCacheStats {
    pub hits: u64,
    pub misses: u64,
}

/// How much memory a [`LayoutCache`] keeps, in bytes as
/// [`TextLayout::storage_bytes`] counts them. Configurable starting points,
/// not measured optima.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LayoutCacheLimits {
    /// Cached layouts. Past it, misses evict the least recently used ones
    /// nothing else holds. Layouts held elsewhere stay cached, since
    /// evicting them frees nothing, so they can keep the cache past this.
    pub history_bytes: usize,
    /// Evicted layouts kept for misses to refill.
    pub spare_bytes: usize,
}

impl Default for LayoutCacheLimits {
    fn default() -> Self {
        Self {
            history_bytes: 8 << 20,
            spare_bytes: 8 << 20,
        }
    }
}

/// Where a [`LayoutCache`]'s memory is, in bytes as
/// [`TextLayout::storage_bytes`] counts them; see [`LayoutCache::memory`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LayoutCacheMemory {
    /// Cached layouts only the cache holds.
    pub resident_bytes: usize,
    /// Cached layouts something else holds too (a scene, a recording, an
    /// element's state). Their holders keep them alive whatever the cache
    /// does, so no limit bounds them.
    pub pinned_bytes: usize,
    /// Evicted layouts nothing else holds, kept for misses to refill.
    pub free_bytes: usize,
    /// Layouts evicted while held elsewhere, kept unchanged until released
    /// and then refilled.
    pub retired_bytes: usize,
}

/// Default [`LayoutCache`] entry cap.
const DEFAULT_MAX_ENTRIES: usize = 8192;

impl LayoutCache {
    pub fn new(max_idle_frames: u64) -> Self {
        let limits = LayoutCacheLimits::default();
        Self {
            index: HalfLoadMap::default(),
            slots: Vec::new(),
            newest: NIL,
            oldest: NIL,
            content_hashes: HalfLoadMap::default(),
            scopes: HashMap::new(),
            scope: 0,
            max_idle_frames,
            max_entries: DEFAULT_MAX_ENTRIES,
            limits,
            cached_bytes: 0,
            evict_at: limits.history_bytes,
            fonts: None,
            stats: LayoutCacheStats::default(),
            pool: LayoutPool::new(limits.spare_bytes),
        }
    }

    /// Caps the number of entries (at least one).
    pub fn with_max_entries(mut self, max_entries: usize) -> Self {
        self.max_entries = max_entries.max(1);
        self
    }

    /// Sets the memory limits. Layouts already kept past them go now, as
    /// a miss would evict them, so a cache whose limits were lowered stays
    /// within them.
    pub fn with_limits(mut self, limits: LayoutCacheLimits) -> Self {
        self.limits = limits;
        self.evict_at = limits.history_bytes;
        self.pool.set_max_free_bytes(limits.spare_bytes);
        self.evict_past_limits();
        debug_assert_eq!(self.verify_integrity(), Ok(()));
        self
    }

    pub fn limits(&self) -> LayoutCacheLimits {
        self.limits
    }

    /// Begins a frame of the default scope; see [`Self::begin_frame_for`].
    pub fn begin_frame(&mut self) {
        self.begin_frame_for(0);
    }

    /// Begins a frame of `scope` (one window). Lookups until the next call
    /// count as that scope's uses.
    pub fn begin_frame_for(&mut self, scope: u64) {
        self.scope = scope;
        let frame = self.scopes.entry(scope).or_insert(0);
        *frame = frame.wrapping_add(1);
    }

    /// Forgets `scope` (a closed window). Entries it used last are evicted at
    /// the next [`Self::trim`].
    pub fn remove_scope(&mut self, scope: u64) {
        self.scopes.remove(&scope);
    }

    /// The current scope's frame count.
    pub fn frame(&self) -> u64 {
        self.scopes.get(&self.scope).copied().unwrap_or(0)
    }

    /// Returns the number of evicted entries.
    pub fn trim(&mut self) -> usize {
        let mut evicted = 0;
        // Removing a slot moves the last one into its place, which this
        // walk has already visited.
        for i in (0..self.slots.len()).rev() {
            let slot = &self.slots[i];
            let live = self
                .scopes
                .get(&slot.scope)
                .is_some_and(|frame| frame.wrapping_sub(slot.last_used) <= self.max_idle_frames);
            if !live {
                self.evict_slot(i as u32);
                evicted += 1;
            }
        }
        self.evict_at = self.limits.history_bytes;
        evicted
    }

    pub fn clear(&mut self) {
        for slot in self.slots.drain(..) {
            self.pool.recycle(slot.layout, slot.bytes);
        }
        self.index.clear();
        self.content_hashes.clear();
        self.newest = NIL;
        self.oldest = NIL;
        self.cached_bytes = 0;
        self.evict_at = self.limits.history_bytes;
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    pub fn stats(&self) -> LayoutCacheStats {
        self.stats
    }

    /// Where the cache's memory is. Walks every entry, so it is for
    /// diagnostics rather than every frame. Whether something else holds a
    /// layout can change at any moment on other threads, so the split
    /// between resident and pinned is a snapshot.
    pub fn memory(&self) -> LayoutCacheMemory {
        let mut memory = LayoutCacheMemory {
            free_bytes: self.pool.free_bytes,
            retired_bytes: self.pool.retired.iter().map(|spare| spare.bytes).sum(),
            ..LayoutCacheMemory::default()
        };
        for slot in &self.slots {
            let held = Arc::strong_count(&slot.layout) > 1 || Arc::weak_count(&slot.layout) > 0;
            if held {
                memory.pinned_bytes += slot.bytes;
            } else {
                memory.resident_bytes += slot.bytes;
            }
        }
        memory
    }

    pub fn layout(
        &mut self,
        system: &mut TextSystem,
        params: &TextParams,
    ) -> Result<Arc<TextLayout>, TextError> {
        self.lookup(system, &params.query())
    }

    /// [`Self::layout`] for borrowed params: a hit allocates nothing, a
    /// miss copies the text and spans into the layout.
    pub fn layout_query(
        &mut self,
        system: &mut TextSystem,
        query: &TextQuery,
    ) -> Result<Arc<TextLayout>, TextError> {
        self.lookup(system, query)
    }

    /// Looks `params` up, laying them out on a miss. A miss copies the text
    /// and spans into the layout's source, over a pooled one's storage when
    /// it can.
    fn lookup(
        &mut self,
        system: &mut TextSystem,
        params: &TextQuery,
    ) -> Result<Arc<TextLayout>, TextError> {
        let fonts = system.font_epoch();
        if self.fonts != Some(fonts) {
            self.clear();
            self.fonts = Some(fonts);
        }
        let content = match self.content_hashes.get(&text_id(params.text)) {
            Some(&content) => content,
            None => hash_text(params.text),
        };
        let key = LayoutKey::with_content(params, content);
        let (scope, frame) = (self.scope, self.frame());
        if let Some(&i) = self.index.get(&key) {
            // Guard against 64-bit hash collisions before trusting the hit.
            let slot = &mut self.slots[i as usize];
            if same_inputs(&slot.layout, params) {
                slot.scope = scope;
                slot.last_used = frame;
                let layout = slot.layout.clone();
                self.make_newest(i);
                self.stats.hits += 1;
                return Ok(layout);
            }
            // A colliding entry gives way.
            self.evict_slot(i);
        }
        self.stats.misses += 1;
        params.validate()?;
        let pooled = self.pool.take(params.text);
        let layout = TextLayout::refill(pooled, |own| {
            own.copy_inputs(params);
            system.rebuild(own);
        });
        let bytes = layout.storage_bytes();
        self.insert_slot(Slot {
            key,
            layout: layout.clone(),
            bytes,
            scope,
            last_used: frame,
            newer: NIL,
            older: NIL,
        });
        self.evict_past_limits();
        debug_assert_eq!(self.verify_integrity(), Ok(()));
        Ok(layout)
    }

    /// Evicts past the entry cap, least recently used first, down to three
    /// quarters of it; then past the byte limit, least recently used first
    /// among layouts nothing else holds, down to three quarters of it. The
    /// quarter of headroom keeps a working set at a limit from evicting on
    /// every miss.
    fn evict_past_limits(&mut self) {
        if self.slots.len() > self.max_entries {
            let keep = (self.max_entries / 4 * 3).max(1);
            while self.slots.len() > keep {
                self.evict_slot(self.oldest);
            }
        }
        if self.cached_bytes <= self.evict_at {
            return;
        }
        let target = self.limits.history_bytes / 4 * 3;
        // A held layout moves to the newest end, so each slot is visited
        // at most once.
        for _ in 0..self.slots.len() {
            if self.cached_bytes <= target {
                break;
            }
            let i = self.oldest;
            // Only `get_mut` proves nothing else holds it; see `LayoutPool`.
            if Arc::get_mut(&mut self.slots[i as usize].layout).is_some() {
                self.evict_slot(i);
            } else {
                // Its holder keeps it alive anyway, and it is likely on
                // screen (a replayed recording looks nothing up).
                self.make_newest(i);
            }
        }
        self.evict_at =
            (self.cached_bytes + self.limits.history_bytes / 4).max(self.limits.history_bytes);
    }

    fn insert_slot(&mut self, mut slot: Slot) {
        let i = self.slots.len() as u32;
        slot.newer = NIL;
        slot.older = self.newest;
        match self.slots.get_mut(self.newest as usize) {
            Some(newest) => newest.newer = i,
            None => self.oldest = i,
        }
        self.newest = i;
        self.index.insert(slot.key, i);
        self.content_hashes
            .insert(text_id(slot.layout.text()), slot.key.content);
        self.cached_bytes += slot.bytes;
        self.slots.push(slot);
    }

    /// Removes slot `i` into the pool, moving the last slot into its place.
    fn evict_slot(&mut self, i: u32) {
        self.unlink(i);
        let slot = self.slots.swap_remove(i as usize);
        self.index.remove(&slot.key);
        self.content_hashes.remove(&text_id(slot.layout.text()));
        self.cached_bytes -= slot.bytes;
        let moved = self.slots.len() as u32;
        if i != moved {
            // The slot that was last is now `i`: repoint its neighbors and
            // its key.
            let (newer, older, key) = {
                let slot = &self.slots[i as usize];
                (slot.newer, slot.older, slot.key)
            };
            match self.slots.get_mut(newer as usize) {
                Some(slot) => slot.older = i,
                None => self.newest = i,
            }
            match self.slots.get_mut(older as usize) {
                Some(slot) => slot.newer = i,
                None => self.oldest = i,
            }
            if let Some(at) = self.index.get_mut(&key) {
                *at = i;
            }
        }
        self.pool.recycle(slot.layout, slot.bytes);
    }

    /// Takes slot `i` out of the recency list.
    fn unlink(&mut self, i: u32) {
        let (newer, older) = {
            let slot = &self.slots[i as usize];
            (slot.newer, slot.older)
        };
        match self.slots.get_mut(newer as usize) {
            Some(slot) => slot.older = older,
            None => self.newest = older,
        }
        match self.slots.get_mut(older as usize) {
            Some(slot) => slot.newer = newer,
            None => self.oldest = newer,
        }
    }

    fn make_newest(&mut self, i: u32) {
        if self.newest == i {
            return;
        }
        self.unlink(i);
        let slot = &mut self.slots[i as usize];
        slot.newer = NIL;
        slot.older = self.newest;
        self.slots[self.newest as usize].newer = i;
        self.newest = i;
    }

    /// Checks that the recency list orders every slot once, the key index
    /// and byte total match the slots, and the pool is sound.
    fn verify_integrity(&self) -> Result<(), CacheError> {
        let mut seen = 0;
        let (mut at, mut newer) = (self.newest, NIL);
        while at != NIL {
            let slot = self.slots.get(at as usize).ok_or(CacheError::Recency)?;
            if slot.newer != newer || seen >= self.slots.len() {
                return Err(CacheError::Recency);
            }
            seen += 1;
            (newer, at) = (at, slot.older);
        }
        if seen != self.slots.len() || self.oldest != newer {
            return Err(CacheError::Recency);
        }
        let indexed = self.index.len() == self.slots.len()
            && (self.slots.iter().enumerate())
                .all(|(i, slot)| self.index.get(&slot.key) == Some(&(i as u32)));
        if !indexed {
            return Err(CacheError::Index);
        }
        let bytes: usize = self.slots.iter().map(|slot| slot.bytes).sum();
        if bytes != self.cached_bytes {
            return Err(CacheError::Bytes {
                counted: self.cached_bytes,
                actual: bytes,
            });
        }
        self.pool.verify_integrity().map_err(CacheError::Pool)
    }
}

/// A [`LayoutCache`] invariant broken.
#[derive(Debug, PartialEq)]
enum CacheError {
    Recency,
    Index,
    Bytes { counted: usize, actual: usize },
    Pool(PoolError),
}

/// Layouts the cache evicted, kept so a miss refills one instead of
/// allocating its storage and `Arc`. `free` layouts are held by nothing
/// else. `retired` ones were still held elsewhere (by a scene, say) when
/// evicted; they stay unchanged for those holders and become free once the
/// last holder drops them.
///
/// Only [`Arc::get_mut`] decides that a layout is unshared. Reading the
/// strong and weak counts one after the other is not enough: another thread
/// can upgrade a `Weak` between the two reads and then drop the `Weak`, so
/// the counts read 1 and 0 while that thread holds the layout. `get_mut`
/// locks out new `Weak`s while it checks, and once a layout is free nothing
/// but the pool can reach it.
#[derive(Debug)]
struct LayoutPool {
    free: Vec<Spare>,
    /// Oldest first.
    retired: VecDeque<Spare>,
    /// The `free` layouts' bytes.
    free_bytes: usize,
    max_free_bytes: usize,
}

/// A pooled layout and its [`TextLayout::storage_bytes`].
#[derive(Debug)]
struct Spare {
    layout: Arc<TextLayout>,
    bytes: usize,
}

/// Most layouts `free` and `retired` each keep.
const POOL_CAP: usize = 256;

/// A [`LayoutPool`] invariant broken.
#[derive(Debug, PartialEq)]
enum PoolError {
    SharedFree { index: usize },
    Overfull { free: usize, retired: usize },
    FreeBytes { counted: usize, actual: usize },
}

impl LayoutPool {
    fn new(max_free_bytes: usize) -> Self {
        Self {
            free: Vec::new(),
            retired: VecDeque::new(),
            free_bytes: 0,
            max_free_bytes,
        }
    }

    fn recycle(&mut self, mut layout: Arc<TextLayout>, bytes: usize) {
        if Arc::get_mut(&mut layout).is_some() {
            self.free(Spare { layout, bytes });
        } else {
            // A holder that never lets go must not block later ones, so
            // the oldest retiree gives way.
            if self.retired.len() == POOL_CAP {
                self.retired.pop_front();
            }
            self.retired.push_back(Spare { layout, bytes });
        }
        debug_assert_eq!(self.verify_integrity(), Ok(()));
    }

    /// Lowers or raises the free bytes kept, dropping free layouts, the
    /// most recently freed first, past a lower limit.
    fn set_max_free_bytes(&mut self, max_free_bytes: usize) {
        self.max_free_bytes = max_free_bytes;
        while self.free_bytes > max_free_bytes {
            let Some(spare) = self.free.pop() else {
                break;
            };
            self.free_bytes -= spare.bytes;
        }
    }

    /// Keeps unshared `spare` if it fits the caps, and drops it otherwise.
    fn free(&mut self, spare: Spare) {
        let fits = self.free_bytes + spare.bytes <= self.max_free_bytes;
        if self.free.len() < POOL_CAP && fits {
            self.free_bytes += spare.bytes;
            self.free.push(spare);
        }
    }

    /// An unshared layout to rebuild for `text`: preferably one with room
    /// for its glyphs (one per char, roughly), then for its text, the least
    /// room among those.
    fn take(&mut self, text: &str) -> Option<Arc<TextLayout>> {
        // One turn of the queue: freed retirees leave it, the rest go
        // back in their order.
        for _ in 0..self.retired.len() {
            let Some(mut spare) = self.retired.pop_front() else {
                break;
            };
            if Arc::get_mut(&mut spare.layout).is_some() {
                self.free(spare);
            } else {
                self.retired.push_back(spare);
            }
        }
        let chars = text.chars().count();
        let i = (0..self.free.len()).max_by_key(|&i| {
            let layout = &self.free[i].layout;
            let room = layout.glyph_capacity();
            let fits = room >= chars;
            let snug = if fits { usize::MAX - room } else { room };
            (fits, layout.source().capacity() >= text.len(), snug)
        })?;
        let spare = self.free.swap_remove(i);
        self.free_bytes -= spare.bytes;
        debug_assert_eq!(self.verify_integrity(), Ok(()));
        Some(spare.layout)
    }

    fn verify_integrity(&self) -> Result<(), PoolError> {
        if self.free.len() > POOL_CAP || self.retired.len() > POOL_CAP {
            return Err(PoolError::Overfull {
                free: self.free.len(),
                retired: self.retired.len(),
            });
        }
        // Free layouts stay unshared once `get_mut` found them so, which
        // makes reading the counts here exact.
        if let Some(index) = self.free.iter().position(|spare| !unshared(&spare.layout)) {
            return Err(PoolError::SharedFree { index });
        }
        let actual = self.free.iter().map(|spare| spare.bytes).sum();
        if self.free_bytes != actual || actual > self.max_free_bytes {
            return Err(PoolError::FreeBytes {
                counted: self.free_bytes,
                actual,
            });
        }
        Ok(())
    }
}

/// Whether `layout` is the only reference. Only exact for a layout no other
/// thread can reach; see [`LayoutPool`].
fn unshared(layout: &Arc<TextLayout>) -> bool {
    Arc::strong_count(layout) == 1 && Arc::weak_count(layout) == 0
}

impl Default for LayoutCache {
    fn default() -> Self {
        Self::new(240)
    }
}

fn same_inputs(layout: &TextLayout, params: &TextQuery) -> bool {
    let text_eq = std::ptr::eq(layout.text(), params.text) || layout.text() == params.text;
    let spans_eq = std::ptr::eq(layout.spans(), params.spans) || layout.spans() == params.spans;
    text_eq && spans_eq && layout.style() == params.style
}

fn kind_tag(kind: FontKind) -> u8 {
    match kind {
        FontKind::Ui => 0,
        FontKind::Mono => 1,
    }
}

/// The CSS weight, or 0 (no weight is) for none: any weight a layout can
/// ask for keys apart from the others.
fn weight_tag(weight: Option<FontWeight>) -> u16 {
    weight.map_or(0, crate::layout::font_weight_value)
}

fn style_tag(style: Option<FontStyle>) -> u8 {
    match style {
        None => 255,
        Some(FontStyle::Normal) => 0,
        Some(FontStyle::Italic) => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fonts::FontSettings;
    use crate::layout::TextStyle;
    use crate::system::test_system;

    const TEXT: &str = "The quick brown fox jumps over the lazy dog.";

    fn params(wrap: f32) -> TextParams {
        TextParams::new(TEXT, TextStyle::new(14.0)).wrap_width(Some(wrap))
    }

    #[test]
    fn layout_cache_equal_params_in_new_arc_return_cached_layout() {
        let mut sys = test_system();
        let mut cache = LayoutCache::new(2);
        cache.begin_frame();
        let first = cache.layout(&mut sys, &params(150.0)).expect("layout");
        let again = cache.layout(&mut sys, &params(150.0)).expect("layout");
        assert!(Arc::ptr_eq(&first, &again));
        let wide = cache.layout(&mut sys, &params(300.0)).expect("layout");
        assert!(!Arc::ptr_eq(&first, &wide));
    }

    #[test]
    fn layout_cache_trim_evicts_only_entries_idle_past_limit() {
        let mut sys = test_system();
        let mut cache = LayoutCache::new(2);
        cache.begin_frame();
        let kept = cache.layout(&mut sys, &params(150.0)).expect("layout");
        cache.layout(&mut sys, &params(300.0)).expect("layout");
        for _ in 0..3 {
            cache.begin_frame();
            cache.layout(&mut sys, &params(150.0)).expect("layout");
        }
        assert_eq!(cache.trim(), 1);
        let again = cache.layout(&mut sys, &params(150.0)).expect("layout");
        assert!(Arc::ptr_eq(&kept, &again));
    }

    // Windows share one cache. Counting idle frames across all of them let an
    // animating window evict the layouts of a window that was not drawing.
    #[test]
    fn layout_cache_idle_scope_keeps_layouts_while_another_scope_draws() {
        let mut sys = test_system();
        let mut cache = LayoutCache::new(2);
        cache.begin_frame_for(1);
        let idle = cache.layout(&mut sys, &params(150.0)).expect("layout");
        for _ in 0..10 {
            cache.begin_frame_for(2);
            cache.layout(&mut sys, &params(300.0)).expect("layout");
            cache.trim();
        }
        cache.begin_frame_for(1);
        let again = cache.layout(&mut sys, &params(150.0)).expect("layout");
        assert!(Arc::ptr_eq(&idle, &again));
    }

    // Entries within the idle horizon still must not grow without bound;
    // past the cap the least recently used layouts go first.
    #[test]
    fn layout_cache_over_capacity_evicts_least_recently_used() {
        let mut sys = test_system();
        let mut cache = LayoutCache::new(240).with_max_entries(4);
        cache.begin_frame();
        let recent = cache.layout(&mut sys, &params(100.0)).expect("layout");
        for wrap in [110.0, 120.0, 130.0, 100.0, 140.0] {
            cache.layout(&mut sys, &params(wrap)).expect("layout");
        }
        assert_eq!(cache.len(), 3);
        let again = cache.layout(&mut sys, &params(100.0)).expect("layout");
        assert!(Arc::ptr_eq(&recent, &again));
    }

    // Misses refill evicted layouts, so one still held when evicted (by a
    // scene, say) must not be among them.
    #[test]
    fn layout_cache_eviction_leaves_held_layout_unchanged() {
        let mut sys = test_system();
        let mut cache = LayoutCache::new(0);
        let query = |text| TextQuery::new(text, TextStyle::new(14.0)).wrap_width(Some(150.0));
        cache.begin_frame();
        let held = cache.layout_query(&mut sys, &query(TEXT)).expect("layout");
        let glyphs = format!("{:?}", held.glyphs());
        cache.begin_frame();
        cache.trim();
        // The same length, so it could be copied over the held text.
        let other: String = TEXT.chars().rev().collect();
        cache
            .layout_query(&mut sys, &query(&other))
            .expect("layout");
        assert_eq!(held.text(), TEXT);
        assert_eq!(format!("{:?}", held.glyphs()), glyphs);
    }

    // Misses refill retired layouts once nothing else holds them. A layout
    // only a `Weak` reaches is retired, but whoever upgrades the `Weak`
    // holds it again, so it must stay unchanged however often the cache
    // evicts and refills around it.
    #[test]
    fn layout_cache_reuse_leaves_a_layout_upgraded_from_weak_unchanged() {
        let mut sys = test_system();
        let mut cache = LayoutCache::new(0);
        let query = |text| TextQuery::new(text, TextStyle::new(14.0)).wrap_width(Some(150.0));
        let reversed: String = TEXT.chars().rev().collect();
        for round in 0..4 {
            let (text, other) = if round % 2 == 0 {
                (TEXT, reversed.as_str())
            } else {
                (reversed.as_str(), TEXT)
            };
            cache.begin_frame();
            let weak = Arc::downgrade(&cache.layout_query(&mut sys, &query(text)).expect("layout"));
            cache.begin_frame();
            cache.trim();
            let held = weak.upgrade().expect("retired, not dropped");
            drop(weak);
            let glyphs = format!("{:?}", held.glyphs());
            cache.begin_frame();
            cache.trim();
            // The same length, so it could be copied over the held text.
            cache.layout_query(&mut sys, &query(other)).expect("layout");
            assert_eq!(held.text(), text, "round {round}");
            assert_eq!(format!("{:?}", held.glyphs()), glyphs, "round {round}");
        }
    }

    /// Row `i` of a stream of distinct lines, of varying length.
    fn fresh_row(i: usize) -> String {
        let words = ["amber", "brisk", "cedar", "dune", "ember", "fjord", "gale"];
        let tail: Vec<&str> = (0..i % 9).map(|w| words[(i + w) % words.len()]).collect();
        format!("row {i} {}", tail.join(" "))
    }

    // Idle eviction keeps everything used within the horizon, however much
    // text that is; a stream of fresh rows must stay within the byte limits
    // all the same, the evicted storage beyond the spare limit dropped.
    #[test]
    fn layout_cache_stream_of_fresh_text_stays_within_byte_limits() {
        let mut sys = test_system();
        let limits = LayoutCacheLimits {
            history_bytes: 160 << 10,
            spare_bytes: 48 << 10,
        };
        let mut cache = LayoutCache::new(240).with_limits(limits);
        for i in 0..300 {
            cache.begin_frame();
            let row = fresh_row(i);
            let layout = cache
                .layout_query(&mut sys, &TextQuery::new(&row, TextStyle::new(14.0)))
                .expect("layout");
            assert_eq!(layout.text(), row);
            drop(layout);
            let memory = cache.memory();
            assert!(
                memory.resident_bytes <= limits.history_bytes
                    && memory.free_bytes <= limits.spare_bytes,
                "row {i}: {memory:?}"
            );
        }
        assert_eq!(cache.memory().pinned_bytes, 0);
    }

    // Layouts held elsewhere cost the cache nothing to keep and free
    // nothing when evicted. They must be reported as pinned rather than
    // resident, and stay unchanged while the byte limit evicts around them.
    #[test]
    fn layout_cache_reports_held_layouts_as_pinned_and_leaves_them_unchanged() {
        let mut sys = test_system();
        let mut cache = LayoutCache::new(240).with_limits(LayoutCacheLimits {
            history_bytes: 128 << 10,
            spare_bytes: 48 << 10,
        });
        let query = |text| TextQuery::new(text, TextStyle::new(14.0));
        let rows: Vec<String> = (0..200).map(fresh_row).collect();
        cache.begin_frame();
        let held: Vec<_> = rows[..3]
            .iter()
            .map(|row| cache.layout_query(&mut sys, &query(row)).expect("layout"))
            .collect();
        let glyphs: Vec<String> = held.iter().map(|l| format!("{:?}", l.glyphs())).collect();
        for row in &rows[3..] {
            cache.begin_frame();
            cache.layout_query(&mut sys, &query(row)).expect("layout");
        }
        let held_bytes: usize = held.iter().map(|layout| layout.storage_bytes()).sum();
        let memory = cache.memory();
        assert_eq!(memory.pinned_bytes, held_bytes, "{memory:?}");
        for ((layout, row), glyphs) in held.iter().zip(&rows).zip(&glyphs) {
            assert_eq!(layout.text(), row.as_str());
            assert_eq!(&format!("{:?}", layout.glyphs()), glyphs);
        }
    }

    // Replacing a key's value adds no entry, so it must not grow the map:
    // a map that grew on replacement allocated where its length said it
    // would not.
    #[test]
    fn half_load_map_replacing_a_key_allocates_nothing() {
        let mut map = HalfLoadMap::default();
        map.insert(1u64, 0u64);
        let ((), allocations) = crate::alloc_budget::count(|| {
            for value in 1..8 {
                map.insert(1, value);
            }
        });
        assert_eq!(allocations, 0);
        assert_eq!(map.get(&1), Some(&7));
    }

    // A cache whose limits are lowered once it keeps layouts must come
    // within them at once: storage kept past the new limits outlived them,
    // and the pool's own bookkeeping assumed it never would.
    #[test]
    fn lowered_limits_apply_to_layouts_already_kept() {
        let mut sys = test_system();
        let mut cache = LayoutCache::new(2);
        cache.begin_frame();
        for wrap in [100.0, 150.0, 200.0] {
            cache.layout(&mut sys, &params(wrap)).expect("layout");
        }
        // Two layouts pooled, one cached.
        cache.clear();
        cache.layout(&mut sys, &params(250.0)).expect("layout");
        let none = LayoutCacheLimits {
            history_bytes: 0,
            spare_bytes: 0,
        };

        let mut cache = cache.with_limits(none);
        assert_eq!(cache.memory(), LayoutCacheMemory::default());
        let held = cache.layout(&mut sys, &params(300.0)).expect("layout");
        let memory = cache.memory();
        assert_eq!((memory.free_bytes, memory.resident_bytes), (0, 0));
        drop(held);
    }

    // Every way the fonts change must reach the cache, or it hands out
    // layouts shaped with the old fonts. A system put in another's place
    // starts at the same generation, and a loaded font can be the family
    // the settings already name.
    #[test]
    fn layout_cache_lays_out_again_after_any_font_change() {
        type Change = fn(&mut TextSystem);
        let changes: [(&str, Change); 3] = [
            ("new settings", |sys| {
                sys.set_font_settings(&FontSettings {
                    ui_family: "Inter".into(),
                    ..FontSettings::default()
                });
            }),
            ("another system", |sys| {
                *sys = TextSystem::vendored_only(&FontSettings {
                    ui_family: "Inter".into(),
                    ..FontSettings::default()
                });
            }),
            ("a loaded font", |sys| {
                sys.load_font_data(Arc::new(crate::system::renamed_inter()));
            }),
        ];
        let probe = TextParams::new("iiiiMMMM", TextStyle::new(14.0));
        for (name, change) in changes {
            // Names a family that only the loaded font has, so until then
            // the default one stands in. Its own system: changing fonts on
            // the shared one would race other tests.
            let mut sys = TextSystem::vendored_only(&FontSettings {
                ui_family: crate::system::RENAMED_INTER.into(),
                ..FontSettings::default()
            });
            let mut cache = LayoutCache::new(2);
            let before = cache.layout(&mut sys, &probe).expect("layout").size();
            change(&mut sys);
            let cached = cache.layout(&mut sys, &probe).expect("layout").size();
            let fresh = sys.layout(&probe).expect("layout").size();
            assert_ne!(before, fresh, "{name} changes the font");
            assert_eq!(cached, fresh, "{name}");
        }
    }
}
