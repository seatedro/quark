//! The text a [`super::buffer::TextBuffer`] edits: a [`Rope`], the
//! contiguous copy made only when a caller asks for a `&str`, and a log of
//! which lines each change replaced, for the editor's per-line caches.
//!
//! Positions come back as [`TextOffset`]s on grapheme boundaries of the
//! rope, with the same results `quark_text::offset` gives on the text as
//! one string.

use std::borrow::Cow;
use std::ops::Range;
use std::sync::{Arc, OnceLock};

use quark_text::TextOffset;

use super::rope::Rope;

/// Lines a change replaced, in the line numbers of the text before it:
/// lines `first..first + removed` became `first..first + inserted`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TextChange {
    /// The whole text was replaced.
    Reset,
    Lines {
        first: usize,
        removed: usize,
        inserted: usize,
    },
}

/// The whole text as one string, made on first request and shared with
/// snapshots of the same revision.
type Flat = Arc<OnceLock<Arc<str>>>;

#[derive(Debug, Clone, Default)]
pub(crate) struct DocText {
    rope: Rope,
    flat: Flat,
    /// Changes since the owner last took them; `None` when nobody reads
    /// them (single-line fields).
    changes: Option<Vec<TextChange>>,
}

impl PartialEq for DocText {
    fn eq(&self, other: &Self) -> bool {
        self.rope == other.rope
    }
}

/// A text revision, shared without copying: the rope, and the one string
/// any holder of this revision materializes for all of them.
#[derive(Debug, Clone, Default)]
pub(crate) struct TextSnapshot {
    rope: Rope,
    flat: Flat,
}

impl TextSnapshot {
    pub(crate) fn rope(&self) -> &Rope {
        &self.rope
    }

    /// The whole text as one string. The first call for a revision copies
    /// it; every holder of the revision shares that copy.
    pub(crate) fn flat(&self) -> Arc<str> {
        self.flat
            .get_or_init(|| Arc::from(self.rope.to_string()))
            .clone()
    }
}

impl DocText {
    /// Start logging changes for [`Self::take_changes`].
    pub(crate) fn track_changes(&mut self) {
        self.changes.get_or_insert_with(Vec::new);
    }

    /// Moves the changes logged since the last call into `out` (which
    /// should be empty), keeping `out`'s storage for the next call.
    pub(crate) fn take_changes(&mut self, out: &mut Vec<TextChange>) {
        if let Some(changes) = &mut self.changes {
            std::mem::swap(changes, out);
        }
    }

    pub(crate) fn rope(&self) -> &Rope {
        &self.rope
    }

    pub(crate) fn len(&self) -> usize {
        self.rope.len()
    }

    /// The text as one string: borrowed when it is one chunk, else copied
    /// once per revision.
    pub(crate) fn as_str(&self) -> &str {
        if let Some(text) = self.rope.as_single_chunk() {
            return text;
        }
        self.flat.get_or_init(|| Arc::from(self.rope.to_string()))
    }

    /// This revision, shared.
    pub(crate) fn snapshot(&self) -> TextSnapshot {
        TextSnapshot {
            rope: self.rope.clone(),
            flat: self.flat.clone(),
        }
    }

    pub(crate) fn snap(&self, byte: usize) -> TextOffset {
        TextOffset::snap_in(&self.rope, byte)
    }

    /// The grapheme boundary at or after `byte`.
    pub(crate) fn snap_up(&self, byte: usize) -> TextOffset {
        self.snap(self.rope.ceil_grapheme(byte))
    }

    pub(crate) fn end(&self) -> TextOffset {
        self.snap(self.rope.len())
    }

    /// `at`, from an older version of the text, snapped onto this one.
    pub(crate) fn within(&self, at: TextOffset) -> TextOffset {
        self.snap(at.get())
    }

    /// `range` snapped and put in order.
    pub(crate) fn ordered(&self, range: Range<usize>) -> Range<TextOffset> {
        let (a, b) = (self.snap(range.start), self.snap(range.end));
        a.min(b)..a.max(b)
    }

    pub(crate) fn prev_grapheme(&self, at: TextOffset) -> TextOffset {
        let at = self.rope.floor_grapheme(at.get());
        self.snap(self.rope.prev_grapheme_boundary(at))
    }

    pub(crate) fn next_grapheme(&self, at: TextOffset) -> TextOffset {
        let at = self.rope.floor_grapheme(at.get());
        self.snap(self.rope.next_grapheme_boundary(at))
    }

    pub(crate) fn prev_word_start(&self, at: TextOffset) -> TextOffset {
        self.snap(self.rope.prev_word_start(at.get()))
    }

    pub(crate) fn next_word_start(&self, at: TextOffset) -> TextOffset {
        self.snap(self.rope.next_word_start(at.get()))
    }

    pub(crate) fn next_word_end(&self, at: TextOffset) -> TextOffset {
        self.snap(self.rope.next_word_end(at.get()))
    }

    pub(crate) fn word_range_at(&self, at: TextOffset) -> Range<TextOffset> {
        let range = self.rope.word_range_at(at.get());
        self.snap(range.start)..self.snap(range.end)
    }

    pub(crate) fn line_start(&self, at: TextOffset) -> TextOffset {
        self.snap(self.rope.line_start_at(at.get()))
    }

    pub(crate) fn line_end(&self, at: TextOffset) -> TextOffset {
        self.snap(self.rope.line_end_at(at.get()))
    }

    pub(crate) fn line_range_at(&self, at: TextOffset) -> Range<TextOffset> {
        let range = self.rope.line_range_at(at.get());
        self.snap(range.start)..self.snap(range.end)
    }

    /// The text in `range` (either order).
    pub(crate) fn slice(&self, range: Range<TextOffset>) -> Cow<'_, str> {
        let range = self.ordered(range.start.get()..range.end.get());
        self.rope.slice(range.start.get()..range.end.get())
    }

    /// Replace the whole text.
    pub(crate) fn set(&mut self, text: &str) {
        self.rope = Rope::from(text);
        self.changed(TextChange::Reset);
    }

    /// Append `text`.
    pub(crate) fn push_str(&mut self, text: &str) {
        let end = self.rope.len();
        self.replace_bytes(end..end, text);
    }

    /// Replace `range` (snapped and ordered) with `with`; returns what it
    /// held.
    pub(crate) fn replace(&mut self, range: Range<TextOffset>, with: &str) -> String {
        let range = self.ordered(range.start.get()..range.end.get());
        let range = range.start.get()..range.end.get();
        let removed = self.rope.slice_to_string(range.clone());
        self.replace_bytes(range, with);
        removed
    }

    /// Replace `expected` at byte `at` with `with`, only if the text really
    /// holds `expected` there.
    pub(crate) fn replace_checked(&mut self, at: usize, expected: &str, with: &str) -> bool {
        if !self.rope.slice_eq(at, expected) {
            return false;
        }
        self.replace_bytes(at..at + expected.len(), with);
        true
    }

    /// `range` must be on char boundaries.
    fn replace_bytes(&mut self, range: Range<usize>, with: &str) {
        let first = self.rope.byte_to_line(range.start);
        let removed = self.rope.byte_to_line(range.end) - first + 1;
        let inserted = with.bytes().filter(|&b| b == b'\n').count() + 1;
        self.rope.replace(range, with);
        self.changed(TextChange::Lines {
            first,
            removed,
            inserted,
        });
    }

    fn changed(&mut self, change: TextChange) {
        // Snapshots of the old revision keep the old copy.
        match Arc::get_mut(&mut self.flat) {
            Some(flat) => {
                flat.take();
            }
            None => self.flat = Flat::default(),
        }
        if let Some(changes) = &mut self.changes {
            if change == TextChange::Reset {
                changes.clear();
            }
            changes.push(change);
        }
    }
}

/// Text whose char and grapheme boundaries the atom and style checks ask
/// about: a plain string, or a rope.
pub(crate) trait TextBounds {
    fn byte_len(&self) -> usize;
    fn is_char_boundary(&self, at: usize) -> bool;
    fn is_grapheme_boundary(&self, at: usize) -> bool;
}

impl TextBounds for str {
    fn byte_len(&self) -> usize {
        self.len()
    }

    fn is_char_boundary(&self, at: usize) -> bool {
        str::is_char_boundary(self, at)
    }

    fn is_grapheme_boundary(&self, at: usize) -> bool {
        at <= self.len() && TextOffset::snap(self, at) == at
    }
}

impl TextBounds for Rope {
    fn byte_len(&self) -> usize {
        self.len()
    }

    fn is_char_boundary(&self, at: usize) -> bool {
        Rope::is_char_boundary(self, at)
    }

    fn is_grapheme_boundary(&self, at: usize) -> bool {
        Rope::is_grapheme_boundary(self, at)
    }
}
