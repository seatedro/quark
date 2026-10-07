use std::ops::Range;
use std::sync::Arc;

use quark_render::scene::{FontKind, FontStyle, FontWeight};
use quark_text::offset;
use quark_text::{TextLayout, TextOffset, TextParams, TextSpan, TextStyle, TextSystem};

use super::atoms::{InlineAtom, RichClipboard, RichText};
use super::buffer::{TextBuffer, WordForward};
use super::hooks::{InputHooks, Insertion, NoHooks};
use super::ime::{Composition, Preedit};
use super::spell::{SpellChecker, SpellResult};
use super::styles::{InlineStyle, RichExport, StyleSpan, TextFormat};
use super::text_edit::{TextEditCommand, TextEditOutcome};
use super::trigger::{TriggerMatch, TriggerRule, find_trigger};
use super::view::FrameScale;

const LINE_HEIGHT_FACTOR: f32 = 1.35;
const SYNTAX_HIGHLIGHT_MAX_BYTES: usize = 256 * 1024;

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

/// Produces syntax spans for the full editor text. Called lazily on flush
/// after the text changes, and only in code modes.
pub type SyntaxHighlighter = Arc<dyn Fn(&str) -> Vec<SyntaxSpan> + Send + Sync>;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum EditorMode {
    #[default]
    ProseInput,
    CodeInput,
    DiffReadOnly,
}

impl EditorMode {
    pub fn is_editable(self) -> bool {
        !matches!(self, Self::DiffReadOnly)
    }

    pub fn is_code(self) -> bool {
        matches!(self, Self::CodeInput | Self::DiffReadOnly)
    }

    pub(super) fn font_kind(self) -> FontKind {
        if self.is_code() {
            FontKind::Mono
        } else {
            FontKind::Ui
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SelectionRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct CursorState {
    pub x: f32,
    pub y: f32,
}

/// Multiline editor model.
///
/// Text, caret, selection, IME preedit, and undo live in the same
/// [`TextBuffer`] a [`super::TextField`] uses; the editor adds a layout,
/// vertical movement, scrolling, and syntax spans. Caret, selection,
/// hit-testing, and vertical movement all read the quark-text
/// [`TextLayout`] that [`super::TextEditorElement`] paints (see
/// [`Editor::paint_layout`]), so they cannot drift from the glyphs. The
/// layout is rebuilt by [`Editor::flush`]; call it once per frame before
/// building the element.
#[derive(Clone)]
pub struct Editor {
    mode: EditorMode,
    buffer: TextBuffer,
    /// Layout of the text, as of the last flush.
    layout: Option<Arc<TextLayout>>,
    /// Layout of the text with the preedit spliced in, while composing.
    display: Option<Arc<TextLayout>>,
    composition: Option<Composition>,
    /// The text (or the wrap width, font, or syntax) changed since the last flush.
    dirty: bool,
    syntax_dirty: bool,
    syntax_highlighter: Option<SyntaxHighlighter>,
    syntax_spans: Arc<[SyntaxSpan]>,
    /// Token kind of each span of `layout`, for paint colors.
    span_kinds: Arc<[SyntaxTokenKind]>,
    /// Token kind of each span of `display`.
    display_span_kinds: Arc<[SyntaxTokenKind]>,
    /// `(1-based logical line, top)` of each logical line of `layout`.
    line_tops: Arc<[(usize, f32)]>,
    desired_x: Option<f32>,
    reveal_cursor_on_flush: bool,
    caret_hidden: bool,
    pub scroll_y: f32,
    pub cursor_pos: CursorState,
    pub cursor_moved_at_ms: u64,
    font_size: f32,
    /// Window scale factor; the layout is shaped at it so it can be painted
    /// as is (see `ElementContext::layout_text`).
    scale_factor: f32,
    /// Scale of the frame the editor's element last painted in; applied
    /// on the next flush.
    pub(crate) frame_scale: FrameScale,
    last_width: f32,
    last_height: f32,
    /// The text system's font generation the layout was shaped with; the
    /// next flush reshapes when the fonts change.
    font_generation: Option<u64>,
    spelling: Spelling,
}

/// The editor's side of spell checking: what was sent, what came back.
#[derive(Debug, Default)]
struct Spelling {
    checker: Option<SpellChecker>,
    /// Bumped on every text change.
    rev: u64,
    /// The revision last sent to the checker.
    sent: Option<u64>,
    /// Misspelled words of revision `checked`; kept, slightly stale, until
    /// the current revision's result arrives.
    misspelled: Vec<Range<usize>>,
    checked: Option<u64>,
}

impl Clone for Spelling {
    /// A clone checks with its own clone of the checker and starts over.
    fn clone(&self) -> Self {
        Self {
            checker: self.checker.clone(),
            ..Self::default()
        }
    }
}

impl Default for Editor {
    fn default() -> Self {
        Self::new(EditorMode::default())
    }
}

impl std::fmt::Debug for Editor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Editor")
            .field("initialized", &self.layout.is_some())
            .field("mode", &self.mode)
            .field("cursor", &self.buffer.cursor())
            .field("anchor", &self.buffer.anchor())
            .field("scroll_y", &self.scroll_y)
            .finish()
    }
}

/// Width of the line-number gutter for `lines` logical lines.
pub(super) fn code_gutter_width(font_size: f32, lines: usize) -> f32 {
    let digits = gutter_digits(lines);
    let char_w = (font_size * 0.62).max(1.0);
    (digits as f32 * char_w + 18.0).ceil()
}

pub(super) fn gutter_digits(lines: usize) -> usize {
    lines.max(1).ilog10() as usize + 1
}

/// The gutter for `lines` logical lines in a view `width` wide; it never
/// takes more than about a third of the view.
pub(super) fn gutter_width_in(font_size: f32, lines: usize, width: f32) -> f32 {
    code_gutter_width(font_size, lines).min((width * 0.35).max(0.0))
}

/// Layout spans (weight and slant only) for the valid syntax spans, plus
/// each kept span's kind so paint can color `GlyphRun::span` indices.
pub(super) fn syntax_layout_spans(
    text: &str,
    syntax_spans: &[SyntaxSpan],
) -> (Vec<TextSpan>, Vec<SyntaxTokenKind>) {
    let mut spans = Vec::with_capacity(syntax_spans.len());
    let mut kinds = Vec::with_capacity(syntax_spans.len());
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
        });
        kinds.push(span.kind);
    }
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
/// for code, plus each span's kind (links are colored).
pub(super) fn style_layout_spans(styles: &[StyleSpan]) -> (Vec<TextSpan>, Vec<SyntaxTokenKind>) {
    let mut spans = Vec::with_capacity(styles.len());
    let mut kinds = Vec::with_capacity(styles.len());
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
        });
        kinds.push(match span.format.link {
            Some(_) => SyntaxTokenKind::Link,
            None => SyntaxTokenKind::Normal,
        });
    }
    (spans, kinds)
}

/// A misspelled word, for a context menu of corrections.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpellingIssue {
    /// Bytes of the word in the editor's text.
    pub range: Range<usize>,
    pub word: String,
    /// Likely corrections, best first.
    pub suggestions: Vec<String>,
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

/// Spans of `text` moved to match `composition`, which replaced
/// `replaced` with the preedit. Spans touching the replaced range drop out.
fn shift_spans(
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

impl Editor {
    pub fn new(mode: EditorMode) -> Self {
        let mut buffer = TextBuffer::new(true, WordForward::NextEnd);
        buffer.set_read_only(!mode.is_editable());
        Self {
            mode,
            buffer,
            layout: None,
            display: None,
            composition: None,
            dirty: true,
            syntax_dirty: true,
            syntax_highlighter: None,
            syntax_spans: Arc::from([]),
            span_kinds: Arc::from([]),
            display_span_kinds: Arc::from([]),
            line_tops: Arc::from([]),
            desired_x: None,
            reveal_cursor_on_flush: false,
            caret_hidden: false,
            scroll_y: 0.0,
            cursor_pos: CursorState::default(),
            cursor_moved_at_ms: 0,
            font_size: 14.0,
            scale_factor: 1.0,
            frame_scale: FrameScale::default(),
            last_width: 0.0,
            last_height: 0.0,
            font_generation: None,
            spelling: Spelling::default(),
        }
    }

    pub fn mode(&self) -> EditorMode {
        self.mode
    }

    pub fn set_mode(&mut self, mode: EditorMode) {
        if self.mode == mode {
            return;
        }
        self.mode = mode;
        self.buffer.set_read_only(!mode.is_editable());
        self.dirty = true;
        self.syntax_dirty = true;
    }

    pub fn set_syntax_highlighter(&mut self, highlighter: SyntaxHighlighter) {
        self.syntax_highlighter = Some(highlighter);
        self.syntax_dirty = true;
    }

    pub fn clear_syntax_highlighter(&mut self) {
        if self.syntax_highlighter.is_none() && self.syntax_spans.is_empty() {
            return;
        }
        self.syntax_highlighter = None;
        self.syntax_spans = Arc::from([]);
        self.syntax_dirty = false;
        self.dirty = true;
    }

    /// Set the app clock used for caret blink and undo coalescing.
    pub fn set_clock(&mut self, now_ms: u64) {
        self.buffer.set_now(now_ms);
    }

    fn line_height(&self) -> f32 {
        self.font_size * LINE_HEIGHT_FACTOR
    }

    fn note_cursor_activity(&mut self) {
        self.reveal_cursor_on_flush = true;
        self.cursor_moved_at_ms = self.buffer.now_ms();
    }

    fn text_changed(&mut self) {
        self.dirty = true;
        self.syntax_dirty = true;
        self.spelling.rev += 1;
    }

    /// Check this field's spelling with `checker` (a clone of one other
    /// fields use shares their worker and dictionary), or stop with `None`.
    /// Off by default.
    pub fn set_spellcheck(&mut self, checker: Option<SpellChecker>) {
        self.spelling = Spelling {
            checker,
            rev: self.spelling.rev,
            ..Spelling::default()
        };
        self.dirty = true;
    }

    /// Send the text to the checker if it changed, and take any result.
    fn refresh_spelling(&mut self) {
        let Some(checker) = self.spelling.checker.as_ref() else {
            return;
        };
        if self.spelling.sent != Some(self.spelling.rev) {
            self.spelling.sent = Some(self.spelling.rev);
            checker.request(self.spelling.rev, Arc::from(self.buffer.text()));
        }
        if let Some(result) = checker.poll() {
            self.accept_spelling(result);
        }
    }

    fn accept_spelling(&mut self, result: SpellResult) {
        if result.rev == self.spelling.rev {
            self.spelling.misspelled = result.misspelled;
            self.spelling.checked = Some(result.rev);
        }
    }

    /// Block until the checker has checked the current text (tests, or an
    /// app that wants marks before the first paint). Returns at once
    /// without a checker.
    pub fn wait_for_spelling(&mut self) {
        self.refresh_spelling();
        while self.spelling.checked != Some(self.spelling.rev) {
            let Some(result) = self.spelling.checker.as_ref().and_then(SpellChecker::wait) else {
                return;
            };
            self.accept_spelling(result);
        }
    }

    /// The misspelled words as of the last check, as byte ranges.
    pub fn misspellings(&self) -> &[Range<usize>] {
        &self.spelling.misspelled
    }

    /// The misspelled word at byte `at` (a right click), with corrections.
    pub fn spelling_at(&self, at: usize) -> Option<SpellingIssue> {
        let checker = self.spelling.checker.as_ref()?;
        let text = self.buffer.text();
        let range = self
            .spelling
            .misspelled
            .iter()
            .find(|r| r.start <= at && at <= r.end)?
            .clone();
        let word = text.get(range.clone())?.to_owned();
        Some(SpellingIssue {
            suggestions: checker.dictionary().suggest(&word),
            range,
            word,
        })
    }

    /// Accept `word` in every field sharing this checker's dictionary
    /// (calling its add hook), and check this field again.
    pub fn learn_word(&mut self, word: &str) {
        if let Some(checker) = &self.spelling.checker {
            checker.dictionary().learn(word);
            self.spelling.rev += 1;
        }
    }

    pub fn cursor(&self) -> TextOffset {
        self.buffer.cursor()
    }

    pub fn anchor(&self) -> TextOffset {
        self.buffer.anchor()
    }

    fn refresh_syntax(&mut self) {
        if !self.syntax_dirty {
            return;
        }
        self.syntax_dirty = false;
        let had_spans = !self.syntax_spans.is_empty();
        self.syntax_spans = Arc::from([]);
        if self.mode.is_code()
            && self.buffer.text().len() <= SYNTAX_HIGHLIGHT_MAX_BYTES
            && let Some(highlighter) = &self.syntax_highlighter
        {
            self.syntax_spans = highlighter(self.buffer.text()).into();
        }
        if had_spans || !self.syntax_spans.is_empty() {
            self.dirty = true;
        }
    }

    pub fn set_font_size(&mut self, font_size: f32) {
        if (self.font_size - font_size).abs() < 0.01 {
            return;
        }
        self.font_size = font_size;
        self.dirty = true;
    }

    /// Shape at `scale_factor`. Painting the editor's element sets this
    /// from the frame on the next flush, so apps rarely need to call it.
    pub fn set_scale_factor(&mut self, scale_factor: f32) {
        if scale_factor.is_finite() && scale_factor > 0.0 && scale_factor != self.scale_factor {
            self.scale_factor = scale_factor;
            self.dirty = true;
        }
    }

    pub fn invalidate_font(&mut self) {
        self.layout = None;
        self.display = None;
        self.dirty = true;
    }

    pub fn request_clear(&mut self) {
        self.set_text("");
    }

    /// Replace the text programmatically. Clears undo history.
    pub fn set_text(&mut self, value: &str) {
        self.buffer.set_text(value);
        self.scroll_y = 0.0;
        self.desired_x = None;
        self.text_changed();
        self.note_cursor_activity();
    }

    /// Append text programmatically (streaming). Earlier undo steps stay
    /// valid because their offsets are untouched.
    pub fn append(&mut self, value: &str) {
        self.buffer.append(value);
        self.reveal_cursor_on_flush = true;
        self.desired_x = None;
        self.text_changed();
    }

    /// Viewport size in pixels. The width less the code gutter is the wrap
    /// width.
    pub fn sync_size(&mut self, width: f32, height: f32) {
        if (self.last_width - width).abs() > 0.5 {
            self.dirty = true;
        }
        self.last_width = width;
        self.last_height = height;
    }

    /// Width of the line-number gutter the element paints left of the text
    /// in code modes; 0 in prose.
    pub fn gutter_width(&self) -> f32 {
        if !self.mode.is_code() {
            return 0.0;
        }
        let lines = self.buffer.text().bytes().filter(|&b| b == b'\n').count() + 1;
        gutter_width_in(self.font_size, lines, self.last_width)
    }

    fn layout_params(&self, text: Arc<str>, spans: Vec<TextSpan>) -> TextParams {
        let style = TextStyle::new(self.font_size)
            .kind(self.mode.font_kind())
            .line_height(self.line_height());
        // The element paints text right of the gutter, so wrap there too.
        let wrap =
            (self.last_width > 0.0).then(|| (self.last_width - self.gutter_width()).max(1.0));
        TextParams::new(text, style)
            .spans(spans)
            .wrap_width(wrap)
            .scale_factor(self.scale_factor)
    }

    /// Rebuild the layout after changes, place the caret, and scroll it
    /// into view if it moved.
    pub fn flush(&mut self, text_system: &mut TextSystem) {
        if let Some(scale) = self.frame_scale.get() {
            self.set_scale_factor(scale);
        }
        self.refresh_syntax();
        if self.font_generation != Some(text_system.generation()) {
            self.font_generation = Some(text_system.generation());
            self.dirty = true;
        }
        self.refresh_spelling();
        let relayout = self.dirty || self.layout.is_none();
        if relayout {
            self.dirty = false;
            let (spans, kinds) = if self.mode.is_code() {
                syntax_layout_spans(self.buffer.text(), &self.syntax_spans)
            } else {
                style_layout_spans(self.buffer.styles())
            };
            let params = self.layout_params(Arc::from(self.buffer.text()), spans);
            self.layout = text_system.layout(&params).ok().map(Arc::new);
            self.span_kinds = kinds.into();
            self.line_tops = self.compute_line_tops().into();
        }
        if self.buffer.take_preedit_dirty() || relayout {
            self.rebuild_composition(text_system);
        }

        let (x, y) = match (&self.composition, &self.display) {
            (Some(composition), Some(display)) => {
                self.caret_hidden = composition.caret.is_none();
                let caret = display.caret(composition.caret.unwrap_or(composition.preedit.end));
                (caret.x, caret.y)
            }
            _ => {
                self.caret_hidden = false;
                self.layout.as_deref().map_or((0.0, 0.0), |layout| {
                    let caret = layout.caret(self.buffer.cursor());
                    (caret.x, caret.y)
                })
            }
        };
        self.cursor_pos = CursorState { x, y };
        let max_scroll = (self.content_height() - self.last_height).max(0.0);
        self.scroll_y = self.scroll_y.clamp(0.0, max_scroll);
        if std::mem::take(&mut self.reveal_cursor_on_flush) && self.last_height > 0.0 {
            let cursor_bottom = y + self.line_height();
            if cursor_bottom > self.scroll_y + self.last_height {
                self.scroll_y = cursor_bottom - self.last_height;
            } else if y < self.scroll_y {
                self.scroll_y = y;
            }
            self.scroll_y = self.scroll_y.clamp(0.0, max_scroll);
        }
    }

    /// Lay out the committed text with the preedit spliced in, reusing the
    /// committed layout's spans shifted around it.
    fn rebuild_composition(&mut self, text_system: &mut TextSystem) {
        self.composition = None;
        self.display = None;
        self.display_span_kinds = Arc::from([]);
        let Some(composition) = self.buffer.composition() else {
            return;
        };
        let selection = self.buffer.selection();
        let spans = self.layout.as_ref().map(|l| l.spans().clone());
        let (spans, kinds) = shift_spans(
            spans.as_deref().unwrap_or_default(),
            &self.span_kinds,
            selection.start.get()..selection.end.get(),
            composition.preedit.end.get() - composition.preedit.start.get(),
        );
        let params = self.layout_params(Arc::from(composition.text.as_str()), spans);
        self.display = text_system.layout(&params).ok().map(Arc::new);
        self.display_span_kinds = kinds;
        self.composition = Some(composition);
    }

    pub fn text(&self) -> &str {
        self.buffer.text()
    }

    /// The text as shared with the layout, without copying it when the
    /// layout is current.
    pub fn text_arc(&self) -> Arc<str> {
        match &self.layout {
            Some(layout) if !self.dirty => layout.text().clone(),
            _ => Arc::from(self.buffer.text()),
        }
    }

    pub fn syntax_spans(&self) -> &Arc<[SyntaxSpan]> {
        &self.syntax_spans
    }

    /// The layout the element should paint: the committed text, or the
    /// composed text while an IME preedit is active.
    pub fn paint_layout(&self) -> Option<Arc<TextLayout>> {
        self.display.clone().or_else(|| self.layout.clone())
    }

    /// Syntax kind of each span of [`Editor::paint_layout`].
    pub fn paint_span_kinds(&self) -> Arc<[SyntaxTokenKind]> {
        match &self.display {
            Some(_) => self.display_span_kinds.clone(),
            None => self.span_kinds.clone(),
        }
    }

    /// The committed-text layout from the last flush.
    pub fn layout(&self) -> Option<&Arc<TextLayout>> {
        self.layout.as_ref()
    }

    pub fn byte_len(&self) -> usize {
        self.buffer.text().len()
    }

    pub fn line_count(&self) -> usize {
        let text = self.buffer.text();
        if text.is_empty() {
            0
        } else {
            text.bytes().filter(|&b| b == b'\n').count() + usize::from(!text.ends_with('\n'))
        }
    }

    pub fn is_empty(&self) -> bool {
        self.buffer.text().is_empty() && self.buffer.preedit().is_none()
    }

    pub fn content_height(&self) -> f32 {
        let line_height = self.line_height();
        let layout = self.display.as_deref().or(self.layout.as_deref());
        layout
            .map_or(line_height, |layout| layout.size().1)
            .max(line_height)
    }

    pub fn scroll_line_height_px(&self) -> f32 {
        self.line_height()
    }

    /// `(1-based logical line, top)` for each logical line, as of the last
    /// flush.
    pub fn logical_line_tops(&self) -> &Arc<[(usize, f32)]> {
        &self.line_tops
    }

    fn compute_line_tops(&self) -> Vec<(usize, f32)> {
        let Some(layout) = self.layout.as_deref() else {
            return if self.buffer.text().is_empty() {
                Vec::new()
            } else {
                vec![(1, 0.0)]
            };
        };
        let text = layout.text().as_bytes();
        let mut out = Vec::new();
        for line in layout.lines() {
            let start = line.byte_range.start;
            let paragraph_start = start == 0 || text.get(start - 1) == Some(&b'\n');
            if paragraph_start {
                out.push((out.len() + 1, line.top));
            }
        }
        out
    }

    pub fn selected_text(&self) -> Option<&str> {
        self.buffer.selected_text()
    }

    /// Selection highlight rects in layout coordinates. Empty while composing.
    pub fn selection_rects(&self) -> Vec<SelectionRect> {
        let selection = self.buffer.selection();
        if selection.is_empty() || self.buffer.preedit().is_some() {
            return Vec::new();
        }
        let Some(layout) = self.layout.as_deref() else {
            return Vec::new();
        };
        layout
            .selection_rects(selection)
            .map(SelectionRect::from)
            .collect()
    }

    /// Rects under the preedit (to underline) and under the IME's
    /// highlighted clause, in paint-layout coordinates.
    pub fn preedit_rects(&self) -> (Vec<SelectionRect>, Vec<SelectionRect>) {
        let (Some(display), Some(composition)) = (&self.display, &self.composition) else {
            return (Vec::new(), Vec::new());
        };
        let rects = |range: Range<TextOffset>| {
            display
                .selection_rects(range)
                .map(SelectionRect::from)
                .collect::<Vec<_>>()
        };
        let clause = composition.clause.clone().map(rects).unwrap_or_default();
        (rects(composition.preedit.clone()), clause)
    }

    /// Whether the caret is drawn (the IME may hide it while composing).
    pub fn caret_visible(&self) -> bool {
        !self.caret_hidden
    }

    pub fn set_preedit(&mut self, text: impl Into<String>, cursor: Option<(usize, usize)>) {
        if self.buffer.set_preedit(text, cursor) {
            self.note_cursor_activity();
        }
    }

    pub fn preedit(&self) -> Option<&Preedit> {
        self.buffer.preedit()
    }

    /// Commit composed IME text over the selection. Each commit is one undo step.
    pub fn commit_ime(&mut self, value: &str) -> bool {
        let changed = self.buffer.commit_ime(value);
        if changed {
            self.text_changed();
            self.desired_x = None;
            self.note_cursor_activity();
        }
        changed
    }

    pub fn can_undo(&self) -> bool {
        self.buffer.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.buffer.can_redo()
    }

    /// The visual line holding the caret. The end of a soft-wrapped line
    /// steps back over its trailing space so the caret stays on that line.
    fn visual_line_range(&self) -> Option<Range<TextOffset>> {
        let layout = self.layout.as_deref()?;
        let text = self.buffer.text();
        let caret = layout.caret(self.buffer.cursor());
        let line = layout.line(caret.line)?;
        let start = TextOffset::snap(text, line.byte_range.start);
        let mut end = TextOffset::snap(text, line.byte_range.end);
        let wrapped = layout
            .line(caret.line + 1)
            .is_some_and(|next| next.byte_range.start == line.byte_range.end);
        if wrapped && end > start {
            end = offset::prev_grapheme(text, end).max(start);
        }
        Some(start..end)
    }

    /// Soft Home or End: the edge of the visual line, or of the logical
    /// line before the first flush.
    fn move_soft(&mut self, to_end: bool, extend: bool) {
        use TextEditCommand::*;
        match self.visual_line_range() {
            Some(range) => self
                .buffer
                .move_to(if to_end { range.end } else { range.start }, extend),
            None => {
                let cmd = match (to_end, extend) {
                    (false, false) => CursorHome,
                    (false, true) => SelectHome,
                    (true, false) => CursorEnd,
                    (true, true) => SelectEnd,
                };
                self.buffer.apply(cmd);
            }
        }
    }

    /// Move to the line above or below at the remembered column. Past the
    /// first or last line the caret goes to the start or end of the text.
    fn move_vertical(&mut self, down: bool, extend: bool) {
        let Some(layout) = self.layout.clone() else {
            return;
        };
        let caret = layout.caret(self.buffer.cursor());
        let x = self.desired_x.unwrap_or(caret.x);
        let target = if down {
            caret.line.checked_add(1)
        } else {
            caret.line.checked_sub(1)
        };
        let (to, desired_x) = match target.and_then(|i| layout.line(i)) {
            Some(line) => (layout.hit(x, line.top + line.height * 0.5), Some(x)),
            None if down => (TextOffset::end(self.buffer.text()), None),
            None => (TextOffset::ZERO, None),
        };
        self.buffer.move_to(to, extend);
        self.desired_x = desired_x;
    }

    pub fn scroll(&mut self, delta_px: f32) {
        let max_scroll = (self.content_height() - self.last_height).max(0.0);
        self.scroll_y = (self.scroll_y + delta_px).clamp(0.0, max_scroll);
    }

    /// [`Editor::apply`] at the app's time `now_ms`.
    pub fn apply_at(&mut self, cmd: TextEditCommand, now_ms: u64) -> TextEditOutcome {
        self.buffer.set_now(now_ms);
        self.apply(cmd)
    }

    /// Apply a text editing command. Up and Down move by visual line, and
    /// soft Home and End to the edges of the visual line; the rest is the
    /// same as in a [`super::TextField`], on logical lines.
    pub fn apply(&mut self, cmd: TextEditCommand) -> TextEditOutcome {
        self.apply_with(cmd, &mut NoHooks)
    }

    /// [`Editor::apply`] with the app's policy for pastes.
    pub fn apply_with(
        &mut self,
        cmd: TextEditCommand,
        hooks: &mut dyn InputHooks,
    ) -> TextEditOutcome {
        use TextEditCommand::*;
        let before = (self.buffer.cursor(), self.buffer.anchor());
        let vertical = matches!(cmd, CursorUp | CursorDown | SelectUp | SelectDown);
        let mut outcome = TextEditOutcome::default();
        match cmd {
            CursorUp => self.move_vertical(false, false),
            CursorDown => self.move_vertical(true, false),
            SelectUp => self.move_vertical(false, true),
            SelectDown => self.move_vertical(true, true),
            CursorSoftHome => self.move_soft(false, false),
            SelectSoftHome => self.move_soft(false, true),
            CursorSoftEnd => self.move_soft(true, false),
            SelectSoftEnd => self.move_soft(true, true),
            // Code is highlighted, not formatted.
            ToggleStyle(_) | SetLink(_) if self.mode.is_code() => {}
            other => outcome = self.buffer.apply_with(other, hooks),
        }
        outcome.selection_changed = (self.buffer.cursor(), self.buffer.anchor()) != before;
        self.edited(&outcome, vertical);
        outcome
    }

    /// Caret, scroll, and relayout bookkeeping after an edit.
    fn edited(&mut self, outcome: &TextEditOutcome, vertical: bool) {
        if outcome.text_changed {
            self.text_changed();
        }
        let moved = outcome.text_changed || outcome.selection_changed;
        if moved || outcome.preedit_changed {
            self.note_cursor_activity();
        }
        if moved && !vertical {
            self.desired_x = None;
        }
    }

    /// Insert at the caret, over any selection, as one undo step.
    pub fn insert(&mut self, insertion: &Insertion) -> TextEditOutcome {
        let outcome = self.buffer.insert_at(None, insertion);
        self.edited(&outcome, false);
        outcome
    }

    /// Replace the byte range `range` (snapped onto the text, and grown to
    /// take whole any atom it touches) with `insertion`, caret after it.
    /// Accepting a completion replaces its trigger this way.
    pub fn replace_range(&mut self, range: Range<usize>, insertion: &Insertion) -> TextEditOutcome {
        let text = self.buffer.text();
        let range = offset::ordered(text, range);
        let outcome = self.buffer.insert_at(Some(range), insertion);
        self.edited(&outcome, false);
        outcome
    }

    /// A file dropped while the editor has focus, inserted at the caret as
    /// `hooks` decides.
    pub fn drop_path(
        &mut self,
        path: &std::path::Path,
        hooks: &mut dyn InputHooks,
    ) -> TextEditOutcome {
        let insertion = hooks.drop_path(path);
        self.insert(&insertion)
    }

    /// The atoms in the text, in order.
    pub fn atoms(&self) -> &[InlineAtom] {
        self.buffer.atoms()
    }

    /// The formatting runs, in order. Formatting is a prose feature: code
    /// modes keep the runs but neither change nor paint them.
    pub fn styles(&self) -> &[StyleSpan] {
        self.buffer.styles()
    }

    /// The format text typed at the caret would get.
    pub fn typing_format(&self) -> TextFormat {
        self.buffer.typing_format()
    }

    /// How copies of formatted text are written for other apps
    /// (Markdown by default).
    pub fn set_rich_export(&mut self, format: RichExport) {
        self.buffer.set_export_format(format);
    }

    /// Underline, strikethrough, and code boxes for the formatting (and
    /// wavy lines under misspellings) in layout coordinates, as of the
    /// last flush. Empty while composing.
    pub fn decoration_rects(&self) -> Vec<(TextDecoration, SelectionRect)> {
        let (Some(layout), false, None) =
            (self.layout.as_deref(), self.dirty, self.buffer.preedit())
        else {
            return Vec::new();
        };
        let text = self.buffer.text();
        let mut out = Vec::new();
        let mut push = |kind, range: Range<usize>| {
            let range = offset::ordered(text, range);
            out.extend(
                layout
                    .selection_rects(range)
                    .map(|r| (kind, SelectionRect::from(r))),
            );
        };
        // The word being typed at the caret is not marked until it is done.
        let caret = self.buffer.cursor().get();
        let typing = self.buffer.selection().is_empty();
        for range in &self.spelling.misspelled {
            let valid = range.end <= text.len()
                && text.is_char_boundary(range.start)
                && text.is_char_boundary(range.end);
            if valid && !(typing && range.end == caret) {
                push(TextDecoration::Misspelled, range.clone());
            }
        }
        if !self.mode.is_code() {
            for span in self.buffer.styles() {
                let style = span.format.style;
                if style.contains(InlineStyle::CODE) {
                    push(TextDecoration::CodeBackground, span.range.clone());
                }
                if style.contains(InlineStyle::UNDERLINE) || span.format.link.is_some() {
                    push(TextDecoration::Underline, span.range.clone());
                }
                if style.contains(InlineStyle::STRIKE) {
                    push(TextDecoration::Strikethrough, span.range.clone());
                }
            }
        }
        out
    }

    /// The whole text with its atoms, for sending, history, or a draft.
    pub fn rich_text(&self) -> RichText {
        self.buffer
            .rich_slice(TextOffset::ZERO..TextOffset::end(self.buffer.text()))
    }

    /// Replace the text and atoms programmatically, caret at the end.
    /// Clears undo history.
    pub fn set_rich_text(&mut self, rich: &RichText) {
        self.buffer.set_rich_text(rich);
        self.scroll_y = 0.0;
        self.desired_x = None;
        self.text_changed();
        self.note_cursor_activity();
    }

    /// Share `clipboard` with other editors so atoms copied in one paste
    /// as atoms in another. Each editor has its own until this is called.
    pub fn set_rich_clipboard(&mut self, clipboard: RichClipboard) {
        self.buffer.set_rich_clipboard(clipboard);
    }

    /// The trigger `rules` find at the caret, if the selection is empty and
    /// no IME composition is open.
    pub fn active_trigger(&self, rules: &[TriggerRule]) -> Option<TriggerMatch> {
        if self.buffer.preedit().is_some() || !self.buffer.selection().is_empty() {
            return None;
        }
        find_trigger(
            self.buffer.text(),
            self.buffer.cursor(),
            self.buffer.atoms(),
            rules,
        )
    }

    /// Rects behind each atom's label in layout coordinates, as of the
    /// last flush. Empty without atoms or while composing.
    pub fn atom_rects(&self) -> Vec<SelectionRect> {
        let atoms = self.buffer.atoms();
        if atoms.is_empty() || self.buffer.preedit().is_some() {
            return Vec::new();
        }
        let (Some(layout), false) = (self.layout.as_deref(), self.dirty) else {
            return Vec::new();
        };
        let text = self.buffer.text();
        atoms
            .iter()
            .flat_map(|atom| {
                let range = offset::ordered(text, atom.range.clone());
                layout.selection_rects(range).map(SelectionRect::from)
            })
            .collect()
    }
}

impl From<quark::Rect> for SelectionRect {
    fn from(r: quark::Rect) -> Self {
        Self {
            x: r.x,
            y: r.y,
            w: r.width,
            h: r.height,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use TextEditCommand::*;

    fn make_editor(width: f32, height: f32) -> (TextSystem, Editor) {
        let text_system = TextSystem::vendored_only(&Default::default());
        let mut editor = Editor::default();
        editor.sync_size(width, height);
        (text_system, editor)
    }

    const TWO_LINES: f32 = 2.0 * 14.0 * LINE_HEIGHT_FACTOR;

    #[test]
    fn manual_scroll_persists_across_flush() {
        let (mut text_system, mut editor) = make_editor(220.0, TWO_LINES);
        editor.apply(InsertText("line0\nline1\nline2\nline3".into()));
        editor.flush(&mut text_system);
        let line_height = editor.scroll_line_height_px();
        assert!((editor.scroll_y - line_height * 2.0).abs() < 0.5);

        editor.scroll(-line_height);
        editor.flush(&mut text_system);

        assert!(
            (editor.scroll_y - line_height).abs() < 0.5,
            "expected manual scroll to persist, got {}",
            editor.scroll_y
        );
    }

    // Catches the content height (and so the scroll range) stopping short
    // of the caret's line, which left the caret unreachable.
    #[test]
    fn typing_scrolls_the_caret_line_into_a_content_height_that_covers_it() {
        let words = "asdf asf asdf asdf fasd fasd fasdf sdaf asdf sadf ".repeat(4);
        let cases = [
            ("trailing newline", "first line\nsecond line\n", 160.0),
            ("wrapped past the viewport", words.as_str(), 120.0),
        ];
        for (name, text, width) in cases {
            let (mut text_system, mut editor) = make_editor(width, TWO_LINES);
            editor.apply(InsertText(text.into()));
            editor.flush(&mut text_system);

            let top = editor.cursor_pos.y;
            let bottom = top + editor.scroll_line_height_px();
            assert!(
                editor.content_height() + 0.5 >= bottom,
                "{name}: content height"
            );
            let visible = editor.scroll_y - 0.5..editor.scroll_y + TWO_LINES + 0.5;
            assert!(
                visible.contains(&top) && visible.contains(&bottom),
                "{name}: caret {top}..{bottom} outside {visible:?}"
            );
        }
    }

    #[test]
    fn diff_read_only_mode_blocks_user_mutations() {
        let mut editor = Editor::new(EditorMode::DiffReadOnly);
        editor.set_text("unchanged");
        editor.apply(InsertText(" edited".into()));
        editor.apply(Backspace);
        editor.apply(Undo);
        assert_eq!(editor.text(), "unchanged");
    }

    // Regression: select_line_at sliced the text at the raw offset, so a
    // triple click inside a multibyte char panicked.
    #[test]
    fn select_line_at_inside_a_multibyte_char_selects_its_line() {
        let mut editor = Editor::default();
        editor.set_text("\u{e9}\nb");
        editor.apply(SelectLineAt(1));
        assert_eq!(editor.selected_text(), Some("\u{e9}\n"));
    }
}
