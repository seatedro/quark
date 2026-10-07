//! Word-level changes inside a pair of changed lines.

use std::ops::Range;

use unicode_segmentation::UnicodeSegmentation;

use crate::myers::{diff, intern};

/// Lines longer than this get no word diff: the token diff is quadratic in
/// the worst case and such lines are rarely read word by word.
pub const MAX_INLINE_LINE_BYTES: usize = 2_000;

/// Below this share of unchanged bytes on either side the lines count as
/// rewritten, and highlighting scattered common words would be noise.
const MIN_KEPT_SHARE: f32 = 0.3;

/// Changed byte ranges of an old and a new line. Ranges are sorted,
/// disjoint, on char boundaries, and never touch.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InlineDiff {
    pub old: Vec<Range<u32>>,
    pub new: Vec<Range<u32>>,
}

/// The words that differ between `old` and `new`. Empty when the lines are
/// equal, too long, or too different to compare word by word.
pub fn inline_diff(old: &str, new: &str) -> InlineDiff {
    if old == new || old.len() > MAX_INLINE_LINE_BYTES || new.len() > MAX_INLINE_LINE_BYTES {
        return InlineDiff::default();
    }
    // UAX #29 words: identifiers, numbers, and CJK runs stay whole, and
    // punctuation, spaces, and emoji are tokens of their own.
    let old_tokens: Vec<(usize, &str)> = old.split_word_bound_indices().collect();
    let new_tokens: Vec<(usize, &str)> = new.split_word_bound_indices().collect();
    let (a, b) = intern(
        old_tokens.iter().map(|t| t.1),
        new_tokens.iter().map(|t| t.1),
    );
    let mut out = InlineDiff::default();
    let byte_range = |tokens: &[(usize, &str)], line: &str, r: &Range<u32>| -> Range<u32> {
        let start = tokens.get(r.start as usize).map_or(line.len(), |t| t.0);
        let end = tokens.get(r.end as usize).map_or(line.len(), |t| t.0);
        start as u32..end as u32
    };
    for change in diff(&a, &b) {
        push_joined(&mut out.old, byte_range(&old_tokens, old, &change.old), old);
        push_joined(&mut out.new, byte_range(&new_tokens, new, &change.new), new);
    }
    let changed = |ranges: &[Range<u32>]| ranges.iter().map(|r| r.len()).sum::<usize>();
    let kept = |line: &str, ranges: &[Range<u32>]| {
        if line.is_empty() {
            1.0
        } else {
            1.0 - changed(ranges) as f32 / line.len() as f32
        }
    };
    if kept(old, &out.old) < MIN_KEPT_SHARE || kept(new, &out.new) < MIN_KEPT_SHARE {
        return InlineDiff::default();
    }
    out
}

/// Appends `range` unless empty, merging it into the previous range when
/// only whitespace separates them, so `a b` replaced by `c d` reads as one
/// change.
fn push_joined(ranges: &mut Vec<Range<u32>>, range: Range<u32>, line: &str) {
    if range.is_empty() {
        return;
    }
    if let Some(last) = ranges.last_mut() {
        let between = &line[last.end as usize..range.start as usize];
        if between.chars().all(char::is_whitespace) {
            last.end = range.end;
            return;
        }
    }
    ranges.push(range);
}

#[cfg(test)]
mod tests {
    use super::inline_diff;

    /// Each side with changed ranges in brackets.
    fn marked(old: &str, new: &str) -> String {
        let d = inline_diff(old, new);
        let mark = |line: &str, ranges: &[std::ops::Range<u32>]| {
            let mut out = String::new();
            let mut at = 0;
            for r in ranges {
                out.push_str(&line[at..r.start as usize]);
                out.push('[');
                out.push_str(&line[r.start as usize..r.end as usize]);
                out.push(']');
                at = r.end as usize;
            }
            out.push_str(&line[at..]);
            out
        };
        format!("{} | {}", mark(old, &d.old), mark(new, &d.new))
    }

    #[test]
    fn changed_words_are_bracketed_on_both_sides() {
        let cases = [
            (
                "let old = 1;",
                "let new = 1;",
                "let [old] = 1; | let [new] = 1;",
            ),
            ("foo(a, b)", "foo(a, b, c)", "foo(a, b) | foo(a, b[, c])"),
            (
                "x = alpha beta;",
                "x = gamma delta;",
                "x = [alpha beta]; | x = [gamma delta];",
            ),
            ("naïve café", "naïve cafés", "naïve [café] | naïve [cafés]"),
            ("say 👋 hi", "say 🎉 hi", "say [👋] hi | say [🎉] hi"),
            ("same", "same", "same | same"),
            // Too different: no word highlights.
            (
                "fn parse(input)",
                "struct Token {",
                "fn parse(input) | struct Token {",
            ),
        ];
        for (old, new, expected) in cases {
            assert_eq!(marked(old, new), expected, "{old} -> {new}");
        }
    }
}
