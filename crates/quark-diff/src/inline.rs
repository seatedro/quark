//! Word-level changes inside a pair of changed lines.

use std::ops::Range;

use unicode_segmentation::UnicodeSegmentation;

use crate::myers::{diff, intern};

/// Lines longer than this get no word diff by default: the token diff is
/// quadratic in the worst case and such lines are rarely read word by word.
pub const MAX_INLINE_LINE_BYTES: usize = 2_000;

/// Below this share of unchanged bytes on either side the lines count as
/// rewritten by default, and highlighting scattered common words would be
/// noise.
pub const MIN_KEPT_SHARE: f32 = 0.3;

/// Changed byte ranges of an old and a new line. Ranges are sorted,
/// disjoint, on char boundaries, and never touch.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InlineDiff {
    pub old: Vec<Range<u32>>,
    pub new: Vec<Range<u32>>,
}

/// The unit an inline diff compares.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum InlineMode {
    /// No inline detail.
    Off,
    /// Unicode words (UAX #29): identifiers, numbers, and CJK runs whole;
    /// punctuation, spaces, and emoji on their own.
    #[default]
    Word,
    /// Extended grapheme clusters: user-visible characters, never
    /// splitting an emoji sequence or a combining mark from its base.
    Grapheme,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InlineOptions {
    pub mode: InlineMode,
    /// Longer lines get no inline detail.
    pub max_line_bytes: usize,
    /// Pairs keeping less than this share of either line are rewritten.
    pub min_kept_share: f32,
}

impl Default for InlineOptions {
    fn default() -> Self {
        Self {
            mode: InlineMode::Word,
            max_line_bytes: MAX_INLINE_LINE_BYTES,
            min_kept_share: MIN_KEPT_SHARE,
        }
    }
}

/// Why a [`PairedInlineDiff`] has the ranges it has.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InlineDetail {
    /// The ranges are the changed units; none when the lines are equal.
    Exact,
    /// Too little is shared to be worth highlighting: no ranges.
    Rewritten,
    /// A line exceeds `max_line_bytes`: no ranges.
    Limited,
    /// [`InlineMode::Off`]: no ranges.
    Off,
}

/// An inline diff with the reason behind its ranges, so an omitted
/// highlight never passes for an unchanged pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairedInlineDiff {
    pub old: Vec<Range<u32>>,
    pub new: Vec<Range<u32>>,
    pub detail: InlineDetail,
}

/// The words that differ between `old` and `new`. Empty when the lines are
/// equal, too long, or too different to compare word by word.
pub fn inline_diff(old: &str, new: &str) -> InlineDiff {
    let d = paired_inline_diff(old, new, &InlineOptions::default());
    InlineDiff {
        old: d.old,
        new: d.new,
    }
}

/// The units of `options.mode` that differ between `old` and `new`, with
/// the reason when there are none to show. Ranges are UTF-8 byte ranges on
/// unit boundaries, merged across whitespace.
pub fn paired_inline_diff(old: &str, new: &str, options: &InlineOptions) -> PairedInlineDiff {
    let none = |detail| PairedInlineDiff {
        old: Vec::new(),
        new: Vec::new(),
        detail,
    };
    if options.mode == InlineMode::Off {
        return none(InlineDetail::Off);
    }
    if old.len() > options.max_line_bytes || new.len() > options.max_line_bytes {
        return none(InlineDetail::Limited);
    }
    if old == new {
        return none(InlineDetail::Exact);
    }
    let (old_tokens, new_tokens) = (tokens(old, options.mode), tokens(new, options.mode));
    let (a, b) = intern(
        old_tokens.iter().map(|t| t.1),
        new_tokens.iter().map(|t| t.1),
    );
    let mut out = none(InlineDetail::Exact);
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
    if kept(old, &out.old) < options.min_kept_share || kept(new, &out.new) < options.min_kept_share
    {
        return none(InlineDetail::Rewritten);
    }
    out
}

fn tokens(line: &str, mode: InlineMode) -> Vec<(usize, &str)> {
    match mode {
        InlineMode::Grapheme => line.grapheme_indices(true).collect(),
        _ => line.split_word_bound_indices().collect(),
    }
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
