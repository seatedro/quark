//! Highlight span types, the highlighter APIs apps plug in, and their
//! conversion to layout spans.

use std::ops::Range;
use std::sync::Arc;

use quark_render::scene::{FontKind, FontStyle, FontWeight};
use quark_text::TextSpan;

use crate::text_input::styles::{InlineStyle, StyleSpan};

/// Syntax category of a highlighted span. The highlighter assigns it and the
/// renderer maps it to a theme color.
#[repr(u8)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SyntaxTokenKind {
    #[default]
    Normal = 0,
    Keyword,
    String,
    Comment,
    Number,
    Type,
    Function,
    Operator,
    Punctuation,
    Variable,
    Constant,
    Builtin,
    Attribute,
    Tag,
    Property,
    Namespace,
    Label,
    Preprocessor,
    /// A link in formatted prose.
    Link,
}

/// A highlighted byte range of the editor text.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SyntaxSpan {
    pub offset: u32,
    pub length: u32,
    pub kind: SyntaxTokenKind,
}

/// Produces syntax spans for the editor text, in code modes. Up to 256 KiB
/// it gets the whole text, again after each change; a longer text is
/// handed the lines around the view instead (offsets relative to them), so
/// highlighting stays proportional to the view. A [`LineHighlighter`]
/// highlights exactly at any length.
pub type SyntaxHighlighter = Arc<dyn Fn(&str) -> Vec<SyntaxSpan> + Send + Sync>;

/// Produces display styles for the editor text in prose modes, the way a
/// [`SyntaxHighlighter`] does for code. They stay out of undo and
/// [`super::Editor::rich_text`].
pub type StyleHighlighter = Arc<dyn Fn(&str) -> Vec<StyleSpan> + Send + Sync>;

/// A highlighter's state between lines (inside a block comment, a string,
/// ...), in whatever encoding it likes. Equal states mean equal futures:
/// when a re-highlighted line ends in the state it ended in before, the
/// lines after it keep their spans.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct LineState(pub u64);

/// Highlights code one logical line at a time, so the editor re-highlights
/// only the lines an edit changed (and those whose start state it changed)
/// and only up to the view. See [`super::Editor::set_line_highlighter`].
pub trait LineHighlighter: Send + Sync {
    /// Pushes the spans of `line` (without its line ending), relative to
    /// it, onto `spans`, starting in `state`; returns the state the line
    /// ends in. The first line starts in `LineState::default()`.
    fn highlight_line(
        &self,
        line: &str,
        state: LineState,
        spans: &mut Vec<SyntaxSpan>,
    ) -> LineState;
}

/// A line or box painted with formatted or checked text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextDecoration {
    Underline,
    Strikethrough,
    /// The box behind inline code.
    CodeBackground,
    /// The wavy line under a misspelled word.
    Misspelled,
}

/// Layout spans (weight and slant only) for the valid syntax spans of
/// `text`, plus each kept span's kind so paint can color `GlyphRun::span`
/// indices. Appends to `spans` and `kinds`.
pub(crate) fn push_syntax_layout_spans(
    text: &str,
    syntax_spans: &[SyntaxSpan],
    spans: &mut Vec<TextSpan>,
    kinds: &mut Vec<SyntaxTokenKind>,
) {
    for span in syntax_spans {
        let start = span.offset as usize;
        let end = start.saturating_add(span.length as usize).min(text.len());
        let valid = start < end && text.is_char_boundary(start) && text.is_char_boundary(end);
        if !valid {
            continue;
        }
        let (weight, style) = syntax_font(span.kind);
        spans.push(TextSpan {
            range: start..end,
            weight,
            style,
            kind: None,
            size: None,
            letter_spacing: None,
            keep_together: false,
        });
        kinds.push(span.kind);
    }
}

/// [`push_syntax_layout_spans`] into new vectors.
pub(crate) fn syntax_layout_spans(
    text: &str,
    syntax_spans: &[SyntaxSpan],
) -> (Vec<TextSpan>, Vec<SyntaxTokenKind>) {
    let mut spans = Vec::with_capacity(syntax_spans.len());
    let mut kinds = Vec::with_capacity(syntax_spans.len());
    push_syntax_layout_spans(text, syntax_spans, &mut spans, &mut kinds);
    (spans, kinds)
}

fn syntax_font(kind: SyntaxTokenKind) -> (Option<FontWeight>, Option<FontStyle>) {
    use SyntaxTokenKind::*;
    match kind {
        Comment => (None, Some(FontStyle::Italic)),
        Keyword | Builtin => (Some(FontWeight::Semibold), None),
        Type | Function | Constant | Attribute | Tag | Property | Namespace | Label
        | Preprocessor => (Some(FontWeight::Medium), None),
        Normal | String | Number | Operator | Punctuation | Variable | Link => (None, None),
    }
}

/// Layout spans for formatted prose: weight, slant, and the mono family
/// for code, plus each span's kind (links are colored). Appends to `spans`
/// and `kinds`.
pub(crate) fn push_style_layout_spans(
    styles: &[StyleSpan],
    spans: &mut Vec<TextSpan>,
    kinds: &mut Vec<SyntaxTokenKind>,
) {
    for span in styles {
        let style = span.format.style;
        spans.push(TextSpan {
            range: span.range.clone(),
            weight: style
                .contains(InlineStyle::BOLD)
                .then_some(FontWeight::Bold),
            style: style
                .contains(InlineStyle::ITALIC)
                .then_some(FontStyle::Italic),
            kind: style.contains(InlineStyle::CODE).then_some(FontKind::Mono),
            size: None,
            letter_spacing: None,
            keep_together: false,
        });
        kinds.push(match span.format.link {
            Some(_) => SyntaxTokenKind::Link,
            None => SyntaxTokenKind::Normal,
        });
    }
}

/// The highlight style flags added to the text's own formatting. Layout
/// spans replace each other where they overlap, so the result has no
/// overlaps.
pub(crate) fn merge_highlight_styles(
    text: &str,
    document: &[StyleSpan],
    highlight: &[StyleSpan],
) -> Vec<StyleSpan> {
    let valid = |r: &Range<usize>| {
        r.start < r.end
            && r.end <= text.len()
            && text.is_char_boundary(r.start)
            && text.is_char_boundary(r.end)
    };
    // (offset, starts, flags): ends sort before starts at the same offset.
    let mut events: Vec<(usize, bool, InlineStyle)> = Vec::new();
    for span in highlight
        .iter()
        .filter(|s| valid(&s.range) && !s.format.style.is_plain())
    {
        events.push((span.range.start, true, span.format.style));
        events.push((span.range.end, false, span.format.style));
    }
    events.sort_by_key(|&(at, starts, _)| (at, starts));
    let mut cuts: Vec<usize> = events.iter().map(|e| e.0).collect();
    cuts.extend(document.iter().flat_map(|s| [s.range.start, s.range.end]));
    cuts.sort_unstable();
    cuts.dedup();

    const FLAGS: [InlineStyle; 5] = [
        InlineStyle::BOLD,
        InlineStyle::ITALIC,
        InlineStyle::UNDERLINE,
        InlineStyle::STRIKE,
        InlineStyle::CODE,
    ];
    // Open count per flag, so overlapping highlight spans union.
    let mut open = [0u32; FLAGS.len()];
    let mut next_event = 0;
    let mut doc = 0;
    let mut merged: Vec<StyleSpan> = Vec::new();
    for (&start, &end) in cuts.iter().zip(cuts.iter().skip(1)) {
        while let Some(&(at, starts, style)) = events.get(next_event)
            && at <= start
        {
            for (flag, count) in FLAGS.iter().zip(&mut open) {
                if style.contains(*flag) {
                    if starts {
                        *count += 1;
                    } else {
                        *count -= 1;
                    }
                }
            }
            next_event += 1;
        }
        while document.get(doc).is_some_and(|s| s.range.end <= start) {
            doc += 1;
        }
        let mut format = document
            .get(doc)
            .filter(|s| s.range.start <= start)
            .map(|s| s.format.clone())
            .unwrap_or_default();
        for (flag, &count) in FLAGS.iter().zip(&open) {
            if count > 0 {
                format.style = format.style.union(*flag);
            }
        }
        if format.is_plain() {
            continue;
        }
        match merged.last_mut() {
            Some(last) if last.range.end == start && last.format == format => last.range.end = end,
            _ => merged.push(StyleSpan {
                range: start..end,
                format,
            }),
        }
    }
    merged
}

/// Spans moved to match `replaced` becoming `inserted_len` bytes (an IME
/// preedit spliced in). Spans touching the replaced range drop out.
pub(crate) fn shift_spans(
    spans: &[TextSpan],
    kinds: &[SyntaxTokenKind],
    replaced: Range<usize>,
    inserted_len: usize,
) -> (Vec<TextSpan>, Arc<[SyntaxTokenKind]>) {
    let mut out = Vec::with_capacity(spans.len());
    let mut out_kinds = Vec::with_capacity(spans.len());
    for (span, kind) in spans.iter().zip(kinds) {
        let range = if span.range.end <= replaced.start {
            span.range.clone()
        } else if span.range.start >= replaced.end {
            let shift = |o: usize| o - replaced.len() + inserted_len;
            shift(span.range.start)..shift(span.range.end)
        } else {
            continue;
        };
        out.push(TextSpan {
            range,
            ..span.clone()
        });
        out_kinds.push(*kind);
    }
    (out, out_kinds.into())
}

/// `spans` (absolute, any order) cut at line breaks of `rope` and sorted,
/// so the pieces of a line are found by binary search. `range_of` and
/// `with_range` read and rebuild a span's byte range.
pub(crate) fn split_at_lines<T: Clone>(
    rope: &crate::text_input::Rope,
    spans: &mut Vec<T>,
    range_of: impl Fn(&T) -> Range<usize>,
    with_range: impl Fn(&T, Range<usize>) -> T,
) {
    let len = spans.len();
    for i in 0..len {
        let Some(span) = spans.get(i).cloned() else {
            continue;
        };
        let range = range_of(&span);
        let (first, last) = (rope.byte_to_line(range.start), rope.byte_to_line(range.end));
        if first == last {
            continue;
        }
        for line in first..=last {
            let lines = rope.line_byte_range(line);
            let piece = range.start.max(lines.start)..range.end.min(lines.end);
            if piece.is_empty() {
                continue;
            }
            if line == first {
                if let Some(slot) = spans.get_mut(i) {
                    *slot = with_range(&span, piece);
                }
            } else {
                spans.push(with_range(&span, piece));
            }
        }
    }
    spans.sort_by_key(|s| range_of(s).start);
}
