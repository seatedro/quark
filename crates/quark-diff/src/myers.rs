//! Myers' O(ND) difference algorithm in linear space, as git's xdiff runs
//! it: common prefix and suffix trimmed, items without a match on the other
//! side marked changed up front, the middle snake found by searching from
//! both ends, a cost cap that settles for the furthest-reaching split on
//! pathological inputs, and change groups slid down past equal items so
//! the output is canonical.
//!
//! Items are interned ids; equal ids are equal items.

use std::collections::HashMap;
use std::hash::Hash;
use std::ops::Range;

/// One run of changed items: `old` replaced by `new`. Either may be empty.
/// Runs are sorted, disjoint, and separated by equal items.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub old: Range<u32>,
    pub new: Range<u32>,
}

/// Interns items to dense ids shared by both sides.
pub(crate) fn intern<'a, T: Hash + Eq + ?Sized + 'a>(
    old: impl Iterator<Item = &'a T>,
    new: impl Iterator<Item = &'a T>,
) -> (Vec<u32>, Vec<u32>) {
    let mut ids: HashMap<&T, u32> = HashMap::new();
    let mut id = |item: &'a T| {
        let next = ids.len() as u32;
        *ids.entry(item).or_insert(next)
    };
    let old: Vec<u32> = old.map(&mut id).collect();
    let new: Vec<u32> = new.map(&mut id).collect();
    (old, new)
}

/// The changes turning `old` into `new`.
pub fn diff(old: &[u32], new: &[u32]) -> Vec<Change> {
    let mut removed = vec![false; old.len()];
    let mut added = vec![false; new.len()];
    mark(old, new, &mut removed, &mut added);
    slide(old, &mut removed);
    slide(new, &mut added);
    changes(&removed, &added)
}

/// Marks changed items. Items absent from the other side cannot be part
/// of any common subsequence, so they are marked directly and the search
/// runs on the rest, which is much shorter when the sides differ a lot.
fn mark(old: &[u32], new: &[u32], removed: &mut [bool], added: &mut [bool]) {
    let ids = old
        .iter()
        .chain(new)
        .copied()
        .max()
        .map_or(0, |m| m as usize + 1);
    let mut in_old = vec![false; ids];
    let mut in_new = vec![false; ids];
    for &id in old {
        in_old[id as usize] = true;
    }
    for &id in new {
        in_new[id as usize] = true;
    }
    let keep_old: Vec<u32> = (0..old.len() as u32)
        .filter(|&i| in_new[old[i as usize] as usize])
        .collect();
    let keep_new: Vec<u32> = (0..new.len() as u32)
        .filter(|&i| in_old[new[i as usize] as usize])
        .collect();
    for (i, &id) in old.iter().enumerate() {
        removed[i] = !in_new[id as usize];
    }
    for (i, &id) in new.iter().enumerate() {
        added[i] = !in_old[id as usize];
    }
    let a: Vec<u32> = keep_old.iter().map(|&i| old[i as usize]).collect();
    let b: Vec<u32> = keep_new.iter().map(|&i| new[i as usize]).collect();
    let mut sub_removed = vec![false; a.len()];
    let mut sub_added = vec![false; b.len()];
    Search::new(&a, &b).run(&mut sub_removed, &mut sub_added);
    for (k, &i) in keep_old.iter().enumerate() {
        removed[i as usize] = sub_removed[k];
    }
    for (k, &i) in keep_new.iter().enumerate() {
        added[i as usize] = sub_added[k];
    }
}

/// Smallest cost after which the search settles for an approximate split.
const MIN_COST_CAP: i64 = 256;

struct Search<'a> {
    a: &'a [u32],
    b: &'a [u32],
    /// Furthest x reached on each diagonal `x - y`, forward and backward,
    /// indexed by diagonal plus `offset`.
    forward: Vec<i64>,
    backward: Vec<i64>,
    offset: i64,
    cost_cap: i64,
}

struct Split {
    x: i64,
    y: i64,
    /// Whether each half must be diffed minimally (it was split exactly).
    min_lo: bool,
    min_hi: bool,
}

impl<'a> Search<'a> {
    fn new(a: &'a [u32], b: &'a [u32]) -> Self {
        let (n, m) = (a.len() as i64, b.len() as i64);
        let diagonals = (n + m + 3) as usize;
        let cost_cap = ((n + m + 3) as f64).sqrt() as i64;
        Self {
            a,
            b,
            forward: vec![0; diagonals],
            backward: vec![0; diagonals],
            offset: m + 1,
            cost_cap: cost_cap.max(MIN_COST_CAP),
        }
    }

    fn run(&mut self, removed: &mut [bool], added: &mut [bool]) {
        // An explicit stack: recursion depth can reach the edit distance.
        let mut stack = vec![(0, self.a.len() as i64, 0, self.b.len() as i64, false)];
        while let Some((mut x0, mut x1, mut y0, mut y1, need_min)) = stack.pop() {
            while x0 < x1 && y0 < y1 && self.a[x0 as usize] == self.b[y0 as usize] {
                x0 += 1;
                y0 += 1;
            }
            while x0 < x1 && y0 < y1 && self.a[x1 as usize - 1] == self.b[y1 as usize - 1] {
                x1 -= 1;
                y1 -= 1;
            }
            if x0 == x1 {
                added[y0 as usize..y1 as usize].fill(true);
            } else if y0 == y1 {
                removed[x0 as usize..x1 as usize].fill(true);
            } else {
                let split = self.split(x0, x1, y0, y1, need_min);
                stack.push((split.x, x1, split.y, y1, split.min_hi));
                stack.push((x0, split.x, y0, split.y, split.min_lo));
            }
        }
    }

    fn f(&mut self, d: i64) -> &mut i64 {
        &mut self.forward[(d + self.offset) as usize]
    }

    fn bk(&mut self, d: i64) -> &mut i64 {
        &mut self.backward[(d + self.offset) as usize]
    }

    /// The middle snake of the box `[x0, x1) x [y0, y1)`, whose corners do
    /// not match (the caller trimmed them).
    fn split(&mut self, x0: i64, x1: i64, y0: i64, y1: i64, need_min: bool) -> Split {
        let (a, b) = (self.a, self.b);
        let (dmin, dmax) = (x0 - y1, x1 - y0);
        let (fmid, bmid) = (x0 - y0, x1 - y1);
        let odd = (fmid - bmid) & 1 != 0;
        let (mut fmin, mut fmax, mut bmin, mut bmax) = (fmid, fmid, bmid, bmid);
        *self.f(fmid) = x0;
        *self.bk(bmid) = x1;
        let mut cost = 1;
        loop {
            // Extend the forward paths by one edit.
            if fmin > dmin {
                fmin -= 1;
                *self.f(fmin - 1) = -1;
            } else {
                fmin += 1;
            }
            if fmax < dmax {
                fmax += 1;
                *self.f(fmax + 1) = -1;
            } else {
                fmax -= 1;
            }
            let mut d = fmax;
            while d >= fmin {
                let (left, right) = (*self.f(d - 1), *self.f(d + 1));
                let mut x = if left >= right { left + 1 } else { right };
                let mut y = x - d;
                while x < x1 && y < y1 && a[x as usize] == b[y as usize] {
                    x += 1;
                    y += 1;
                }
                *self.f(d) = x;
                if odd && bmin <= d && d <= bmax && *self.bk(d) <= x {
                    return Split {
                        x,
                        y,
                        min_lo: true,
                        min_hi: true,
                    };
                }
                d -= 2;
            }

            // Extend the backward paths by one edit.
            if bmin > dmin {
                bmin -= 1;
                *self.bk(bmin - 1) = i64::MAX;
            } else {
                bmin += 1;
            }
            if bmax < dmax {
                bmax += 1;
                *self.bk(bmax + 1) = i64::MAX;
            } else {
                bmax -= 1;
            }
            let mut d = bmax;
            while d >= bmin {
                let (left, right) = (*self.bk(d - 1), *self.bk(d + 1));
                let mut x = if left < right { left } else { right - 1 };
                let mut y = x - d;
                while x > x0 && y > y0 && a[x as usize - 1] == b[y as usize - 1] {
                    x -= 1;
                    y -= 1;
                }
                *self.bk(d) = x;
                if !odd && fmin <= d && d <= fmax && x <= *self.f(d) {
                    return Split {
                        x,
                        y,
                        min_lo: true,
                        min_hi: true,
                    };
                }
                d -= 2;
            }

            if !need_min && cost >= self.cost_cap {
                return self.best_split(x0, x1, y0, y1, (fmin, fmax), (bmin, bmax));
            }
            cost += 1;
        }
    }

    /// Too expensive to finish: split where the forward or the backward
    /// search got furthest, whichever covers more of the box.
    fn best_split(
        &mut self,
        x0: i64,
        x1: i64,
        y0: i64,
        y1: i64,
        (fmin, fmax): (i64, i64),
        (bmin, bmax): (i64, i64),
    ) -> Split {
        let (mut fbest, mut fbest_x) = (-1, -1);
        let mut d = fmax;
        while d >= fmin {
            let mut x = (*self.f(d)).min(x1);
            let mut y = x - d;
            if y1 < y {
                x = y1 + d;
                y = y1;
            }
            if fbest < x + y {
                fbest = x + y;
                fbest_x = x;
            }
            d -= 2;
        }
        let (mut bbest, mut bbest_x) = (i64::MAX, i64::MAX);
        let mut d = bmax;
        while d >= bmin {
            let mut x = (*self.bk(d)).max(x0);
            let mut y = x - d;
            if y < y0 {
                x = y0 + d;
                y = y0;
            }
            if x + y < bbest {
                bbest = x + y;
                bbest_x = x;
            }
            d -= 2;
        }
        if (x1 + y1) - bbest < fbest - (x0 + y0) {
            Split {
                x: fbest_x,
                y: fbest - fbest_x,
                min_lo: true,
                min_hi: false,
            }
        } else {
            Split {
                x: bbest_x,
                y: bbest - bbest_x,
                min_lo: false,
                min_hi: true,
            }
        }
    }
}

/// Slides every run of changed items as far down as equal items allow, so
/// equivalent scripts come out the same: in `a b a` with one `a` removed,
/// the second goes.
fn slide(items: &[u32], changed: &mut [bool]) {
    let n = items.len();
    let mut start = 0;
    while start < n {
        if !changed[start] {
            start += 1;
            continue;
        }
        let mut end = start;
        while end < n && changed[end] {
            end += 1;
        }
        // Moving the run down one item keeps the unchanged items in
        // order: the item leaving the run's top equals the one entering
        // below it.
        while end < n && items[start] == items[end] {
            changed[start] = false;
            changed[end] = true;
            start += 1;
            end += 1;
            while end < n && changed[end] {
                end += 1;
            }
        }
        start = end;
    }
}

/// Pairs runs of removed and added items. Both sides keep the same number
/// of unchanged items, so walking them in step lines the runs up.
fn changes(removed: &[bool], added: &[bool]) -> Vec<Change> {
    let (n, m) = (removed.len(), added.len());
    let (mut i, mut j) = (0, 0);
    let mut out = Vec::new();
    while i < n || j < m {
        if (i < n && removed[i]) || (j < m && added[j]) {
            let (si, sj) = (i, j);
            while i < n && removed[i] {
                i += 1;
            }
            while j < m && added[j] {
                j += 1;
            }
            out.push(Change {
                old: si as u32..i as u32,
                new: sj as u32..j as u32,
            });
        } else {
            i += 1;
            j += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{Change, diff, intern};
    use proptest::prelude::*;

    fn chars(old: &str, new: &str) -> Vec<Change> {
        let old: Vec<String> = old.chars().map(String::from).collect();
        let new: Vec<String> = new.chars().map(String::from).collect();
        let (a, b) = intern(
            old.iter().map(|s| s.as_str()),
            new.iter().map(|s| s.as_str()),
        );
        diff(&a, &b)
    }

    /// The changes as `-old+new` runs, for table tests.
    fn script(old: &str, new: &str) -> String {
        let (o, n): (Vec<char>, Vec<char>) = (old.chars().collect(), new.chars().collect());
        chars(old, new)
            .iter()
            .map(|c| {
                let take = |s: &[char], r: &std::ops::Range<u32>| -> String {
                    s[r.start as usize..r.end as usize].iter().collect()
                };
                format!("{}:-{}+{}", c.old.start, take(&o, &c.old), take(&n, &c.new))
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn scripts_are_minimal_and_slid_down() {
        let cases = [
            ("abc", "abc", ""),
            ("", "ab", "0:-+ab"),
            ("ab", "", "0:-ab+"),
            ("abcabba", "cbabac", "0:-ab+ 3:-a+ 5:-+a 7:-+c"),
            ("aba", "ab", "2:-a+"),
            ("xaay", "xay", "2:-a+"),
            ("abc", "axc", "1:-b+x"),
        ];
        for (old, new, expected) in cases {
            assert_eq!(script(old, new), expected, "{old} -> {new}");
        }
    }

    /// Applies `changes` to `old` taking replacement items from `new`.
    fn apply(old: &[u32], new: &[u32], changes: &[Change]) -> Vec<u32> {
        let mut out = Vec::new();
        let mut at = 0;
        for c in changes {
            out.extend_from_slice(&old[at..c.old.start as usize]);
            out.extend_from_slice(&new[c.new.start as usize..c.new.end as usize]);
            at = c.old.end as usize;
        }
        out.extend_from_slice(&old[at..]);
        out
    }

    /// Length of the longest common subsequence, by dynamic programming.
    fn lcs(a: &[u32], b: &[u32]) -> usize {
        let mut row = vec![0usize; b.len() + 1];
        for &x in a {
            let mut prev = 0;
            for (j, &y) in b.iter().enumerate() {
                let keep = row[j + 1];
                row[j + 1] = if x == y {
                    prev + 1
                } else {
                    row[j + 1].max(row[j])
                };
                prev = keep;
            }
        }
        row[b.len()]
    }

    proptest! {
        #[test]
        fn changes_rebuild_new_with_a_minimal_script(
            a in prop::collection::vec(0u32..4, 0..40),
            b in prop::collection::vec(0u32..4, 0..40),
        ) {
            let changes = diff(&a, &b);
            prop_assert_eq!(apply(&a, &b, &changes), b.clone());
            let edits: u32 = changes.iter().map(|c| c.old.len() as u32 + c.new.len() as u32).sum();
            prop_assert_eq!(edits as usize, a.len() + b.len() - 2 * lcs(&a, &b));
            for pair in changes.windows(2) {
                prop_assert!(pair[0].old.end < pair[1].old.start);
            }
        }
    }
}
