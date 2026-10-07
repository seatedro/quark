//! Fenwick (binary indexed) tree over integer row heights.
//!
//! Heights are stored in fixed-point units (see `UNITS_PER_PX`) so prefix
//! sums are exact: incremental `add` and an O(n) `build` always agree, and
//! the integrity check can compare trees for equality instead of guessing a
//! float tolerance.

/// Fixed-point resolution of row heights. 1/256 px is far below what a
/// screen can show, and i64 sums stay exact up to ~3.6e16 px.
pub(crate) const UNITS_PER_PX: f64 = 256.0;

pub(crate) fn px_to_units(px: f32) -> i64 {
    if px.is_finite() && px > 0.0 {
        (f64::from(px) * UNITS_PER_PX).round() as i64
    } else {
        0
    }
}

pub(crate) fn units_to_px(units: i64) -> f32 {
    (units as f64 / UNITS_PER_PX) as f32
}

/// `tree[i]` (1-based) holds the sum of values in `(i - lowbit(i), i]`.
/// `tree[0]` is an unused sentinel so indices match the textbook layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Fenwick {
    tree: Vec<i64>,
}

impl Default for Fenwick {
    fn default() -> Self {
        Self { tree: vec![0] }
    }
}

fn lowbit(i: usize) -> usize {
    i & i.wrapping_neg()
}

impl Fenwick {
    /// O(n): each node pushes its sum into its parent once.
    pub(crate) fn build(values: impl IntoIterator<Item = i64>) -> Self {
        let mut tree = vec![0];
        tree.extend(values);
        let n = tree.len() - 1;
        for i in 1..=n {
            let parent = i + lowbit(i);
            if parent <= n {
                tree[parent] += tree[i];
            }
        }
        Self { tree }
    }

    pub(crate) fn len(&self) -> usize {
        self.tree.len() - 1
    }

    /// Appends a value in O(log n): the new node covers
    /// `(i - lowbit(i), i]`, whose earlier part is a difference of prefixes.
    pub(crate) fn push(&mut self, value: i64) {
        let i = self.tree.len();
        let covered = self.prefix(i - 1) - self.prefix(i - lowbit(i));
        self.tree.push(value + covered);
    }

    /// Adds `delta` to the value at 0-based `index`.
    pub(crate) fn add(&mut self, index: usize, delta: i64) {
        let mut i = index + 1;
        while i < self.tree.len() {
            self.tree[i] += delta;
            i += lowbit(i);
        }
    }

    /// Sum of the first `count` values.
    pub(crate) fn prefix(&self, count: usize) -> i64 {
        let mut i = count.min(self.len());
        let mut sum = 0;
        while i > 0 {
            sum += self.tree[i];
            i -= lowbit(i);
        }
        sum
    }

    pub(crate) fn total(&self) -> i64 {
        self.prefix(self.len())
    }

    /// Largest `count` with `prefix(count) <= target` (or `< target` when
    /// `strict`), by binary lifting. Requires non-negative values.
    pub(crate) fn search(&self, target: i64, strict: bool) -> usize {
        let n = self.len();
        if n == 0 {
            return 0;
        }
        let mut pos = 0;
        let mut remaining = target;
        let mut step = 1 << (usize::BITS - 1 - n.leading_zeros());
        while step > 0 {
            let next = pos + step;
            if next <= n {
                let node = self.tree[next];
                let fits = if strict {
                    node < remaining
                } else {
                    node <= remaining
                };
                if fits {
                    pos = next;
                    remaining -= node;
                }
            }
            step >>= 1;
        }
        pos
    }
}
