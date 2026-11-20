//! The multiline editor model.
//!
//! The text lives in a [`Rope`] (inside the same [`TextBuffer`] a
//! [`super::TextField`] uses), so an edit costs O(log n) whatever the
//! document's size. Layout is kept per paragraph (logical line) in
//! `paragraphs`: [`Editor::flush`] shapes only the lines around the view
//! and the caret's, and a line keeps its layout across edits elsewhere.
//! Highlighting is per line and per window too (`highlight`). Caret,
//! selection, hit-testing, and vertical movement read the same layouts the
//! element paints, so they cannot drift from the glyphs.
//!
//! [`Editor::text`] still returns the whole text as a `&str`, but makes it
//! only when asked (once per revision); editing, layout, painting, and
//! pointer input never need it.

mod compose;
mod geometry;
mod highlight;
mod layout;
mod paragraphs;
mod spans;
mod spelling;
#[cfg(test)]
mod tests;
mod window;

use std::borrow::Cow;
use std::ops::Range;
use std::sync::Arc;

use quark_render::scene::FontKind;
use quark_text::TextOffset;

use super::anchor::CaretAnchor;
use super::atoms::{InlineAtom, RichClipboard, RichText};
use super::buffer::{TextBuffer, WordForward};
use super::doc_text::{TextChange, TextSnapshot};
use super::hooks::{InputHooks, Insertion, NoHooks};
use super::ime::Preedit;
use super::rope::Rope;
use super::styles::{RichExport, StyleSpan, TextFormat};
use super::text_edit::{TextEditCommand, TextEditOutcome};
use super::trigger::{TriggerMatch, TriggerRule, find_trigger};
use super::view::{FrameScale, FrameViewport};
use compose::ComposedBlock;
use highlight::{Highlighting, SyntaxSource};
use paragraphs::Paragraphs;
use spelling::Spelling;

pub(crate) use spans::syntax_layout_spans;
pub use spans::{
    LineHighlighter, LineState, StyleHighlighter, SyntaxHighlighter, SyntaxSpan, SyntaxTokenKind,
    TextDecoration,
};
pub use spelling::SpellingIssue;
pub use window::{EditorWindow, WindowLine};

const LINE_HEIGHT_FACTOR: f32 = 1.35;

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

#[derive(Debug, Clone, Copy, Default)]
pub struct CursorState {
    pub x: f32,
    pub y: f32,
}

/// Whether the next flush scrolls the caret into view.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Reveal {
    #[default]
    Keep,
    Caret,
}

/// Whether the caret is drawn.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum CaretVisibility {
    #[default]
    Shown,
    /// The IME hides it while composing.
    HiddenByIme,
}

/// Whether lines past the view are still being highlighted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum HighlightProgress {
    #[default]
    Done,
    CatchingUp,
}

/// Font size, line height, and scale the text is shaped at.
#[derive(Debug, Clone, Copy)]
struct TextMetrics {
    font_size: f32,
    /// In points; `None` follows the font size.
    line_height: Option<f32>,
    /// Window scale factor; the layout is shaped at it so it can be painted
    /// as is (see `ElementContext::layout_text`).
    scale_factor: f32,
}

impl TextMetrics {
    fn line_height(&self) -> f32 {
        self.line_height
            .unwrap_or(self.font_size * LINE_HEIGHT_FACTOR)
    }
}

/// Multiline editor model.
///
/// Text, caret, selection, IME preedit, and undo live in the same
/// [`TextBuffer`] a [`super::TextField`] uses; the editor adds a layout,
/// vertical movement, scrolling, and highlighting. Call [`Editor::flush`]
/// once per frame before building the element: it shapes the lines around
/// the view and places the caret. See the module docs.
#[derive(Clone)]
pub struct Editor {
    mode: EditorMode,
    buffer: TextBuffer,
    metrics: TextMetrics,
    /// Viewport size from the app, in points.
    viewport: (f32, f32),
    paragraphs: Paragraphs,
    highlight: Highlighting,
    highlight_progress: HighlightProgress,
    /// The lines an IME composition replaces on screen, while composing.
    composition: Option<ComposedBlock>,
    reveal: Reveal,
    caret: CaretVisibility,
    desired_x: Option<f32>,
    pub scroll_y: f32,
    pub cursor_pos: CursorState,
    pub cursor_moved_at_ms: u64,
    /// Scale of the frame the editor's element last painted in; applied
    /// on the next flush.
    pub(crate) frame_scale: FrameScale,
    /// Height the element last painted the editor at; the next flush
    /// shapes at least that much.
    pub(crate) frame_viewport: FrameViewport,
    /// Bumped to reshape every line.
    font_rev: u32,
    spelling: Spelling,
    /// Where the element last placed the caret on screen.
    caret_anchor: CaretAnchor,
    /// The lines shaped for the view at the last flush.
    window: EditorWindow,
    /// `(1-based logical line, top)` of each line of `window`.
    line_tops: Arc<[(usize, f32)]>,
    /// Changes taken from the buffer, kept for its storage.
    changes: Vec<TextChange>,
}

impl Default for Editor {
    fn default() -> Self {
        Self::new(EditorMode::default())
    }
}

impl std::fmt::Debug for Editor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Editor")
            .field("mode", &self.mode)
            .field("len", &self.buffer.rope().len())
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

impl Editor {
    pub fn new(mode: EditorMode) -> Self {
        let mut buffer = TextBuffer::new(true, WordForward::NextEnd);
        buffer.set_read_only(!mode.is_editable());
        buffer.track_changes();
        Self {
            mode,
            buffer,
            metrics: TextMetrics {
                font_size: 14.0,
                line_height: None,
                scale_factor: 1.0,
            },
            viewport: (0.0, 0.0),
            paragraphs: Paragraphs::default(),
            highlight: Highlighting::default(),
            highlight_progress: HighlightProgress::Done,
            composition: None,
            reveal: Reveal::Keep,
            caret: CaretVisibility::Shown,
            desired_x: None,
            scroll_y: 0.0,
            cursor_pos: CursorState::default(),
            cursor_moved_at_ms: 0,
            frame_scale: FrameScale::default(),
            frame_viewport: FrameViewport::default(),
            font_rev: 0,
            spelling: Spelling::default(),
            caret_anchor: CaretAnchor::default(),
            window: EditorWindow::default(),
            line_tops: Arc::from([]),
            changes: Vec::new(),
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
        self.highlight_changed();
    }

    /// Highlight code with a whole-text highlighter; see
    /// [`SyntaxHighlighter`].
    pub fn set_syntax_highlighter(&mut self, highlighter: SyntaxHighlighter) {
        let rope = self.buffer.rope().clone();
        self.highlight
            .set_syntax(SyntaxSource::Whole(highlighter), &rope);
    }

    /// Highlight code a line at a time, re-highlighting only what an edit
    /// changed and what comes into view; see [`LineHighlighter`].
    pub fn set_line_highlighter(&mut self, highlighter: Arc<dyn LineHighlighter>) {
        let rope = self.buffer.rope().clone();
        self.highlight
            .set_syntax(SyntaxSource::Lines(highlighter), &rope);
    }

    pub fn clear_syntax_highlighter(&mut self) {
        if self.highlight.has_syntax() {
            self.highlight.clear_syntax();
        }
    }

    /// Style prose for display with `highlighter`; see [`StyleHighlighter`].
    pub fn set_style_highlighter(&mut self, highlighter: StyleHighlighter) {
        self.highlight.set_style(highlighter);
    }

    pub fn clear_style_highlighter(&mut self) {
        if self.highlight.has_style() {
            self.highlight.clear_style();
        }
    }

    /// Every line's spans may have changed (the mode switched).
    fn highlight_changed(&mut self) {
        let rope = self.buffer.rope().clone();
        self.highlight.text_changed(TextChange::Reset, &rope);
    }

    /// Set the app clock used for caret blink and undo coalescing.
    pub fn set_clock(&mut self, now_ms: u64) {
        self.buffer.set_now(now_ms);
    }

    fn line_height(&self) -> f32 {
        self.metrics.line_height()
    }

    fn note_cursor_activity(&mut self) {
        self.reveal = Reveal::Caret;
        self.cursor_moved_at_ms = self.buffer.now_ms();
    }

    /// Follow the buffer's text (or formatting) changes in the layout,
    /// highlighting, and spelling.
    fn text_changed(&mut self) {
        self.sync_changes();
        // A formatting change edits no text but restyles lines.
        self.highlight.formatting_changed();
        self.spelling.text_changed();
    }

    /// Apply the line changes the buffer logged to the layout and
    /// highlighting. Also run by flush, for changes no outcome reported
    /// (an undo that found the text changed and put it back).
    fn sync_changes(&mut self) {
        self.buffer.take_changes(&mut self.changes);
        if self.changes.is_empty() {
            return;
        }
        let rope = self.buffer.rope().clone();
        for change in self.changes.drain(..) {
            match change {
                TextChange::Reset => self.paragraphs.reset(rope.line_count()),
                TextChange::Lines {
                    first,
                    removed,
                    inserted,
                } => self.paragraphs.splice(first, removed, inserted),
            }
            self.highlight.text_changed(change, &rope);
        }
        debug_assert_eq!(self.paragraphs.len(), rope.line_count());
        if self.paragraphs.len() != rope.line_count() {
            self.paragraphs.reset(rope.line_count());
        }
    }

    pub fn cursor(&self) -> TextOffset {
        self.buffer.cursor()
    }

    pub fn anchor(&self) -> TextOffset {
        self.buffer.anchor()
    }

    /// The selection in order; empty at the caret when nothing is selected.
    pub fn selection(&self) -> Range<TextOffset> {
        self.buffer.selection()
    }

    pub fn set_font_size(&mut self, font_size: f32) {
        if (self.metrics.font_size - font_size).abs() >= 0.01 {
            self.metrics.font_size = font_size;
        }
    }

    /// Space each line takes, in points; `None` (the default) uses 1.35
    /// times the font size.
    pub fn set_line_height(&mut self, line_height: Option<f32>) {
        self.metrics.line_height = line_height.filter(|h| h.is_finite() && *h > 0.0);
    }

    /// Shape at `scale_factor`. Painting the editor's element sets this
    /// from the frame on the next flush, so apps rarely need to call it.
    pub fn set_scale_factor(&mut self, scale_factor: f32) {
        if scale_factor.is_finite() && scale_factor > 0.0 {
            self.metrics.scale_factor = scale_factor;
        }
    }

    /// Reshape every line on the next flush.
    pub fn invalidate_font(&mut self) {
        self.font_rev = self.font_rev.wrapping_add(1);
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
        self.reveal = Reveal::Caret;
        self.desired_x = None;
        self.text_changed();
    }

    /// Viewport size in pixels. The width less the code gutter is the wrap
    /// width.
    pub fn sync_size(&mut self, width: f32, height: f32) {
        let (last_width, _) = self.viewport;
        let width = if (last_width - width).abs() > 0.5 {
            width
        } else {
            last_width
        };
        self.viewport = (width, height);
    }

    /// Width of the line-number gutter the element paints left of the text
    /// in code modes; 0 in prose.
    pub fn gutter_width(&self) -> f32 {
        if !self.mode.is_code() {
            return 0.0;
        }
        gutter_width_in(
            self.metrics.font_size,
            self.paragraph_count(),
            self.viewport.0,
        )
    }

    /// The caret's last painted position, for anchoring a completion popup
    /// to it (see [`super::caret_popup`]). Clones of the editor share it.
    pub fn caret_anchor(&self) -> &CaretAnchor {
        &self.caret_anchor
    }

    /// The whole text as one string. Made from the rope on the first call
    /// after a change (borrowed directly while the text fits one chunk),
    /// so a large document pays for the copy only when someone asks; the
    /// editor itself never does. Prefer [`Self::rope`], [`Self::line_str`],
    /// or [`Self::slice`].
    pub fn text(&self) -> &str {
        self.buffer.text()
    }

    /// [`Self::text`] shared.
    pub fn text_arc(&self) -> Arc<str> {
        self.buffer.snapshot().flat()
    }

    /// The current text revision, shared with whoever paints it.
    pub(crate) fn text_snapshot(&self) -> TextSnapshot {
        self.buffer.snapshot()
    }

    /// The text as a rope: cheap to clone, slice, and query by line.
    pub fn rope(&self) -> &Rope {
        self.buffer.rope()
    }

    /// Length of the text in bytes.
    pub fn text_len(&self) -> usize {
        self.buffer.rope().len()
    }

    /// The text in `range` (bytes, snapped onto grapheme boundaries),
    /// borrowed when it lies in one chunk of the rope.
    pub fn slice(&self, range: Range<usize>) -> Cow<'_, str> {
        let doc = self.buffer.doc();
        doc.slice(doc.snap(range.start)..doc.snap(range.end))
    }

    /// Logical line `line` (0-based) without its line ending.
    pub fn line_str(&self, line: usize) -> Cow<'_, str> {
        let rope = self.buffer.rope();
        rope.slice(rope.line_content_range(line))
    }

    /// Lines as the layout counts them: one more than the newlines, so an
    /// empty text has one and a trailing newline starts another.
    pub fn paragraph_count(&self) -> usize {
        self.buffer.rope().line_count()
    }

    /// The syntax spans a whole-text highlighter made for the text (or the
    /// lines around the view, for a long text), cut at line breaks.
    pub fn syntax_spans(&self) -> &Arc<[SyntaxSpan]> {
        self.highlight.whole_syntax_spans()
    }

    pub fn byte_len(&self) -> usize {
        self.text_len()
    }

    /// Lines of text: 0 when empty, and a trailing newline adds none.
    pub fn line_count(&self) -> usize {
        let rope = self.buffer.rope();
        if rope.is_empty() {
            return 0;
        }
        let ends_with_newline = rope.char_at(rope.len() - 1) == Some('\n');
        rope.summary().newlines + usize::from(!ends_with_newline)
    }

    pub fn is_empty(&self) -> bool {
        self.buffer.rope().is_empty() && self.buffer.preedit().is_none()
    }

    pub fn selected_text(&self) -> Option<&str> {
        self.buffer.selected_text()
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

    pub fn scroll(&mut self, delta_px: f32) {
        let max_scroll = (self.content_height() - self.viewport.1).max(0.0);
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
        let range = self.buffer.doc().ordered(range);
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

    /// The whole text with its atoms, for sending, history, or a draft.
    pub fn rich_text(&self) -> RichText {
        let doc = self.buffer.doc();
        self.buffer.rich_slice(TextOffset::ZERO..doc.end())
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
    /// no IME composition is open. Reads only the caret's line.
    pub fn active_trigger(&self, rules: &[TriggerRule]) -> Option<TriggerMatch> {
        if self.buffer.preedit().is_some() || !self.buffer.selection().is_empty() {
            return None;
        }
        let doc = self.buffer.doc();
        let cursor = self.buffer.cursor().get();
        let start = doc.rope().line_start_at(cursor);
        let line = doc.rope().slice(start..cursor);
        let atoms: Vec<InlineAtom> = self
            .buffer
            .atoms()
            .iter()
            .filter(|a| a.range.start >= start && a.range.end <= cursor)
            .map(|a| InlineAtom {
                range: a.range.start - start..a.range.end - start,
                ..a.clone()
            })
            .collect();
        let found = find_trigger(
            &line,
            TextOffset::snap(&line, cursor - start),
            &atoms,
            rules,
        )?;
        Some(TriggerMatch {
            range: doc.snap(found.range.start.get() + start)
                ..doc.snap(found.range.end.get() + start),
            ..found
        })
    }
}
