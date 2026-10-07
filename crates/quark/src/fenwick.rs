//! Fenwick (binary indexed) tree over integer row heights.
//!
//! Heights are stored in fixed-point units (see `UNITS_PER_PX`) so prefix
//! sums are exact: incremental `add` and an O(n) `build` always agree, and
//! the integrity check can compare trees for equality instead of guessing a
//! float tolerance.

/// Fixed-point resolution of row heights. 1/256 px is far below what a
/// screen can show, and i64 sums stay exact up to ~3.6e16 px.
pub const UNITS_PER_PX: f64 = 256.0;

pub fn px_to_units(px: f32) -> i64 {
    if px.is_finite() && px > 0.0 {
        (f64::from(px) * UNITS_PER_PX).round() as i64
    } else {
        0
    }
}

pub fn units_to_px(units: i64) -> f32 {
    (units as f64 / UNITS_PER_PX) as f32
}

/// `tree[i]` (1-based) holds the sum of values in `(i - lowbit(i), i]`.
/// `tree[0]` is an unused sentinel so indices match the textbook layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fenwick {
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
    pub fn build(values: impl IntoIterator<Item = i64>) -> Self {
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

    pub fn len(&self) -> usize {
        self.tree.len() - 1
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Appends a value in O(log n): the new node covers
    /// `(i - lowbit(i), i]`, whose earlier part is a difference of prefixes.
    pub fn push(&mut self, value: i64) {
        let i = self.tree.len();
        let covered = self.prefix(i - 1) - self.prefix(i - lowbit(i));
        self.tree.push(value + covered);
    }

    /// Adds `delta` to the value at 0-based `index`.
    pub fn add(&mut self, index: usize, delta: i64) {
        let mut i = index + 1;
        while i < self.tree.len() {
            self.tree[i] += delta;
            i += lowbit(i);
        }
    }

    /// Sum of the first `count` values.
    pub fn prefix(&self, count: usize) -> i64 {
        let mut i = count.min(self.len());
        let mut sum = 0;
        while i > 0 {
            sum += self.tree[i];
            i -= lowbit(i);
        }
        sum
    }

    pub fn total(&self) -> i64 {
        self.prefix(self.len())
    }

    /// Largest `count` with `prefix(count) <= target` (or `< target` when
    /// `strict`), by binary lifting. Requires non-negative values.
    pub fn search(&self, target: i64, strict: bool) -> usize {
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

/// Bounded model checking; run with `cargo kani -p quark`.
#[cfg(kani)]
mod verification {
    use super::*;

    const N: usize = 4;

    /// Non-negative and small so sums cannot overflow; heights are never
    /// negative in practice.
    fn any_values() -> [i64; N] {
        let values: [i64; N] = kani::any();
        for v in values {
            kani::assume((0..1 << 20).contains(&v));
        }
        values
    }

    /// Element-wise so CBMC unrolls a short loop instead of a byte-wise
    /// `memcmp` over the whole Vec, which needs a much larger unwind bound.
    fn same_tree(a: &Fenwick, b: &Fenwick) -> bool {
        if a.tree.len() != b.tree.len() {
            return false;
        }
        let mut i = 0;
        while i < a.tree.len() {
            if a.tree[i] != b.tree[i] {
                return false;
            }
            i += 1;
        }
        true
    }

    #[kani::proof]
    #[kani::unwind(6)]
    fn fenwick_push_matches_build_and_prefix_matches_naive_sum() {
        // A concrete length: a symbolic one makes the Vec's growth symbolic
        // and CBMC ran out of memory past 16 GB. `prefix` still covers
        // every shorter count.
        let values = any_values();
        let built = Fenwick::build(values);
        let mut pushed = Fenwick::default();
        for v in values {
            pushed.push(v);
        }
        assert!(same_tree(&built, &pushed));
        let mut sum = 0;
        for count in 0..=N {
            assert!(built.prefix(count) == sum);
            if count < N {
                sum += values[count];
            }
        }
    }

    #[kani::proof]
    #[kani::unwind(6)]
    fn fenwick_add_matches_rebuild() {
        let mut values = any_values();
        let index: usize = kani::any();
        let delta: i64 = kani::any();
        kani::assume(index < N);
        // Checked so the assumption itself cannot overflow on an arbitrary delta.
        let updated = values[index].checked_add(delta);
        kani::assume(updated.is_some_and(|v| (0..1 << 20).contains(&v)));
        let mut tree = Fenwick::build(values);
        tree.add(index, delta);
        values[index] += delta;
        assert!(same_tree(&tree, &Fenwick::build(values)));
    }

    #[kani::proof]
    #[kani::unwind(6)]
    fn fenwick_search_returns_largest_fitting_prefix() {
        let tree = Fenwick::build(any_values());
        let target: i64 = kani::any();
        kani::assume((-1..1 << 23).contains(&target));
        let strict: bool = kani::any();
        let fits = |count: usize| {
            let p = tree.prefix(count);
            if strict { p < target } else { p <= target }
        };
        let found = tree.search(target, strict);
        assert!(found <= N);
        // Prefixes are monotonic, so `found` fitting and `found + 1` not
        // fitting pins it as the largest (count 0 always counts as found).
        assert!(found == 0 || fits(found));
        assert!(found == N || !fits(found + 1));
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[derive(Debug, Clone)]
    enum Op {
        Push(i64),
        /// Index and new value, applied as a delta so values stay
        /// non-negative as `search` requires.
        Set(usize, i64),
    }

    fn op() -> impl Strategy<Value = Op> {
        prop_oneof![
            (0..1_000i64).prop_map(Op::Push),
            (0..64usize, 0..1_000i64).prop_map(|(i, v)| Op::Set(i, v)),
        ]
    }

    /// Largest `count` with `sum(model[..count]) <= target` (`<` if strict).
    fn model_search(model: &[i64], target: i64, strict: bool) -> usize {
        let mut sum = 0;
        let mut found = 0;
        for (i, v) in model.iter().enumerate() {
            sum += v;
            if (strict && sum < target) || (!strict && sum <= target) {
                found = i + 1;
            } else {
                break;
            }
        }
        found
    }

    proptest! {
        // Catches lowbit slips in `push`, `add`, `prefix`, and the binary
        // lifting in `search`, which would misplace rows in virtual lists.
        #[test]
        fn fenwick_agrees_with_vec_model(
            ops in prop::collection::vec(op(), 0..40),
            targets in prop::collection::vec(-1..20_000i64, 4),
        ) {
            let mut tree = Fenwick::default();
            let mut model: Vec<i64> = Vec::new();
            for op in ops {
                match op {
                    Op::Push(v) => {
                        tree.push(v);
                        model.push(v);
                    }
                    Op::Set(i, v) if !model.is_empty() => {
                        let i = i % model.len();
                        tree.add(i, v - model[i]);
                        model[i] = v;
                    }
                    Op::Set(..) => {}
                }
                prop_assert_eq!(&tree, &Fenwick::build(model.iter().copied()));
            }
            prop_assert_eq!(tree.len(), model.len());
            for count in 0..=model.len() + 1 {
                let sum: i64 = model.iter().take(count).sum();
                prop_assert_eq!(tree.prefix(count), sum, "prefix({})", count);
            }
            for target in targets {
                for strict in [false, true] {
                    prop_assert_eq!(
                        tree.search(target, strict),
                        model_search(&model, target, strict),
                        "search({}, {})", target, strict
                    );
                }
            }
        }
    }
}
