use std::ops::Range;

use quark_text::TextOffset;
use quark_text::offset;

/// In-progress IME composition. It is shown at the caret but is not part of
/// the committed text until the IME commits it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Preedit {
    pub text: String,
    /// The IME's cursor as a range inside `text`. `None` hides the caret.
    pub cursor: Option<Range<TextOffset>>,
}

impl Preedit {
    /// `None` for empty text, which is how IMEs cancel a composition. The
    /// IME's raw cursor bytes are snapped onto grapheme boundaries of `text`.
    pub fn new(text: impl Into<String>, cursor: Option<(usize, usize)>) -> Option<Self> {
        let text = text.into();
        if text.is_empty() {
            return None;
        }
        let cursor = cursor.map(|(start, end)| offset::ordered(&text, start..end));
        Some(Self { text, cursor })
    }
}

/// Committed text with a preedit spliced in over the selection, as painted
/// while composing. Offsets are into `text`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Composition {
    pub text: String,
    /// Where the preedit sits.
    pub preedit: Range<TextOffset>,
    /// The IME's highlighted clause, when its cursor is a range.
    pub clause: Option<Range<TextOffset>>,
    /// Caret offset, `None` when the IME hides it.
    pub caret: Option<TextOffset>,
}

/// Splice `preedit` into `text` in place of `selection`.
pub fn compose(text: &str, selection: Range<TextOffset>, preedit: &Preedit) -> Composition {
    let selection = offset::ordered(text, selection);
    let start = selection.start;
    let composed = offset::splice(text, selection, &preedit.text);
    // The preedit's own offsets, moved past the committed text before it.
    let at = |o: TextOffset| offset::shifted(&composed, start, o.get());
    let clause = preedit
        .cursor
        .clone()
        .filter(|cursor| !cursor.is_empty())
        .map(|cursor| at(cursor.start)..at(cursor.end));
    Composition {
        preedit: start.within(&composed)..offset::shifted(&composed, start, preedit.text.len()),
        clause,
        caret: preedit.cursor.as_ref().map(|cursor| at(cursor.end)),
        text: composed,
    }
}
