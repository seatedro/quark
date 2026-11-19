//! Line and word navigation on a [`Rope`], with the same results as
//! `quark_text::offset`'s on the whole text as one string.
//!
//! Word and line boundaries never cross a line break (UAX #29 breaks
//! around `'\n'`, and words do not span one), so each function reads only
//! the line it is on, or the next lines while it looks for a word. Results
//! are snapped onto grapheme boundaries of the whole text.

use std::ops::Range;

use quark_text::offset;

use super::Rope;

/// Whether `text` has a word (a run with a letter, digit, or `_`), as
/// `quark_text::offset`'s word navigation counts them.
fn has_word(text: &str) -> bool {
    text.chars().any(|c| c.is_alphanumeric() || c == '_')
}

impl Rope {
    /// Where the `'\n'` ending line `line` is, or the end of the text for
    /// the last line.
    fn line_break_at(&self, line: usize) -> usize {
        if line < self.summary().newlines {
            self.line_to_byte(line + 1) - 1
        } else {
            self.len()
        }
    }

    /// Start of the line holding `byte` (snapped to a grapheme boundary).
    pub fn line_start_at(&self, byte: usize) -> usize {
        self.line_to_byte(self.byte_to_line(self.floor_grapheme(byte)))
    }

    /// End of the line holding `byte`, before its line ending: a `'\r'`
    /// before the `'\n'` belongs to the ending.
    pub fn line_end_at(&self, byte: usize) -> usize {
        let line = self.byte_to_line(self.floor_grapheme(byte));
        self.floor_grapheme(self.line_break_at(line))
    }

    /// The line holding `byte`, including its `'\n'`.
    pub fn line_range_at(&self, byte: usize) -> Range<usize> {
        let line = self.byte_to_line(self.floor_grapheme(byte));
        self.line_to_byte(line)..self.floor_grapheme(self.line_to_byte(line + 1))
    }

    /// Start of the word before `byte`.
    pub fn prev_word_start(&self, byte: usize) -> usize {
        let at = self.floor_grapheme(byte);
        let mut line = self.byte_to_line(at);
        let mut end = at;
        loop {
            let start = self.line_to_byte(line);
            let piece = self.slice(start..end);
            if has_word(&piece) {
                let local = offset::prev_word_start(&piece, piece.len()).get();
                return self.floor_grapheme(start + local);
            }
            let Some(prev) = line.checked_sub(1) else {
                return 0;
            };
            line = prev;
            end = self.line_break_at(line);
        }
    }

    /// Start of the word after `byte`: past the rest of the current word,
    /// then any separators.
    pub fn next_word_start(&self, byte: usize) -> usize {
        let at = self.floor_grapheme(byte);
        let mut line = self.byte_to_line(at);
        let piece = self.slice(at..self.line_break_at(line));
        let local = offset::next_word_start(&piece, 0).get();
        if local < piece.len() {
            return self.floor_grapheme(at + local);
        }
        // Past the line break every word starts a new one: the first word
        // of a later line.
        while line < self.summary().newlines {
            line += 1;
            let start = self.line_to_byte(line);
            let piece = self.slice(start..self.line_break_at(line));
            if has_word(&piece) {
                let end = offset::next_word_end(&piece, 0);
                let local = offset::prev_word_start(&piece, end).get();
                return self.floor_grapheme(start + local);
            }
        }
        self.len()
    }

    /// End of the word after `byte`: past any separators, then the word.
    pub fn next_word_end(&self, byte: usize) -> usize {
        let at = self.floor_grapheme(byte);
        let mut line = self.byte_to_line(at);
        let mut start = at;
        loop {
            let piece = self.slice(start..self.line_break_at(line));
            if has_word(&piece) {
                let local = offset::next_word_end(&piece, 0).get();
                return self.floor_grapheme(start + local);
            }
            if line >= self.summary().newlines {
                return self.len();
            }
            line += 1;
            start = self.line_to_byte(line);
        }
    }

    /// The word run around `byte` for double-click selection; see
    /// `quark_text::offset::word_range_at`.
    pub fn word_range_at(&self, byte: usize) -> Range<usize> {
        let at = self.floor_grapheme(byte);
        let line = self.byte_to_line(at);
        let start = self.line_to_byte(line);
        let piece = self.slice(start..self.line_break_at(line));
        let local = offset::word_range_at(&piece, at - start);
        self.floor_grapheme(start + local.start.get())..self.floor_grapheme(start + local.end.get())
    }
}
