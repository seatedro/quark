use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;

use quark::scene::FontStyle;
use quark::{FontKind, FontWeight};

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
        attrs.write_u8(weight_tag(Some(style.font_weight)));
        attrs.write_u32(style.line_height.to_bits());
        attrs.write_usize(params.spans.len());
        for span in params.spans.iter() {
            attrs.write_usize(span.range.start);
            attrs.write_usize(span.range.end);
            attrs.write_u8(weight_tag(span.weight));
            attrs.write_u8(style_tag(span.style));
            attrs.write_u8(span.kind.map_or(255, kind_tag));
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

#[derive(Debug)]
struct Entry {
    layout: Arc<TextLayout>,
    /// The scope that used the entry last, and that scope's frame then.
    scope: u64,
    last_used: u64,
    /// [`LayoutCache::lookups`] at the last use, for evicting past the cap.
    touched: u64,
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
/// Past `max_entries` the least recently used entries are evicted even when
/// they are within the idle horizon.
///
/// Evicted layouts nothing else holds are kept, up to 256 of them and about
/// 8 MiB, and a miss rebuilds one in place, so fresh text reuses their
/// storage instead of allocating its own. A layout still held when evicted
/// is left untouched until its last holder drops it.
#[derive(Debug)]
pub struct LayoutCache {
    entries: HashMap<LayoutKey, Entry>,
    /// Content hash of every cached layout's text, by its `Arc<str>`. The
    /// entries keep those allocations alive, so an address found here holds
    /// the same text, and a lookup with that `Arc` skips hashing the text.
    content_hashes: HashMap<(usize, usize), u64>,
    /// Frame count of each scope that has begun a frame.
    scopes: HashMap<u64, u64>,
    scope: u64,
    /// Lookups so far; stamps entries in recency order.
    lookups: u64,
    max_idle_frames: u64,
    max_entries: usize,
    font_generation: u64,
    stats: LayoutCacheStats,
    /// Evicted layouts, refilled by later misses.
    pool: LayoutPool,
    /// Recency stamps, kept for [`Self::evict_least_recent`].
    stamps: Vec<u64>,
}

/// Lifetime lookup counters. A miss is a lookup that shaped a new layout.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LayoutCacheStats {
    pub hits: u64,
    pub misses: u64,
}

/// Default [`LayoutCache`] entry cap.
const DEFAULT_MAX_ENTRIES: usize = 8192;

impl LayoutCache {
    pub fn new(max_idle_frames: u64) -> Self {
        Self {
            entries: HashMap::new(),
            content_hashes: HashMap::new(),
            scopes: HashMap::new(),
            scope: 0,
            lookups: 0,
            max_idle_frames,
            max_entries: DEFAULT_MAX_ENTRIES,
            font_generation: 0,
            stats: LayoutCacheStats::default(),
            pool: LayoutPool::default(),
            stamps: Vec::new(),
        }
    }

    /// Caps the number of entries (at least one).
    pub fn with_max_entries(mut self, max_entries: usize) -> Self {
        self.max_entries = max_entries.max(1);
        self
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
        let (scopes, max_idle) = (&self.scopes, self.max_idle_frames);
        let idle = self.entries.extract_if(|_, entry| {
            !scopes
                .get(&entry.scope)
                .is_some_and(|frame| frame.wrapping_sub(entry.last_used) <= max_idle)
        });
        let mut evicted = 0;
        for (_, entry) in idle {
            self.pool.recycle(entry.layout);
            evicted += 1;
        }
        if evicted > 0 {
            self.reindex();
        }
        evicted
    }

    pub fn clear(&mut self) {
        for (_, entry) in self.entries.drain() {
            self.pool.recycle(entry.layout);
        }
        self.content_hashes.clear();
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn stats(&self) -> LayoutCacheStats {
        self.stats
    }

    pub fn layout(
        &mut self,
        system: &mut TextSystem,
        params: &TextParams,
    ) -> Result<Arc<TextLayout>, TextError> {
        self.lookup(system, &params.query(), Some(params))
    }

    /// [`Self::layout`] for borrowed params: a hit allocates nothing, a
    /// miss copies the text and spans into the layout.
    pub fn layout_query(
        &mut self,
        system: &mut TextSystem,
        query: &TextQuery,
    ) -> Result<Arc<TextLayout>, TextError> {
        self.lookup(system, query, None)
    }

    /// Looks `params` up, laying them out on a miss. The layout shares
    /// `shared`'s text and spans when given, and copies them otherwise.
    fn lookup(
        &mut self,
        system: &mut TextSystem,
        params: &TextQuery,
        shared: Option<&TextParams>,
    ) -> Result<Arc<TextLayout>, TextError> {
        if system.generation() != self.font_generation {
            self.clear();
            self.font_generation = system.generation();
        }
        let content = match self.content_hashes.get(&text_id(params.text)) {
            Some(&content) => content,
            None => hash_text(params.text),
        };
        let key = LayoutKey::with_content(params, content);
        self.lookups += 1;
        let (scope, frame, touched) = (self.scope, self.frame(), self.lookups);
        if let Some(entry) = self.entries.get_mut(&key) {
            // Guard against 64-bit hash collisions before trusting the hit.
            if same_inputs(&entry.layout, params) {
                entry.scope = scope;
                entry.last_used = frame;
                entry.touched = touched;
                self.stats.hits += 1;
                return Ok(entry.layout.clone());
            }
        }
        self.stats.misses += 1;
        params.validate()?;
        let mut layout = self
            .pool
            .take(params.text)
            .unwrap_or_else(|| Arc::new(TextLayout::empty()));
        let own = Arc::get_mut(&mut layout).expect("pooled layouts are unshared");
        match shared {
            Some(shared) => own.share_inputs(shared),
            None => own.copy_inputs(params),
        }
        system.rebuild(own);
        self.content_hashes
            .insert(text_id(layout.text()), key.content);
        let replaced = self.entries.insert(
            key,
            Entry {
                layout: layout.clone(),
                scope,
                last_used: frame,
                touched,
            },
        );
        if let Some(replaced) = replaced {
            self.pool.recycle(replaced.layout);
            // A colliding entry may have held the only reference to its text.
            self.reindex();
        }
        if self.entries.len() > self.max_entries {
            self.evict_least_recent();
        }
        Ok(layout)
    }

    /// Evicts the least recently used entries down to three quarters of the
    /// cap, so a working set at the cap does not evict on every miss.
    fn evict_least_recent(&mut self) {
        let keep = self.max_entries / 4 * 3;
        let touched = &mut self.stamps;
        touched.clear();
        touched.extend(self.entries.values().map(|e| e.touched));
        let cut = touched.len() - keep.max(1);
        let (_, &mut threshold, _) = touched.select_nth_unstable(cut - 1);
        for (_, entry) in self
            .entries
            .extract_if(|_, entry| entry.touched <= threshold)
        {
            self.pool.recycle(entry.layout);
        }
        self.reindex();
    }

    /// Rebuilds [`Self::content_hashes`] from the surviving entries, dropping
    /// addresses whose text may have been freed and reused.
    fn reindex(&mut self) {
        self.content_hashes.clear();
        for (key, entry) in &self.entries {
            self.content_hashes
                .insert(text_id(entry.layout.text()), key.content);
        }
    }
}

/// Layouts the cache evicted, kept so a miss refills one instead of
/// allocating its storage and `Arc`. `free` layouts are held by nothing
/// else. `retired` ones were still held elsewhere (by a scene, say) when
/// evicted; they stay unchanged for those holders and become free once the
/// last holder drops them.
#[derive(Debug, Default)]
struct LayoutPool {
    free: Vec<Arc<TextLayout>>,
    retired: Vec<Arc<TextLayout>>,
    /// [`spare_bytes`] of the `free` layouts.
    free_bytes: usize,
}

/// Most layouts `free` and `retired` each keep.
const POOL_CAP: usize = 256;

/// Most storage the free layouts keep, by [`spare_bytes`].
const MAX_FREE_BYTES: usize = 8 << 20;

/// Storage a pooled layout keeps: its glyph capacity at a rough 256 bytes a
/// glyph (its own columns plus cosmic-text's shaped and laid-out glyphs),
/// and its text.
fn spare_bytes(layout: &TextLayout) -> usize {
    layout.glyph_capacity() * 256 + layout.text().len()
}

/// A [`LayoutPool`] invariant broken.
#[derive(Debug, PartialEq)]
enum PoolError {
    SharedFree { index: usize },
    Overfull { free: usize, retired: usize },
    FreeBytes { counted: usize, actual: usize },
}

impl LayoutPool {
    fn recycle(&mut self, layout: Arc<TextLayout>) {
        if unshared(&layout) {
            self.free(layout);
        } else {
            // A holder that never lets go must not block later ones, so
            // the oldest retiree gives way.
            if self.retired.len() == POOL_CAP {
                self.retired.swap_remove(0);
            }
            self.retired.push(layout);
        }
        debug_assert_eq!(self.verify_integrity(), Ok(()));
    }

    /// Keeps unshared `layout` if it fits the caps, and drops it otherwise.
    fn free(&mut self, layout: Arc<TextLayout>) {
        let bytes = spare_bytes(&layout);
        if self.free.len() < POOL_CAP && self.free_bytes + bytes <= MAX_FREE_BYTES {
            self.free_bytes += bytes;
            self.free.push(layout);
        }
    }

    /// An unshared layout to rebuild for `text`: preferably one with room
    /// for its glyphs (one per char, roughly), the least room among those,
    /// and text of the same length so the copy can overwrite it.
    fn take(&mut self, text: &str) -> Option<Arc<TextLayout>> {
        let mut i = 0;
        while i < self.retired.len() {
            if unshared(&self.retired[i]) {
                let layout = self.retired.swap_remove(i);
                self.free(layout);
            } else {
                i += 1;
            }
        }
        let chars = text.chars().count();
        let i = (0..self.free.len()).max_by_key(|&i| {
            let layout = &self.free[i];
            let room = layout.glyph_capacity();
            let fits = room >= chars;
            let snug = if fits { usize::MAX - room } else { room };
            (fits, layout.text().len() == text.len(), snug)
        })?;
        let layout = self.free.swap_remove(i);
        self.free_bytes -= spare_bytes(&layout);
        debug_assert_eq!(self.verify_integrity(), Ok(()));
        Some(layout)
    }

    fn verify_integrity(&self) -> Result<(), PoolError> {
        if self.free.len() > POOL_CAP || self.retired.len() > POOL_CAP {
            return Err(PoolError::Overfull {
                free: self.free.len(),
                retired: self.retired.len(),
            });
        }
        if let Some(index) = self.free.iter().position(|layout| !unshared(layout)) {
            return Err(PoolError::SharedFree { index });
        }
        let actual = self.free.iter().map(|layout| spare_bytes(layout)).sum();
        if self.free_bytes != actual || actual > MAX_FREE_BYTES {
            return Err(PoolError::FreeBytes {
                counted: self.free_bytes,
                actual,
            });
        }
        Ok(())
    }
}

/// Whether `layout` is the only reference, so it can be rebuilt without
/// any holder seeing it change.
fn unshared(layout: &Arc<TextLayout>) -> bool {
    Arc::strong_count(layout) == 1 && Arc::weak_count(layout) == 0
}

impl Default for LayoutCache {
    fn default() -> Self {
        Self::new(240)
    }
}

fn same_inputs(layout: &TextLayout, params: &TextQuery) -> bool {
    let text_eq =
        std::ptr::eq(layout.text().as_ref(), params.text) || layout.text().as_ref() == params.text;
    let spans_eq = std::ptr::eq(layout.spans().as_ref(), params.spans)
        || layout.spans().as_ref() == params.spans;
    text_eq && spans_eq && layout.style() == params.style
}

fn kind_tag(kind: FontKind) -> u8 {
    match kind {
        FontKind::Ui => 0,
        FontKind::Mono => 1,
    }
}

fn weight_tag(weight: Option<FontWeight>) -> u8 {
    match weight {
        None => 255,
        Some(FontWeight::Normal) => 0,
        Some(FontWeight::Medium) => 1,
        Some(FontWeight::Semibold) => 2,
        Some(FontWeight::Bold) => 3,
    }
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
        assert_eq!(held.text().as_ref(), TEXT);
        assert_eq!(format!("{:?}", held.glyphs()), glyphs);
    }

    #[test]
    fn layout_cache_font_settings_change_relayouts() {
        // Its own system: changing fonts on the shared one would race other
        // tests.
        let mut sys = TextSystem::vendored_only(&FontSettings::default());
        let mut cache = LayoutCache::new(2);
        let before = cache.layout(&mut sys, &params(150.0)).expect("layout");
        sys.set_font_settings(&FontSettings {
            ui_family: "Inter".into(),
            ..FontSettings::default()
        });
        let after = cache.layout(&mut sys, &params(150.0)).expect("layout");
        assert!(!Arc::ptr_eq(&before, &after));
    }
}
