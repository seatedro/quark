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
    use std::ops::Range;

    use proptest::prelude::*;
    use unicode_segmentation::UnicodeSegmentation;

    use super::{InlineDetail, InlineMode, InlineOptions, inline_diff, paired_inline_diff};

    /// Each side with changed ranges in brackets.
    fn marked(old: &str, new: &str) -> String {
        let d = inline_diff(old, new);
        bracket(old, new, &d.old, &d.new)
    }

    fn marked_by(old: &str, new: &str, mode: InlineMode) -> String {
        let options = InlineOptions {
            mode,
            ..InlineOptions::default()
        };
        let d = paired_inline_diff(old, new, &options);
        bracket(old, new, &d.old, &d.new)
    }

    fn bracket(
        old: &str,
        new: &str,
        old_ranges: &[Range<u32>],
        new_ranges: &[Range<u32>],
    ) -> String {
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
        format!("{} | {}", mark(old, old_ranges), mark(new, new_ranges))
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

    #[test]
    fn grapheme_mode_marks_whole_user_visible_characters() {
        let family_a = "the family emoji \u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467} here";
        let family_b = "the family emoji \u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F466} here";
        let cases = [
            (
                "let count = 1;",
                "let counter = 1;",
                "let count = 1; | let count[er] = 1;",
            ),
            ("colour", "color", "colo[u]r | color"),
            // The accent is a combining mark: it changes with its base.
            ("cafe\u{301}", "cafe", "caf[e\u{301}] | caf[e]"),
        ];
        for (old, new, expected) in cases {
            assert_eq!(
                marked_by(old, new, InlineMode::Grapheme),
                expected,
                "{old} -> {new}"
            );
        }
        let marked = marked_by(family_a, family_b, InlineMode::Grapheme);
        assert_eq!(
            marked,
            "the family emoji [\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}] here | \
             the family emoji [\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F466}] here"
        );
    }

    #[test]
    fn a_pair_without_ranges_says_why() {
        let long = "x".repeat(2_001);
        let cases = [
            ("a b", "a c", InlineMode::Off, InlineDetail::Off),
            (long.as_str(), "x", InlineMode::Word, InlineDetail::Limited),
            (
                "fn parse(input)",
                "struct Token {",
                InlineMode::Word,
                InlineDetail::Rewritten,
            ),
            ("same", "same", InlineMode::Grapheme, InlineDetail::Exact),
        ];
        for (old, new, mode, expected) in cases {
            let options = InlineOptions {
                mode,
                ..InlineOptions::default()
            };
            let d = paired_inline_diff(old, new, &options);
            assert_eq!(
                (d.detail, d.old.len() + d.new.len()),
                (expected, 0),
                "{mode:?}"
            );
        }
    }

    fn line() -> impl Strategy<Value = String> {
        let unit = prop::sample::select(vec![
            "a",
            "ab",
            " ",
            ",",
            "\u{e9}",
            "e\u{301}",
            "\u{1F468}\u{200D}\u{1F469}",
            "\u{1F44B}",
        ]);
        prop::collection::vec(unit, 0..12).prop_map(|units| units.concat())
    }

    proptest! {
        #[test]
        fn inline_ranges_are_ordered_apart_and_on_grapheme_boundaries(
            old in line(),
            new in line(),
            grapheme in any::<bool>(),
        ) {
            let mode = if grapheme { InlineMode::Grapheme } else { InlineMode::Word };
            let options = InlineOptions { mode, min_kept_share: 0.0, ..InlineOptions::default() };
            let d = paired_inline_diff(&old, &new, &options);
            for (line, ranges) in [(&old, &d.old), (&new, &d.new)] {
                let bounds: Vec<usize> = line
                    .grapheme_indices(true)
                    .map(|(i, _)| i)
                    .chain([line.len()])
                    .collect();
                let mut end = None;
                for r in ranges {
                    prop_assert!(r.start < r.end);
                    prop_assert!(end.is_none_or(|e| e < r.start), "{:?} touch", ranges);
                    prop_assert!(bounds.contains(&(r.start as usize)));
                    prop_assert!(bounds.contains(&(r.end as usize)));
                    end = Some(r.end);
                }
            }
        }
    }
}
