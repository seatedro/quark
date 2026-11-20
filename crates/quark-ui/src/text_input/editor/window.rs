//! [`EditorWindow`]: the lines an editor shaped around its view, shared
//! with the element that paints them and the hit area pointers map
//! through.

use std::ops::Range;
use std::sync::Arc;

use quark_text::{TextLayout, TextOffset};

use super::spans::SyntaxTokenKind;
use crate::text_input::rope::Rope;

/// One logical line of an editor's text as shaped for its view.
#[derive(Debug, Clone)]
pub struct WindowLine {
    /// 0-based logical line.
    pub line: usize,
    /// Where the line starts in the editor's text.
    pub start: usize,
    /// Top of the line in content coordinates (before scrolling).
    pub top: f32,
    /// The line without its line ending. While an IME composes, the lines
    /// it composes in are one layout of their text with the preedit.
    pub layout: Arc<TextLayout>,
    /// The token kind of each of `layout`'s spans, for paint colors.
    pub kinds: Arc<[SyntaxTokenKind]>,
}

/// The lines an editor shaped around its view at its last flush. Cloning
/// shares them.
#[derive(Debug, Clone, Default)]
pub struct EditorWindow {
    lines: Arc<[WindowLine]>,
    /// The text they were shaped from, for snapping hits onto it.
    text: Rope,
    range: Range<usize>,
    composing: bool,
}

impl EditorWindow {
    pub(super) fn new(
        lines: Vec<WindowLine>,
        text: Rope,
        range: Range<usize>,
        composing: bool,
    ) -> Self {
        Self {
            lines: lines.into(),
            text,
            range,
            composing,
        }
    }

    /// The shaped lines, top to bottom.
    pub fn lines(&self) -> &[WindowLine] {
        &self.lines
    }

    /// The logical lines covered.
    pub fn line_range(&self) -> Range<usize> {
        self.range.clone()
    }

    /// Whether an IME composition replaces some of the lines.
    pub fn is_composing(&self) -> bool {
        self.composing
    }

    /// Whether the lines reach from `top` down to `bottom` (content
    /// coordinates), or to the end of the text.
    pub fn covers(&self, top: f32, bottom: f32) -> bool {
        let (Some(first), Some(last)) = (self.lines.first(), self.lines.last()) else {
            return false;
        };
        let last_bottom = last.top + last.layout.size().1;
        let reaches_end = last.line + 1 >= self.text.line_count();
        (first.top <= top || first.line == 0) && (last_bottom >= bottom || reaches_end)
    }

    /// The grapheme boundary nearest to the content point `(x, y)`. A
    /// point above or below the shaped lines maps into the first or last
    /// of them. `None` with no lines, or while composing.
    pub fn hit(&self, x: f32, y: f32) -> Option<TextOffset> {
        if self.composing {
            return None;
        }
        let below = self.lines.partition_point(|line| line.top <= y);
        let line = self.lines.get(below.saturating_sub(1))?;
        let local = line.layout.hit(x, y - line.top);
        Some(TextOffset::snap_in(&self.text, line.start + local.get()))
    }
}
