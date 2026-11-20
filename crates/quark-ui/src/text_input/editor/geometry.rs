//! Positions in the per-line layout: caret, rects, pointer hits, and
//! vertical movement. Everything is in content coordinates (the top of the
//! text at 0, before scrolling): a line's own layout coordinates moved
//! down by its top.

use std::ops::Range;
use std::sync::Arc;

use quark_text::{TextLayout, TextOffset};

use super::spans::{SyntaxTokenKind, TextDecoration};
use super::{Editor, SelectionRect};
use crate::text_input::styles::InlineStyle;

impl Editor {
    /// Line `line`'s start, and the line holding byte `at`.
    fn locate(&self, at: usize) -> (usize, usize) {
        let rope = self.buffer.rope();
        let line = rope.byte_to_line(at);
        (line, rope.line_to_byte(line))
    }

    /// The caret at `at` as `(x, y)`, when its line is shaped (its layout
    /// may predate edits since the last flush).
    pub(super) fn caret_point(&self, at: TextOffset) -> Option<(f32, f32)> {
        let (line, start) = self.locate(at.get());
        let shaped = self.paragraphs.shaped(line)?;
        let caret = shaped.layout.caret(at.get() - start);
        Some((caret.x, self.paragraphs.top(line) + caret.y))
    }

    /// The layout of the line holding the caret, as of the last flush.
    /// Offsets and coordinates in it are relative to that line; see
    /// [`Self::window`] for every shaped line.
    pub fn layout(&self) -> Option<&Arc<TextLayout>> {
        let line = self.caret_line();
        self.paragraphs.shaped(line).map(|s| &s.layout)
    }

    /// The layout of what the caret is in as painted: the composed lines
    /// while an IME composes, else the caret's line. Relative to its top.
    pub fn paint_layout(&self) -> Option<Arc<TextLayout>> {
        match &self.composition {
            Some(block) => Some(block.layout.clone()),
            None => self.layout().cloned(),
        }
    }

    /// Syntax kind of each span of [`Editor::paint_layout`].
    pub fn paint_span_kinds(&self) -> Arc<[SyntaxTokenKind]> {
        match &self.composition {
            Some(block) => block.kinds.clone(),
            None => self
                .paragraphs
                .shaped(self.caret_line())
                .map_or_else(|| Arc::from([]), |s| s.kinds.clone()),
        }
    }

    /// The offset under the content point `(x, y)`, through the lines
    /// shaped at the last flush. `None` before the first flush and while
    /// composing.
    pub fn offset_at(&self, x: f32, y: f32) -> Option<TextOffset> {
        self.window.hit(x, y)
    }

    /// Calls `push` with the rects covering bytes `range` on the lines
    /// shaped around the view, as a selection would highlight them: an
    /// empty line inside the range gets a narrow box.
    fn rects_for(&self, range: Range<usize>, mut push: impl FnMut(SelectionRect)) {
        if range.is_empty() {
            return;
        }
        let rope = self.buffer.rope();
        let font_size = self.metrics.font_size;
        for line in self.window.lines() {
            let next = rope.line_to_byte(line.line + 1);
            if range.start >= next.max(line.start + 1) || range.end <= line.start {
                continue;
            }
            let text_len = line.layout.text().len();
            let local = range.start.max(line.start) - line.start
                ..range.end.min(line.start + text_len) - line.start;
            if text_len == 0 {
                push(SelectionRect {
                    x: 0.0,
                    y: line.top,
                    w: font_size * 0.3,
                    h: line.layout.size().1,
                });
                continue;
            }
            if local.start >= local.end {
                continue;
            }
            for rect in line.layout.selection_rects(local) {
                let mut rect = SelectionRect::from(rect);
                rect.y += line.top;
                push(rect);
            }
        }
    }

    /// Selection highlight rects in content coordinates, on the lines
    /// shaped around the view. Empty while composing.
    pub fn selection_rects(&self) -> Vec<SelectionRect> {
        let selection = self.buffer.selection();
        if selection.is_empty() || self.composition.is_some() {
            return Vec::new();
        }
        let mut out = Vec::new();
        self.rects_for(selection.start.get()..selection.end.get(), |r| out.push(r));
        out
    }

    /// Rects under the preedit (to underline) and under the IME's
    /// highlighted clause, in content coordinates.
    pub fn preedit_rects(&self) -> (Vec<SelectionRect>, Vec<SelectionRect>) {
        let Some(block) = &self.composition else {
            return (Vec::new(), Vec::new());
        };
        let top = self.paragraphs.top(block.first);
        let rects = |range: Range<TextOffset>| {
            block
                .layout
                .selection_rects(range)
                .map(|r| {
                    let mut rect = SelectionRect::from(r);
                    rect.y += top;
                    rect
                })
                .collect::<Vec<_>>()
        };
        let composition = &block.composition;
        let clause = composition.clause.clone().map(rects).unwrap_or_default();
        (rects(composition.preedit.clone()), clause)
    }

    /// Whether the caret is drawn (the IME may hide it while composing).
    pub fn caret_visible(&self) -> bool {
        self.caret == super::CaretVisibility::Shown
    }

    /// Underline, strikethrough, and code boxes for the formatting (and
    /// wavy lines under misspellings) in content coordinates, on the lines
    /// shaped around the view. Empty while composing.
    pub fn decoration_rects(&self) -> Vec<(TextDecoration, SelectionRect)> {
        if self.composition.is_some() || self.buffer.preedit().is_some() {
            return Vec::new();
        }
        let rope = self.buffer.rope();
        let mut out = Vec::new();
        let mut push = |kind, range: Range<usize>| {
            self.rects_for(range, |r| out.push((kind, r)));
        };
        // The word being typed at the caret is not marked until it is done.
        let caret = self.buffer.cursor().get();
        let typing = self.buffer.selection().is_empty();
        let view = self.window_bytes();
        for range in self.misspellings() {
            let valid = range.end <= rope.len()
                && rope.is_char_boundary(range.start)
                && rope.is_char_boundary(range.end);
            let visible = range.start < view.end && view.start < range.end;
            if valid && visible && !(typing && range.end == caret) {
                push(TextDecoration::Misspelled, range.clone());
            }
        }
        if !self.mode.is_code() {
            let styles = self.buffer.styles();
            let first = styles.partition_point(|s| s.range.end <= view.start);
            for span in styles
                .iter()
                .skip(first)
                .take_while(|s| s.range.start < view.end)
            {
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

    /// The bytes of the lines shaped around the view.
    fn window_bytes(&self) -> Range<usize> {
        let rope = self.buffer.rope();
        let lines = self.window.line_range();
        rope.line_to_byte(lines.start)..rope.line_to_byte(lines.end)
    }

    /// Rects behind each atom's label in content coordinates, on the lines
    /// shaped around the view. Empty without atoms or while composing.
    pub fn atom_rects(&self) -> Vec<SelectionRect> {
        let atoms = self.buffer.atoms();
        if atoms.is_empty() || self.composition.is_some() || self.buffer.preedit().is_some() {
            return Vec::new();
        }
        let view = self.window_bytes();
        let first = atoms.partition_point(|a| a.range.end <= view.start);
        let mut out = Vec::new();
        for atom in atoms
            .iter()
            .skip(first)
            .take_while(|a| a.range.start < view.end)
        {
            self.rects_for(atom.range.clone(), |r| out.push(r));
        }
        out
    }

    /// The visual line holding the caret. The end of a soft-wrapped line
    /// steps back over its trailing space so the caret stays on that line.
    fn visual_line_range(&self) -> Option<Range<TextOffset>> {
        let cursor = self.buffer.cursor().get();
        let (line, start) = self.locate(cursor);
        let layout = &self.paragraphs.shaped(line)?.layout;
        let caret = layout.caret(cursor - start);
        let visual = layout.line(caret.line)?;
        let doc = self.buffer.doc();
        let from = doc.snap(start + visual.byte_range.start);
        let mut to = doc.snap(start + visual.byte_range.end);
        let wrapped = layout
            .line(caret.line + 1)
            .is_some_and(|next| next.byte_range.start == visual.byte_range.end);
        if wrapped && to > from {
            to = doc.prev_grapheme(to).max(from);
        }
        Some(from..to)
    }

    /// Soft Home or End: the edge of the visual line, or of the logical
    /// line before the line is shaped.
    pub(super) fn move_soft(&mut self, to_end: bool, extend: bool) {
        use crate::text_input::TextEditCommand::*;
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

    /// Move to the visual line above or below at the remembered column.
    /// Past the first or last line the caret goes to the start or end of
    /// the text.
    pub(super) fn move_vertical(&mut self, down: bool, extend: bool) {
        let cursor = self.buffer.cursor().get();
        let (line, start) = self.locate(cursor);
        let Some(shaped) = self.paragraphs.shaped(line).cloned() else {
            return;
        };
        let layout = &shaped.layout;
        let caret = layout.caret(cursor - start);
        let x = self.desired_x.unwrap_or(caret.x);
        let doc = self.buffer.doc();
        let within = if down {
            caret.line.checked_add(1)
        } else {
            caret.line.checked_sub(1)
        };
        let target = within
            .and_then(|i| layout.line(i))
            .map(|visual| start + layout.hit(x, visual.top + visual.height * 0.5).get());
        let next_line = if down {
            Some(line + 1).filter(|&l| l < self.paragraphs.len())
        } else {
            line.checked_sub(1)
        };
        let target = target.or_else(|| {
            let next = next_line?;
            let next_start = self.buffer.rope().line_to_byte(next);
            let Some(next_shaped) = self.paragraphs.shaped(next) else {
                // Not shaped yet: the same byte column, clamped.
                let end = self.buffer.rope().line_content_range(next).end;
                return Some((next_start + (cursor - start)).min(end));
            };
            let layout = &next_shaped.layout;
            let visual = if down {
                layout.line(0)
            } else {
                layout.line(layout.line_count().saturating_sub(1))
            }?;
            Some(next_start + layout.hit(x, visual.top + visual.height * 0.5).get())
        });
        let (to, desired_x) = match target {
            Some(at) => (doc.snap(at), Some(x)),
            None if down => (doc.end(), None),
            None => (TextOffset::ZERO, None),
        };
        self.buffer.move_to(to, extend);
        self.desired_x = desired_x;
    }
}
