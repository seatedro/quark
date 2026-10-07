use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;

use quark::scene::FontStyle;
use quark::{FontKind, FontWeight};

use crate::layout::{TextError, TextLayout, TextParams};
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
        Self::with_content(params, hash_text(&params.text))
    }

    /// The key for `params` whose text hashes to `content`.
    fn with_content(params: &TextParams, content: u64) -> Self {
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

/// Identifies one `Arc<str>` allocation by address and length.
fn text_id(text: &Arc<str>) -> (usize, usize) {
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
        let before = self.entries.len();
        let (scopes, max_idle) = (&self.scopes, self.max_idle_frames);
        self.entries.retain(|_, entry| {
            scopes
                .get(&entry.scope)
                .is_some_and(|frame| frame.wrapping_sub(entry.last_used) <= max_idle)
        });
        let evicted = before - self.entries.len();
        if evicted > 0 {
            self.reindex();
        }
        evicted
    }

    pub fn clear(&mut self) {
        self.entries.clear();
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
        if system.generation() != self.font_generation {
            self.clear();
            self.font_generation = system.generation();
        }
        let content = match self.content_hashes.get(&text_id(&params.text)) {
            Some(&content) => content,
            None => hash_text(&params.text),
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
        let layout = Arc::new(system.layout(params)?);
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
        if replaced.is_some() {
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
        let mut touched: Vec<u64> = self.entries.values().map(|e| e.touched).collect();
        let cut = touched.len() - keep.max(1);
        let (_, &mut threshold, _) = touched.select_nth_unstable(cut - 1);
        self.entries.retain(|_, entry| entry.touched > threshold);
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

impl Default for LayoutCache {
    fn default() -> Self {
        Self::new(240)
    }
}

fn same_inputs(layout: &TextLayout, params: &TextParams) -> bool {
    let text_eq =
        Arc::ptr_eq(layout.text(), &params.text) || layout.text().as_ref() == params.text.as_ref();
    let spans_eq = Arc::ptr_eq(layout.spans(), &params.spans)
        || layout.spans().as_ref() == params.spans.as_ref();
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
