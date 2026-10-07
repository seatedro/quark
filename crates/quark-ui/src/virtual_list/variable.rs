//! Variable-height virtual list core for chat transcripts: a column-wise
//! row table with Fenwick offsets, plus a scroll model that anchors the
//! first visible row and pins to the bottom while the user is there.
//!
//! No rendering here. An element measures rows through `measure_visible`
//! (any `FnMut(u64, f32) -> f32`, which `quark_text::RowHeights` fits via a
//! closure) and lays out `VariableList::window`.

use std::collections::{HashMap, HashSet};

use super::VirtualListWindow;
use super::fenwick::{Fenwick, UNITS_PER_PX, px_to_units, units_to_px};

/// Stable identity of a row across inserts, removals, and remeasurement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RowKey(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowError {
    DuplicateKey(RowKey),
    UnknownKey(RowKey),
    IndexOutOfBounds { index: usize, len: usize },
}

#[derive(Debug, Clone, PartialEq)]
pub enum RowIntegrityError {
    ColumnLength {
        column: &'static str,
        len: usize,
        expected: usize,
    },
    IndexMapLength {
        len: usize,
        expected: usize,
    },
    IndexMap {
        key: RowKey,
        index: usize,
        mapped: Option<usize>,
    },
    InvalidHeight {
        index: usize,
        height: f32,
    },
    FenwickMismatch,
    MeasuredStats {
        count: usize,
        expected_count: usize,
        units: i64,
        expected_units: i64,
    },
}

fn sanitize_height(height: f32) -> f32 {
    if height.is_finite() && height > 0.0 {
        height
    } else {
        0.0
    }
}

/// Rows stored column-wise. Unmeasured rows hold an estimate; an
/// invalidated row keeps its last height as its estimate so the layout does
/// not jump before it is measured again.
#[derive(Debug, Clone)]
pub struct RowTable {
    keys: Vec<RowKey>,
    heights: Vec<f32>,
    measured: Vec<bool>,
    index: HashMap<RowKey, usize>,
    tree: Fenwick,
    default_estimate: f32,
    estimate_override: Option<f32>,
    measured_count: usize,
    /// Sum of measured heights in Fenwick units, for the running average.
    measured_units: i64,
}

impl RowTable {
    /// `default_estimate` is used until any row has been measured.
    pub fn new(default_estimate: f32) -> Self {
        Self {
            keys: Vec::new(),
            heights: Vec::new(),
            measured: Vec::new(),
            index: HashMap::new(),
            tree: Fenwick::default(),
            default_estimate: sanitize_height(default_estimate),
            estimate_override: None,
            measured_count: 0,
            measured_units: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.keys.len()
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    pub fn keys(&self) -> &[RowKey] {
        &self.keys
    }

    pub fn index_of(&self, key: RowKey) -> Option<usize> {
        self.index.get(&key).copied()
    }

    pub fn height_of(&self, key: RowKey) -> Option<f32> {
        self.index_of(key).map(|i| self.heights[i])
    }

    pub fn is_measured(&self, key: RowKey) -> Option<bool> {
        self.index_of(key).map(|i| self.measured[i])
    }

    /// Height given to new rows: the caller's override, else the average
    /// measured height, else the default. Existing estimated rows keep the
    /// estimate they were given so a changing average does not move them.
    pub fn estimate(&self) -> f32 {
        if let Some(estimate) = self.estimate_override {
            return estimate;
        }
        if self.measured_count == 0 {
            return self.default_estimate;
        }
        let average = self.measured_units as f64 / self.measured_count as f64;
        (average / UNITS_PER_PX) as f32
    }

    pub fn set_estimate(&mut self, estimate: Option<f32>) {
        self.estimate_override = estimate.map(sanitize_height);
    }

    /// O(log n) amortized.
    pub fn append(&mut self, key: RowKey) -> Result<(), RowError> {
        if self.index.contains_key(&key) {
            return Err(RowError::DuplicateKey(key));
        }
        let height = self.estimate();
        self.index.insert(key, self.keys.len());
        self.keys.push(key);
        self.heights.push(height);
        self.measured.push(false);
        self.tree.push(px_to_units(height));
        self.debug_check();
        Ok(())
    }

    /// O(n + k) for a batch of k keys, all estimated.
    pub fn prepend(&mut self, keys: &[RowKey]) -> Result<(), RowError> {
        self.insert_batch(0, keys)
    }

    /// O(n). Inserting at `len()` is the same as `append`.
    pub fn insert(&mut self, index: usize, key: RowKey) -> Result<(), RowError> {
        if index == self.len() {
            return self.append(key);
        }
        self.insert_batch(index, &[key])
    }

    /// O(n + k). Fails without changes when any key already exists or
    /// repeats within the batch.
    pub fn insert_batch(&mut self, index: usize, keys: &[RowKey]) -> Result<(), RowError> {
        let len = self.len();
        if index > len {
            return Err(RowError::IndexOutOfBounds { index, len });
        }
        let mut seen = HashSet::with_capacity(keys.len());
        for &key in keys {
            if self.index.contains_key(&key) || !seen.insert(key) {
                return Err(RowError::DuplicateKey(key));
            }
        }
        let height = self.estimate();
        let count = keys.len();
        self.keys.splice(index..index, keys.iter().copied());
        self.heights
            .splice(index..index, std::iter::repeat_n(height, count));
        self.measured
            .splice(index..index, std::iter::repeat_n(false, count));
        self.reindex_from(index);
        self.rebuild_tree();
        self.debug_check();
        Ok(())
    }

    /// O(n).
    pub fn remove(&mut self, key: RowKey) -> Result<(), RowError> {
        let index = self.index_of(key).ok_or(RowError::UnknownKey(key))?;
        if self.measured[index] {
            self.measured_count -= 1;
            self.measured_units -= px_to_units(self.heights[index]);
        }
        self.index.remove(&key);
        self.keys.remove(index);
        self.heights.remove(index);
        self.measured.remove(index);
        self.reindex_from(index);
        self.rebuild_tree();
        self.debug_check();
        Ok(())
    }

    /// Records a measured height. O(log n).
    pub fn set_height(&mut self, key: RowKey, height: f32) -> Result<(), RowError> {
        let index = self.index_of(key).ok_or(RowError::UnknownKey(key))?;
        self.set_height_at(index, height);
        Ok(())
    }

    fn set_height_at(&mut self, index: usize, height: f32) {
        let height = sanitize_height(height);
        let old_units = px_to_units(self.heights[index]);
        let new_units = px_to_units(height);
        self.tree.add(index, new_units - old_units);
        if self.measured[index] {
            self.measured_units += new_units - old_units;
        } else {
            self.measured[index] = true;
            self.measured_count += 1;
            self.measured_units += new_units;
        }
        self.heights[index] = height;
        self.debug_check();
    }

    /// Marks a row for remeasurement. O(1); offsets do not change.
    pub fn invalidate(&mut self, key: RowKey) -> Result<(), RowError> {
        let index = self.index_of(key).ok_or(RowError::UnknownKey(key))?;
        if self.measured[index] {
            self.measured[index] = false;
            self.measured_count -= 1;
            self.measured_units -= px_to_units(self.heights[index]);
        }
        self.debug_check();
        Ok(())
    }

    /// Marks every row for remeasurement, as after a width change. O(n);
    /// offsets do not change until rows are measured again.
    pub fn invalidate_all(&mut self) {
        self.measured.fill(false);
        self.measured_count = 0;
        self.measured_units = 0;
        self.debug_check();
    }

    /// Top of the row at `index`; `offset_of_index(len())` is the total
    /// extent. O(log n).
    pub fn offset_of_index(&self, index: usize) -> f32 {
        units_to_px(self.tree.prefix(index))
    }

    /// O(log n).
    pub fn offset_of(&self, key: RowKey) -> Option<f32> {
        self.index_of(key).map(|i| self.offset_of_index(i))
    }

    /// Index of the row whose `[top, top + height)` contains `offset`, or
    /// `None` outside `[0, total_extent)`. O(log n).
    pub fn row_at(&self, offset: f32) -> Option<usize> {
        if !offset.is_finite() || offset < 0.0 {
            return None;
        }
        let target = (f64::from(offset) * UNITS_PER_PX).floor() as i64;
        let index = self.tree.search(target, false);
        (index < self.len()).then_some(index)
    }

    /// O(log n).
    pub fn total_extent(&self) -> f32 {
        units_to_px(self.tree.total())
    }

    /// Rows intersecting `[scroll - overscan, scroll + viewport + overscan)`,
    /// with spacer heights for the rows outside. O(log n).
    pub fn visible_range(
        &self,
        scroll_offset: f32,
        viewport_height: f32,
        overscan_px: f32,
    ) -> VirtualListWindow {
        let total = self.tree.total();
        let len = self.len();
        let scroll = finite_or_zero(scroll_offset).max(0.0);
        let overscan = finite_or_zero(overscan_px).max(0.0);
        let viewport = finite_or_zero(viewport_height).max(0.0);
        let top = (f64::from(scroll) - f64::from(overscan)).max(0.0) * UNITS_PER_PX;
        let bottom = (f64::from(scroll) + f64::from(viewport) + f64::from(overscan)) * UNITS_PER_PX;

        // First row whose bottom lies below `top`.
        let start = self.tree.search(top.floor() as i64, false).min(len);
        // One past the last row whose top lies above `bottom`.
        let bottom_units = bottom.ceil() as i64;
        let end = if bottom_units <= 0 {
            0
        } else {
            (self.tree.search(bottom_units, true) + 1).min(len)
        }
        .max(start);

        let top_units = self.tree.prefix(start);
        let end_units = self.tree.prefix(end);
        VirtualListWindow {
            range: start..end,
            top_spacer: units_to_px(top_units),
            bottom_spacer: units_to_px(total - end_units),
            total_extent: units_to_px(total),
        }
    }

    pub fn verify_integrity(&self) -> Result<(), RowIntegrityError> {
        let expected = self.keys.len();
        for (column, len) in [
            ("heights", self.heights.len()),
            ("measured", self.measured.len()),
            ("tree", self.tree.len()),
        ] {
            if len != expected {
                return Err(RowIntegrityError::ColumnLength {
                    column,
                    len,
                    expected,
                });
            }
        }
        if self.index.len() != expected {
            return Err(RowIntegrityError::IndexMapLength {
                len: self.index.len(),
                expected,
            });
        }
        // Equal lengths plus every key mapping to its own row make the map a
        // bijection, which also rules out duplicate keys.
        for (index, &key) in self.keys.iter().enumerate() {
            let mapped = self.index.get(&key).copied();
            if mapped != Some(index) {
                return Err(RowIntegrityError::IndexMap { key, index, mapped });
            }
        }
        for (index, &height) in self.heights.iter().enumerate() {
            if sanitize_height(height) != height {
                return Err(RowIntegrityError::InvalidHeight { index, height });
            }
        }
        // Integer node sums depend only on the leaves, so a fresh O(n) build
        // must equal the incrementally updated tree exactly.
        if Fenwick::build(self.heights.iter().map(|&h| px_to_units(h))) != self.tree {
            return Err(RowIntegrityError::FenwickMismatch);
        }
        let (expected_count, expected_units) = self
            .heights
            .iter()
            .zip(&self.measured)
            .filter(|(_, measured)| **measured)
            .fold((0, 0), |(count, units), (&h, _)| {
                (count + 1, units + px_to_units(h))
            });
        if expected_count != self.measured_count || expected_units != self.measured_units {
            return Err(RowIntegrityError::MeasuredStats {
                count: self.measured_count,
                expected_count,
                units: self.measured_units,
                expected_units,
            });
        }
        Ok(())
    }

    fn reindex_from(&mut self, start: usize) {
        for (index, &key) in self.keys.iter().enumerate().skip(start) {
            self.index.insert(key, index);
        }
    }

    fn rebuild_tree(&mut self) {
        self.tree = Fenwick::build(self.heights.iter().map(|&h| px_to_units(h)));
    }

    fn debug_check(&self) {
        debug_assert_eq!(self.verify_integrity(), Ok(()));
    }
}

fn finite_or_zero(value: f32) -> f32 {
    if value.is_finite() { value } else { 0.0 }
}

/// Distance from the bottom within which the view counts as pinned.
pub const STICK_EPSILON_PX: f32 = 1.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollAlign {
    Top,
    Center,
    Bottom,
}

/// The first visible row and where its top sits relative to the viewport
/// top (zero or negative).
#[derive(Debug, Clone, Copy)]
struct Anchor {
    key: RowKey,
    top_in_view: f64,
}

/// A row table plus the scroll state of one viewport.
///
/// Every mutation returns the scroll offset delta the caller must apply to
/// its scroll container. While pinned to the bottom the view follows the
/// end of the content; otherwise the first visible row keeps its on-screen
/// position.
#[derive(Debug, Clone)]
pub struct VariableList {
    rows: RowTable,
    scroll_offset: f32,
    viewport_height: f32,
    stick_to_bottom: bool,
    /// Width the measured heights belong to; a different width remeasures.
    measured_width: Option<f32>,
}

impl VariableList {
    /// Starts pinned to the bottom, as a transcript opens at its newest row.
    pub fn new(default_estimate: f32, viewport_height: f32) -> Self {
        Self {
            rows: RowTable::new(default_estimate),
            scroll_offset: 0.0,
            viewport_height: finite_or_zero(viewport_height).max(0.0),
            stick_to_bottom: true,
            measured_width: None,
        }
    }

    pub fn rows(&self) -> &RowTable {
        &self.rows
    }

    pub fn scroll_offset(&self) -> f32 {
        self.scroll_offset
    }

    pub fn viewport_height(&self) -> f32 {
        self.viewport_height
    }

    pub fn is_stuck_to_bottom(&self) -> bool {
        self.stick_to_bottom
    }

    pub fn max_scroll_offset(&self) -> f32 {
        (self.rows.total_extent() - self.viewport_height).max(0.0)
    }

    pub fn window(&self, overscan_px: f32) -> VirtualListWindow {
        self.rows
            .visible_range(self.scroll_offset, self.viewport_height, overscan_px)
    }

    /// A user scroll. Pins when it lands within `STICK_EPSILON_PX` of the
    /// bottom and unpins otherwise. Returns the clamped offset.
    pub fn set_scroll_offset(&mut self, offset: f32) -> f32 {
        if offset.is_finite() {
            let max = self.max_scroll_offset();
            self.scroll_offset = offset.clamp(0.0, max);
            self.stick_to_bottom = self.scroll_offset >= max - STICK_EPSILON_PX;
        }
        self.scroll_offset
    }

    pub fn set_viewport_height(&mut self, height: f32) -> f32 {
        let anchor = self.capture_anchor();
        self.viewport_height = finite_or_zero(height).max(0.0);
        self.restore(anchor)
    }

    pub fn set_estimate(&mut self, estimate: Option<f32>) {
        self.rows.set_estimate(estimate);
    }

    pub fn append(&mut self, key: RowKey) -> Result<f32, RowError> {
        self.anchored(|rows| rows.append(key))
    }

    pub fn prepend(&mut self, keys: &[RowKey]) -> Result<f32, RowError> {
        self.anchored(|rows| rows.prepend(keys))
    }

    pub fn insert(&mut self, index: usize, key: RowKey) -> Result<f32, RowError> {
        self.anchored(|rows| rows.insert(index, key))
    }

    pub fn remove(&mut self, key: RowKey) -> Result<f32, RowError> {
        self.anchored(|rows| rows.remove(key))
    }

    pub fn set_height(&mut self, key: RowKey, height: f32) -> Result<f32, RowError> {
        self.anchored(|rows| rows.set_height(key, height))
    }

    pub fn invalidate(&mut self, key: RowKey) -> Result<(), RowError> {
        self.rows.invalidate(key)
    }

    pub fn invalidate_all(&mut self) {
        self.rows.invalidate_all();
    }

    /// Measures every unmeasured row in the overscanned window at `width`,
    /// repeating while new measurements pull more rows into the window.
    /// A width different from the last call invalidates all rows first.
    pub fn measure_visible(
        &mut self,
        width: f32,
        overscan_px: f32,
        mut measure: impl FnMut(u64, f32) -> f32,
    ) -> f32 {
        if self.measured_width.map(f32::to_bits) != Some(width.to_bits()) {
            self.rows.invalidate_all();
            self.measured_width = Some(width);
        }
        let start = self.scroll_offset;
        let anchor = self.capture_anchor();
        // Terminates: each pass measures at least one row or stops.
        loop {
            let range = self.window(overscan_px).range;
            let mut measured_any = false;
            for index in range {
                if !self.rows.measured[index] {
                    let height = measure(self.rows.keys[index].0, width);
                    self.rows.set_height_at(index, height);
                    measured_any = true;
                }
            }
            if !measured_any {
                break;
            }
            self.restore(anchor);
        }
        self.scroll_offset - start
    }

    /// Scrolls so `key` sits at the top, center, or bottom of the viewport,
    /// clamped to the content. Landing at the bottom pins the view.
    pub fn scroll_to(&mut self, key: RowKey, align: ScrollAlign) -> Result<f32, RowError> {
        let index = self.rows.index_of(key).ok_or(RowError::UnknownKey(key))?;
        let top = self.rows.offset_of_index(index);
        let height = self.rows.heights[index];
        let target = match align {
            ScrollAlign::Top => top,
            ScrollAlign::Center => top + height * 0.5 - self.viewport_height * 0.5,
            ScrollAlign::Bottom => top + height - self.viewport_height,
        };
        let start = self.scroll_offset;
        Ok(self.set_scroll_offset(target) - start)
    }

    fn anchored(
        &mut self,
        op: impl FnOnce(&mut RowTable) -> Result<(), RowError>,
    ) -> Result<f32, RowError> {
        let anchor = self.capture_anchor();
        op(&mut self.rows)?;
        Ok(self.restore(anchor))
    }

    fn capture_anchor(&self) -> Option<Anchor> {
        let index = self.rows.row_at(self.scroll_offset)?;
        let top = f64::from(self.rows.offset_of_index(index));
        Some(Anchor {
            key: self.rows.keys[index],
            top_in_view: top - f64::from(self.scroll_offset),
        })
    }

    /// Moves the scroll offset to honor the pin or the anchor and returns
    /// the change. A removed anchor row leaves the offset where it was,
    /// which keeps the rows above it in place.
    fn restore(&mut self, anchor: Option<Anchor>) -> f32 {
        let old = self.scroll_offset;
        let max = self.max_scroll_offset();
        let target = if self.stick_to_bottom {
            max
        } else {
            anchor
                .and_then(|a| {
                    let top = f64::from(self.rows.offset_of(a.key)?);
                    Some((top - a.top_in_view) as f32)
                })
                .unwrap_or(old)
        };
        self.scroll_offset = target.clamp(0.0, max);
        self.scroll_offset - old
    }
}
