use std::ops::Range;
use std::sync::Arc;

use quark_render::scene::{FontKind, FontStyle, FontWeight};
use quark_text::{TextLayout, TextParams, TextSpan, TextStyle, TextSystem};

use super::ime::{Composition, Preedit, compose, floor_char_boundary};
use super::text_edit::{
    TextEditCommand, TextEditOutcome, next_grapheme_boundary, next_word_end,
    prev_grapheme_boundary, prev_word_boundary, word_range_at,
};
use super::undo::{Edit, EditKind, EditLog};
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
/// Caret, selection, hit-testing, and vertical movement all read the same
/// quark-text [`TextLayout`] that [`super::TextEditorElement`] paints (see
/// [`Editor::paint_layout`]), so they cannot drift from the glyphs. The
/// layout is rebuilt by [`Editor::flush`]; call it once per frame before
/// building the element.
#[derive(Clone)]
pub struct Editor {
    mode: EditorMode,
    text: String,
    cursor: usize,
    anchor: usize,
    /// Layout of `text`, as of the last flush.
    layout: Option<Arc<TextLayout>>,
    /// Layout of `text` with the preedit spliced in, while composing.
    display: Option<Arc<TextLayout>>,
    composition: Option<Composition>,
    preedit: Option<Preedit>,
    /// `text` (or the wrap width, font, or syntax) changed since the last flush.
    dirty: bool,
    preedit_dirty: bool,
    syntax_dirty: bool,
    syntax_highlighter: Option<SyntaxHighlighter>,
    syntax_spans: Vec<SyntaxSpan>,
    /// Token kind of each span of `layout`, for paint colors.
    span_kinds: Vec<SyntaxTokenKind>,
    history: EditLog,
    /// The app's clock, from [`Editor::apply_at`] or [`Editor::set_clock`].
    now_ms: u64,
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
}

impl Default for Editor {
    fn default() -> Self {
        Self {
            mode: EditorMode::default(),
            text: String::new(),
            cursor: 0,
            anchor: 0,
            layout: None,
            display: None,
            composition: None,
            preedit: None,
            dirty: true,
            preedit_dirty: false,
            syntax_dirty: true,
            syntax_highlighter: None,
            syntax_spans: Vec::new(),
            span_kinds: Vec::new(),
            history: EditLog::default(),
            now_ms: 0,
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
        }
    }
}

impl std::fmt::Debug for Editor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Editor")
            .field("initialized", &self.layout.is_some())
            .field("mode", &self.mode)
            .field("cursor", &self.cursor)
            .field("anchor", &self.anchor)
            .field("scroll_y", &self.scroll_y)
            .finish()
    }
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
        Normal | String | Number | Operator | Punctuation | Variable => (None, None),
    }
}

/// Spans of `text` moved to match `composition`, which replaced
/// `replaced` with the preedit. Spans touching the replaced range drop out.
fn shift_spans(
    spans: &[TextSpan],
    kinds: &[SyntaxTokenKind],
    replaced: Range<usize>,
    inserted_len: usize,
) -> (Vec<TextSpan>, Vec<SyntaxTokenKind>) {
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
    (out, out_kinds)
}

impl Editor {
    pub fn new(mode: EditorMode) -> Self {
        Self {
            mode,
            ..Self::default()
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
        self.syntax_spans.clear();
        self.syntax_dirty = false;
        self.dirty = true;
    }

    /// Set the app clock used for caret blink and undo coalescing.
    pub fn set_clock(&mut self, now_ms: u64) {
        self.now_ms = now_ms;
    }

    fn line_height(&self) -> f32 {
        self.font_size * LINE_HEIGHT_FACTOR
    }

    fn note_cursor_activity(&mut self) {
        self.reveal_cursor_on_flush = true;
        self.cursor_moved_at_ms = self.now_ms;
    }

    fn global_to_line_col(&self, offset: usize) -> (usize, usize) {
        let offset = offset.min(self.text.len());
        let line = self.text.as_bytes()[..offset]
            .iter()
            .filter(|&&b| b == b'\n')
            .count();
        let line_start = self.text[..offset].rfind('\n').map_or(0, |i| i + 1);
        (line, offset - line_start)
    }

    fn line_start(&self, target_line: usize) -> usize {
        if target_line == 0 {
            return 0;
        }
        self.text
            .match_indices('\n')
            .nth(target_line - 1)
            .map_or(self.text.len(), |(i, _)| i + 1)
    }

    fn line_end(&self, target_line: usize) -> usize {
        self.text
            .match_indices('\n')
            .nth(target_line)
            .map_or(self.text.len(), |(i, _)| i)
    }

    fn has_selection(&self) -> bool {
        self.anchor != self.cursor
    }

    fn selection_range(&self) -> (usize, usize) {
        (self.anchor.min(self.cursor), self.anchor.max(self.cursor))
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn anchor(&self) -> usize {
        self.anchor
    }

    /// Replace `range` with `inserted`, record it for undo, and collapse
    /// the caret after it. The single place the text is edited by the user.
    fn replace(&mut self, range: Range<usize>, inserted: &str, kind: EditKind) -> bool {
        if !self.mode.is_editable() {
            return false;
        }
        self.clear_preedit();
        if range.is_empty() && inserted.is_empty() {
            return false;
        }
        let Some(removed) = self.text.get(range.clone()).map(str::to_owned) else {
            return false;
        };
        let before = (self.anchor, self.cursor);
        self.text.replace_range(range.clone(), inserted);
        self.cursor = range.start + inserted.len();
        self.anchor = self.cursor;
        let edit = Edit {
            at: range.start,
            removed,
            inserted: inserted.to_owned(),
            before,
            after: (self.anchor, self.cursor),
        };
        self.history.record(edit, kind, self.now_ms);
        self.text_changed();
        true
    }

    fn text_changed(&mut self) {
        self.dirty = true;
        self.syntax_dirty = true;
        self.desired_x = None;
        self.note_cursor_activity();
    }

    fn delete_selection(&mut self) -> bool {
        if !self.has_selection() {
            return false;
        }
        let (start, end) = self.selection_range();
        self.replace(start..end, "", EditKind::Other)
    }

    /// Layout to use for geometry, or `None` before the first flush.
    fn geometry(&self) -> Option<&TextLayout> {
        self.layout.as_deref()
    }

    fn offset_to_point(&self, offset: usize) -> (f32, f32) {
        self.geometry().map_or((0.0, 0.0), |layout| {
            let caret = layout.caret(offset);
            (caret.x, caret.y)
        })
    }

    fn point_to_offset(&self, px: f32, py: f32) -> usize {
        let hit = self.geometry().map_or(0, |layout| layout.hit(px, py));
        floor_char_boundary(&self.text, hit)
    }

    fn refresh_syntax(&mut self) {
        if !self.syntax_dirty {
            return;
        }
        self.syntax_dirty = false;
        let had_spans = !self.syntax_spans.is_empty();
        self.syntax_spans.clear();
        if self.mode.is_code()
            && self.text.len() <= SYNTAX_HIGHLIGHT_MAX_BYTES
            && let Some(highlighter) = &self.syntax_highlighter
        {
            self.syntax_spans = highlighter(&self.text);
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
        self.text.clear();
        self.text.push_str(value);
        self.cursor = self.text.len();
        self.anchor = self.cursor;
        self.scroll_y = 0.0;
        self.history.clear();
        self.clear_preedit();
        self.text_changed();
    }

    /// Append text programmatically (streaming). Earlier undo steps stay
    /// valid because their offsets are untouched.
    pub fn append(&mut self, value: &str) {
        let at_end = self.cursor == self.text.len() && self.anchor == self.cursor;
        self.text.push_str(value);
        if at_end {
            self.cursor = self.text.len();
            self.anchor = self.cursor;
        }
        self.history.break_coalescing();
        self.reveal_cursor_on_flush = true;
        self.dirty = true;
        self.syntax_dirty = true;
        self.desired_x = None;
    }

    /// Viewport size in pixels; the width is the wrap width.
    pub fn sync_size(&mut self, width: f32, height: f32) {
        if (self.last_width - width).abs() > 0.5 {
            self.dirty = true;
        }
        self.last_width = width;
        self.last_height = height;
    }

    fn layout_params(&self, text: &str, spans: Vec<TextSpan>) -> TextParams {
        let style = TextStyle::new(self.font_size)
            .kind(self.mode.font_kind())
            .line_height(self.line_height());
        let wrap = (self.last_width > 0.0).then_some(self.last_width.max(1.0));
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
        let relayout = self.dirty || self.layout.is_none();
        if relayout {
            self.dirty = false;
            let (spans, kinds) = syntax_layout_spans(&self.text, &self.syntax_spans);
            let params = self.layout_params(&self.text, spans);
            self.layout = text_system.layout(&params).ok().map(Arc::new);
            self.span_kinds = kinds;
        }
        if relayout || self.preedit_dirty {
            self.preedit_dirty = false;
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
                self.offset_to_point(self.cursor)
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

    fn rebuild_composition(&mut self, text_system: &mut TextSystem) {
        self.composition = None;
        self.display = None;
        let Some(preedit) = &self.preedit else {
            return;
        };
        let (start, end) = self.selection_range();
        let composition = compose(&self.text, (start, end), preedit);
        let (spans, _) = syntax_layout_spans(&self.text, &self.syntax_spans);
        let (spans, _) = shift_spans(
            &spans,
            &self.span_kinds,
            start..end,
            composition.preedit.len(),
        );
        let params = self.layout_params(&composition.text, spans);
        self.display = text_system.layout(&params).ok().map(Arc::new);
        self.composition = Some(composition);
    }

    pub fn text(&self) -> String {
        self.text.clone()
    }

    pub fn text_str(&self) -> &str {
        &self.text
    }

    pub fn text_arc(&self) -> Arc<str> {
        Arc::from(self.text.as_str())
    }

    pub fn syntax_spans(&self) -> &[SyntaxSpan] {
        &self.syntax_spans
    }

    /// The layout the element should paint: the committed text, or the
    /// composed text while an IME preedit is active.
    pub fn paint_layout(&self) -> Option<Arc<TextLayout>> {
        self.display.clone().or_else(|| self.layout.clone())
    }

    /// Syntax kind of each span of [`Editor::paint_layout`].
    pub fn paint_span_kinds(&self) -> Vec<SyntaxTokenKind> {
        match (&self.display, &self.composition) {
            (Some(_), Some(composition)) => {
                let (start, end) = self.selection_range();
                let (spans, kinds) = syntax_layout_spans(&self.text, &self.syntax_spans);
                shift_spans(&spans, &kinds, start..end, composition.preedit.len()).1
            }
            _ => self.span_kinds.clone(),
        }
    }

    /// The committed-text layout from the last flush.
    pub fn layout(&self) -> Option<&Arc<TextLayout>> {
        self.layout.as_ref()
    }

    pub fn byte_len(&self) -> usize {
        self.text.len()
    }

    pub fn line_count(&self) -> usize {
        if self.text.is_empty() {
            0
        } else {
            self.text.as_bytes().iter().filter(|&&b| b == b'\n').count()
                + usize::from(!self.text.ends_with('\n'))
        }
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty() && self.preedit.is_none()
    }

    pub fn content_height(&self) -> f32 {
        let line_height = self.line_height();
        let layout = self.display.as_deref().or(self.geometry());
        layout
            .map_or(line_height, |layout| layout.size().1)
            .max(line_height)
    }

    pub fn scroll_line_height_px(&self) -> f32 {
        self.line_height()
    }

    /// `(1-based logical line, top)` for each logical line.
    pub fn logical_line_tops(&self) -> Vec<(usize, f32)> {
        let Some(layout) = self.geometry() else {
            return if self.text.is_empty() {
                Vec::new()
            } else {
                vec![(1, 0.0)]
            };
        };
        let text = layout.text();
        let mut out = Vec::new();
        for line in layout.lines() {
            let start = line.byte_range.start;
            let paragraph_start = start == 0 || text.as_bytes().get(start - 1) == Some(&b'\n');
            if paragraph_start {
                out.push((out.len() + 1, line.top));
            }
        }
        out
    }

    pub fn selected_text(&self) -> Option<String> {
        if !self.has_selection() {
            return None;
        }
        let (start, end) = self.selection_range();
        self.text
            .get(start..end)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    }

    /// Selection highlight rects in layout coordinates. Empty while composing.
    pub fn selection_rects(&self) -> Vec<SelectionRect> {
        if !self.has_selection() || self.preedit.is_some() {
            return Vec::new();
        }
        let Some(layout) = self.geometry() else {
            return Vec::new();
        };
        let (start, end) = self.selection_range();
        layout
            .selection_rects(start..end)
            .map(|r| SelectionRect {
                x: r.x,
                y: r.y,
                w: r.width,
                h: r.height,
            })
            .collect()
    }

    /// Rects under the preedit (to underline) and under the IME's
    /// highlighted clause, in paint-layout coordinates.
    pub fn preedit_rects(&self) -> (Vec<SelectionRect>, Vec<SelectionRect>) {
        let (Some(display), Some(composition)) = (&self.display, &self.composition) else {
            return (Vec::new(), Vec::new());
        };
        let rects = |range: Range<usize>| {
            display
                .selection_rects(range)
                .map(|r| SelectionRect {
                    x: r.x,
                    y: r.y,
                    w: r.width,
                    h: r.height,
                })
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
        let preedit = Preedit::new(text, cursor);
        if preedit != self.preedit {
            self.preedit = preedit;
            self.preedit_dirty = true;
            self.note_cursor_activity();
        }
    }

    pub fn preedit(&self) -> Option<&Preedit> {
        self.preedit.as_ref()
    }

    fn clear_preedit(&mut self) {
        if self.preedit.take().is_some() {
            self.preedit_dirty = true;
        }
    }

    /// Commit composed IME text over the selection. Each commit is one undo step.
    pub fn commit_ime(&mut self, value: &str) -> bool {
        self.history.break_coalescing();
        let changed = self.insert_text(value);
        self.history.break_coalescing();
        changed
    }

    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    pub fn undo(&mut self) -> bool {
        if !self.mode.is_editable() {
            return false;
        }
        self.clear_preedit();
        let Some((anchor, cursor)) = self.history.undo(&mut self.text) else {
            return false;
        };
        self.anchor = floor_char_boundary(&self.text, anchor);
        self.cursor = floor_char_boundary(&self.text, cursor);
        self.text_changed();
        true
    }

    pub fn redo(&mut self) -> bool {
        if !self.mode.is_editable() {
            return false;
        }
        self.clear_preedit();
        let Some((anchor, cursor)) = self.history.redo(&mut self.text) else {
            return false;
        };
        self.anchor = floor_char_boundary(&self.text, anchor);
        self.cursor = floor_char_boundary(&self.text, cursor);
        self.text_changed();
        true
    }

    pub fn insert_char(&mut self, ch: char) -> bool {
        self.insert_text(ch.encode_utf8(&mut [0; 4]))
    }

    pub fn insert_newline(&mut self) -> bool {
        self.insert_char('\n')
    }

    pub fn insert_text(&mut self, s: &str) -> bool {
        let (start, end) = self.selection_range();
        self.replace(start..end, s, EditKind::Typing)
    }

    /// Insert clipboard text over the selection as one undo step.
    pub fn paste(&mut self, s: &str) -> bool {
        let (start, end) = self.selection_range();
        self.replace(start..end, s, EditKind::Other)
    }

    pub fn delete_backward(&mut self) -> bool {
        if self.delete_selection() {
            return true;
        }
        let prev = prev_grapheme_boundary(&self.text, self.cursor);
        self.replace(prev..self.cursor, "", EditKind::Deleting)
    }

    pub fn delete_forward(&mut self) -> bool {
        if self.delete_selection() {
            return true;
        }
        let next = next_grapheme_boundary(&self.text, self.cursor);
        self.replace(self.cursor..next, "", EditKind::Deleting)
    }

    pub fn delete_backward_word(&mut self) -> bool {
        if self.delete_selection() {
            return true;
        }
        let target = prev_word_boundary(&self.text, self.cursor);
        self.replace(target..self.cursor, "", EditKind::Other)
    }

    pub fn delete_forward_word(&mut self) -> bool {
        if self.delete_selection() {
            return true;
        }
        let target = next_word_end(&self.text, self.cursor);
        self.replace(self.cursor..target, "", EditKind::Other)
    }

    pub fn delete_backward_line(&mut self) -> bool {
        if self.delete_selection() {
            return true;
        }
        let (line, _col) = self.global_to_line_col(self.cursor);
        let start = self.line_start(line);
        self.replace(start..self.cursor, "", EditKind::Other)
    }

    /// Move the caret to `offset` (anchor too unless `selecting`), resetting
    /// the remembered column.
    fn move_to(&mut self, offset: usize, selecting: bool) {
        let previous = (self.cursor, self.anchor);
        self.cursor = floor_char_boundary(&self.text, offset);
        if !selecting {
            self.anchor = self.cursor;
        }
        self.desired_x = None;
        self.after_move(previous);
    }

    fn after_move(&mut self, previous: (usize, usize)) {
        if (self.cursor, self.anchor) != previous {
            self.history.break_coalescing();
            self.clear_preedit();
            self.note_cursor_activity();
        }
    }

    /// Collapse a selection to its start or end; returns true if there was one.
    fn collapse(&mut self, to_end: bool, selecting: bool) -> bool {
        if selecting || !self.has_selection() {
            return false;
        }
        let (start, end) = self.selection_range();
        self.move_to(if to_end { end } else { start }, false);
        true
    }

    pub fn move_left(&mut self, selecting: bool) {
        if !self.collapse(false, selecting) {
            self.move_to(prev_grapheme_boundary(&self.text, self.cursor), selecting);
        }
    }

    pub fn move_right(&mut self, selecting: bool) {
        if !self.collapse(true, selecting) {
            self.move_to(next_grapheme_boundary(&self.text, self.cursor), selecting);
        }
    }

    pub fn move_word_left(&mut self, selecting: bool) {
        if !self.collapse(false, selecting) {
            self.move_to(prev_word_boundary(&self.text, self.cursor), selecting);
        }
    }

    pub fn move_word_right(&mut self, selecting: bool) {
        if !self.collapse(true, selecting) {
            self.move_to(next_word_end(&self.text, self.cursor), selecting);
        }
    }

    pub fn move_home(&mut self, selecting: bool) {
        let (line, _col) = self.global_to_line_col(self.cursor);
        self.move_to(self.line_start(line), selecting);
    }

    pub fn move_end(&mut self, selecting: bool) {
        let (line, _col) = self.global_to_line_col(self.cursor);
        self.move_to(self.line_end(line), selecting);
    }

    /// Byte range of the visual line holding the caret. The end of a
    /// soft-wrapped line steps back over its trailing space so the caret
    /// stays on that line.
    fn visual_line_range(&self) -> Option<Range<usize>> {
        let layout = self.geometry()?;
        let caret = layout.caret(self.cursor);
        let line = layout.line(caret.line)?;
        let mut range = line.byte_range.clone();
        let wrapped = layout
            .line(caret.line + 1)
            .is_some_and(|next| next.byte_range.start == range.end);
        if wrapped && range.end > range.start {
            range.end = prev_grapheme_boundary(&self.text, range.end).max(range.start);
        }
        Some(range)
    }

    pub fn move_soft_home(&mut self, selecting: bool) {
        match self.visual_line_range() {
            Some(range) => self.move_to(range.start, selecting),
            None => self.move_home(selecting),
        }
    }

    pub fn move_soft_end(&mut self, selecting: bool) {
        match self.visual_line_range() {
            Some(range) => self.move_to(range.end, selecting),
            None => self.move_end(selecting),
        }
    }

    fn move_vertical(&mut self, down: bool, selecting: bool) {
        let Some(layout) = self.layout.clone() else {
            return;
        };
        let previous = (self.cursor, self.anchor);
        let caret = layout.caret(self.cursor);
        let x = self.desired_x.unwrap_or(caret.x);
        let target = if down {
            caret.line.checked_add(1)
        } else {
            caret.line.checked_sub(1)
        };
        match target.and_then(|i| layout.line(i)) {
            Some(line) => {
                let hit = layout.hit(x, line.top + line.height * 0.5);
                self.cursor = floor_char_boundary(&self.text, hit);
                self.desired_x = Some(x);
            }
            None => {
                self.cursor = if down { self.text.len() } else { 0 };
                self.desired_x = None;
            }
        }
        if !selecting {
            self.anchor = self.cursor;
        }
        self.after_move(previous);
    }

    pub fn move_up(&mut self, selecting: bool) {
        self.move_vertical(false, selecting);
    }

    pub fn move_down(&mut self, selecting: bool) {
        self.move_vertical(true, selecting);
    }

    pub fn select_all(&mut self) {
        self.move_to(0, false);
        self.move_to(self.text.len(), true);
    }

    /// Select the word run around `offset`.
    pub fn select_word_at(&mut self, offset: usize) {
        let range = word_range_at(&self.text, offset);
        self.move_to(range.start, false);
        self.move_to(range.end, true);
    }

    /// Select the logical line around `offset`, including its newline.
    pub fn select_line_at(&mut self, offset: usize) {
        let (line, _col) = self.global_to_line_col(offset);
        let start = self.line_start(line);
        let end = self.line_start(line + 1);
        self.move_to(start, false);
        self.move_to(end, true);
    }

    pub fn click(&mut self, x: i32, y: i32) {
        self.multi_click(x, y, 1);
    }

    /// A press in viewport coordinates: 1 places the caret, 2 selects the
    /// word under it, 3 the line (see [`super::ClickCounter`]).
    pub fn multi_click(&mut self, x: i32, y: i32, count: u8) {
        let offset = self.point_to_offset(x as f32, y as f32 + self.scroll_y);
        match count {
            2 => self.select_word_at(offset),
            3 => self.select_line_at(offset),
            _ => self.move_to(offset, false),
        }
    }

    /// Extend the selection to the pointer. Past the bottom or top edge
    /// the caret lands outside the viewport, and the next flush scrolls
    /// it into view.
    pub fn drag(&mut self, x: i32, y: i32) {
        let offset = self.point_to_offset(x as f32, y as f32 + self.scroll_y);
        self.move_to(offset, true);
    }

    pub fn scroll(&mut self, delta_px: f32) {
        let max_scroll = (self.content_height() - self.last_height).max(0.0);
        self.scroll_y = (self.scroll_y + delta_px).clamp(0.0, max_scroll);
    }

    /// [`Editor::apply`] at the app's time `now_ms`.
    pub fn apply_at(&mut self, cmd: TextEditCommand, now_ms: u64) -> TextEditOutcome {
        self.now_ms = now_ms;
        self.apply(cmd)
    }

    /// Apply a text editing command.
    pub fn apply(&mut self, cmd: TextEditCommand) -> TextEditOutcome {
        use TextEditCommand::*;
        let before = (self.cursor, self.anchor);
        let mut outcome = TextEditOutcome::default();
        let changed = match cmd {
            InsertText(value) => self.insert_text(&value),
            Paste(value) => self.paste(&value),
            Backspace => self.delete_backward(),
            BackspaceWord => self.delete_backward_word(),
            BackspaceLine => self.delete_backward_line(),
            DeleteForward => self.delete_forward(),
            DeleteForwardWord => self.delete_forward_word(),
            Undo => self.undo(),
            Redo => self.redo(),
            Cut => {
                outcome.clipboard_write = self.selected_text();
                outcome.clipboard_write.is_some() && self.delete_selection()
            }
            other => {
                match other {
                    CursorLeft => self.move_left(false),
                    CursorRight => self.move_right(false),
                    CursorUp => self.move_up(false),
                    CursorDown => self.move_down(false),
                    CursorWordLeft => self.move_word_left(false),
                    CursorWordRight => self.move_word_right(false),
                    CursorHome => self.move_home(false),
                    CursorEnd => self.move_end(false),
                    CursorSoftHome => self.move_soft_home(false),
                    CursorSoftEnd => self.move_soft_end(false),
                    SelectLeft => self.move_left(true),
                    SelectRight => self.move_right(true),
                    SelectUp => self.move_up(true),
                    SelectDown => self.move_down(true),
                    SelectWordLeft => self.move_word_left(true),
                    SelectWordRight => self.move_word_right(true),
                    SelectHome => self.move_home(true),
                    SelectEnd => self.move_end(true),
                    SelectSoftHome => self.move_soft_home(true),
                    SelectSoftEnd => self.move_soft_end(true),
                    SelectAll => self.select_all(),
                    SelectWordAt(offset) => self.select_word_at(offset),
                    SelectLineAt(offset) => self.select_line_at(offset),
                    SetTextCursor(offset) => self.move_to(offset, false),
                    ExtendTextSelection(offset) => self.move_to(offset, true),
                    Copy => outcome.clipboard_write = self.selected_text(),
                    CancelPreedit => self.clear_preedit(),
                    InsertText(_) | Paste(_) | Backspace | BackspaceWord | BackspaceLine
                    | DeleteForward | DeleteForwardWord | Undo | Redo | Cut => {}
                }
                false
            }
        };
        outcome.text_changed = changed;
        outcome.selection_changed = (self.cursor, self.anchor) != before;
        outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_editor(width: f32, height: f32) -> (quark_text::TextSystem, Editor) {
        let font_system = quark_text::TextSystem::vendored_only(&Default::default());

        let mut editor = Editor::default();
        editor.sync_size(width, height);

        (font_system, editor)
    }

    #[test]
    fn apply_reports_text_selection_and_clipboard_changes() {
        use TextEditCommand::*;
        let mut editor = Editor::default();
        let out = editor.apply(InsertText("hello world".into()));
        assert!(out.text_changed);
        let out = editor.apply(SelectWordLeft);
        assert!(out.selection_changed && !out.text_changed);
        let out = editor.apply(Cut);
        assert_eq!(out.clipboard_write.as_deref(), Some("world"));
        assert!(out.text_changed);
        assert_eq!(editor.text_str(), "hello ");
        editor.apply(CursorHome);
        assert_eq!(editor.apply(Backspace), TextEditOutcome::default());
    }

    #[test]
    fn manual_scroll_persists_across_flush() {
        let (mut font_system, mut editor) = make_editor(220.0, 2.0 * 14.0 * LINE_HEIGHT_FACTOR);

        editor.insert_text("line0\nline1\nline2\nline3");
        editor.flush(&mut font_system);

        let line_height = editor.scroll_line_height_px();
        assert!((editor.scroll_y - line_height * 2.0).abs() < 0.5);

        editor.scroll(-line_height);
        editor.flush(&mut font_system);

        assert!(
            (editor.scroll_y - line_height).abs() < 0.5,
            "expected manual scroll to persist, got {}",
            editor.scroll_y
        );
    }

    #[test]
    fn click_uses_visible_coordinates_after_manual_scroll() {
        let (mut font_system, mut editor) = make_editor(220.0, 2.0 * 14.0 * LINE_HEIGHT_FACTOR);

        editor.insert_text("line0\nline1\nline2\nline3");
        editor.flush(&mut font_system);

        let line_height = editor.scroll_line_height_px();
        editor.scroll(-line_height);
        editor.flush(&mut font_system);

        editor.click(0, 0);
        editor.flush(&mut font_system);

        assert_eq!(editor.cursor, "line0\n".len());
        assert_eq!(editor.anchor, "line0\n".len());
    }

    #[test]
    fn content_height_covers_cursor_line_for_trailing_newline() {
        let (mut font_system, mut editor) = make_editor(160.0, 2.0 * 14.0 * LINE_HEIGHT_FACTOR);

        editor.insert_text("first line\nsecond line\n");
        editor.flush(&mut font_system);

        let line_height = editor.scroll_line_height_px();
        let cursor_bottom = editor.cursor_pos.y + line_height;

        assert!(
            editor.content_height() + 0.5 >= cursor_bottom,
            "content height {} did not cover cursor bottom {}",
            editor.content_height(),
            cursor_bottom
        );
    }

    #[test]
    fn wrapped_content_beyond_viewport_height_is_fully_counted() {
        let (mut font_system, mut editor) = make_editor(120.0, 2.0 * 14.0 * LINE_HEIGHT_FACTOR);

        editor.insert_text(
            "asdf asf asdf asdf fasd fasd fasdf sdaf asdf sadf \
             sdaf asdf asdf asdf asd fasdf asdf asdf asdf asdf \
             asdf asf asdf asdf fasd fasd fasdf sdaf asdf sadf \
             sdaf asdf asdf asdf asd fasdf asdf asdf asdf asdf",
        );
        editor.flush(&mut font_system);

        let line_height = editor.scroll_line_height_px();
        let cursor_bottom = editor.cursor_pos.y + line_height;
        let viewport_height = 2.0 * 14.0 * LINE_HEIGHT_FACTOR;

        assert!(
            cursor_bottom > viewport_height + 0.5,
            "test text did not extend beyond the viewport: cursor_bottom={cursor_bottom} viewport_height={viewport_height}"
        );
        assert!(
            editor.content_height() + 0.5 >= cursor_bottom,
            "wrapped content height {} did not cover offscreen cursor bottom {}",
            editor.content_height(),
            cursor_bottom
        );
        assert!(
            (editor.scroll_y - (cursor_bottom - viewport_height)).abs() < line_height + 0.5,
            "expected scroll to reveal bottom line, got scroll_y={} cursor_bottom={} viewport_height={viewport_height}",
            editor.scroll_y,
            cursor_bottom
        );
    }

    #[test]
    fn click_can_target_lower_wrapped_visual_line() {
        let (mut font_system, mut editor) = make_editor(120.0, 10.0 * 14.0 * LINE_HEIGHT_FACTOR);

        editor.insert_text(
            "asdf asf asdf asdf fasd fasd fasdf sdaf asdf sadf \
             sdaf asdf asdf asdf asd fasdf asdf asdf asdf asdf",
        );
        editor.flush(&mut font_system);

        let layout = editor.layout().expect("layout").clone();
        let second = layout.line(1).expect("wrapped text");
        let click_y = (second.top - editor.scroll_y + second.height * 0.5) as i32;

        editor.click(1, click_y);
        editor.flush(&mut font_system);

        assert_eq!(editor.cursor, second.byte_range.start);
    }

    #[test]
    fn default_editor_uses_prose_mode() {
        let editor = Editor::default();

        assert_eq!(editor.mode(), EditorMode::ProseInput);
        assert!(!editor.mode().is_code());
    }

    #[test]
    fn code_input_mode_keeps_editing_enabled() {
        let (_font_system, mut editor) = make_editor(220.0, 80.0);
        editor.set_mode(EditorMode::CodeInput);

        editor.insert_text("fn main() {}\n");

        assert_eq!(editor.mode(), EditorMode::CodeInput);
        assert_eq!(editor.text(), "fn main() {}\n");
        assert_eq!(editor.line_count(), 1);
    }

    #[test]
    fn diff_read_only_mode_blocks_user_mutations() {
        let (_font_system, mut editor) = make_editor(220.0, 80.0);
        editor.set_mode(EditorMode::DiffReadOnly);
        editor.set_text("unchanged");

        editor.insert_text(" edited");
        editor.delete_backward();

        assert_eq!(editor.text(), "unchanged");
        assert_eq!(editor.mode(), EditorMode::DiffReadOnly);
    }
}
