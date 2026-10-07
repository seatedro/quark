//! A virtualized chat transcript with document-wide text selection.
//!
//! [`Transcript`] is app-owned state: the row heights and scroll model
//! ([`VariableList`]), the document order of every text block
//! ([`BlockOrder`]), the selection, and the geometry of the rows currently
//! materialized. The message text itself stays in the app's model and is
//! read through [`TranscriptSource`].
//!
//! Each frame the app calls [`Transcript::prepare`] with a
//! [`BlockMeasurer`] (normally [`TextMeasurer`] over the frame's shared
//! `LayoutCache`), which measures only the rows in the overscanned window,
//! then builds [`Transcript::element`] from the result. Pointer and wheel
//! input comes back as [`TranscriptEvent`]s in the element's local
//! coordinates, which the app passes to [`Transcript::handle`].
//!
//! Selection endpoints are `(BlockKey, byte)` pairs, so a selection
//! survives its rows scrolling out of the window, history being prepended,
//! and text streaming into the last message.

mod element;
mod measure;
#[cfg(test)]
mod tests;

pub use element::{TranscriptElement, TranscriptEvent};
pub use measure::{TextGeometry, TextMeasurer};

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use quark::selection::{BlockKey, BlockOrder, Selection, SelectionPoint, SelectionText};
use quark_render::FontWeight;
use quark_render::scene::Rect;

use crate::element::StyledSpan;
use crate::virtual_list::{RowError, RowIntegrityError, RowKey, VariableList};

/// What separates blocks in copied text.
pub const BLOCK_SEPARATOR: &str = "\n\n";

/// Autoscroll speed per pixel of pointer travel past the edge zone, in
/// pixels per millisecond.
const AUTOSCROLL_GAIN: f32 = 0.02;
/// Fastest autoscroll, in pixels per millisecond.
const AUTOSCROLL_MAX: f32 = 4.0;
/// Longest frame gap autoscroll integrates over, so a stalled frame does
/// not jump the view.
const AUTOSCROLL_MAX_DT_MS: u64 = 50;

/// Height of a rule block, in multiples of its font size.
const RULE_HEIGHT: f32 = 1.0;
/// Width of one quote level's bar column, in multiples of the font size.
const QUOTE_STEP: f32 = 1.0;
/// Width of one list level's marker gutter, in multiples of the font size.
const LIST_STEP: f32 = 1.75;

/// The styled content of one block. The concatenation of the span texts
/// (code lines joined with `\n`) is the plain text that selection offsets
/// index and copy reads.
#[derive(Debug, Clone)]
pub enum BlockContent {
    /// Wrapped text, painted by `SelectableText`.
    Prose(Arc<[StyledSpan]>),
    /// Unwrapped monospace lines, painted by `CodeBlock`, with an optional
    /// label (the fence language) above them.
    Code {
        lines: Arc<[Vec<StyledSpan>]>,
        label: Option<Arc<str>>,
    },
    /// A horizontal rule. Its text is `---`, so copy keeps it.
    Rule,
}

/// How a block sits in its message: size, indent, list marker, quote bars,
/// and the markdown prefixes copy restores. Display never draws the
/// prefixes; the marker is painted in a gutter outside the selectable text.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockStyle {
    /// Font size relative to the transcript's.
    pub scale: f32,
    /// Base weight of prose.
    pub weight: FontWeight,
    /// List nesting depth; each level indents one marker gutter.
    pub list_depth: u8,
    /// Quote nesting depth; each level indents one bar column.
    pub quote_depth: u8,
    /// Drawn right-aligned in the innermost list gutter on the first line.
    pub marker: Option<Arc<str>>,
    /// Paints prose in the muted text color (block quotes).
    pub muted: bool,
    /// Half the block gap above, for consecutive list items.
    pub tight: bool,
    /// Copied before the block's first line when the selection covers the
    /// block's start, as `"- "`, `"> 1. "`, or `"## "`.
    pub copy_prefix: Arc<str>,
    /// Copied after every line break inside the block, as `"> "`.
    pub copy_line_prefix: Arc<str>,
}

impl Default for BlockStyle {
    fn default() -> Self {
        Self {
            scale: 1.0,
            weight: FontWeight::Normal,
            list_depth: 0,
            quote_depth: 0,
            marker: None,
            muted: false,
            tight: false,
            copy_prefix: Arc::from(""),
            copy_line_prefix: Arc::from(""),
        }
    }
}

impl BlockStyle {
    /// Left inset of the block's content, in pixels at `font_size`.
    pub fn inset(&self, font_size: f32) -> f32 {
        (self.quote_depth as f32 * QUOTE_STEP + self.list_depth as f32 * LIST_STEP) * font_size
    }
}

/// One selectable text block of a message. Markdown rendering produces a
/// list of these per message.
#[derive(Debug, Clone)]
pub struct TranscriptBlock {
    pub key: BlockKey,
    pub content: BlockContent,
    pub style: BlockStyle,
    text: Arc<str>,
}

impl TranscriptBlock {
    pub fn plain(key: BlockKey, text: impl Into<String>) -> Self {
        Self::prose(key, vec![StyledSpan::plain(text)])
    }

    pub fn prose(key: BlockKey, spans: Vec<StyledSpan>) -> Self {
        let text: String = spans.iter().map(|span| span.text.as_str()).collect();
        Self {
            key,
            content: BlockContent::Prose(spans.into()),
            style: BlockStyle::default(),
            text: text.into(),
        }
    }

    pub fn rule(key: BlockKey) -> Self {
        Self {
            key,
            content: BlockContent::Rule,
            style: BlockStyle::default(),
            text: Arc::from("---"),
        }
    }

    pub fn with_style(mut self, style: BlockStyle) -> Self {
        self.style = style;
        self
    }

    /// Sets the label of a code block; other blocks are unchanged.
    pub fn with_label(mut self, label: Option<Arc<str>>) -> Self {
        if let BlockContent::Code { label: slot, .. } = &mut self.content {
            *slot = label.filter(|l| !l.is_empty());
        }
        self
    }

    /// Each inner `Vec` is one source line.
    pub fn code(key: BlockKey, lines: Vec<Vec<StyledSpan>>) -> Self {
        let mut text = String::new();
        for (i, line) in lines.iter().enumerate() {
            if i > 0 {
                text.push('\n');
            }
            for span in line {
                text.push_str(&span.text);
            }
        }
        Self {
            key,
            content: BlockContent::Code {
                lines: lines.into(),
                label: None,
            },
            style: BlockStyle::default(),
            text: text.into(),
        }
    }

    /// The plain text selection and copy operate on.
    pub fn text(&self) -> &str {
        &self.text
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TranscriptRole {
    User,
    Assistant,
    System,
}

/// One row of the transcript.
#[derive(Debug, Clone)]
pub struct TranscriptMessage {
    pub key: RowKey,
    pub role: TranscriptRole,
    pub author: Arc<str>,
    pub blocks: Vec<TranscriptBlock>,
}

/// The app's message store, read by key.
pub trait TranscriptSource {
    fn message(&self, key: RowKey) -> Option<&TranscriptMessage>;
}

impl TranscriptSource for HashMap<RowKey, TranscriptMessage> {
    fn message(&self, key: RowKey) -> Option<&TranscriptMessage> {
        self.get(&key)
    }
}

/// Layout of a row: `pad_y`, a header line of `header_height` for the
/// author, the blocks separated by `block_gap`, and `pad_y` again. Blocks
/// are inset `pad_x` on both sides.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TranscriptStyle {
    pub font_size: f32,
    pub pad_x: f32,
    pub pad_y: f32,
    pub header_height: f32,
    pub block_gap: f32,
    /// Pixels per wheel line.
    pub line_scroll: f32,
    /// Height of the band at the top and bottom edges where a drag
    /// autoscrolls.
    pub edge: f32,
    /// Extra pixels above and below the viewport that are materialized.
    pub overscan: f32,
}

impl TranscriptStyle {
    /// Proportions for a body font of `font_size` physical pixels.
    pub fn for_font_size(font_size: f32) -> Self {
        Self {
            font_size,
            pad_x: (font_size * 1.2).round(),
            pad_y: (font_size * 0.6).round(),
            header_height: (font_size * 1.6).round(),
            block_gap: (font_size * 0.6).round(),
            line_scroll: (font_size * 3.0).round(),
            edge: (font_size * 2.0).round(),
            overscan: (font_size * 20.0).round(),
        }
    }
}

impl Default for TranscriptStyle {
    fn default() -> Self {
        Self::for_font_size(14.0)
    }
}

/// Measured geometry of one block at one width.
pub trait BlockGeometry {
    fn height(&self) -> f32;
    /// Byte offset nearest to a point relative to the block's top left.
    fn hit(&self, x: f32, y: f32) -> usize;
}

/// Measures blocks; [`TextMeasurer`] does it with the shared text layouts
/// the block elements paint.
pub trait BlockMeasurer {
    type Geometry: BlockGeometry;
    fn measure(&mut self, block: &TranscriptBlock, width: f32) -> Self::Geometry;
}

/// A materialized row, in viewport coordinates.
#[derive(Debug, Clone)]
pub struct VisibleRow {
    pub key: RowKey,
    /// Position among all rows.
    pub index: usize,
    pub role: TranscriptRole,
    pub top: f32,
    pub height: f32,
    /// This row's entries in [`Transcript::visible_blocks`].
    pub blocks: std::ops::Range<usize>,
}

/// A materialized block, in viewport coordinates.
#[derive(Debug, Clone)]
pub struct VisibleBlock<G> {
    pub key: BlockKey,
    pub row: RowKey,
    pub rect: Rect,
    pub text_len: usize,
    pub geometry: G,
}

#[derive(Debug, Clone, Copy)]
struct Drag {
    /// Latest pointer position in viewport coordinates.
    pointer: (f32, f32),
    /// Clock of the last autoscroll step, while autoscrolling.
    last_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TranscriptIntegrityError {
    Rows(RowIntegrityError),
    /// The blocks of the rows, in row order, are not the block order.
    BlockOrder,
    BlockRow {
        block: BlockKey,
    },
    UnknownRow {
        row: RowKey,
    },
    SelectionOutsideDocument,
}

/// Keyboard commands a transcript responds to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptCommand {
    Copy,
    SelectAll,
}

/// Maps a keymap binding (`"cmd+c"`, `"ctrl+a"`) to a command. Cmd and
/// Ctrl both work so one binding table serves every platform.
pub fn key_command(binding: &str) -> Option<TranscriptCommand> {
    let binding = binding.to_ascii_lowercase();
    match binding.as_str() {
        "cmd+c" | "ctrl+c" => Some(TranscriptCommand::Copy),
        "cmd+a" | "ctrl+a" => Some(TranscriptCommand::SelectAll),
        _ => None,
    }
}

/// Transcript state. `G` is the block geometry the measurer produces.
#[derive(Debug, Clone)]
pub struct Transcript<G = TextGeometry> {
    list: VariableList,
    order: BlockOrder,
    block_row: HashMap<BlockKey, RowKey>,
    row_blocks: HashMap<RowKey, Vec<BlockKey>>,
    selection: Option<Selection>,
    drag: Option<Drag>,
    unseen: bool,
    style: TranscriptStyle,
    size: (f32, f32),
    rows: Vec<VisibleRow>,
    blocks: Vec<VisibleBlock<G>>,
}

impl<G: BlockGeometry> Transcript<G> {
    pub fn new(style: TranscriptStyle) -> Self {
        let estimate = style.pad_y * 2.0 + style.header_height + style.font_size * 2.0;
        Self {
            list: VariableList::new(estimate, 0.0),
            order: BlockOrder::new(),
            block_row: HashMap::new(),
            row_blocks: HashMap::new(),
            selection: None,
            drag: None,
            unseen: false,
            style,
            size: (0.0, 0.0),
            rows: Vec::new(),
            blocks: Vec::new(),
        }
    }

    pub fn style(&self) -> &TranscriptStyle {
        &self.style
    }

    pub fn len(&self) -> usize {
        self.list.rows().len()
    }

    pub fn is_empty(&self) -> bool {
        self.list.rows().is_empty()
    }

    pub fn list(&self) -> &VariableList {
        &self.list
    }

    // -- Document changes --

    /// Appends a message at the end.
    pub fn push(&mut self, message: &TranscriptMessage) -> Result<(), RowError> {
        self.list.append(message.key)?;
        let blocks = message
            .blocks
            .iter()
            .map(|block| block.key)
            .filter(|key| self.order.append(*key))
            .collect();
        self.adopt(message.key, blocks);
        self.mark_new_content();
        self.debug_check();
        Ok(())
    }

    /// Appends many messages, checking integrity once. Into an empty
    /// transcript this is one batch insert.
    pub fn extend<'a>(
        &mut self,
        messages: impl IntoIterator<Item = &'a TranscriptMessage>,
    ) -> Result<(), RowError> {
        let messages: Vec<&TranscriptMessage> = messages.into_iter().collect();
        if self.is_empty() {
            let keys: Vec<RowKey> = messages.iter().map(|m| m.key).collect();
            self.list.prepend(&keys)?;
        } else {
            for message in &messages {
                self.list.append(message.key)?;
            }
        }
        for message in messages {
            let blocks = message
                .blocks
                .iter()
                .map(|block| block.key)
                .filter(|key| self.order.append(*key))
                .collect();
            self.adopt(message.key, blocks);
        }
        self.mark_new_content();
        self.debug_check();
        Ok(())
    }

    /// Inserts older messages before the first one. The rows on screen and
    /// the selection stay where they are.
    pub fn prepend<'a>(
        &mut self,
        messages: impl IntoIterator<Item = &'a TranscriptMessage>,
    ) -> Result<(), RowError> {
        let messages: Vec<&TranscriptMessage> = messages.into_iter().collect();
        let keys: Vec<RowKey> = messages.iter().map(|m| m.key).collect();
        self.list.prepend(&keys)?;
        let mut fresh = Vec::new();
        let mut seen = HashSet::new();
        for message in &messages {
            let blocks: Vec<BlockKey> = message
                .blocks
                .iter()
                .map(|block| block.key)
                .filter(|key| !self.order.contains(*key) && seen.insert(*key))
                .collect();
            fresh.extend_from_slice(&blocks);
            self.adopt(message.key, blocks);
        }
        self.order.prepend(fresh);
        self.debug_check();
        Ok(())
    }

    /// The message's blocks or text changed, as while streaming. New
    /// blocks join the document order, removed ones leave it (shrinking the
    /// selection inward), and the row is remeasured on the next prepare.
    pub fn update(&mut self, message: &TranscriptMessage) -> Result<(), RowError> {
        let key = message.key;
        let old = self
            .row_blocks
            .remove(&key)
            .ok_or(RowError::UnknownKey(key))?;
        let wanted: HashSet<BlockKey> = message.blocks.iter().map(|b| b.key).collect();
        for block in old.iter().filter(|b| !wanted.contains(b)) {
            self.forget_block(*block);
        }
        let mut kept: Vec<BlockKey> = Vec::with_capacity(message.blocks.len());
        for block in message.blocks.iter().map(|b| b.key) {
            if self.block_row.get(&block) == Some(&key) {
                kept.push(block);
                continue;
            }
            if self.order.contains(block) {
                // Owned by another row; a block belongs to one row.
                continue;
            }
            let inserted = match kept.last().copied().or_else(|| self.last_block_before(key)) {
                Some(after) => self.order.insert_after(after, block),
                None => self.order.prepend([block]) == 1,
            };
            if inserted {
                kept.push(block);
            }
        }
        self.adopt(key, kept);
        self.list.invalidate(key)?;
        self.mark_new_content();
        self.debug_check();
        Ok(())
    }

    pub fn remove(&mut self, key: RowKey) -> Result<(), RowError> {
        self.list.remove(key)?;
        for block in self.row_blocks.remove(&key).unwrap_or_default() {
            self.forget_block(block);
        }
        self.debug_check();
        Ok(())
    }

    fn adopt(&mut self, row: RowKey, blocks: Vec<BlockKey>) {
        for block in &blocks {
            self.block_row.insert(*block, row);
        }
        self.row_blocks.insert(row, blocks);
    }

    fn forget_block(&mut self, block: BlockKey) {
        self.block_row.remove(&block);
        let Some(pos) = self.order.remove(block) else {
            return;
        };
        // No text source here: an endpoint that moves to the end of the
        // previous block gets `usize::MAX`, which selection reads as the
        // end of that block.
        self.selection = self
            .selection
            .and_then(|s| s.after_remove(block, pos, &self.order, &NoText));
    }

    /// Last block of the nearest earlier row that has blocks.
    fn last_block_before(&self, row: RowKey) -> Option<BlockKey> {
        let index = self.list.rows().index_of(row)?;
        self.list.rows().keys()[..index]
            .iter()
            .rev()
            .find_map(|key| self.row_blocks.get(key)?.last().copied())
    }

    fn mark_new_content(&mut self) {
        if !self.list.is_stuck_to_bottom() {
            self.unseen = true;
        }
    }

    // -- Scrolling --

    pub fn scroll_offset(&self) -> f32 {
        self.list.scroll_offset()
    }

    pub fn max_scroll_offset(&self) -> f32 {
        self.list.max_scroll_offset()
    }

    /// A user scroll; landing at the bottom pins the view there.
    pub fn set_scroll_offset(&mut self, offset: f32) -> f32 {
        let offset = self.list.set_scroll_offset(offset);
        if self.list.is_stuck_to_bottom() {
            self.unseen = false;
        }
        offset
    }

    pub fn scroll_by(&mut self, delta: f32) -> f32 {
        self.set_scroll_offset(self.scroll_offset() + delta)
    }

    pub fn is_stuck_to_bottom(&self) -> bool {
        self.list.is_stuck_to_bottom()
    }

    /// New content arrived while the view was scrolled up; show a "jump to
    /// latest" affordance.
    pub fn has_unseen(&self) -> bool {
        self.unseen
    }

    /// Scrolls to the newest content and pins there.
    pub fn jump_to_latest(&mut self) {
        self.set_scroll_offset(f32::MAX);
    }

    // -- Selection --

    pub fn selection(&self) -> Option<Selection> {
        self.selection
    }

    pub fn set_selection(&mut self, selection: Option<Selection>) {
        self.selection = selection
            .filter(|s| self.order.contains(s.anchor.block) && self.order.contains(s.focus.block));
    }

    /// Selects every block from the start of the first to the end of the
    /// last, including text that streams into the last one later.
    pub fn select_all(&mut self) {
        let keys = self.order.keys();
        if let (Some(first), Some(last)) = (keys.first(), keys.last()) {
            self.selection = Some(Selection::new(
                SelectionPoint::new(*first, 0),
                SelectionPoint::new(*last, usize::MAX),
            ));
        }
    }

    /// The selected text from the app's model, blocks joined with blank
    /// lines. Empty when nothing is selected. A block whose start is
    /// selected gets its [`BlockStyle::copy_prefix`], and every line break
    /// inside a block its `copy_line_prefix`, so list markers, heading
    /// hashes, and quote marks survive the copy.
    pub fn selected_text(&self, source: &impl TranscriptSource) -> String {
        let mut out = String::new();
        let Some((start, end)) = self.selection.and_then(|s| s.ordered(&self.order)) else {
            return out;
        };
        if start == end {
            return out;
        }
        let texts = SourceText {
            source,
            block_row: &self.block_row,
        };
        let keys = self.order.range(start.block, end.block);
        let last = keys.len().saturating_sub(1);
        for (i, key) in keys.iter().enumerate() {
            let Some(block) = texts.block(*key) else {
                continue;
            };
            let text = block.text();
            let from = if i == 0 {
                text.floor_char_boundary(start.byte)
            } else {
                0
            };
            let to = if i == last {
                text.floor_char_boundary(end.byte)
            } else {
                text.len()
            };
            if i > 0 {
                out.push_str(BLOCK_SEPARATOR);
            }
            if from >= to {
                continue;
            }
            if from == 0 {
                out.push_str(&block.style.copy_prefix);
            }
            let line_prefix = &block.style.copy_line_prefix;
            for (n, line) in text[from..to].split('\n').enumerate() {
                if n > 0 {
                    out.push('\n');
                    out.push_str(line_prefix);
                }
                out.push_str(line);
            }
        }
        out
    }

    /// Selected byte range within `block`, clamped to `len`, or `None`
    /// when the block is outside the selection.
    pub fn block_selection(&self, block: BlockKey, len: usize) -> Option<(usize, usize)> {
        let (start, end) = self.selection?.ordered(&self.order)?;
        let pos = self.order.position(block)?;
        let start_pos = self.order.position(start.block)?;
        let end_pos = self.order.position(end.block)?;
        if pos < start_pos || pos > end_pos {
            return None;
        }
        let lo = if block == start.block { start.byte } else { 0 };
        let hi = if block == end.block { end.byte } else { len };
        let (lo, hi) = (lo.min(len), hi.min(len));
        (lo < hi).then_some((lo, hi))
    }

    // -- Input --

    /// Applies one input event from the element. Coordinates are relative
    /// to the transcript's top left.
    pub fn handle(&mut self, event: TranscriptEvent) {
        match event {
            TranscriptEvent::PointerDown { x, y } => {
                self.drag = Some(Drag {
                    pointer: (x, y),
                    last_ms: None,
                });
                self.selection = self.point_at(x, y).map(Selection::collapsed);
            }
            TranscriptEvent::PointerDrag { x, y } => {
                if let Some(drag) = &mut self.drag {
                    drag.pointer = (x, y);
                }
                self.extend_selection_to(x, y);
            }
            TranscriptEvent::PointerUp => self.drag = None,
            TranscriptEvent::Wheel(lines) => {
                self.scroll_by(lines as f32 * self.style.line_scroll);
            }
            TranscriptEvent::JumpToLatest => self.jump_to_latest(),
        }
    }

    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// A drag is autoscrolling; draw another frame.
    pub fn wants_frame(&self) -> bool {
        self.drag
            .is_some_and(|drag| self.autoscroll_velocity(drag.pointer.1) != 0.0)
    }

    fn extend_selection_to(&mut self, x: f32, y: f32) {
        let Some(point) = self.point_at(x, y) else {
            return;
        };
        self.selection = Some(match self.selection {
            Some(selection) => Selection::new(selection.anchor, point),
            None => Selection::collapsed(point),
        });
    }

    /// Signed autoscroll speed in pixels per millisecond for a pointer at
    /// `y`: zero inside the viewport away from the edges, growing with the
    /// distance into or past an edge band.
    fn autoscroll_velocity(&self, y: f32) -> f32 {
        let (edge, height) = (self.style.edge, self.size.1);
        let past = if y < edge {
            y - edge
        } else if y > height - edge {
            y - (height - edge)
        } else {
            0.0
        };
        (past * AUTOSCROLL_GAIN).clamp(-AUTOSCROLL_MAX, AUTOSCROLL_MAX)
    }

    /// Selection point under a viewport point: the nearest visible block
    /// vertically, the start of it when above, the end when below, else a
    /// hit test of its layout. Points outside the viewport clamp to it.
    pub fn point_at(&self, x: f32, y: f32) -> Option<SelectionPoint> {
        let y = y.clamp(0.0, (self.size.1 - 1.0).max(0.0));
        let block = self.blocks.iter().min_by(|a, b| {
            vertical_distance(&a.rect, y).total_cmp(&vertical_distance(&b.rect, y))
        })?;
        let rect = block.rect;
        let byte = if y < rect.y {
            0
        } else if y >= rect.y + rect.height {
            block.text_len
        } else {
            block.geometry.hit(x - rect.x, y - rect.y)
        };
        Some(SelectionPoint::new(block.key, byte))
    }

    // -- Frame --

    /// Advances autoscroll to `now_ms`, measures the rows entering the
    /// window at `width`, and rebuilds the materialized rows. A width
    /// change remeasures every row as it becomes visible.
    pub fn prepare<M: BlockMeasurer<Geometry = G>>(
        &mut self,
        width: f32,
        height: f32,
        now_ms: u64,
        source: &impl TranscriptSource,
        measurer: &mut M,
    ) {
        if height != self.size.1 {
            self.list.set_viewport_height(height);
        }
        self.size = (width, height);
        self.autoscroll(now_ms);

        let style = self.style;
        self.list
            .measure_visible(width, style.overscan, |key, width| {
                let block_width = block_width(&style, width);
                let blocks = source
                    .message(RowKey(key))
                    .map_or(&[][..], |m| m.blocks.as_slice());
                let mut height = style.pad_y * 2.0 + style.header_height;
                for (i, block) in blocks.iter().enumerate() {
                    height += gap_before(&style, i, block);
                    height += measurer.measure(block, block_width).height();
                }
                height
            });
        if self.list.is_stuck_to_bottom() {
            self.unseen = false;
        }
        self.materialize(source, measurer);

        // Rows moved under a held pointer (autoscroll, streaming); keep the
        // selection end under it.
        if let Some(drag) = self.drag {
            self.extend_selection_to(drag.pointer.0, drag.pointer.1);
        }
    }

    fn autoscroll(&mut self, now_ms: u64) {
        let Some(mut drag) = self.drag else {
            return;
        };
        let velocity = self.autoscroll_velocity(drag.pointer.1);
        if velocity == 0.0 {
            drag.last_ms = None;
        } else {
            if let Some(last) = drag.last_ms {
                let dt = now_ms.saturating_sub(last).min(AUTOSCROLL_MAX_DT_MS);
                self.scroll_by(velocity * dt as f32);
            }
            drag.last_ms = Some(now_ms);
        }
        self.drag = Some(drag);
    }

    fn materialize<M: BlockMeasurer<Geometry = G>>(
        &mut self,
        source: &impl TranscriptSource,
        measurer: &mut M,
    ) {
        let style = self.style;
        let block_width = block_width(&style, self.size.0);
        let window = self.list.window(style.overscan);
        let scroll = self.list.scroll_offset();
        let rows = self.list.rows();
        self.rows.clear();
        self.blocks.clear();
        for index in window.range {
            let key = rows.keys()[index];
            let top = rows.offset_of_index(index) - scroll;
            let height = rows.height_of(key).unwrap_or(0.0);
            let message = source.message(key);
            let first = self.blocks.len();
            let mut y = top + style.pad_y + style.header_height;
            for (i, block) in message.iter().flat_map(|m| m.blocks.iter()).enumerate() {
                y += gap_before(&style, i, block);
                let geometry = measurer.measure(block, block_width);
                let block_height = geometry.height();
                self.blocks.push(VisibleBlock {
                    key: block.key,
                    row: key,
                    rect: Rect {
                        x: style.pad_x,
                        y,
                        width: block_width,
                        height: block_height,
                    },
                    text_len: block.text().len(),
                    geometry,
                });
                y += block_height;
            }
            self.rows.push(VisibleRow {
                key,
                index,
                role: message.map_or(TranscriptRole::System, |m| m.role),
                top,
                height,
                blocks: first..self.blocks.len(),
            });
        }
    }

    /// Rows materialized by the last prepare, top to bottom.
    pub fn visible_rows(&self) -> &[VisibleRow] {
        &self.rows
    }

    /// Blocks of the materialized rows, in document order.
    pub fn visible_blocks(&self) -> &[VisibleBlock<G>] {
        &self.blocks
    }

    pub fn viewport_size(&self) -> (f32, f32) {
        self.size
    }

    // -- Integrity --

    pub fn verify_integrity(&self) -> Result<(), TranscriptIntegrityError> {
        self.list
            .rows()
            .verify_integrity()
            .map_err(TranscriptIntegrityError::Rows)?;
        let rows = self.list.rows().keys();
        if self.row_blocks.len() != rows.len() {
            return Err(TranscriptIntegrityError::BlockOrder);
        }
        let mut expected = Vec::with_capacity(self.order.len());
        for row in rows {
            let blocks = self
                .row_blocks
                .get(row)
                .ok_or(TranscriptIntegrityError::UnknownRow { row: *row })?;
            for block in blocks {
                if self.block_row.get(block) != Some(row) {
                    return Err(TranscriptIntegrityError::BlockRow { block: *block });
                }
            }
            expected.extend_from_slice(blocks);
        }
        if expected != self.order.keys() || self.block_row.len() != expected.len() {
            return Err(TranscriptIntegrityError::BlockOrder);
        }
        if let Some(selection) = self.selection
            && selection.ordered(&self.order).is_none()
        {
            return Err(TranscriptIntegrityError::SelectionOutsideDocument);
        }
        Ok(())
    }

    fn debug_check(&self) {
        debug_assert_eq!(self.verify_integrity(), Ok(()));
    }
}

/// Space above the `index`-th block of a message.
fn gap_before(style: &TranscriptStyle, index: usize, block: &TranscriptBlock) -> f32 {
    match index {
        0 => 0.0,
        _ if block.style.tight => (style.block_gap * 0.5).round(),
        _ => style.block_gap,
    }
}

fn block_width(style: &TranscriptStyle, width: f32) -> f32 {
    (width - style.pad_x * 2.0).max(1.0)
}

fn vertical_distance(rect: &Rect, y: f32) -> f32 {
    if y < rect.y {
        rect.y - y
    } else if y >= rect.y + rect.height {
        y - (rect.y + rect.height)
    } else {
        0.0
    }
}

/// Block text read from the app's model by way of the owning row.
struct SourceText<'a, S> {
    source: &'a S,
    block_row: &'a HashMap<BlockKey, RowKey>,
}

impl<'a, S: TranscriptSource> SourceText<'a, S> {
    fn block(&self, key: BlockKey) -> Option<&'a TranscriptBlock> {
        let row = self.block_row.get(&key)?;
        self.source
            .message(*row)?
            .blocks
            .iter()
            .find(|block| block.key == key)
    }
}

struct NoText;

impl SelectionText for NoText {
    fn text(&self, _key: BlockKey) -> Option<&str> {
        None
    }
}
