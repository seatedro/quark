//! The editor's layout, one paragraph (logical line) at a time: a slot per
//! line holding its shaped layout when it has one, and each line's height
//! in a Fenwick tree, so a line's top, the line at a given height, and the
//! total height are O(log n).
//!
//! Only lines near the view are shaped. The rest stand at an estimate:
//! one line's height, or the height a line last had when it was shaped,
//! which stays exact until its text or the layout's parameters change.
//! Heights are fixed-point units (see `quark::fenwick`), so sums are exact.

use std::ops::Range;
use std::sync::Arc;

use quark::fenwick::{Fenwick, px_to_units, units_to_offset};
use quark_render::scene::FontKind;
use quark_text::TextLayout;

use super::spans::SyntaxTokenKind;

/// Everything shaping a line depends on besides its text and spans.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct ShapeParams {
    pub font_size: f32,
    pub line_height: f32,
    pub kind: FontKind,
    pub wrap: Option<f32>,
    pub scale: f32,
    pub fonts: quark_text::FontEpoch,
    /// Bumped by `Editor::invalidate_font`.
    pub font_rev: u32,
}

/// A shaped line.
#[derive(Debug)]
pub(crate) struct Shaped {
    pub layout: Arc<TextLayout>,
    /// The token kind of each of `layout`'s spans, for paint colors.
    pub kinds: Arc<[SyntaxTokenKind]>,
}

/// Whether a slot's layout still matches its line.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Freshness {
    /// Shaped from the line's current text with the current parameters.
    #[default]
    Current,
    /// The line's text or the parameters changed since; the layout stays
    /// for approximate geometry until it is shaped again.
    Stale,
}

#[derive(Debug, Clone, Default)]
struct Slot {
    shaped: Option<Arc<Shaped>>,
    freshness: Freshness,
    /// The highlight generation its spans were last found current at.
    checked: u64,
}

/// A broken paragraph store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ParagraphIntegrityError {
    Columns { slots: usize, heights: usize },
    Fenwick,
    ShapedCount { counted: usize, stored: usize },
}

#[derive(Debug, Clone)]
pub(super) struct Paragraphs {
    slots: Vec<Slot>,
    heights: Vec<i64>,
    tree: Fenwick,
    params: Option<ShapeParams>,
    /// Height of a line never shaped.
    estimate: i64,
    /// Slots holding a layout.
    shaped: usize,
}

impl Default for Paragraphs {
    fn default() -> Self {
        let mut store = Self {
            slots: Vec::new(),
            heights: Vec::new(),
            tree: Fenwick::default(),
            params: None,
            estimate: px_to_units(1.0),
            shaped: 0,
        };
        store.reset(1);
        store
    }
}

impl Paragraphs {
    pub(super) fn len(&self) -> usize {
        self.slots.len()
    }

    pub(super) fn params(&self) -> Option<ShapeParams> {
        self.params
    }

    /// Lay out with `params` from now on: every layout goes stale, keeping
    /// its height as its estimate.
    pub(super) fn set_params(&mut self, params: ShapeParams) {
        self.params = Some(params);
        let estimate = px_to_units(params.line_height);
        if estimate != self.estimate {
            // Lines never shaped move to the new line height.
            self.estimate = estimate;
            for (i, slot) in self.slots.iter().enumerate() {
                if slot.shaped.is_none()
                    && let Some(height) = self.heights.get_mut(i)
                {
                    *height = estimate;
                }
            }
            self.tree = Fenwick::build(self.heights.iter().copied());
        }
        for slot in &mut self.slots {
            slot.freshness = Freshness::Stale;
        }
        self.debug_check();
    }

    /// `count` lines, none shaped.
    pub(super) fn reset(&mut self, count: usize) {
        self.slots.clear();
        self.slots.resize(count.max(1), Slot::default());
        self.heights.clear();
        self.heights.resize(count.max(1), self.estimate);
        self.tree = Fenwick::build(self.heights.iter().copied());
        self.shaped = 0;
        self.debug_check();
    }

    /// Lines `first..first + removed` became `inserted` lines. Lines that
    /// stay in place keep their layout (stale) and height as an estimate;
    /// new ones start at the estimate.
    pub(super) fn splice(&mut self, first: usize, removed: usize, inserted: usize) {
        let end = (first + removed).min(self.slots.len());
        let first = first.min(end);
        let kept = inserted.min(end - first);
        for slot in self.slots.get_mut(first..first + kept).unwrap_or_default() {
            slot.freshness = Freshness::Stale;
        }
        let dropped = self
            .slots
            .splice(
                first + kept..end,
                std::iter::repeat_n(Slot::default(), inserted - kept),
            )
            .filter(|slot| slot.shaped.is_some())
            .count();
        self.shaped -= dropped;
        if end - first != inserted {
            self.heights.splice(
                first + kept..end,
                std::iter::repeat_n(self.estimate, inserted - kept),
            );
            self.tree = Fenwick::build(self.heights.iter().copied());
        }
        self.debug_check();
    }

    pub(super) fn shaped(&self, line: usize) -> Option<&Arc<Shaped>> {
        self.slots.get(line)?.shaped.as_ref()
    }

    /// The layout of `line` if it is current: shaped from its text with
    /// the current parameters, and its spans checked at highlight
    /// `generation`.
    pub(super) fn current(&self, line: usize, generation: u64) -> Option<&Arc<Shaped>> {
        let slot = self.slots.get(line)?;
        (slot.freshness == Freshness::Current && slot.checked == generation)
            .then_some(slot.shaped.as_ref())
            .flatten()
    }

    /// Whether `line`'s layout is shaped from its current text with the
    /// current parameters (its spans may be unchecked).
    pub(super) fn is_fresh(&self, line: usize) -> bool {
        self.slots
            .get(line)
            .is_some_and(|s| s.shaped.is_some() && s.freshness == Freshness::Current)
    }

    /// Record that `line`'s spans are still current at `generation`.
    pub(super) fn mark_checked(&mut self, line: usize, generation: u64) {
        if let Some(slot) = self.slots.get_mut(line) {
            slot.checked = generation;
        }
    }

    /// `line` shaped as `shaped`, current at highlight `generation`.
    pub(super) fn store(&mut self, line: usize, shaped: Shaped, generation: u64) {
        let height = px_to_units(shaped.layout.size().1.max(0.0));
        let Some(slot) = self.slots.get_mut(line) else {
            return;
        };
        if slot.shaped.is_none() {
            self.shaped += 1;
        }
        *slot = Slot {
            shaped: Some(Arc::new(shaped)),
            freshness: Freshness::Current,
            checked: generation,
        };
        self.set_height(line, height);
    }

    fn set_height(&mut self, line: usize, height: i64) {
        if let Some(old) = self.heights.get_mut(line)
            && *old != height
        {
            self.tree.add(line, height - *old);
            *old = height;
        }
    }

    pub(super) fn top(&self, line: usize) -> f32 {
        units_to_offset(self.tree.prefix(line)) as f32
    }

    pub(super) fn height(&self, line: usize) -> f32 {
        units_to_offset(self.heights.get(line).copied().unwrap_or(0)) as f32
    }

    pub(super) fn total(&self) -> f32 {
        units_to_offset(self.tree.total()) as f32
    }

    /// The line at height `y`, clamped to the first and last.
    pub(super) fn at_y(&self, y: f32) -> usize {
        let target = px_to_units(y.max(0.0));
        self.tree
            .search(target, false)
            .min(self.slots.len().saturating_sub(1))
    }

    /// How many lines hold a layout.
    pub(super) fn shaped_count(&self) -> usize {
        self.shaped
    }

    /// Drops the layouts of lines outside `keep` once more than `budget`
    /// lines hold one. Their heights stay as estimates.
    pub(super) fn evict(&mut self, keep: Range<usize>, budget: usize) {
        if self.shaped <= budget {
            return;
        }
        for (line, slot) in self.slots.iter_mut().enumerate() {
            if !keep.contains(&line) && slot.shaped.take().is_some() {
                self.shaped -= 1;
            }
        }
        self.debug_check();
    }

    pub(super) fn verify_integrity(&self) -> Result<(), ParagraphIntegrityError> {
        if self.slots.len() != self.heights.len() {
            return Err(ParagraphIntegrityError::Columns {
                slots: self.slots.len(),
                heights: self.heights.len(),
            });
        }
        // Every prefix difference is that line's height; checked in place,
        // so the check allocates nothing.
        let mut sum = 0;
        for (i, height) in self.heights.iter().enumerate() {
            sum += height;
            if self.tree.len() != self.heights.len() || self.tree.prefix(i + 1) != sum {
                return Err(ParagraphIntegrityError::Fenwick);
            }
        }
        let counted = self.slots.iter().filter(|s| s.shaped.is_some()).count();
        if counted != self.shaped {
            return Err(ParagraphIntegrityError::ShapedCount {
                counted,
                stored: self.shaped,
            });
        }
        Ok(())
    }

    fn debug_check(&self) {
        if cfg!(all(debug_assertions, feature = "integrity-checks")) {
            debug_assert_eq!(self.verify_integrity(), Ok(()));
        } else {
            debug_assert_eq!(self.slots.len(), self.heights.len());
        }
    }
}
