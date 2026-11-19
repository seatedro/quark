//! Highlighting kept per line and per window, so a change re-highlights
//! what it touched and the view, never the whole document.
//!
//! A whole-text highlighter ([`SyntaxHighlighter`], [`StyleHighlighter`])
//! sees the whole text up to [`WHOLE_TEXT_MAX_BYTES`], and the lines
//! around the view past that. A [`LineHighlighter`] keeps each line's
//! spans with the state it started and ended in: lines before `frontier`
//! are known to start in the state the line before them ended in, so a
//! change only moves the frontier back to its first line, and walking it
//! forward again re-highlights a line only when its text or start state
//! differs. Lines past a far-behind frontier are highlighted from an
//! assumed state until the frontier (moved on by a budget each flush)
//! reaches them.

use std::borrow::Cow;
use std::ops::Range;
use std::sync::Arc;

use quark_text::TextSpan;

use super::EditorMode;
use super::spans::{
    LineHighlighter, LineState, StyleHighlighter, SyntaxHighlighter, SyntaxSpan, SyntaxTokenKind,
    merge_highlight_styles, push_style_layout_spans, push_syntax_layout_spans, split_at_lines,
};
use crate::text_input::doc_text::{DocText, TextChange};
use crate::text_input::rope::Rope;
use crate::text_input::styles::StyleSpan;

/// The longest text a whole-text highlighter is handed whole.
pub(super) const WHOLE_TEXT_MAX_BYTES: usize = 256 * 1024;
/// Lines around the view a long text's whole-text highlighter also sees.
const WINDOW_MARGIN_LINES: usize = 64;
/// Lines the line highlighter's frontier may re-highlight per refresh.
const FRONTIER_BUDGET: usize = 2048;

/// Where code spans come from.
#[derive(Clone)]
pub(super) enum SyntaxSource {
    Whole(SyntaxHighlighter),
    Lines(Arc<dyn LineHighlighter>),
}

/// Spans a whole-text highlighter made over `covered` bytes, cut at line
/// breaks and sorted by start.
#[derive(Clone)]
struct Covered<T> {
    /// `None` until made, and once the text changes.
    covered: Option<Range<usize>>,
    spans: Arc<[T]>,
}

impl<T> Default for Covered<T> {
    fn default() -> Self {
        Self {
            covered: None,
            spans: Arc::from(Vec::new()),
        }
    }
}

impl<T> Covered<T> {
    fn covers(&self, range: &Range<usize>) -> bool {
        self.covered
            .as_ref()
            .is_some_and(|c| c.start <= range.start && range.end <= c.end)
    }
}

/// One line's spans from a [`LineHighlighter`].
#[derive(Debug)]
struct LineSpans {
    start: LineState,
    end: LineState,
    spans: Box<[SyntaxSpan]>,
}

#[derive(Clone, Default)]
struct LineCache {
    /// One per line while a line highlighter is set, else empty.
    entries: Vec<Option<Arc<LineSpans>>>,
    /// Lines before it start in the state the line before them ended in.
    frontier: usize,
}

#[derive(Clone, Default)]
pub(super) struct Highlighting {
    syntax: Option<SyntaxSource>,
    style: Option<StyleHighlighter>,
    whole_syntax: Covered<SyntaxSpan>,
    whole_styles: Covered<StyleSpan>,
    lines: LineCache,
    scratch: Vec<SyntaxSpan>,
    /// Bumped whenever the spans some line would get may have changed.
    generation: u64,
}

impl Highlighting {
    pub(super) fn generation(&self) -> u64 {
        self.generation
    }

    /// The text's own formatting changed: every line's spans may have.
    pub(super) fn formatting_changed(&mut self) {
        self.generation += 1;
    }

    fn invalidate(&mut self) {
        self.whole_syntax.covered = None;
        self.whole_styles.covered = None;
        self.generation += 1;
    }

    pub(super) fn has_syntax(&self) -> bool {
        self.syntax.is_some()
    }

    pub(super) fn has_style(&self) -> bool {
        self.style.is_some()
    }

    pub(super) fn set_syntax(&mut self, source: SyntaxSource, rope: &Rope) {
        self.lines = LineCache::default();
        if matches!(source, SyntaxSource::Lines(_)) {
            self.lines.entries = vec![None; rope.line_count()];
        }
        self.syntax = Some(source);
        self.whole_syntax = Covered::default();
        self.invalidate();
    }

    pub(super) fn clear_syntax(&mut self) {
        self.syntax = None;
        self.lines = LineCache::default();
        self.whole_syntax = Covered::default();
        self.invalidate();
    }

    pub(super) fn set_style(&mut self, highlighter: StyleHighlighter) {
        self.style = Some(highlighter);
        self.whole_styles = Covered::default();
        self.invalidate();
    }

    pub(super) fn clear_style(&mut self) {
        self.style = None;
        self.whole_styles = Covered::default();
        self.invalidate();
    }

    /// Follow a change of the text (now `rope`).
    pub(super) fn text_changed(&mut self, change: TextChange, rope: &Rope) {
        self.invalidate();
        if self.lines.entries.is_empty() && !matches!(self.syntax, Some(SyntaxSource::Lines(_))) {
            return;
        }
        let lines = &mut self.lines;
        match change {
            TextChange::Lines {
                first,
                removed,
                inserted,
            } if first + removed <= lines.entries.len() => {
                lines
                    .entries
                    .splice(first..first + removed, std::iter::repeat_n(None, inserted));
                lines.frontier = lines.frontier.min(first);
            }
            _ => {
                lines.entries.clear();
                lines.entries.resize(rope.line_count(), None);
                lines.frontier = 0;
            }
        }
    }

    /// The spans a whole-text highlighter made, as of the last refresh.
    pub(super) fn whole_syntax_spans(&self) -> &Arc<[SyntaxSpan]> {
        &self.whole_syntax.spans
    }

    /// Bring highlighting up to date for `window` (lines of `doc`).
    /// Returns whether lines past the window are still being caught up.
    pub(super) fn refresh(
        &mut self,
        doc: &DocText,
        mode: EditorMode,
        window: Range<usize>,
    ) -> bool {
        let rope = doc.rope();
        let bytes = rope.line_to_byte(window.start)..rope.line_to_byte(window.end);
        if mode.is_code() {
            match self.syntax.clone() {
                Some(SyntaxSource::Whole(highlighter)) => {
                    self.refresh_whole_syntax(doc, &highlighter, bytes)
                }
                Some(SyntaxSource::Lines(highlighter)) => {
                    return self.refresh_lines(rope, &*highlighter, window);
                }
                None => {}
            }
        } else if let Some(highlighter) = self.style.clone() {
            self.refresh_whole_styles(doc, &highlighter, bytes);
        }
        false
    }

    /// The text a whole-text highlighter gets for `view`: all of it when
    /// short, else the lines around `view`. Returns where it starts.
    fn whole_text_for<'a>(doc: &'a DocText, view: &Range<usize>) -> (Range<usize>, Cow<'a, str>) {
        if doc.len() <= WHOLE_TEXT_MAX_BYTES {
            return (0..doc.len(), Cow::Borrowed(doc.as_str()));
        }
        let rope = doc.rope();
        let first = rope
            .byte_to_line(view.start)
            .saturating_sub(WINDOW_MARGIN_LINES);
        let last = rope.byte_to_line(view.end) + WINDOW_MARGIN_LINES;
        let range = rope.line_to_byte(first)..rope.line_to_byte(last + 1);
        let text = rope.slice(range.clone());
        (range, text)
    }

    fn refresh_whole_syntax(
        &mut self,
        doc: &DocText,
        highlighter: &SyntaxHighlighter,
        view: Range<usize>,
    ) {
        if self.whole_syntax.covers(&view) {
            return;
        }
        let (range, text) = Self::whole_text_for(doc, &view);
        let mut spans = highlighter(&text);
        let base = range.start as u32;
        for span in &mut spans {
            span.offset = span.offset.saturating_add(base);
        }
        split_at_lines(
            doc.rope(),
            &mut spans,
            |s| s.offset as usize..(s.offset as usize).saturating_add(s.length as usize),
            |s, r| SyntaxSpan {
                offset: r.start as u32,
                length: r.len() as u32,
                kind: s.kind,
            },
        );
        self.whole_syntax = Covered {
            covered: Some(range),
            spans: spans.into(),
        };
        self.generation += 1;
    }

    fn refresh_whole_styles(
        &mut self,
        doc: &DocText,
        highlighter: &StyleHighlighter,
        view: Range<usize>,
    ) {
        if self.whole_styles.covers(&view) {
            return;
        }
        let (range, text) = Self::whole_text_for(doc, &view);
        let mut spans = highlighter(&text);
        for span in &mut spans {
            span.range = span.range.start + range.start..span.range.end + range.start;
        }
        split_at_lines(
            doc.rope(),
            &mut spans,
            |s| s.range.clone(),
            |s, r| StyleSpan {
                range: r,
                format: s.format.clone(),
            },
        );
        self.whole_styles = Covered {
            covered: Some(range),
            spans: spans.into(),
        };
        self.generation += 1;
    }

    /// Moves the frontier toward the end of `window` within the budget,
    /// then highlights what of `window` it did not reach from an assumed
    /// state. Returns whether the frontier is still short of the window.
    fn refresh_lines(
        &mut self,
        rope: &Rope,
        highlighter: &dyn LineHighlighter,
        window: Range<usize>,
    ) -> bool {
        let end = window.end.min(self.lines.entries.len());
        let reached = self.walk(
            rope,
            highlighter,
            self.lines.frontier,
            end,
            FRONTIER_BUDGET,
            true,
        );
        if reached < end {
            let from = window.start.max(reached);
            self.walk(rope, highlighter, from, end, usize::MAX, false);
        }
        reached < end
    }

    /// Walks lines `from..to`, re-highlighting each whose entry is missing
    /// or started in another state, until `budget` lines were highlighted.
    /// Moves the frontier when `exact` (`from` must then be the frontier).
    /// Returns the line it stopped at.
    fn walk(
        &mut self,
        rope: &Rope,
        highlighter: &dyn LineHighlighter,
        from: usize,
        to: usize,
        budget: usize,
        exact: bool,
    ) -> usize {
        let mut state = from
            .checked_sub(1)
            .and_then(|prev| self.lines.entries.get(prev)?.as_ref().map(|e| e.end))
            .unwrap_or_default();
        let mut line = from;
        let mut work = 0;
        while line < to && work < budget {
            let Some(slot) = self.lines.entries.get_mut(line) else {
                break;
            };
            match slot {
                Some(entry) if entry.start == state => state = entry.end,
                _ => {
                    let text = rope.slice(rope.line_content_range(line));
                    self.scratch.clear();
                    let end = highlighter.highlight_line(&text, state, &mut self.scratch);
                    *slot = Some(Arc::new(LineSpans {
                        start: state,
                        end,
                        spans: self.scratch.as_slice().into(),
                    }));
                    state = end;
                    work += 1;
                }
            }
            line += 1;
        }
        if work > 0 {
            self.generation += 1;
        }
        if exact {
            self.lines.frontier = line;
        }
        line
    }

    /// Appends the layout spans and their kinds for line `line` of `rope`,
    /// whose text without its line ending is `text`. `document` is the
    /// text's own formatting (prose).
    pub(super) fn line_spans(
        &mut self,
        rope: &Rope,
        mode: EditorMode,
        line: usize,
        text: &str,
        document: &[StyleSpan],
        out: &mut LayoutSpans,
    ) {
        let LayoutSpans { spans, kinds } = out;
        let bytes = rope.line_byte_range(line);
        if mode.is_code() {
            match &self.syntax {
                Some(SyntaxSource::Whole(_)) => {
                    let pieces = pieces_in(&self.whole_syntax.spans, &bytes, |s| s.offset as usize);
                    let base = bytes.start as u32;
                    self.scratch.clear();
                    self.scratch.extend(pieces.iter().map(|s| SyntaxSpan {
                        offset: s.offset - base,
                        ..*s
                    }));
                    push_syntax_layout_spans(text, &self.scratch, spans, kinds);
                }
                Some(SyntaxSource::Lines(highlighter)) => {
                    let highlighter = highlighter.clone();
                    if self.lines.entries.get(line).is_some_and(Option::is_none) {
                        self.walk(rope, &*highlighter, line, line + 1, 1, false);
                    }
                    if let Some(Some(entry)) = self.lines.entries.get(line) {
                        push_syntax_layout_spans(text, &entry.spans, spans, kinds);
                    }
                }
                None => {}
            }
            return;
        }
        let content = rope.line_content_range(line);
        // The text's own runs are sorted and disjoint, so their ends are too.
        let first = document.partition_point(|s| s.range.end <= content.start);
        let document = document.get(first..).unwrap_or_default();
        let document = relative(clip(document, &content), content.start);
        let highlight = pieces_in(&self.whole_styles.spans, &bytes, |s| s.range.start);
        if highlight.is_empty() || self.style.is_none() {
            push_style_layout_spans(&document, spans, kinds);
        } else {
            let highlight = relative(clip(highlight, &content), content.start);
            let merged = merge_highlight_styles(text, &document, &highlight);
            push_style_layout_spans(&merged, spans, kinds);
        }
    }
}

/// A line's layout spans and the token kind of each.
#[derive(Debug, Default)]
pub(super) struct LayoutSpans {
    pub spans: Vec<TextSpan>,
    pub kinds: Vec<SyntaxTokenKind>,
}

/// The pieces of `spans` (sorted by `start_of`, none crossing a line
/// break) that start inside `line`.
fn pieces_in<'a, T>(
    spans: &'a [T],
    line: &Range<usize>,
    start_of: impl Fn(&T) -> usize,
) -> &'a [T] {
    let first = spans.partition_point(|s| start_of(s) < line.start);
    let last = spans.partition_point(|s| start_of(s) < line.end);
    spans.get(first..last.max(first)).unwrap_or_default()
}

/// The runs of `styles` (sorted by start) that overlap `range`, cut to it.
fn clip(styles: &[StyleSpan], range: &Range<usize>) -> Vec<StyleSpan> {
    styles
        .iter()
        .take_while(|s| s.range.start < range.end)
        .filter(|s| s.range.end > range.start)
        .map(|s| StyleSpan {
            range: s.range.start.max(range.start)..s.range.end.min(range.end),
            format: s.format.clone(),
        })
        .collect()
}

/// `styles` moved to be relative to `base`.
fn relative(mut styles: Vec<StyleSpan>, base: usize) -> Vec<StyleSpan> {
    for span in &mut styles {
        span.range = span.range.start - base..span.range.end - base;
    }
    styles
}
