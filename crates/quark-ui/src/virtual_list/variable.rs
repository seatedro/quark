//! Variable-height virtual list core for chat transcripts: a column-wise
//! row table with Fenwick offsets, plus a scroll model that anchors the
//! first visible row and pins to the bottom while the user is there.
//!
//! No rendering here. An element measures rows through `measure_visible`
//! (any `FnMut(u64, f32) -> f32`, which `quark_text::RowHeights` fits via a
//! closure) and lays out `VariableList::window`.

use std::collections::{HashMap, HashSet};

use super::VirtualListWindow;
use quark::fenwick::{Fenwick, UNITS_PER_PX, px_to_units, units_to_px};
use quark::selection::{FULL_INTEGRITY_CHECKS, count_integrity_steps};

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

    /// Whether the row at `index` holds a measured height. O(1).
    pub fn is_measured_at(&self, index: usize) -> bool {
        self.measured[index]
    }

    /// Number of rows holding a measured height. O(1).
    pub fn measured_count(&self) -> usize {
        self.measured_count
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
        self.debug_check_row(self.keys.len() - 1);
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
        self.debug_check_row(index);
    }

    /// Marks a row for remeasurement. O(1); offsets do not change.
    pub fn invalidate(&mut self, key: RowKey) -> Result<(), RowError> {
        let index = self.index_of(key).ok_or(RowError::UnknownKey(key))?;
        if self.measured[index] {
            self.measured[index] = false;
            self.measured_count -= 1;
            self.measured_units -= px_to_units(self.heights[index]);
        }
        self.debug_check_row(index);
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

    /// Checks row `index` only: column and map lengths, its key's map
    /// entry, its height, its Fenwick leaf, and that the measured totals are
    /// in range. O(log n); single-row mutations call it through
    /// `debug_assert!`.
    pub fn verify_row(&self, index: usize) -> Result<(), RowIntegrityError> {
        count_integrity_steps(1);
        self.verify_lengths()?;
        let key = self.keys[index];
        let mapped = self.index.get(&key).copied();
        if mapped != Some(index) {
            return Err(RowIntegrityError::IndexMap { key, index, mapped });
        }
        let height = self.heights[index];
        if sanitize_height(height) != height {
            return Err(RowIntegrityError::InvalidHeight { index, height });
        }
        if self.tree.prefix(index + 1) - self.tree.prefix(index) != px_to_units(height) {
            return Err(RowIntegrityError::FenwickMismatch);
        }
        if self.measured_count > self.keys.len() || self.measured_units < 0 {
            return Err(RowIntegrityError::MeasuredStats {
                count: self.measured_count,
                expected_count: self.measured_count.min(self.keys.len()),
                units: self.measured_units,
                expected_units: self.measured_units.max(0),
            });
        }
        Ok(())
    }

    fn verify_lengths(&self) -> Result<(), RowIntegrityError> {
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
        Ok(())
    }

    /// Checks every row, rebuilds the Fenwick tree to compare, and recounts
    /// the measured totals. O(n); batch mutations call it through
    /// `debug_assert!`, as do single-row ones with `integrity-checks`.
    pub fn verify_integrity(&self) -> Result<(), RowIntegrityError> {
        count_integrity_steps(self.keys.len());
        self.verify_lengths()?;
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

    fn debug_check_row(&self, index: usize) {
        if FULL_INTEGRITY_CHECKS {
            self.debug_check();
        } else {
            debug_assert_eq!(self.verify_row(index), Ok(()));
        }
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

    /// Appends a batch of rows, indexing and checking once.
    pub fn extend(&mut self, keys: &[RowKey]) -> Result<f32, RowError> {
        self.anchored(|rows| rows.insert_batch(rows.len(), keys))
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

#[cfg(test)]
mod tests {
    use super::{RowKey, RowTable, ScrollAlign, VariableList};
    use proptest::prelude::*;

    /// Rows keyed `0..n`, all measured at the given heights.
    fn table(heights: &[f32]) -> RowTable {
        let mut rows = RowTable::new(10.0);
        for (i, &h) in heights.iter().enumerate() {
            rows.append(RowKey(i as u64)).unwrap();
            rows.set_height(RowKey(i as u64), h).unwrap();
        }
        rows
    }

    fn list(heights: &[f32], viewport: f32) -> VariableList {
        let mut list = VariableList::new(10.0, viewport);
        for (i, &h) in heights.iter().enumerate() {
            list.append(RowKey(i as u64)).unwrap();
            list.set_height(RowKey(i as u64), h).unwrap();
        }
        list
    }

    /// Where `key`'s top sits relative to the viewport top.
    fn screen_top(list: &VariableList, key: u64) -> f32 {
        list.rows().offset_of(RowKey(key)).unwrap() - list.scroll_offset()
    }

    fn anchor_key(list: &VariableList) -> u64 {
        let index = list.rows().row_at(list.scroll_offset()).unwrap();
        list.rows().keys()[index].0
    }

    /// Quarter-pixel heights are exact in the fixed-point tree, so a naive
    /// f64 model can be compared without tolerance.
    fn quarter_px(max_quarters: u16) -> impl Strategy<Value = f32> {
        (0..=max_quarters).prop_map(|q| f32::from(q) / 4.0)
    }

    #[derive(Debug, Clone)]
    enum Op {
        Append,
        Prepend(usize),
        Insert(usize),
        Remove(usize),
        SetHeight(usize, f32),
        Invalidate(usize),
        InvalidateAll,
    }

    fn op() -> impl Strategy<Value = Op> {
        prop_oneof![
            Just(Op::Append),
            (0..6usize).prop_map(Op::Prepend),
            any::<usize>().prop_map(Op::Insert),
            any::<usize>().prop_map(Op::Remove),
            (any::<usize>(), quarter_px(800)).prop_map(|(i, h)| Op::SetHeight(i, h)),
            any::<usize>().prop_map(Op::Invalidate),
            Just(Op::InvalidateAll),
        ]
    }

    const ESTIMATE: f32 = 12.5;

    proptest! {
        #[test]
        fn random_ops_keep_offsets_equal_to_naive_prefix_sums(
            ops in prop::collection::vec(op(), 1..60)
        ) {
            let mut rows = RowTable::new(1.0);
            rows.set_estimate(Some(ESTIMATE));
            // Model: (key, height) in display order.
            let mut model: Vec<(u64, f32)> = Vec::new();
            let mut next_key = 0u64;
            let mut fresh = || {
                next_key += 1;
                next_key
            };
            for op in ops {
                match op {
                    Op::Append => {
                        let key = fresh();
                        rows.append(RowKey(key)).unwrap();
                        model.push((key, ESTIMATE));
                    }
                    Op::Prepend(count) => {
                        let keys: Vec<u64> = (0..count).map(|_| fresh()).collect();
                        let row_keys: Vec<RowKey> = keys.iter().map(|&k| RowKey(k)).collect();
                        rows.prepend(&row_keys).unwrap();
                        model.splice(0..0, keys.iter().map(|&k| (k, ESTIMATE)));
                    }
                    Op::Insert(seed) => {
                        let index = seed % (model.len() + 1);
                        let key = fresh();
                        rows.insert(index, RowKey(key)).unwrap();
                        model.insert(index, (key, ESTIMATE));
                    }
                    Op::Remove(seed) if !model.is_empty() => {
                        let (key, _) = model.remove(seed % model.len());
                        rows.remove(RowKey(key)).unwrap();
                    }
                    Op::SetHeight(seed, height) if !model.is_empty() => {
                        let index = seed % model.len();
                        model[index].1 = height;
                        rows.set_height(RowKey(model[index].0), height).unwrap();
                    }
                    Op::Invalidate(seed) if !model.is_empty() => {
                        rows.invalidate(RowKey(model[seed % model.len()].0)).unwrap();
                    }
                    Op::InvalidateAll => rows.invalidate_all(),
                    _ => {}
                }
                prop_assert_eq!(rows.verify_integrity(), Ok(()));
                let keys: Vec<u64> = rows.keys().iter().map(|k| k.0).collect();
                let model_keys: Vec<u64> = model.iter().map(|r| r.0).collect();
                prop_assert_eq!(keys, model_keys);
                let mut top = 0.0f64;
                for &(key, height) in &model {
                    prop_assert_eq!(rows.offset_of(RowKey(key)), Some(top as f32));
                    top += f64::from(height);
                }
                prop_assert_eq!(rows.total_extent(), top as f32);
            }
        }

        #[test]
        fn row_at_inverts_offset_of(
            heights in prop::collection::vec(quarter_px(800).prop_map(|h| h + 0.25), 1..80)
        ) {
            let rows = table(&heights);
            for (i, &h) in heights.iter().enumerate() {
                let top = rows.offset_of(RowKey(i as u64)).unwrap();
                prop_assert_eq!(rows.row_at(top), Some(i));
                prop_assert_eq!(rows.row_at(top + h - 0.25), Some(i));
            }
            prop_assert_eq!(rows.row_at(rows.total_extent()), None);
        }

        #[test]
        fn visible_range_is_exactly_the_rows_crossing_the_window(
            heights in prop::collection::vec(quarter_px(400), 0..60),
            scroll in quarter_px(8000),
            viewport in quarter_px(2000),
            overscan in quarter_px(400),
        ) {
            let rows = table(&heights);
            let window = rows.visible_range(scroll, viewport, overscan);
            let lo = (f64::from(scroll) - f64::from(overscan)).max(0.0);
            let hi = f64::from(scroll) + f64::from(viewport) + f64::from(overscan);
            let mut top = 0.0f64;
            let mut start = heights.len();
            let mut end = 0;
            for (i, &h) in heights.iter().enumerate() {
                let bottom = top + f64::from(h);
                if bottom > lo && start == heights.len() {
                    start = i;
                }
                if top < hi {
                    end = i + 1;
                }
                top = bottom;
            }
            prop_assert_eq!(window.range, start..end.max(start));
        }

        #[test]
        fn anchor_holds_when_rows_above_change_height(
            heights in prop::collection::vec(1.0f32..200.0, 20..60),
            scroll_frac in 0.2f32..0.7,
            changes in prop::collection::vec((any::<usize>(), 0.0f32..400.0), 1..10),
        ) {
            let mut list = list(&heights, 300.0);
            let max = list.max_scroll_offset();
            prop_assume!(max > 10.0);
            list.set_scroll_offset(max * scroll_frac);
            let anchor = anchor_key(&list);
            let anchor_index = list.rows().index_of(RowKey(anchor)).unwrap();
            prop_assume!(anchor_index > 0);
            let before = screen_top(&list, anchor);
            for (seed, height) in changes {
                list.set_height(RowKey((seed % anchor_index) as u64), height).unwrap();
                prop_assert!((screen_top(&list, anchor) - before).abs() <= 0.5);
            }
        }
    }

    #[test]
    fn prepending_history_keeps_the_first_visible_row_in_place() {
        let mut list = list(&[30.0; 10], 100.0);
        list.set_scroll_offset(125.0);
        let before = screen_top(&list, 4);

        let history: Vec<RowKey> = (100..105).map(RowKey).collect();
        let delta = list.prepend(&history).unwrap();

        assert_eq!(delta, 150.0);
        assert_eq!(screen_top(&list, 4), before);
    }

    #[test]
    fn measuring_estimated_rows_above_keeps_the_view_still() {
        let mut list = VariableList::new(20.0, 200.0);
        for key in 0..50 {
            list.append(RowKey(key)).unwrap();
        }
        list.set_scroll_offset(400.0);
        let before = screen_top(&list, 20);

        // The overscan pulls estimated rows above the viewport into the
        // window, and each measures larger than its estimate.
        list.measure_visible(320.0, 100.0, |key, _| 30.0 + key as f32);

        assert!(list.rows().is_measured(RowKey(16)).unwrap());
        assert!((screen_top(&list, 20) - before).abs() <= 0.5);
    }

    #[test]
    fn streaming_growth_stays_pinned_to_the_bottom() {
        let mut list = list(&[40.0, 40.0], 100.0);
        for key in 2..6 {
            list.append(RowKey(key)).unwrap();
            for step in 1..20 {
                list.set_height(RowKey(key), step as f32 * 7.0).unwrap();
                assert_eq!(list.scroll_offset(), list.max_scroll_offset());
            }
        }
        assert!(list.max_scroll_offset() > 0.0);
    }

    #[test]
    fn scrolling_up_releases_the_bottom_pin() {
        let mut list = list(&[40.0; 10], 100.0);
        list.set_scroll_offset(list.max_scroll_offset() - 50.0);
        let offset = list.scroll_offset();

        list.append(RowKey(10)).unwrap();
        list.set_height(RowKey(10), 300.0).unwrap();
        list.set_height(RowKey(9), 120.0).unwrap();

        assert_eq!(list.scroll_offset(), offset);
    }

    #[test]
    fn scrolling_back_to_the_bottom_pins_again() {
        let mut list = list(&[40.0; 10], 100.0);
        list.set_scroll_offset(0.0);
        list.set_scroll_offset(list.max_scroll_offset() - 0.5);

        list.append(RowKey(10)).unwrap();
        list.set_height(RowKey(10), 300.0).unwrap();

        assert_eq!(list.scroll_offset(), list.max_scroll_offset());
    }

    #[test]
    fn scroll_to_aligns_and_clamps() {
        // Ten 100px rows in a 250px viewport: max offset 750.
        let cases = [
            (5, ScrollAlign::Top, 500.0),
            (5, ScrollAlign::Center, 425.0),
            (5, ScrollAlign::Bottom, 350.0),
            (9, ScrollAlign::Top, 750.0),
            (0, ScrollAlign::Bottom, 0.0),
        ];
        for (key, align, expected) in cases {
            let mut list = list(&[100.0; 10], 250.0);
            list.scroll_to(RowKey(key), align).unwrap();
            assert_eq!(list.scroll_offset(), expected, "{key} {align:?}");
        }
    }
}
