//! Document-wide text selection over long, virtualized, streaming documents.
//!
//! Selection endpoints name a block by its stable [`BlockKey`] and a UTF-8
//! byte offset into that block's text in the document model. Nothing here
//! refers to layouts or materialized rows, so a selection survives rows being
//! scrolled out, history being prepended, and text streaming into the last
//! block.
//!
//! Byte offsets are not required to be in range or on a char boundary. They
//! are clamped when text is read: offsets past the end mean "end of block",
//! and offsets inside a multibyte char round down to that char's start.
//!
//! Block removal rule (see [`Selection::after_remove`]): the selection shrinks
//! inward. An endpoint in the removed block that was the start moves to byte 0
//! of the next surviving block; one that was the end moves to the end of the
//! previous surviving block. If both endpoints were in the removed block, the
//! selection collapses at byte 0 of the next surviving block, or at the end of
//! the previous one when the removed block was last.

use std::cell::Cell;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

thread_local! {
    static INTEGRITY_STEPS: Cell<u64> = const { Cell::new(0) };
}

/// Entries visited by integrity checks on this thread so far. Debug checks
/// report their work here so a test can tell a linear load from a
/// quadratic one without timing it; release builds never add to it.
pub fn integrity_steps() -> u64 {
    INTEGRITY_STEPS.with(Cell::get)
}

/// Adds `n` to [`integrity_steps`]. For integrity checks in other crates.
#[doc(hidden)]
pub fn count_integrity_steps(n: usize) {
    INTEGRITY_STEPS.with(|steps| steps.set(steps.get().saturating_add(n as u64)));
}

/// Whether mutations run the full O(n) integrity check: debug builds with
/// the `integrity-checks` feature. Without it they check only what they
/// touched, so debug builds stay linear on large documents.
pub const FULL_INTEGRITY_CHECKS: bool = cfg!(all(debug_assertions, feature = "integrity-checks"));

/// Stable identity of a text block. Never an index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BlockKey(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SelectionPoint {
    pub block: BlockKey,
    pub byte: usize,
}

impl SelectionPoint {
    pub fn new(block: BlockKey, byte: usize) -> Self {
        Self { block, byte }
    }
}

/// `anchor` is where the selection started, `focus` is where it currently
/// ends; `focus` may precede `anchor` in document order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Selection {
    pub anchor: SelectionPoint,
    pub focus: SelectionPoint,
}

impl Selection {
    pub fn new(anchor: SelectionPoint, focus: SelectionPoint) -> Self {
        Self { anchor, focus }
    }

    pub fn collapsed(point: SelectionPoint) -> Self {
        Self {
            anchor: point,
            focus: point,
        }
    }

    pub fn is_collapsed(&self) -> bool {
        self.anchor == self.focus
    }

    /// `(start, end)` in document order, or `None` if either block is not in
    /// `order`.
    pub fn ordered(&self, order: &BlockOrder) -> Option<(SelectionPoint, SelectionPoint)> {
        let a = order.position(self.anchor.block)?;
        let f = order.position(self.focus.block)?;
        let anchor_first = match a.cmp(&f) {
            Ordering::Less => true,
            Ordering::Greater => false,
            Ordering::Equal => self.anchor.byte <= self.focus.byte,
        };
        Some(if anchor_first {
            (self.anchor, self.focus)
        } else {
            (self.focus, self.anchor)
        })
    }

    /// Repairs the selection after `removed` was taken out of `order`.
    ///
    /// `former_pos` is the value returned by [`BlockOrder::remove`]. `source`
    /// supplies the length of the previous surviving block when an end point
    /// has to move there. Returns `None` when `order` is now empty or the
    /// selection references another block that is also missing.
    pub fn after_remove<S: SelectionText + ?Sized>(
        &self,
        removed: BlockKey,
        former_pos: u32,
        order: &BlockOrder,
        source: &S,
    ) -> Option<Selection> {
        let anchor_gone = self.anchor.block == removed;
        let focus_gone = self.focus.block == removed;
        if !anchor_gone && !focus_gone {
            return Some(*self);
        }

        // The block now at `former_pos` is the next survivor; the one before
        // it is the previous survivor.
        let next = order.keys.get(former_pos as usize).copied();
        let prev = former_pos
            .checked_sub(1)
            .and_then(|p| order.keys.get(p as usize).copied());
        let start_of_next = next.map(|k| SelectionPoint::new(k, 0));
        let end_of_prev = prev.map(|k| SelectionPoint::new(k, block_len(source, k)));

        if anchor_gone && focus_gone {
            return start_of_next.or(end_of_prev).map(Selection::collapsed);
        }

        let kept = if anchor_gone { self.focus } else { self.anchor };
        let kept_pos = order.position(kept.block)?;
        // Surviving positions at or after `former_pos` were after the removed
        // block before removal.
        let gone_was_start = former_pos <= kept_pos;
        let moved = if gone_was_start {
            start_of_next?
        } else {
            end_of_prev?
        };
        Some(if anchor_gone {
            Selection::new(moved, kept)
        } else {
            Selection::new(kept, moved)
        })
    }
}

fn block_len<S: SelectionText + ?Sized>(source: &S, key: BlockKey) -> usize {
    source.text(key).map_or(usize::MAX, str::len)
}

/// Document order of blocks. `keys[i]` is the block at position `i`;
/// `index` is its inverse and is kept exactly in sync.
#[derive(Debug, Clone, Default)]
pub struct BlockOrder {
    keys: Vec<BlockKey>,
    index: HashMap<BlockKey, u32>,
}

impl BlockOrder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.keys.len()
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    pub fn keys(&self) -> &[BlockKey] {
        &self.keys
    }

    pub fn contains(&self, key: BlockKey) -> bool {
        self.index.contains_key(&key)
    }

    pub fn position(&self, key: BlockKey) -> Option<u32> {
        self.index.get(&key).copied()
    }

    /// Appends `key` at the end. Returns `false` if it is already present.
    pub fn append(&mut self, key: BlockKey) -> bool {
        if self.index.contains_key(&key) {
            return false;
        }
        self.index.insert(key, self.keys.len() as u32);
        self.keys.push(key);
        self.debug_check_key(key);
        true
    }

    /// Appends `keys` (in their given order) after the current last block.
    /// Keys already present, or repeated in `keys`, are skipped. Indexes and
    /// checks once per batch. Returns how many were added.
    pub fn extend(&mut self, keys: impl IntoIterator<Item = BlockKey>) -> usize {
        let start = self.keys.len();
        for key in keys {
            if let std::collections::hash_map::Entry::Vacant(slot) = self.index.entry(key) {
                slot.insert(self.keys.len() as u32);
                self.keys.push(key);
            }
        }
        if self.keys.len() > start {
            debug_assert_eq!(self.verify_integrity(), Ok(()));
        }
        self.keys.len() - start
    }

    /// Prepends `keys` (in their given order) before the current first block.
    /// Keys already present are skipped. Rebuilds the index once per batch.
    pub fn prepend(&mut self, keys: impl IntoIterator<Item = BlockKey>) -> usize {
        let mut fresh: Vec<BlockKey> = Vec::new();
        let mut seen: HashSet<BlockKey> = HashSet::new();
        for key in keys {
            if !self.index.contains_key(&key) && seen.insert(key) {
                fresh.push(key);
            }
        }
        if fresh.is_empty() {
            return 0;
        }
        let added = fresh.len();
        fresh.extend_from_slice(&self.keys);
        self.keys = fresh;
        self.reindex_from(0);
        debug_assert_eq!(self.verify_integrity(), Ok(()));
        added
    }

    /// Inserts `key` right after `after`. Returns `false` if `after` is
    /// missing or `key` is already present.
    pub fn insert_after(&mut self, after: BlockKey, key: BlockKey) -> bool {
        if self.index.contains_key(&key) {
            return false;
        }
        let Some(pos) = self.position(after) else {
            return false;
        };
        let at = pos as usize + 1;
        self.keys.insert(at, key);
        self.reindex_from(at);
        debug_assert_eq!(self.verify_integrity(), Ok(()));
        true
    }

    /// Removes `key`, returning the position it occupied. Pass that position
    /// to [`Selection::after_remove`].
    pub fn remove(&mut self, key: BlockKey) -> Option<u32> {
        let pos = self.index.remove(&key)?;
        self.keys.remove(pos as usize);
        self.reindex_from(pos as usize);
        debug_assert_eq!(self.verify_integrity(), Ok(()));
        Some(pos)
    }

    /// Document order of two blocks, or `None` if either is missing.
    pub fn compare(&self, a: BlockKey, b: BlockKey) -> Option<Ordering> {
        Some(self.position(a)?.cmp(&self.position(b)?))
    }

    /// Blocks from `a` through `b` inclusive. Empty if either is missing or
    /// `a` comes after `b`.
    pub fn range(&self, a: BlockKey, b: BlockKey) -> &[BlockKey] {
        match (self.position(a), self.position(b)) {
            (Some(a), Some(b)) if a <= b => &self.keys[a as usize..=b as usize],
            _ => &[],
        }
    }

    /// Checks the entries for `key` only: it sits where `index` says and the
    /// two sides have the same length. O(1); single-key mutations call it
    /// through `debug_assert!`.
    pub fn verify_key(&self, key: BlockKey) -> Result<(), IntegrityError> {
        count_integrity_steps(1);
        if self.index.len() != self.keys.len() {
            return Err(IntegrityError::IndexLength {
                keys: self.keys.len(),
                index: self.index.len(),
            });
        }
        match self.index.get(&key) {
            Some(&pos) if self.keys.get(pos as usize) == Some(&key) => Ok(()),
            found => Err(IntegrityError::Position {
                key,
                position: self
                    .keys
                    .iter()
                    .position(|k| *k == key)
                    .unwrap_or(usize::MAX),
                index: found.copied(),
            }),
        }
    }

    fn debug_check_key(&self, key: BlockKey) {
        if FULL_INTEGRITY_CHECKS {
            debug_assert_eq!(self.verify_integrity(), Ok(()));
        } else {
            debug_assert_eq!(self.verify_key(key), Ok(()));
        }
    }

    /// Checks that `index` is exactly the inverse of `keys`. Batch and O(n)
    /// mutations call this through `debug_assert!`, so release builds skip
    /// it. O(n).
    pub fn verify_integrity(&self) -> Result<(), IntegrityError> {
        count_integrity_steps(self.keys.len());
        if self.keys.len() > u32::MAX as usize {
            return Err(IntegrityError::TooManyBlocks(self.keys.len()));
        }
        if self.index.len() != self.keys.len() {
            return Err(IntegrityError::IndexLength {
                keys: self.keys.len(),
                index: self.index.len(),
            });
        }
        for (i, key) in self.keys.iter().enumerate() {
            match self.index.get(key) {
                Some(&pos) if pos as usize == i => {}
                // `index` holds one position per key, so a second
                // occurrence of a key in `keys` lands here.
                Some(&pos) if self.keys.get(pos as usize) == Some(key) => {
                    return Err(IntegrityError::DuplicateKey(*key));
                }
                found => {
                    return Err(IntegrityError::Position {
                        key: *key,
                        position: i,
                        index: found.copied(),
                    });
                }
            }
        }
        Ok(())
    }

    fn reindex_from(&mut self, start: usize) {
        for (i, key) in self.keys.iter().enumerate().skip(start) {
            self.index.insert(*key, i as u32);
        }
    }
}

/// A broken [`BlockOrder`] invariant, reported by
/// [`BlockOrder::verify_integrity`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntegrityError {
    TooManyBlocks(usize),
    IndexLength {
        keys: usize,
        index: usize,
    },
    DuplicateKey(BlockKey),
    /// `keys[position] == key` but `index[key]` is `index`.
    Position {
        key: BlockKey,
        position: usize,
        index: Option<u32>,
    },
}

impl std::fmt::Display for IntegrityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooManyBlocks(n) => write!(f, "{n} blocks exceed u32 positions"),
            Self::IndexLength { keys, index } => {
                write!(f, "{keys} keys but {index} index entries")
            }
            Self::DuplicateKey(key) => write!(f, "{key:?} appears more than once"),
            Self::Position {
                key,
                position,
                index,
            } => write!(f, "{key:?} is at {position} but indexed at {index:?}"),
        }
    }
}

impl std::error::Error for IntegrityError {}

/// Supplies block text from the document model by key.
pub trait SelectionText {
    fn text(&self, key: BlockKey) -> Option<&str>;
}

impl SelectionText for HashMap<BlockKey, String> {
    fn text(&self, key: BlockKey) -> Option<&str> {
        self.get(&key).map(String::as_str)
    }
}

/// Largest char boundary `<= byte`, clamped to `text.len()`.
fn floor_boundary(text: &str, byte: usize) -> usize {
    if byte >= text.len() {
        return text.len();
    }
    let mut i = byte;
    while !text.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Joins the selected text across blocks with `separator` between blocks.
///
/// Blocks without text in `source` are skipped without a separator. Returns
/// an empty string for a collapsed selection or one whose endpoints are not
/// in `order`.
pub fn copy_text<S: SelectionText + ?Sized>(
    selection: &Selection,
    order: &BlockOrder,
    source: &S,
    separator: &str,
) -> String {
    let mut out = String::new();
    let Some((start, end)) = selection.ordered(order) else {
        return out;
    };
    if start == end {
        return out;
    }
    let blocks = order.range(start.block, end.block);
    let last = blocks.len().saturating_sub(1);
    let mut first_piece = true;
    for (i, key) in blocks.iter().enumerate() {
        let Some(text) = source.text(*key) else {
            continue;
        };
        let from = if i == 0 {
            floor_boundary(text, start.byte)
        } else {
            0
        };
        let to = if i == last {
            floor_boundary(text, end.byte)
        } else {
            text.len()
        };
        if !first_piece {
            out.push_str(separator);
        }
        first_piece = false;
        if from < to {
            out.push_str(&text[from..to]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn k(n: u64) -> BlockKey {
        BlockKey(n)
    }

    fn p(block: u64, byte: usize) -> SelectionPoint {
        SelectionPoint::new(k(block), byte)
    }

    fn doc(blocks: &[(u64, &str)]) -> (BlockOrder, HashMap<BlockKey, String>) {
        let mut order = BlockOrder::new();
        let mut text = HashMap::new();
        for (key, body) in blocks {
            order.append(k(*key));
            text.insert(k(*key), (*body).to_owned());
        }
        (order, text)
    }

    use crate::test_support::proptest_config as config;

    /// Mixes 1-, 2-, 3- and 4-byte chars and a combining mark so arbitrary
    /// byte offsets land inside multibyte chars.
    const PIECES: &[&str] = &["a", "b", " ", "\u{e9}", "\u{65e5}", "\u{1f600}", "\u{301}"];

    fn block_text() -> impl Strategy<Value = String> {
        prop::collection::vec(prop::sample::select(PIECES), 0..8).prop_map(|v| v.concat())
    }

    #[derive(Debug, Clone)]
    enum Op {
        Append(u64),
        Extend(Vec<u64>),
        Prepend(Vec<u64>),
        InsertAfter(u64, u64),
        Remove(u64),
    }

    const KEYS: u64 = 10;

    fn op() -> impl Strategy<Value = Op> {
        let key = 0..KEYS;
        prop_oneof![
            key.clone().prop_map(Op::Append),
            prop::collection::vec(key.clone(), 0..5).prop_map(Op::Extend),
            prop::collection::vec(key.clone(), 0..5).prop_map(Op::Prepend),
            (key.clone(), key.clone()).prop_map(|(a, b)| Op::InsertAfter(a, b)),
            key.prop_map(Op::Remove),
        ]
    }

    proptest! {
        #![proptest_config(config(256))]

        // Catches index drift after batch prepends and mid-list inserts or
        // removals, which would misorder selections spanning those blocks.
        #[test]
        fn block_order_arbitrary_ops_agree_with_vec_model(
            ops in prop::collection::vec(op(), 0..24),
        ) {
            let mut order = BlockOrder::new();
            let mut model: Vec<u64> = Vec::new();
            for op in ops {
                match op {
                    Op::Append(key) => {
                        let fresh = !model.contains(&key);
                        if fresh {
                            model.push(key);
                        }
                        prop_assert_eq!(order.append(k(key)), fresh);
                    }
                    Op::Extend(keys) => {
                        let before = model.len();
                        for key in &keys {
                            if !model.contains(key) {
                                model.push(*key);
                            }
                        }
                        prop_assert_eq!(order.extend(keys.into_iter().map(k)), model.len() - before);
                    }
                    Op::Prepend(keys) => {
                        let mut fresh: Vec<u64> = Vec::new();
                        for key in &keys {
                            if !model.contains(key) && !fresh.contains(key) {
                                fresh.push(*key);
                            }
                        }
                        let added = fresh.len();
                        fresh.extend(&model);
                        model = fresh;
                        prop_assert_eq!(order.prepend(keys.into_iter().map(k)), added);
                    }
                    Op::InsertAfter(after, key) => {
                        let at = model.iter().position(|&m| m == after);
                        let ok = at.is_some() && !model.contains(&key);
                        if let (true, Some(at)) = (ok, at) {
                            model.insert(at + 1, key);
                        }
                        prop_assert_eq!(order.insert_after(k(after), k(key)), ok);
                    }
                    Op::Remove(key) => {
                        let at = model.iter().position(|&m| m == key);
                        if let Some(at) = at {
                            model.remove(at);
                        }
                        prop_assert_eq!(order.remove(k(key)), at.map(|a| a as u32));
                    }
                }
                prop_assert_eq!(order.verify_integrity(), Ok(()));
                prop_assert_eq!(order.keys(), &model.iter().copied().map(k).collect::<Vec<_>>()[..]);
            }
            let pos = |key: u64| model.iter().position(|&m| m == key);
            for a in 0..KEYS {
                for b in 0..KEYS {
                    let expected = pos(a).zip(pos(b)).map(|(pa, pb)| pa.cmp(&pb));
                    prop_assert_eq!(order.compare(k(a), k(b)), expected);
                    let range: Vec<BlockKey> = match (pos(a), pos(b)) {
                        (Some(pa), Some(pb)) if pa <= pb => model[pa..=pb].iter().copied().map(k).collect(),
                        _ => Vec::new(),
                    };
                    prop_assert_eq!(order.range(k(a), k(b)), &range[..]);
                }
            }
        }

        // Catches slicing inside a multibyte char (panic) and off-by-one
        // clamping at either endpoint or around blocks without text.
        #[test]
        fn copy_text_arbitrary_offsets_match_char_filter_model(
            texts in prop::collection::vec(prop::option::weighted(0.85, block_text()), 1..5),
            anchor in (0usize..6, 0usize..24),
            focus in (0usize..6, 0usize..24),
            separator in prop::sample::select(&["\n", "|", ""][..]),
        ) {
            let mut order = BlockOrder::new();
            let mut source = HashMap::new();
            for (i, text) in texts.iter().enumerate() {
                order.append(k(i as u64));
                if let Some(text) = text {
                    source.insert(k(i as u64), text.clone());
                }
            }
            // Block indexes past the end name a key that is not in `order`.
            let sel = Selection::new(p(anchor.0 as u64, anchor.1), p(focus.0 as u64, focus.1));
            let got = copy_text(&sel, &order, &source, separator);

            let n = texts.len();
            let expected = if anchor.0 >= n || focus.0 >= n || anchor == focus {
                String::new()
            } else {
                let (s, e) = if anchor <= focus { (anchor, focus) } else { (focus, anchor) };
                // A char is in the copy when its end lies after the start
                // offset and at or before the end offset (in its block).
                let pieces: Vec<String> = (s.0..=e.0)
                    .filter_map(|i| {
                        let text = texts[i].as_ref()?;
                        Some(
                            text.char_indices()
                                .filter(|(at, c)| {
                                    let end = at + c.len_utf8();
                                    (i != s.0 || end > s.1) && (i != e.0 || end <= e.1)
                                })
                                .map(|(_, c)| c)
                                .collect(),
                        )
                    })
                    .collect();
                pieces.join(separator)
            };
            prop_assert_eq!(got, expected);
        }

        // Catches repair moving an endpoint into a removed or wrong block, or
        // flipping the selection's direction.
        #[test]
        fn selection_after_remove_lands_in_survivors_and_keeps_direction(
            texts in prop::collection::vec(block_text(), 1..6),
            anchor in (0usize..6, 0usize..24),
            focus in (0usize..6, 0usize..24),
            removed in 0usize..6,
        ) {
            let n = texts.len();
            let (anchor, focus, removed) = ((anchor.0 % n, anchor.1), (focus.0 % n, focus.1), removed % n);
            let blocks: Vec<(u64, &str)> =
                texts.iter().enumerate().map(|(i, t)| (i as u64, t.as_str())).collect();
            let (mut order, source) = doc(&blocks);
            let sel = Selection::new(p(anchor.0 as u64, anchor.1), p(focus.0 as u64, focus.1));
            // Offsets past the end or inside a char read as the clamped offset,
            // so direction is judged on clamped points.
            let doc_point = |order: &BlockOrder, point: SelectionPoint| {
                let text = &source[&point.block];
                (order.position(point.block), floor_boundary(text, point.byte))
            };
            let forward = doc_point(&order, sel.anchor) <= doc_point(&order, sel.focus);

            let former = order.remove(k(removed as u64));
            prop_assert_eq!(former, Some(removed as u32));
            let fixed = sel.after_remove(k(removed as u64), removed as u32, &order, &source);

            if n == 1 {
                prop_assert_eq!(fixed, None);
                return Ok(());
            }
            let Some(fixed) = fixed else {
                return Err(TestCaseError::fail("repair dropped a selection with survivors"));
            };
            if anchor.0 != removed && focus.0 != removed {
                prop_assert_eq!(fixed, sel);
            }
            prop_assert!(order.contains(fixed.anchor.block));
            prop_assert!(order.contains(fixed.focus.block));
            let (a, f) = (doc_point(&order, fixed.anchor), doc_point(&order, fixed.focus));
            if forward {
                prop_assert!(a <= f, "forward selection flipped: {:?}", fixed);
            } else {
                prop_assert!(a >= f, "backward selection flipped: {:?}", fixed);
            }
        }
    }

    // Catches a per-append check that walks the whole order, which made
    // debug builds quadratic while a transcript loads block by block.
    #[test]
    fn appending_30k_blocks_one_at_a_time_checks_linearly() {
        if FULL_INTEGRITY_CHECKS {
            return;
        }
        let mut order = BlockOrder::new();
        let before = integrity_steps();
        for key in 0..30_000 {
            order.append(k(key));
        }
        let steps = integrity_steps() - before;
        assert!(
            cfg!(not(debug_assertions)) || steps <= 2 * 30_000,
            "{steps} steps"
        );
    }

    #[test]
    fn selection_after_remove_cases_shrink_inward() {
        let three: &[(u64, &str)] = &[(1, "aaa"), (2, "bbb"), (3, "ccc")];
        // (name, blocks, selection, removed key, repaired selection)
        type Case<'a> = (
            &'a str,
            &'a [(u64, &'a str)],
            Selection,
            u64,
            Option<Selection>,
        );
        let cases: &[Case] = &[
            (
                "start removed moves to next block start",
                &[(1, "aaa"), (2, "bbb"), (3, "ccc"), (4, "ddd")],
                Selection::new(p(2, 1), p(4, 2)),
                2,
                Some(Selection::new(p(3, 0), p(4, 2))),
            ),
            (
                "end removed (backward anchor) moves to previous block end",
                three,
                Selection::new(p(3, 1), p(1, 1)),
                3,
                Some(Selection::new(p(2, 3), p(1, 1))),
            ),
            (
                "unrelated block removed leaves selection alone",
                three,
                Selection::new(p(1, 1), p(3, 1)),
                2,
                Some(Selection::new(p(1, 1), p(3, 1))),
            ),
            (
                "both endpoints removed collapse at next block start",
                three,
                Selection::new(p(2, 0), p(2, 2)),
                2,
                Some(Selection::collapsed(p(3, 0))),
            ),
            (
                "both endpoints in removed last block collapse at previous end",
                three,
                Selection::new(p(3, 0), p(3, 2)),
                3,
                Some(Selection::collapsed(p(2, 3))),
            ),
            (
                "only block removed leaves nothing",
                &[(1, "aaa")],
                Selection::collapsed(p(1, 1)),
                1,
                None,
            ),
        ];
        for (name, blocks, sel, removed, expected) in cases {
            let (mut order, text) = doc(blocks);
            let pos = order.remove(k(*removed)).unwrap_or(u32::MAX);
            assert_eq!(
                sel.after_remove(k(*removed), pos, &order, &text),
                *expected,
                "{name}"
            );
        }
    }
}
