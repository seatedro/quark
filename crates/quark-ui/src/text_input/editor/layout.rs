//! Flushing: shaping the caret's line and the lines around the view, then
//! placing the caret and scroll.
//!
//! The view is shaped from the line at its top (less an overscan) down,
//! re-anchoring the scroll after each line so that a line whose real height
//! differs from its estimate never moves what is on screen: the anchor is
//! the caret's line while revealing it, else the line at the top of the
//! view. Lines far from the view drop their layouts once more than
//! [`RETAINED_LINES`] hold one.

use std::sync::Arc;

use quark_text::{TextParams, TextSpan, TextStyle, TextSystem};

use super::highlight::LayoutSpans;
use super::paragraphs::{ShapeParams, Shaped};
use super::window::{EditorWindow, WindowLine};
use super::{Editor, HighlightProgress, Reveal};

/// Most lines that keep a layout; past this, those far from the view drop
/// theirs.
const RETAINED_LINES: usize = 1024;

impl Editor {
    /// What the next shape depends on besides each line's text and spans.
    fn shape_params(&self, text_system: &TextSystem) -> ShapeParams {
        // The element paints text right of the gutter, so wrap there too.
        let width = self.viewport.0;
        let wrap = (width > 0.0).then(|| (width - self.gutter_width()).max(1.0));
        ShapeParams {
            font_size: self.metrics.font_size,
            line_height: self.line_height(),
            kind: self.mode.font_kind(),
            wrap,
            scale: self.metrics.scale_factor,
            fonts: text_system.font_epoch(),
            font_rev: self.font_rev,
        }
    }

    pub(super) fn text_style(&self) -> TextStyle {
        TextStyle::new(self.metrics.font_size)
            .kind(self.mode.font_kind())
            .line_height(self.line_height())
    }

    /// Params for shaping `text` with `spans` as this editor does.
    pub(super) fn layout_params(&self, text: Arc<str>, spans: Vec<TextSpan>) -> TextParams {
        let params = self.paragraphs.params();
        TextParams::new(text, self.text_style())
            .spans(spans)
            .wrap_width(params.and_then(|p| p.wrap))
            .scale_factor(self.metrics.scale_factor)
    }

    /// Shapes `line` unless its layout is current; a fresh layout whose
    /// spans turn out unchanged is kept. Returns whether it shaped.
    fn ensure_shaped(&mut self, text_system: &mut TextSystem, line: usize) -> bool {
        let generation = self.highlight.generation();
        if self.paragraphs.current(line, generation).is_some() {
            return false;
        }
        let rope = self.buffer.rope().clone();
        let content = rope.line_content_range(line);
        let text = rope.slice(content);
        let mut out = LayoutSpans::default();
        self.highlight.line_spans(
            &rope,
            self.mode,
            line,
            &text,
            self.buffer.styles(),
            &mut out,
        );
        let LayoutSpans { spans, kinds } = out;
        // The highlight may have moved on while finding the spans.
        let generation = self.highlight.generation();
        if self.paragraphs.is_fresh(line)
            && let Some(shaped) = self.paragraphs.shaped(line)
            && shaped.layout.spans() == spans.as_slice()
            && *shaped.kinds == *kinds
        {
            self.paragraphs.mark_checked(line, generation);
            return false;
        }
        let params = self.layout_params(Arc::from(text.as_ref()), spans);
        let Ok(layout) = text_system.layout(&params) else {
            return false;
        };
        let shaped = Shaped {
            layout: Arc::new(layout),
            kinds: kinds.into(),
        };
        self.paragraphs.store(line, shaped, generation);
        true
    }

    /// The height the view is shaped for: the app's, or what the element
    /// last painted when that was taller.
    fn view_height(&self) -> f32 {
        self.viewport.1.max(self.frame_viewport.get())
    }

    /// Shaped beyond the view on each side.
    fn overscan(&self) -> f32 {
        (self.view_height() * 0.5).max(4.0 * self.line_height())
    }

    pub(super) fn caret_line(&self) -> usize {
        self.buffer.rope().byte_to_line(self.buffer.cursor().get())
    }

    /// Clamps the scroll and, when asked, scrolls the caret into view.
    /// Returns the line to keep in place while the view is shaped, and its
    /// top less the scroll.
    fn reveal_caret(&mut self) -> (usize, f32) {
        let height = self.viewport.1;
        let max_scroll = (self.content_height() - height).max(0.0);
        self.scroll_y = self.scroll_y.clamp(0.0, max_scroll);
        if self.reveal == Reveal::Caret && height > 0.0 {
            let y = self.cursor_pos.y;
            let bottom = y + self.line_height();
            if bottom > self.scroll_y + height {
                self.scroll_y = bottom - height;
            } else if y < self.scroll_y {
                self.scroll_y = y;
            }
            self.scroll_y = self.scroll_y.clamp(0.0, max_scroll);
            let line = self.caret_line();
            return (line, self.paragraphs.top(line) - self.scroll_y);
        }
        let line = self.paragraphs.at_y(self.scroll_y);
        (line, self.paragraphs.top(line) - self.scroll_y)
    }

    /// Shapes the lines around the view, keeping `anchor` where it is on
    /// screen. Returns the lines shaped and whether any was reshaped.
    fn shape_view(
        &mut self,
        text_system: &mut TextSystem,
        (anchor, offset): (usize, f32),
    ) -> (std::ops::Range<usize>, bool) {
        let (view, over) = (self.view_height(), self.overscan());
        let first = self.paragraphs.at_y(self.scroll_y - over);
        let guess = self.paragraphs.at_y(self.scroll_y + view + over);
        let progress = self
            .highlight
            .refresh(self.buffer.doc(), self.mode, first..guess + 1);
        self.highlight_progress = if progress {
            HighlightProgress::CatchingUp
        } else {
            HighlightProgress::Done
        };
        let mut changed = false;
        let mut last = first;
        loop {
            changed |= self.ensure_shaped(text_system, last);
            self.scroll_y = self.paragraphs.top(anchor) - offset;
            let bottom = self.paragraphs.top(last) + self.paragraphs.height(last);
            if bottom >= self.scroll_y + view + over || last + 1 >= self.paragraphs.len() {
                break;
            }
            last += 1;
        }
        let mut first = first;
        while first > 0 && self.paragraphs.top(first) > self.scroll_y - over {
            first -= 1;
            changed |= self.ensure_shaped(text_system, first);
            self.scroll_y = self.paragraphs.top(anchor) - offset;
        }
        (first..last + 1, changed)
    }

    /// The caret's position in content coordinates, from the composition
    /// while composing.
    fn place_caret(&mut self) {
        let (x, y, hidden) = match &self.composition {
            Some(block) => {
                let composition = &block.composition;
                let at = composition.caret.unwrap_or(composition.preedit.end);
                let caret = block.layout.caret(at);
                let top = self.paragraphs.top(block.first);
                (caret.x, top + caret.y, composition.caret.is_none())
            }
            None => {
                let (x, y) = self.caret_point(self.buffer.cursor()).unwrap_or_default();
                (x, y, false)
            }
        };
        self.caret = if hidden {
            super::CaretVisibility::HiddenByIme
        } else {
            super::CaretVisibility::Shown
        };
        self.cursor_pos = super::CursorState { x, y };
    }

    /// Rebuild the layout of the lines near the view (and the caret's)
    /// after changes, place the caret, and scroll it into view if it
    /// moved. Shapes O(view) lines however long the text is.
    pub fn flush(&mut self, text_system: &mut TextSystem) {
        self.sync_changes();
        if let Some(scale) = self.frame_scale.get() {
            self.set_scale_factor(scale);
        }
        let params = self.shape_params(text_system);
        let relaid = self.paragraphs.params() != Some(params);
        if relaid {
            self.paragraphs.set_params(params);
        }
        let caret_line = self.caret_line();
        let mut changed = relaid | self.ensure_shaped(text_system, caret_line);
        let preedit_changed = self.buffer.take_preedit_dirty();
        if preedit_changed || changed {
            self.rebuild_composition(text_system);
        }
        self.place_caret();
        let anchor = self.reveal_caret();
        let (lines, view_changed) = self.shape_view(text_system, anchor);
        changed |= view_changed;
        if view_changed {
            self.rebuild_composition(text_system);
        }
        self.place_caret();
        // Shaping may have changed the content height; keep the caret in
        // view if it was being revealed.
        self.reveal_caret();
        self.reveal = Reveal::Keep;
        self.paragraphs.evict(
            lines.start.min(caret_line)..lines.end.max(caret_line + 1),
            RETAINED_LINES,
        );
        if changed || preedit_changed || self.window.line_range() != lines || self.window_stale() {
            self.rebuild_window(lines);
        }
        self.refresh_spelling();
    }

    /// Whether the painted window no longer matches the text (an edit since
    /// it was built moved or changed its lines).
    fn window_stale(&self) -> bool {
        self.window
            .lines()
            .iter()
            .any(|l| match self.paragraphs.shaped(l.line) {
                Some(shaped) if self.composition.is_none() => {
                    !Arc::ptr_eq(&shaped.layout, &l.layout)
                        || self.buffer.rope().line_to_byte(l.line) != l.start
                        || self.paragraphs.top(l.line) != l.top
                }
                Some(_) => false,
                None => true,
            })
    }

    /// The window for paint and pointers: the shaped lines in `lines`,
    /// with the composition's block in place of the lines it covers.
    fn rebuild_window(&mut self, lines: std::ops::Range<usize>) {
        let rope = self.buffer.rope().clone();
        let mut out = Vec::with_capacity(lines.len());
        let mut tops = Vec::with_capacity(lines.len());
        let block = self.composition.as_ref();
        let shift = block.map_or(0.0, |b| b.height - self.block_committed_height(b));
        let mut line = lines.start;
        while line < lines.end {
            let top = self.paragraphs.top(line);
            tops.push((line + 1, top));
            if let Some(block) = block.filter(|b| b.first == line) {
                out.push(WindowLine {
                    line,
                    start: block.start,
                    top,
                    layout: block.layout.clone(),
                    kinds: block.kinds.clone(),
                });
                for skipped in line + 1..=block.last.min(lines.end.saturating_sub(1)) {
                    tops.push((skipped + 1, self.paragraphs.top(skipped)));
                }
                line = block.last + 1;
                continue;
            }
            let moved = block.is_some_and(|b| line > b.last);
            if let Some(shaped) = self.paragraphs.shaped(line) {
                out.push(WindowLine {
                    line,
                    start: rope.line_to_byte(line),
                    top: if moved { top + shift } else { top },
                    layout: shaped.layout.clone(),
                    kinds: shaped.kinds.clone(),
                });
            }
            line += 1;
        }
        self.window = EditorWindow::new(out, rope, lines, self.composition.is_some());
        self.line_tops = tops.into();
    }

    /// [`Self::flush`] at `width`, sizing the viewport to the text: as tall
    /// as its lines but at least `min_height`, and past `max_height` the
    /// text scrolls inside. Returns the height to give the element, so a
    /// chat input grows as the user types and stops growing at a line cap.
    pub fn flush_fit(
        &mut self,
        text_system: &mut TextSystem,
        width: f32,
        min_height: f32,
        max_height: f32,
    ) -> f32 {
        let max_height = max_height.max(min_height);
        // Reveal the caret in the tallest viewport first: when the text
        // fits, the height below shows all of it anyway.
        self.sync_size(width, max_height);
        self.flush(text_system);
        let height = self.content_height().clamp(min_height, max_height);
        self.viewport.1 = height;
        let max_scroll = (self.content_height() - height).max(0.0);
        self.scroll_y = self.scroll_y.clamp(0.0, max_scroll);
        height
    }

    /// Height of the text: every line, shaped or estimated, with any IME
    /// composition in place of the lines it covers.
    pub fn content_height(&self) -> f32 {
        let line_height = self.line_height();
        let composed = self
            .composition
            .as_ref()
            .map_or(0.0, |b| b.height - self.block_committed_height(b));
        (self.paragraphs.total() + composed).max(line_height)
    }

    pub fn scroll_line_height_px(&self) -> f32 {
        self.line_height()
    }

    /// `(1-based logical line, top)` for each line shaped around the view,
    /// as of the last flush.
    pub fn logical_line_tops(&self) -> &Arc<[(usize, f32)]> {
        &self.line_tops
    }

    /// The lines shaped around the view at the last flush, for painting
    /// and pointer hits.
    pub fn window(&self) -> &EditorWindow {
        &self.window
    }

    /// How many lines hold a shaped layout: about the view's, however long
    /// the text, for memory budgets and tests.
    pub fn shaped_line_count(&self) -> usize {
        self.paragraphs.shaped_count()
    }

    /// Whether highlighting has lines left to catch up on past the view;
    /// the element paints again until it does.
    pub fn is_highlighting(&self) -> bool {
        self.highlight_progress == HighlightProgress::CatchingUp
    }
}
