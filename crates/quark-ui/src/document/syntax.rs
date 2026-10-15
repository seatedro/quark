//! Code block highlighting for markdown rows.
//!
//! Quark has no highlighting engine of its own. The app supplies one as a
//! [`CodeHighlighter`] ([`SyntaxHighlighter::with`],
//! [`super::MarkdownDocument::set_code_highlighter`]); without one every
//! block renders plain. A code block renders plain (or with its previous
//! highlight, while its source only grew) until its result arrives; the
//! app calls [`SyntaxHighlighter::poll`] each frame and rebuilds its
//! markdown rows when it returns blocks.

use std::ops::Range;

use quark::selection::BlockKey;
use quark_render::FontKind;

use super::{SpanTone, SyntaxTone};
use crate::element::StyledSpan;

/// One line of code as spans with their tones.
pub(crate) type CodeLine = Vec<(StyledSpan, SpanTone)>;

/// The bytes `range` of a code block's source take the color of `tone`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToneSpan {
    pub range: Range<u32>,
    pub tone: SyntaxTone,
}

/// A highlighting engine for document code blocks, keyed by block.
///
/// The document calls [`Self::version`] for every code block each time it
/// rebuilds the block's row, and reads [`Self::spans`] only when that
/// returned a nonzero version, so `version` must not allocate when the
/// source is unchanged. Engines that work off the UI thread deliver
/// results through [`Self::poll`], which the app calls each frame.
pub trait CodeHighlighter {
    /// Requests a highlight of `code`, in the fence language `lang`, for
    /// the block `slot` when its source or language changed, and returns
    /// the version of the highlight that applies to `code` now: 0 for
    /// plain. A held result for an earlier source may still apply while
    /// the source only grew. Every result taken must get a distinct
    /// version, since blocks with an unchanged version are reused as
    /// they are.
    fn version(&mut self, slot: BlockKey, lang: &str, code: &str) -> u64;

    /// The spans of the highlight [`Self::version`] last reported for
    /// `slot`: sorted, disjoint, and on char boundaries of the code it
    /// was given. Spans reaching past the end of that code are clipped.
    fn spans(&self, slot: BlockKey) -> &[ToneSpan];

    /// Takes finished highlights and returns the blocks they are for.
    fn poll(&mut self) -> Vec<BlockKey>;

    /// Blocks until every requested highlight has arrived and been taken,
    /// and returns the blocks that got one, as [`Self::poll`] does.
    fn finish_pending(&mut self) -> Vec<BlockKey>;

    /// Drops everything kept for the block `slot`.
    fn forget(&mut self, slot: BlockKey);

    /// Number of blocks with kept state.
    fn len(&self) -> usize;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Plain monospace lines of `code`.
pub(crate) fn plain_lines(code: &str) -> Vec<CodeLine> {
    code.split('\n')
        .map(|line| vec![(mono(line), SpanTone::Plain)])
        .collect()
}

fn mono(text: &str) -> StyledSpan {
    StyledSpan {
        font_kind: FontKind::Mono,
        ..StyledSpan::plain(text)
    }
}

/// Highlights code blocks with the app's [`CodeHighlighter`]. One per
/// app, shared by every markdown row.
#[derive(Default)]
pub struct SyntaxHighlighter {
    inner: Option<Box<dyn CodeHighlighter>>,
}

impl SyntaxHighlighter {
    /// Renders every block plain.
    pub fn new() -> Self {
        Self::default()
    }

    /// Highlights with `highlighter`.
    pub fn with(highlighter: impl CodeHighlighter + 'static) -> Self {
        Self {
            inner: Some(Box::new(highlighter)),
        }
    }

    /// Highlights with `highlighter` from now on. Blocks are requested
    /// from it when their rows are next built.
    pub fn set(&mut self, highlighter: impl CodeHighlighter + 'static) {
        self.inner = Some(Box::new(highlighter));
    }

    /// Takes finished highlights and returns the code blocks they are
    /// for; rebuild those blocks' rows so they pick the colors up.
    pub fn poll(&mut self) -> Vec<BlockKey> {
        self.inner.as_mut().map_or_else(Vec::new, |h| h.poll())
    }

    /// Blocks until every requested highlight has arrived and been taken.
    /// For tests and screenshots, which need the colored state at once.
    /// Returns the blocks that got a highlight, as [`Self::poll`] does.
    pub fn finish_pending(&mut self) -> Vec<BlockKey> {
        self.inner
            .as_mut()
            .map_or_else(Vec::new, |h| h.finish_pending())
    }

    /// Drops everything kept for the block `slot`: its source, its
    /// highlight, and any pending request's result. Call it when the block
    /// leaves its row or its row is removed.
    pub fn forget(&mut self, slot: BlockKey) {
        if let Some(h) = &mut self.inner {
            h.forget(slot);
        }
    }

    /// Number of blocks with kept state.
    pub fn len(&self) -> usize {
        self.inner.as_ref().map_or(0, |h| h.len())
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// See [`CodeHighlighter::version`].
    pub(crate) fn version(&mut self, slot: BlockKey, lang: &str, code: &str) -> u64 {
        self.inner
            .as_mut()
            .map_or(0, |h| h.version(slot, lang, code))
    }

    /// The lines of `code` toned by the highlight `version`, which
    /// [`Self::version`] just reported for `slot`.
    pub(crate) fn lines(&self, slot: BlockKey, version: u64, code: &str) -> Vec<CodeLine> {
        match &self.inner {
            Some(h) if version != 0 => colored_lines(code, h.spans(slot)),
            _ => plain_lines(code),
        }
    }
}

/// Splits `code` into lines of spans, toning the bytes each highlight
/// span covers. `spans` are sorted and disjoint; spans reaching past the
/// end of `code` are clipped to it, and span edges off a char boundary
/// (an engine bug) leave that span plain instead of panicking.
pub(crate) fn colored_lines(code: &str, spans: &[ToneSpan]) -> Vec<CodeLine> {
    let range = |span: &ToneSpan| span.range.start as usize..span.range.end as usize;
    let mut lines = Vec::new();
    let mut next = 0;
    let mut line_start = 0;
    for line in code.split('\n') {
        let line_end = line_start + line.len();
        let mut out = Vec::new();
        let mut at = line_start;
        while next < spans.len() && range(&spans[next]).end <= line_start {
            next += 1;
        }
        for span in &spans[next..] {
            let range = range(span);
            if range.start >= line_end {
                break;
            }
            let (start, end) = (range.start.max(at), range.end.min(line_end));
            if end <= start || !code.is_char_boundary(start) || !code.is_char_boundary(end) {
                continue;
            }
            if start > at {
                out.push((mono(&code[at..start]), SpanTone::Plain));
            }
            out.push((mono(&code[start..end]), SpanTone::Syntax(span.tone)));
            at = end;
        }
        if at < line_end || out.is_empty() {
            out.push((mono(&code[at..line_end]), SpanTone::Plain));
        }
        lines.push(out);
        line_start = line_end + 1;
    }
    lines
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    /// 1- to 4-byte chars and newlines, so span edges land next to
    /// multibyte chars and line breaks.
    const PIECES: &[&str] = &["a", " ", "\n", "\u{e9}", "\u{65e5}", "\u{1f600}", "{"];

    proptest! {
        // Catches dropped, repeated, or misordered bytes when highlight
        // spans cross line breaks or sit next to multibyte chars.
        #[test]
        fn colored_lines_join_back_into_the_code(
            pieces in prop::collection::vec(prop::sample::select(PIECES), 0..24),
            cuts in prop::collection::vec(any::<prop::sample::Index>(), 0..12),
        ) {
            let code: String = pieces.concat();
            // Sorted, distinct char boundaries pair up into disjoint spans.
            let bounds: Vec<usize> = code.char_indices().map(|(i, _)| i).chain([code.len()]).collect();
            let mut cuts: Vec<usize> = cuts.iter().map(|c| bounds[c.index(bounds.len())]).collect();
            cuts.sort_unstable();
            cuts.dedup();
            let spans: Vec<ToneSpan> = cuts
                .as_chunks::<2>()
                .0
                .iter()
                .map(|&[start, end]| ToneSpan {
                    range: start as u32..end as u32,
                    tone: SyntaxTone::Keyword,
                })
                .collect();

            let lines = colored_lines(&code, &spans);

            let joined: Vec<String> = lines
                .iter()
                .map(|line| line.iter().map(|(span, _)| span.text.as_str()).collect())
                .collect();
            prop_assert_eq!(joined.join("\n"), code);
        }
    }

    // Catches a span off a char boundary panicking on the slice instead
    // of leaving its bytes plain.
    #[test]
    fn spans_off_char_boundaries_stay_plain() {
        let code = "\u{e9}a";
        let spans = [ToneSpan {
            range: 1..3,
            tone: SyntaxTone::Keyword,
        }];

        let lines = colored_lines(code, &spans);

        assert_eq!(
            lines[0]
                .iter()
                .map(|(span, tone)| (span.text.as_str(), *tone))
                .collect::<Vec<_>>(),
            [(code, SpanTone::Plain)]
        );
    }
}
