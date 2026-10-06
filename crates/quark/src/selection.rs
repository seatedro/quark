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

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

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
        true
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
        true
    }

    /// Removes `key`, returning the position it occupied. Pass that position
    /// to [`Selection::after_remove`].
    pub fn remove(&mut self, key: BlockKey) -> Option<u32> {
        let pos = self.index.remove(&key)?;
        self.keys.remove(pos as usize);
        self.reindex_from(pos as usize);
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

    fn reindex_from(&mut self, start: usize) {
        for (i, key) in self.keys.iter().enumerate().skip(start) {
            self.index.insert(*key, i as u32);
        }
    }
}

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

    #[test]
    fn prepend_keeps_order_and_positions() {
        let mut order = BlockOrder::new();
        order.append(k(10));
        order.append(k(11));
        assert_eq!(order.prepend([k(7), k(8), k(9), k(10)]), 3);
        assert_eq!(order.keys(), &[k(7), k(8), k(9), k(10), k(11)]);
        for (i, key) in order.keys().iter().enumerate() {
            assert_eq!(order.position(*key), Some(i as u32));
        }
        assert_eq!(order.compare(k(7), k(11)), Some(Ordering::Less));
        assert_eq!(order.compare(k(11), k(9)), Some(Ordering::Greater));
        assert_eq!(order.compare(k(1), k(9)), None);
        assert_eq!(order.range(k(8), k(10)), &[k(8), k(9), k(10)]);
        assert!(order.range(k(10), k(8)).is_empty());

        // A selection made before the prepend still orders correctly.
        let sel = Selection::new(p(11, 2), p(10, 1));
        assert_eq!(sel.ordered(&order), Some((p(10, 1), p(11, 2))));
    }

    #[test]
    fn insert_after_and_duplicates() {
        let mut order = BlockOrder::new();
        order.append(k(1));
        order.append(k(3));
        assert!(order.insert_after(k(1), k(2)));
        assert!(!order.insert_after(k(1), k(2)));
        assert!(!order.insert_after(k(99), k(4)));
        assert!(!order.append(k(3)));
        assert_eq!(order.keys(), &[k(1), k(2), k(3)]);
        assert_eq!(order.position(k(3)), Some(2));
    }

    #[test]
    fn copy_across_three_blocks_with_partial_ends() {
        let (order, text) = doc(&[(1, "hello world"), (2, "middle"), (3, "goodbye")]);
        let sel = Selection::new(p(3, 4), p(1, 6));
        assert_eq!(copy_text(&sel, &order, &text, "\n"), "world\nmiddle\ngood");
    }

    #[test]
    fn copy_within_one_block_and_out_of_range() {
        let (order, text) = doc(&[(1, "abcdef")]);
        let sel = Selection::new(p(1, 1), p(1, 4));
        assert_eq!(copy_text(&sel, &order, &text, "\n"), "bcd");
        let sel = Selection::new(p(1, 2), p(1, 999));
        assert_eq!(copy_text(&sel, &order, &text, "\n"), "cdef");
        assert!(copy_text(&Selection::collapsed(p(1, 2)), &order, &text, "\n").is_empty());
    }

    #[test]
    fn copy_clamps_multibyte_boundaries() {
        // "é" is 2 bytes, "日" is 3 bytes.
        let (order, text) = doc(&[(1, "aé日b"), (2, "日本")]);
        // Start inside "é" rounds down to include it; end inside "本" rounds
        // down to exclude it.
        let sel = Selection::new(p(1, 2), p(2, 4));
        assert_eq!(copy_text(&sel, &order, &text, "|"), "é日b|日");
    }

    #[test]
    fn copy_skips_missing_blocks() {
        let (mut order, text) = doc(&[(1, "one"), (3, "three")]);
        order.insert_after(k(1), k(2));
        let sel = Selection::new(p(1, 0), p(3, 5));
        assert_eq!(copy_text(&sel, &order, &text, "\n"), "one\nthree");
    }

    #[test]
    fn streaming_append_keeps_points_valid() {
        let (order, mut text) = doc(&[(1, "first"), (2, "stre")]);
        let sel = Selection::new(p(1, 2), p(2, 4));
        assert_eq!(copy_text(&sel, &order, &text, "\n"), "rst\nstre");
        if let Some(t) = text.get_mut(&k(2)) {
            t.push_str("aming");
        }
        assert_eq!(copy_text(&sel, &order, &text, "\n"), "rst\nstre");
        let to_end = Selection::new(p(1, 2), p(2, usize::MAX));
        assert_eq!(copy_text(&to_end, &order, &text, "\n"), "rst\nstreaming");
    }

    #[test]
    fn removal_shrinks_selection_inward() {
        let (mut order, text) = doc(&[(1, "aaa"), (2, "bbb"), (3, "ccc"), (4, "ddd")]);

        // Start endpoint removed: moves to byte 0 of the next block.
        let sel = Selection::new(p(2, 1), p(4, 2));
        let pos = order.remove(k(2)).unwrap_or(u32::MAX);
        let fixed = sel.after_remove(k(2), pos, &order, &text);
        assert_eq!(fixed, Some(Selection::new(p(3, 0), p(4, 2))));

        // End endpoint (here the anchor, selection is backwards) removed:
        // moves to the end of the previous block.
        let (mut order, text) = doc(&[(1, "aaa"), (2, "bbb"), (3, "ccc")]);
        let sel = Selection::new(p(3, 1), p(1, 1));
        let pos = order.remove(k(3)).unwrap_or(u32::MAX);
        let fixed = sel.after_remove(k(3), pos, &order, &text);
        assert_eq!(fixed, Some(Selection::new(p(2, 3), p(1, 1))));

        // Unrelated removal leaves the selection alone.
        let (mut order, text) = doc(&[(1, "aaa"), (2, "bbb"), (3, "ccc")]);
        let sel = Selection::new(p(1, 1), p(3, 1));
        let pos = order.remove(k(2)).unwrap_or(u32::MAX);
        assert_eq!(sel.after_remove(k(2), pos, &order, &text), Some(sel));
        assert_eq!(copy_text(&sel, &order, &text, "\n"), "aa\nc");
    }

    #[test]
    fn removal_of_block_holding_both_endpoints_collapses() {
        let (mut order, text) = doc(&[(1, "aaa"), (2, "bbb"), (3, "ccc")]);
        let sel = Selection::new(p(2, 0), p(2, 2));
        let pos = order.remove(k(2)).unwrap_or(u32::MAX);
        let fixed = sel.after_remove(k(2), pos, &order, &text);
        assert_eq!(fixed, Some(Selection::collapsed(p(3, 0))));

        // Last block removed: collapse at the end of the previous one.
        let sel = Selection::new(p(3, 0), p(3, 2));
        let pos = order.remove(k(3)).unwrap_or(u32::MAX);
        let fixed = sel.after_remove(k(3), pos, &order, &text);
        assert_eq!(fixed, Some(Selection::collapsed(p(1, 3))));

        // Nothing left.
        let sel = Selection::collapsed(p(1, 1));
        let pos = order.remove(k(1)).unwrap_or(u32::MAX);
        assert_eq!(sel.after_remove(k(1), pos, &order, &text), None);
    }
}
