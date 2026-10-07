use std::ops::Range;

/// In-progress IME composition. It is shown at the caret but is not part of
/// the committed text until the IME commits it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Preedit {
    pub text: String,
    /// The IME's cursor as a byte range inside `text`. `None` hides the caret.
    pub cursor: Option<(usize, usize)>,
}

impl Preedit {
    /// `None` for empty text, which is how IMEs cancel a composition.
    /// Cursor offsets are clamped onto char boundaries of `text`.
    pub fn new(text: impl Into<String>, cursor: Option<(usize, usize)>) -> Option<Self> {
        let text = text.into();
        if text.is_empty() {
            return None;
        }
        let cursor = cursor.map(|(start, end)| {
            let start = floor_char_boundary(&text, start);
            let end = floor_char_boundary(&text, end);
            (start.min(end), start.max(end))
        });
        Some(Self { text, cursor })
    }
}

/// Committed text with a preedit spliced in over the selection, as painted
/// while composing. Offsets are bytes into `text`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Composition {
    pub text: String,
    /// Where the preedit sits.
    pub preedit: Range<usize>,
    /// The IME's highlighted clause, when its cursor is a range.
    pub clause: Option<Range<usize>>,
    /// Caret offset, `None` when the IME hides it.
    pub caret: Option<usize>,
}

/// Splice `preedit` into `text` in place of `selection` (start..end).
pub fn compose(text: &str, selection: (usize, usize), preedit: &Preedit) -> Composition {
    let start = floor_char_boundary(text, selection.0.min(selection.1));
    let end = floor_char_boundary(text, selection.0.max(selection.1));
    let mut composed = String::with_capacity(text.len() + preedit.text.len());
    composed.push_str(&text[..start]);
    composed.push_str(&preedit.text);
    composed.push_str(&text[end..]);
    let clause = preedit
        .cursor
        .filter(|(a, b)| a != b)
        .map(|(a, b)| start + a..start + b);
    Composition {
        text: composed,
        preedit: start..start + preedit.text.len(),
        clause,
        caret: preedit.cursor.map(|(_, b)| start + b),
    }
}

pub(super) fn floor_char_boundary(text: &str, offset: usize) -> usize {
    let mut offset = offset.min(text.len());
    while offset > 0 && !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}
