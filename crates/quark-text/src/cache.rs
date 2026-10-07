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
        let mut content = DefaultHasher::new();
        params.text.hash(&mut content);

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
            content: content.finish(),
            attrs: attrs.finish(),
            size_bits: style.font_size.to_bits(),
            wrap_bits: params.wrap_width.map(f32::to_bits),
            scale_bits: params.scale_factor.to_bits(),
        }
    }
}

#[derive(Debug)]
struct Entry {
    layout: Arc<TextLayout>,
    last_used: u64,
}

/// Frame-scoped layout cache. Call [`Self::begin_frame`] once per frame and
/// [`Self::trim`] whenever convenient (e.g. end of frame); entries not used in
/// the last `max_idle_frames` frames are evicted.
#[derive(Debug)]
pub struct LayoutCache {
    entries: HashMap<LayoutKey, Entry>,
    frame: u64,
    max_idle_frames: u64,
    font_generation: u64,
    stats: LayoutCacheStats,
}

/// Lifetime lookup counters. A miss is a lookup that shaped a new layout.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LayoutCacheStats {
    pub hits: u64,
    pub misses: u64,
}

impl LayoutCache {
    pub fn new(max_idle_frames: u64) -> Self {
        Self {
            entries: HashMap::new(),
            frame: 0,
            max_idle_frames,
            font_generation: 0,
            stats: LayoutCacheStats::default(),
        }
    }

    pub fn begin_frame(&mut self) {
        self.frame = self.frame.wrapping_add(1);
    }

    pub fn frame(&self) -> u64 {
        self.frame
    }

    /// Returns the number of evicted entries.
    pub fn trim(&mut self) -> usize {
        let before = self.entries.len();
        let (frame, max_idle) = (self.frame, self.max_idle_frames);
        self.entries
            .retain(|_, entry| frame.wrapping_sub(entry.last_used) <= max_idle);
        before - self.entries.len()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
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
            self.entries.clear();
            self.font_generation = system.generation();
        }
        let key = LayoutKey::new(params);
        if let Some(entry) = self.entries.get_mut(&key) {
            // Guard against 64-bit hash collisions before trusting the hit.
            if same_inputs(&entry.layout, params) {
                entry.last_used = self.frame;
                self.stats.hits += 1;
                return Ok(entry.layout.clone());
            }
        }
        self.stats.misses += 1;
        let layout = Arc::new(system.layout(params)?);
        self.entries.insert(
            key,
            Entry {
                layout: layout.clone(),
                last_used: self.frame,
            },
        );
        Ok(layout)
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
