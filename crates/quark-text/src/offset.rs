//! Byte offsets that sit on grapheme boundaries, and the text navigation
//! built on them.
//!
//! This is the one module in quark-text and quark-ui's text code allowed to
//! slice a string by byte index (`clippy::string_slice` is denied
//! elsewhere). Offsets cross APIs as [`TextOffset`], which is made only by
//! snapping a raw index onto a boundary of some text or by a function here
//! that walks that text. A `TextOffset` carries no proof of *which* text it
//! came from (a field's text can change between a frame's hit test and the
//! command it produced), so every function here snaps it again against the
//! text it is given. Stale or foreign offsets therefore clamp instead of
//! panicking.
#![allow(clippy::string_slice)]

use std::ops::Range;

use unicode_segmentation::{GraphemeCursor, UnicodeSegmentation};

/// A byte offset on a grapheme boundary of the text it was snapped to.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TextOffset(u32);

impl TextOffset {
    pub const ZERO: Self = Self(0);

    /// The grapheme boundary at or before `byte`, clamped to `text`.
    pub fn snap(text: &str, byte: usize) -> Self {
        // Texts past u32::MAX bytes are refused by layout; clamping here
        // keeps the conversion total.
        let mut byte = byte.min(text.len()).min(u32::MAX as usize);
        while !text.is_char_boundary(byte) {
            byte -= 1;
        }
        let mut cursor = GraphemeCursor::new(byte, text.len(), true);
        let byte = match cursor.is_boundary(text, 0) {
            Ok(true) => byte,
            _ => cursor.prev_boundary(text, 0).ok().flatten().unwrap_or(0),
        };
        Self(byte as u32)
    }

    /// The grapheme boundary at or after `byte`, clamped to `text`.
    pub fn snap_up(text: &str, byte: usize) -> Self {
        let mut byte = byte.min(text.len()).min(u32::MAX as usize);
        while !text.is_char_boundary(byte) {
            byte += 1;
        }
        let mut cursor = GraphemeCursor::new(byte, text.len(), true);
        let byte = match cursor.is_boundary(text, 0) {
            Ok(true) => byte,
            _ => cursor
                .next_boundary(text, 0)
                .ok()
                .flatten()
                .unwrap_or(text.len()),
        };
        Self::snap(text, byte)
    }

    /// The end of `text`.
    pub fn end(text: &str) -> Self {
        Self::snap(text, text.len())
    }

    pub fn get(self) -> usize {
        self.0 as usize
    }

    /// This offset snapped onto `text`, for an offset that may have come
    /// from an older version of it.
    pub fn within(self, text: &str) -> Self {
        Self::snap(text, self.get())
    }
}

impl std::fmt::Display for TextOffset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl PartialEq<usize> for TextOffset {
    fn eq(&self, other: &usize) -> bool {
        self.get() == *other
    }
}

/// Anything a text API accepts as a position: a [`TextOffset`] or a raw
/// byte index. Either is snapped onto the text it is used with.
pub trait ToTextOffset: Copy {
    fn to_offset(self, text: &str) -> TextOffset;
}

impl ToTextOffset for TextOffset {
    fn to_offset(self, text: &str) -> TextOffset {
        self.within(text)
    }
}

impl ToTextOffset for usize {
    fn to_offset(self, text: &str) -> TextOffset {
        TextOffset::snap(text, self)
    }
}

/// `range` snapped onto `text` and put in order.
pub fn ordered(text: &str, range: Range<impl ToTextOffset>) -> Range<TextOffset> {
    let (a, b) = (range.start.to_offset(text), range.end.to_offset(text));
    a.min(b)..a.max(b)
}

/// The text in `range` (either order).
pub fn slice(text: &str, range: Range<impl ToTextOffset>) -> &str {
    let range = ordered(text, range);
    &text[range.start.get()..range.end.get()]
}

/// The text before `at`.
pub fn prefix(text: &str, at: impl ToTextOffset) -> &str {
    &text[..at.to_offset(text).get()]
}

/// The text from `at` on.
pub fn suffix(text: &str, at: impl ToTextOffset) -> &str {
    &text[at.to_offset(text).get()..]
}

/// Replace `range` of `text` with `with`; returns the removed text.
pub fn replace(text: &mut String, range: Range<TextOffset>, with: &str) -> String {
    let range = ordered(text, range);
    let range = range.start.get()..range.end.get();
    let removed = text[range.clone()].to_owned();
    text.replace_range(range, with);
    removed
}

/// `text` with `range` replaced by `with`.
pub fn splice(text: &str, range: Range<TextOffset>, with: &str) -> String {
    let range = ordered(text, range);
    let mut out = String::with_capacity(text.len() + with.len());
    out.push_str(&text[..range.start.get()]);
    out.push_str(with);
    out.push_str(&text[range.end.get()..]);
    out
}

/// `at` moved `by` bytes forward; for offsets into text spliced after a
/// known prefix.
pub fn shifted(text: &str, at: TextOffset, by: usize) -> TextOffset {
    TextOffset::snap(text, at.get().saturating_add(by))
}

/// Every grapheme boundary of `text`, from 0 through `text.len()`.
pub fn grapheme_boundaries(text: &str) -> impl Iterator<Item = TextOffset> + '_ {
    text.grapheme_indices(true)
        .map(|(i, _)| TextOffset(i as u32))
        .chain(std::iter::once(TextOffset::end(text)))
}

pub fn prev_grapheme(text: &str, at: impl ToTextOffset) -> TextOffset {
    let at = at.to_offset(text);
    text[..at.get()]
        .grapheme_indices(true)
        .next_back()
        .map_or(TextOffset::ZERO, |(i, _)| TextOffset(i as u32))
}

pub fn next_grapheme(text: &str, at: impl ToTextOffset) -> TextOffset {
    let at = at.to_offset(text).get();
    text[at..]
        .graphemes(true)
        .next()
        .map_or(TextOffset::end(text), |g| TextOffset((at + g.len()) as u32))
}

/// How many graphemes precede `at`.
pub fn graphemes_before(text: &str, at: impl ToTextOffset) -> usize {
    prefix(text, at).graphemes(true).count()
}

/// The start of grapheme `n`, or the end of `text` past its last.
pub fn nth_grapheme(text: &str, n: usize) -> TextOffset {
    text.grapheme_indices(true)
        .nth(n)
        .map_or(TextOffset::end(text), |(i, _)| TextOffset(i as u32))
}

/// Start of the logical line (after the previous `\n`) holding `at`.
pub fn line_start(text: &str, at: impl ToTextOffset) -> TextOffset {
    let at = at.to_offset(text).get();
    TextOffset(text[..at].rfind('\n').map_or(0, |i| i + 1) as u32)
}

/// End of the logical line holding `at`, before its `\n`. A `\r` before
/// the `\n` belongs to the line ending, so this is a grapheme boundary.
pub fn line_end(text: &str, at: impl ToTextOffset) -> TextOffset {
    let at = at.to_offset(text).get();
    let end = text[at..].find('\n').map_or(text.len(), |i| at + i);
    TextOffset::snap(text, end)
}

/// The logical line holding `at`, including its `\n`.
pub fn line_range_at(text: &str, at: impl ToTextOffset) -> Range<TextOffset> {
    let start = line_start(text, at);
    let at = at.to_offset(text).get();
    let end = text[at..].find('\n').map_or(text.len(), |i| at + i + 1);
    start..TextOffset::snap(text, end)
}

/// A piece of text for word navigation: `(start, end, is_word)`.
type Piece = (usize, usize, bool);

/// Split one UAX #29 word-bound segment at ASCII punctuation (other than `_`
/// and `'`), so `foo.bar` stops at the dot as code editors do while `don't`
/// stays one word.
fn push_pieces(base: usize, segment: &str, out: &mut Vec<Piece>) {
    let is_word = |s: &str| s.chars().any(|c| c.is_alphanumeric() || c == '_');
    let mut start = 0;
    for (i, c) in segment.char_indices() {
        if c.is_ascii_punctuation() && c != '_' && c != '\'' {
            if start < i {
                out.push((base + start, base + i, is_word(&segment[start..i])));
            }
            out.push((base + i, base + i + 1, false));
            start = i + 1;
        }
    }
    if start < segment.len() {
        out.push((
            base + start,
            base + segment.len(),
            is_word(&segment[start..]),
        ));
    }
}

/// Start of the word before `at`, by Unicode word boundaries. Adjacent
/// word segments (a run of CJK ideographs, say) count as one word; emoji
/// and punctuation separate words.
pub fn prev_word_start(text: &str, at: impl ToTextOffset) -> TextOffset {
    let at = at.to_offset(text).get();
    let mut pieces = Vec::new();
    let mut word_start = None;
    for (idx, segment) in text[..at].split_word_bound_indices().rev() {
        pieces.clear();
        push_pieces(idx, segment, &mut pieces);
        for &(start, _, is_word) in pieces.iter().rev() {
            if is_word {
                word_start = Some(start);
            } else if let Some(start) = word_start {
                return TextOffset::snap(text, start);
            }
        }
    }
    TextOffset::ZERO
}

/// Start of the word after `at` (skips the rest of the current word, then
/// any separators).
pub fn next_word_start(text: &str, at: impl ToTextOffset) -> TextOffset {
    let at = at.to_offset(text).get();
    let mut pieces = Vec::new();
    let mut left_word = false;
    for (idx, segment) in text[at..].split_word_bound_indices() {
        pieces.clear();
        push_pieces(at + idx, segment, &mut pieces);
        for &(start, _, is_word) in &pieces {
            if !is_word {
                left_word = true;
            } else if left_word {
                return TextOffset::snap(text, start);
            }
        }
    }
    TextOffset::end(text)
}

/// End of the word after `at` (skips separators, then the word).
pub fn next_word_end(text: &str, at: impl ToTextOffset) -> TextOffset {
    let at = at.to_offset(text).get();
    let mut pieces = Vec::new();
    let mut word_end = None;
    for (idx, segment) in text[at..].split_word_bound_indices() {
        pieces.clear();
        push_pieces(at + idx, segment, &mut pieces);
        for &(_, end, is_word) in &pieces {
            if is_word {
                word_end = Some(end);
            } else if let Some(end) = word_end {
                return TextOffset::snap(text, end);
            }
        }
    }
    TextOffset::end(text)
}

/// The word run around `at` for double-click selection. Between a word and
/// a separator the word wins; a whitespace or punctuation run is selected
/// on its own when no word touches `at`.
pub fn word_range_at(text: &str, at: impl ToTextOffset) -> Range<TextOffset> {
    let at_offset = at.to_offset(text);
    let at = at_offset.get();
    let line_start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    let line_end = text[at..].find('\n').map_or(text.len(), |i| at + i);
    let mut pieces = Vec::new();
    for (idx, segment) in text[line_start..line_end].split_word_bound_indices() {
        push_pieces(line_start + idx, segment, &mut pieces);
    }
    let containing = pieces.iter().position(|p| p.0 <= at && at < p.1);
    let before = pieces.iter().position(|p| p.1 == at);
    let index = match (containing, before) {
        (Some(i), Some(b)) if !pieces[i].2 && pieces[b].2 => b,
        (Some(i), _) => i,
        (None, Some(b)) => b,
        (None, None) => return at_offset..at_offset,
    };
    let (mut first, mut last) = (index, index);
    if pieces[index].2 {
        while first > 0 && pieces[first - 1].2 {
            first -= 1;
        }
        while last + 1 < pieces.len() && pieces[last + 1].2 {
            last += 1;
        }
    }
    // Pieces split only at ASCII punctuation and word bounds, which are
    // grapheme boundaries, so snapping keeps them as they are.
    TextOffset::snap(text, pieces[first].0)..TextOffset::snap(text, pieces[last].1)
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    const PIECES: &[&str] = &[
        "a",
        " ",
        "\n",
        "\r\n",
        "\u{e9}",
        "e\u{301}",
        "\u{65e5}",
        "\u{1f44d}\u{1f3fd}",
        "\u{1f469}\u{200d}\u{1f4bb}",
        "\u{1f1fa}\u{1f1f8}",
        ".",
    ];

    fn text() -> impl Strategy<Value = String> {
        prop::collection::vec(prop::sample::select(PIECES), 0..16).prop_map(|v| v.concat())
    }

    fn boundaries(text: &str) -> Vec<usize> {
        let mut out: Vec<usize> = text.grapheme_indices(true).map(|(i, _)| i).collect();
        out.push(text.len());
        out
    }

    type Nav = fn(&str, usize) -> TextOffset;

    proptest! {
        // Catches any navigation function returning an offset inside a
        // grapheme (or panicking) for a raw index inside a char, past the
        // end, or between the code points of a cluster.
        #[test]
        fn any_raw_index_lands_on_a_grapheme_boundary(text in text(), raw in 0usize..96) {
            let boundaries = boundaries(&text);
            let snapped = TextOffset::snap(&text, raw);
            let floor = boundaries.iter().rev().find(|&&b| b <= raw).copied().unwrap_or(0);
            prop_assert_eq!(snapped, floor, "snap({:?}, {})", text, raw);
            let nav: [(&str, Nav); 9] = [
                ("prev_grapheme", |t, i| prev_grapheme(t, i)),
                ("next_grapheme", |t, i| next_grapheme(t, i)),
                ("prev_word_start", |t, i| prev_word_start(t, i)),
                ("next_word_start", |t, i| next_word_start(t, i)),
                ("next_word_end", |t, i| next_word_end(t, i)),
                ("line_start", |t, i| line_start(t, i)),
                ("line_end", |t, i| line_end(t, i)),
                ("word_range_at.end", |t, i| word_range_at(t, i).end),
                ("line_range_at.end", |t, i| line_range_at(t, i).end),
            ];
            for (name, f) in nav {
                let at = f(&text, raw);
                prop_assert!(boundaries.contains(&at.get()), "{}({:?}, {}) = {:?}", name, text, raw, at);
            }
        }
    }
}
