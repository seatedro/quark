//! Row heights with exact prefix sums, for [`super::RowTable`].
//!
//! Most long lists are rows of one height with a few others: a diff's
//! lines between its headers, a transcript of estimated rows. Those keep
//! only the rows that differ ([`Heights::Sparse`]), which builds and
//! stores millions of rows in the space of their exceptions. A list whose
//! rows vary (or that changes much) keeps every height plus a Fenwick tree
//! ([`Heights::Dense`]). Both answer in fixed-point units (see
//! [`quark::fenwick`]), so sums are exact in either form.

use quark::fenwick::{Fenwick, px_to_units};

use super::variable::RowIntegrityError;

/// A sparse table with more exceptions than this turns dense before it
/// changes: each change costs O(exceptions) there.
const SPARSE_MUTABLE: usize = 1 << 14;

#[derive(Debug, Clone)]
pub(super) enum Heights {
    Sparse(Sparse),
    Dense { heights: Vec<f32>, tree: Fenwick },
}

/// `len` rows `base` high, except those listed in `at`.
#[derive(Debug, Clone)]
pub(super) struct Sparse {
    len: usize,
    base: f32,
    base_units: i64,
    /// Rows whose height is not `base`, ascending.
    at: Vec<usize>,
    heights: Vec<f32>,
    /// `deltas[k]` sums `units(heights[j]) - base_units` over `j < k`; one
    /// longer than `at`.
    deltas: Vec<i64>,
}

impl Sparse {
    fn new(base: f32) -> Self {
        Self {
            len: 0,
            base,
            base_units: px_to_units(base),
            at: Vec::new(),
            heights: Vec::new(),
            deltas: vec![0],
        }
    }

    /// Exceptions before row `count`.
    fn before(&self, count: usize) -> usize {
        self.at.partition_point(|&j| j < count)
    }

    fn prefix(&self, count: usize) -> i64 {
        let count = count.min(self.len);
        count as i64 * self.base_units + self.deltas[self.before(count)]
    }

    fn height(&self, index: usize) -> f32 {
        match self.at.binary_search(&index) {
            Ok(k) => self.heights[k],
            Err(_) => self.base,
        }
    }

    /// Recomputes `deltas` from exception `k` on.
    fn resum_from(&mut self, k: usize) {
        self.deltas.truncate(k + 1);
        for j in k..self.at.len() {
            let next = self.deltas[j] + px_to_units(self.heights[j]) - self.base_units;
            self.deltas.push(next);
        }
    }

    /// Appends a row in O(1).
    fn push(&mut self, height: f32) {
        if height.to_bits() != self.base.to_bits() {
            self.at.push(self.len);
            self.heights.push(height);
            let last = *self.deltas.last().expect("deltas start at zero");
            self.deltas
                .push(last + px_to_units(height) - self.base_units);
        }
        self.len += 1;
    }

    fn set(&mut self, index: usize, height: f32) {
        let base = height.to_bits() == self.base.to_bits();
        let k = match self.at.binary_search(&index) {
            Ok(k) if base => {
                self.at.remove(k);
                self.heights.remove(k);
                k
            }
            Ok(k) => {
                self.heights[k] = height;
                k
            }
            Err(_) if base => return,
            Err(k) => {
                self.at.insert(k, index);
                self.heights.insert(k, height);
                k
            }
        };
        self.resum_from(k);
    }

    fn dense(&self) -> Vec<f32> {
        let mut heights = vec![self.base; self.len];
        for (&i, &h) in self.at.iter().zip(&self.heights) {
            heights[i] = h;
        }
        heights
    }
}

impl Heights {
    /// Rows of `heights` (already sanitized), kept sparse around `base`
    /// while at most a quarter of them differ from it.
    pub(super) fn build(base: f32, heights: impl IntoIterator<Item = f32>) -> Self {
        let heights = heights.into_iter();
        // Rows to come, so a burst of exceptions at the start of a long
        // list does not decide for all of it.
        let expected = heights.size_hint().0;
        let mut sparse = Sparse::new(base);
        let mut dense: Option<Vec<f32>> = None;
        for height in heights {
            match &mut dense {
                Some(dense) => dense.push(height),
                None => {
                    sparse.push(height);
                    // Past a quarter (and a floor, so short lists stay
                    // sparse) the exceptions cost more than the rows.
                    let k = sparse.at.len();
                    if k > 64 && k * 4 > sparse.len.max(expected) {
                        let mut heights = sparse.dense();
                        heights.reserve(expected.saturating_sub(heights.len()));
                        dense = Some(heights);
                    }
                }
            }
        }
        match dense {
            Some(heights) => Self::dense_from(heights),
            None => Self::Sparse(sparse),
        }
    }

    fn dense_from(heights: Vec<f32>) -> Self {
        let tree = Fenwick::build(heights.iter().map(|&h| px_to_units(h)));
        Self::Dense { heights, tree }
    }

    pub(super) fn len(&self) -> usize {
        match self {
            Self::Sparse(s) => s.len,
            Self::Dense { heights, .. } => heights.len(),
        }
    }

    pub(super) fn height(&self, index: usize) -> f32 {
        assert!(index < self.len(), "row {index} of {}", self.len());
        match self {
            Self::Sparse(s) => s.height(index),
            Self::Dense { heights, .. } => heights[index],
        }
    }

    /// Sum of the first `count` heights, in units.
    pub(super) fn prefix(&self, count: usize) -> i64 {
        match self {
            Self::Sparse(s) => s.prefix(count),
            Self::Dense { tree, .. } => tree.prefix(count),
        }
    }

    pub(super) fn total(&self) -> i64 {
        self.prefix(self.len())
    }

    /// Largest `count` with `prefix(count) <= target` (`< target` when
    /// `strict`), as [`Fenwick::search`].
    pub(super) fn search(&self, target: i64, strict: bool) -> usize {
        let s = match self {
            Self::Sparse(s) => s,
            Self::Dense { tree, .. } => return tree.search(target, strict),
        };
        let fits = |count: usize| {
            let p = s.prefix(count);
            if strict { p < target } else { p <= target }
        };
        // Prefixes never fall, so the counts that fit run from 0 up; count
        // 0 counts as found, as in the tree.
        let (mut lo, mut hi) = (0, s.len);
        while lo < hi {
            let mid = lo + (hi - lo).div_ceil(2);
            if fits(mid) {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        lo
    }

    /// Sets row `index`'s height (sanitized). O(log n) dense; O(k) for k
    /// exceptions sparse.
    pub(super) fn set(&mut self, index: usize, height: f32) {
        if let Self::Sparse(s) = self
            && s.at.len() >= SPARSE_MUTABLE
        {
            self.make_dense();
        }
        match self {
            Self::Sparse(s) => s.set(index, height),
            Self::Dense { heights, tree } => {
                tree.add(index, px_to_units(height) - px_to_units(heights[index]));
                heights[index] = height;
            }
        }
    }

    /// Appends a row: O(1) sparse, O(log n) amortized dense.
    pub(super) fn push(&mut self, height: f32) {
        match self {
            Self::Sparse(s) if s.at.len() < SPARSE_MUTABLE => s.push(height),
            Self::Sparse(_) => {
                self.make_dense();
                self.push(height);
            }
            Self::Dense { heights, tree } => {
                heights.push(height);
                tree.push(px_to_units(height));
            }
        }
    }

    /// Every height, in a dense table, for changes that move rows.
    pub(super) fn dense_mut(&mut self) -> &mut Vec<f32> {
        self.make_dense();
        match self {
            Self::Dense { heights, .. } => heights,
            Self::Sparse(_) => unreachable!("just made dense"),
        }
    }

    /// Rebuilds the sums after [`Self::dense_mut`] moved rows. O(n).
    pub(super) fn rebuild(&mut self) {
        if let Self::Dense { heights, tree } = self {
            *tree = Fenwick::build(heights.iter().map(|&h| px_to_units(h)));
        }
    }

    fn make_dense(&mut self) {
        if let Self::Sparse(s) = self {
            *self = Self::dense_from(s.dense());
        }
    }

    /// Every height valid and every sum equal to a fresh one. O(n).
    pub(super) fn verify(&self) -> Result<(), RowIntegrityError> {
        let valid = |index: usize, height: f32| {
            if height.is_finite() && height >= 0.0 {
                Ok(())
            } else {
                Err(RowIntegrityError::InvalidHeight { index, height })
            }
        };
        match self {
            Self::Sparse(s) => {
                for (column, len) in [("heights", s.heights.len()), ("deltas", s.deltas.len() - 1)]
                {
                    if len != s.at.len() {
                        return Err(RowIntegrityError::ColumnLength {
                            column,
                            len,
                            expected: s.at.len(),
                        });
                    }
                }
                for (k, (&index, &height)) in s.at.iter().zip(&s.heights).enumerate() {
                    let ordered = k == 0 || s.at[k - 1] < index;
                    if !ordered || index >= s.len || height.to_bits() == s.base.to_bits() {
                        return Err(RowIntegrityError::SparseRow { index });
                    }
                    valid(index, height)?;
                }
                let mut sum = 0;
                for (k, &height) in s.heights.iter().enumerate() {
                    if s.deltas[k] != sum {
                        return Err(RowIntegrityError::FenwickMismatch);
                    }
                    sum += px_to_units(height) - s.base_units;
                }
                if s.deltas[s.at.len()] != sum {
                    return Err(RowIntegrityError::FenwickMismatch);
                }
            }
            Self::Dense { heights, tree } => {
                for (index, &height) in heights.iter().enumerate() {
                    valid(index, height)?;
                }
                if tree.len() != heights.len() {
                    return Err(RowIntegrityError::ColumnLength {
                        column: "tree",
                        len: tree.len(),
                        expected: heights.len(),
                    });
                }
                // Integer node sums depend only on the leaves, so a fresh
                // O(n) build must equal the incrementally updated tree.
                if Fenwick::build(heights.iter().map(|&h| px_to_units(h))) != *tree {
                    return Err(RowIntegrityError::FenwickMismatch);
                }
            }
        }
        Ok(())
    }

    /// Row `index`'s height valid and its sum consistent. O(log n).
    pub(super) fn verify_row(&self, index: usize) -> Result<(), RowIntegrityError> {
        let height = self.height(index);
        if !(height.is_finite() && height >= 0.0) {
            return Err(RowIntegrityError::InvalidHeight { index, height });
        }
        if self.prefix(index + 1) - self.prefix(index) != px_to_units(height) {
            return Err(RowIntegrityError::FenwickMismatch);
        }
        Ok(())
    }
}
