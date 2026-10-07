//! Trigger detection: which completion (mentions after `@`, commands after
//! `/`, ...) the caret is typing, found from the text alone.

use std::ops::Range;

use quark_text::{TextOffset, offset};

use super::atoms::InlineAtom;

/// Where a trigger character may start a query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerBoundary {
    /// At the start of a word: the start of the text or a line, or after
    /// whitespace or an atom.
    Word,
    /// Only as the first character of a line, as slash commands are.
    LineStart,
}

/// A character that opens a completion query, registered by the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TriggerRule {
    pub ch: char,
    pub boundary: TriggerBoundary,
}

impl TriggerRule {
    pub const fn word(ch: char) -> Self {
        Self {
            ch,
            boundary: TriggerBoundary::Word,
        }
    }

    pub const fn line_start(ch: char) -> Self {
        Self {
            ch,
            boundary: TriggerBoundary::LineStart,
        }
    }
}

/// An active trigger: `range` runs from the trigger character to the caret
/// and is what accepting a completion replaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TriggerMatch {
    /// Index of the matching rule in the rules passed in.
    pub rule: usize,
    pub ch: char,
    pub range: Range<TextOffset>,
}

impl TriggerMatch {
    /// The query typed after the trigger character.
    pub fn query<'t>(&self, text: &'t str) -> &'t str {
        let start = self.range.start.get() + self.ch.len_utf8();
        offset::slice(text, start..self.range.end.get())
    }
}

/// The trigger the caret at `cursor` is in: the word before the caret
/// (back to whitespace, a line start, or an atom) starts with a rule's
/// character, where that rule allows it. The first matching rule wins.
pub fn find_trigger(
    text: &str,
    cursor: TextOffset,
    atoms: &[InlineAtom],
    rules: &[TriggerRule],
) -> Option<TriggerMatch> {
    let cursor = cursor.within(text);
    let before = offset::prefix(text, cursor);
    // The word ends at the caret and starts after the nearest whitespace or
    // atom end; a caret right after an atom has no word.
    let atom_end = atoms
        .iter()
        .map(|a| a.range.end)
        .filter(|&end| end <= cursor.get())
        .max()
        .unwrap_or(0);
    let space = before
        .char_indices()
        .rev()
        .find(|(_, c)| c.is_whitespace())
        .map_or(0, |(i, c)| i + c.len_utf8());
    let start = space.max(atom_end);
    let word = offset::slice(text, start..cursor.get());
    let first = word.chars().next()?;
    let line_start = offset::line_start(text, cursor).get();
    rules.iter().enumerate().find_map(|(rule, r)| {
        let allowed = match r.boundary {
            TriggerBoundary::Word => true,
            TriggerBoundary::LineStart => start == line_start,
        };
        (r.ch == first && allowed).then(|| TriggerMatch {
            rule,
            ch: r.ch,
            range: TextOffset::snap(text, start)..cursor,
        })
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::super::atoms::AtomId;
    use super::*;

    const RULES: &[TriggerRule] = &[
        TriggerRule::word('@'),
        TriggerRule::line_start('/'),
        TriggerRule::word('#'),
        TriggerRule::word('$'),
    ];

    /// `|` marks the caret; `[..]` an atom. Returns `"<ch>query"` or "-".
    fn detect(marked: &str) -> String {
        let mut text = String::new();
        let mut atoms = Vec::new();
        let mut cursor = 0;
        let mut open = 0;
        for ch in marked.chars() {
            match ch {
                '|' => cursor = text.len(),
                '[' => open = text.len(),
                ']' => atoms.push(InlineAtom {
                    range: open..text.len(),
                    id: AtomId::new(0, 0),
                    export: Arc::from(""),
                }),
                _ => text.push(ch),
            }
        }
        let cursor = TextOffset::snap(&text, cursor);
        match find_trigger(&text, cursor, &atoms, RULES) {
            Some(m) => format!("{}{}", m.ch, m.query(&text)),
            None => "-".to_owned(),
        }
    }

    #[test]
    fn trigger_detection_table() {
        let cases = [
            ("@|", "@"),
            ("see @src/ma|", "@src/ma"),
            ("see @src/ma| tail", "@src/ma"),
            ("email a@b|", "-"),
            ("@done |", "-"),
            ("/|", "/"),
            ("/mod|", "/mod"),
            ("line\n/cl|", "/cl"),
            ("say /cl|", "-"),
            ("fix #12|", "#12"),
            ("$sk|", "$sk"),
            ("[@chip]@x|", "@x"),
            ("[@chip]|", "-"),
            ("caf\u{e9} @\u{65e5}\u{672c}|", "@\u{65e5}\u{672c}"),
        ];
        for (marked, expected) in cases {
            assert_eq!(detect(marked), expected, "{marked:?}");
        }
    }
}
