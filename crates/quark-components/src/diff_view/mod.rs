//! A virtualized diff view over a [`quark_diff::DiffDocument`]: unified or
//! side by side, with line numbers, word-level change highlights, syntax
//! colors, collapsible unchanged regions, file headers, selection and copy,
//! and keyboard navigation between hunks and files.
//!
//! The app owns a [`DiffViewState`]. Each frame it calls
//! [`DiffViewState::prepare`] with the frame's text system, which shapes
//! the rows entering the window (and nothing on a frame that repeats the
//! last one), then builds [`diff_view`]. Input comes back as
//! [`DiffEvent`]s for [`DiffViewState::handle`].
//!
//! Rows live in a [`VariableList`] (a Fenwick row table), so a 100,000-line
//! diff builds only the rows on screen. Text is laid out by `quark-text`:
//! word wrap when [`DiffStyle::wrap`] is on, and hit-testing through the
//! shaped glyphs, so wide characters, emoji, and right-to-left text select
//! where they are drawn. Each text column scrolls sideways on its own
//! [`ScrollHandle`] when wrap is off.
//!
//! Selection endpoints are [`SelectionPoint`]s keyed by side, file, and
//! line, so a selection spans hunks and survives scrolling and expanding.
//! Copy takes the side under the selection when side by side; unified
//! copies [`DiffStyle::copy_side`] (new text by default).
//!
//! Syntax colors come from a `quark-syntax` worker thread
//! ([`DiffViewState::enable_syntax`]); rows repaint as files finish.

mod view;

use std::cell::Cell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use quark::selection::{BlockKey, Selection, SelectionPoint};
use quark_diff::{
    BlockKind, DiffDocument, Expansion, FileStatus, GapId, Mode, Projection, Reveal, RowKind, Side,
    inline_diff,
};
use quark_render::FontKind;
use quark_render::scene::Rect;
use quark_syntax::{
    GrammarStore, HighlightKind, HighlightSpan, HighlightWorker, Highlighted, LanguageId,
};
use quark_text::{LayoutCache, TextLayout, TextParams, TextSpan, TextStyle, TextSystem};
use quark_ui::FocusId;
use quark_ui::element::{ScrollHandle, ScrollbarVisibility, WHEEL_LINE_PX};
use quark_ui::virtual_list::{RowKey, VariableList};

pub use view::diff_view;

/// Lines one click on an expand control reveals.
pub const REVEAL_STEP: u32 = 20;

/// Autoscroll speed per point the pointer is past the edge band, in
/// points per millisecond.
const AUTOSCROLL_GAIN: f32 = 0.02;
/// Fastest autoscroll, in points per millisecond.
const AUTOSCROLL_MAX: f32 = 4.0;
/// Longest frame gap autoscroll integrates over, so a stalled frame does
/// not jump the view.
const AUTOSCROLL_MAX_DT_MS: u64 = 50;
/// Frame interval the view asks for while a drag autoscrolls.
pub(crate) const AUTOSCROLL_FRAME_MS: u64 = 16;

/// Which text a unified selection copies.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum CopySide {
    /// Context and added lines: the new file's text.
    #[default]
    New,
    /// Context and removed lines: the old file's text.
    Old,
    /// Every selected line as shown.
    Shown,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DiffStyle {
    /// Monospace text size in points.
    pub font_size: f32,
    /// Line height as a multiple of the font size.
    pub line_height: f32,
    /// Wrap long lines at word boundaries instead of scrolling sideways.
    pub wrap: bool,
    pub copy_side: CopySide,
}

impl Default for DiffStyle {
    fn default() -> Self {
        Self {
            font_size: 13.0,
            line_height: 1.5,
            wrap: false,
            copy_side: CopySide::New,
        }
    }
}

/// Input from [`diff_view`], in window coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DiffEvent {
    Press { x: f32, y: f32 },
    Drag { x: f32, y: f32 },
    Release,
    Scroll(i32),
    ScrollTo(f32),
    Key(DiffKey),
    Expand(GapId, Reveal),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DiffKey {
    NextHunk,
    PrevHunk,
    NextFile,
    PrevFile,
    LineUp,
    LineDown,
    PageUp,
    PageDown,
    Home,
    End,
    Copy,
    SelectAll,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffOutcome {
    Unchanged,
    Changed,
    /// Copy was pressed: put this on the clipboard.
    Copy(String),
}

/// One side of a materialized line row.
#[derive(Debug)]
pub(crate) struct LinePaint {
    pub layout: Arc<TextLayout>,
    /// Highlight kind of each layout span.
    pub tones: Arc<[HighlightKind]>,
    /// Changed words, as byte ranges of the line.
    pub words: Vec<Range<usize>>,
}

/// A materialized row: its lines or its header text. Kept while the row
/// stays in the window and its inputs (`stamp`) stay the same.
#[derive(Debug)]
pub(crate) struct RowPaint {
    pub stamp: u64,
    pub kind: RowKind,
    pub file: u32,
    pub sides: [Option<LinePaint>; 2],
    /// One-based line number on each side, or zero.
    pub numbers: [u32; 2],
    /// File, hunk, or gap header text.
    pub title: Arc<str>,
    pub gap: Option<GapId>,
    pub status: FileStatus,
    pub stats: (u32, u32),
    pub binary: bool,
}

impl RowPaint {
    fn height(&self, m: &Metrics) -> f32 {
        let lines = self.sides.iter().flatten();
        lines
            .map(|l| l.layout.size().1.ceil())
            .fold(row_height(self.kind, m), f32::max)
    }
}

/// Sizes derived from the style and the font, in logical points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Metrics {
    pub font_size: f32,
    pub line_h: f32,
    pub char_w: f32,
    /// Width of one line number column.
    pub number_w: f32,
    /// Width of the `+`/`-` column.
    pub sign_w: f32,
    /// Space left of the text inside its column.
    pub text_pad: f32,
}

/// Where the gutters and text columns sit, from the left of the view.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ColumnBox {
    pub gutter_x: f32,
    pub gutter_w: f32,
    pub text_x: f32,
    pub text_w: f32,
}

/// The columns of one layout: side by side has one per side, unified one
/// shared column stored under [`Side::New`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Columns {
    pub mode: Mode,
    pub sides: [Option<ColumnBox>; 2],
}

impl Columns {
    fn new(mode: Mode, width: f32, m: &Metrics) -> Self {
        match mode {
            Mode::Unified => {
                let gutter_w = m.number_w * 2.0 + m.sign_w;
                let text = ColumnBox {
                    gutter_x: 0.0,
                    gutter_w,
                    text_x: gutter_w,
                    text_w: (width - gutter_w).max(1.0),
                };
                Self {
                    mode,
                    sides: [None, Some(text)],
                }
            }
            Mode::Split => {
                let gutter_w = m.number_w + m.sign_w;
                let half = (width / 2.0).floor();
                let side = |x: f32, w: f32| ColumnBox {
                    gutter_x: x,
                    gutter_w,
                    text_x: x + gutter_w,
                    text_w: (w - gutter_w).max(1.0),
                };
                Self {
                    mode,
                    sides: [Some(side(0.0, half - 1.0)), Some(side(half, width - half))],
                }
            }
        }
    }

    /// The column showing `side` of a row.
    pub fn of(&self, side: Side) -> ColumnBox {
        match self.mode {
            Mode::Unified => self.sides[1].expect("unified column"),
            Mode::Split => self.sides[side as usize].expect("split column"),
        }
    }

    /// Wrap width of `side`'s text.
    fn wrap_width(&self, side: Side, m: &Metrics) -> f32 {
        (self.of(side).text_w - m.text_pad * 2.0).max(m.char_w)
    }
}

/// A row in the frame the view paints.
#[derive(Debug, Clone)]
pub(crate) struct FrameRow {
    pub key: u64,
    /// Position among all rows, for accessibility.
    pub index: u32,
    pub top: f32,
    pub height: f32,
    pub paint: Rc<RowPaint>,
    /// Selected byte range of each side's line.
    pub selected: [Option<(usize, usize)>; 2],
}

/// Everything [`diff_view`] reads, built by [`DiffViewState::prepare`]
/// and shared with the cached build closures.
#[derive(Debug)]
pub(crate) struct ViewFrame {
    pub id: &'static str,
    pub label: &'static str,
    pub focus: FocusId,
    pub viewport: (f32, f32),
    pub metrics: Metrics,
    pub columns: Columns,
    pub wrap: bool,
    pub rows: Vec<FrameRow>,
    /// Width of each side's widest line seen, padding included.
    pub content_w: [f32; 2],
    pub scroll: f32,
    pub total: f32,
    pub row_count: u32,
    pub hscroll: [ScrollHandle; 2],
    pub scrollbar_auto_hide: bool,
    pub scrollbar: ScrollbarVisibility,
}

/// What the last prepare materialized from.
#[derive(Debug, Clone, Copy, PartialEq)]
struct PrepareKey {
    window: (usize, usize),
    scroll: u32,
    revision: u64,
    scale: u32,
}

/// A selection drag in progress.
#[derive(Debug, Clone, Copy)]
struct Drag {
    /// The pointer, view-local.
    pointer: (f32, f32),
    /// Clock of the last autoscroll step, while past an edge.
    last_ms: Option<u64>,
}

/// App-owned diff view state. See the [module docs](self).
pub struct DiffViewState {
    id: &'static str,
    label: &'static str,
    focus: FocusId,
    doc: Rc<DiffDocument>,
    /// Bumped with every new document, so stamps never match an old one.
    doc_generation: u64,
    expansion: Expansion,
    projection: Projection,
    list: VariableList,
    style: DiffStyle,
    viewport: (f32, f32),
    hscroll: [ScrollHandle; 2],
    /// The vertical scrollbar shows only on demand, as `scrollbar`
    /// decides.
    scrollbar_auto_hide: bool,
    scrollbar: ScrollbarVisibility,
    selection: Option<Selection>,
    /// The side a side-by-side selection copies.
    selection_side: Side,
    drag: Option<Drag>,
    /// The view's bounds in the window as of the last frame.
    bounds: Rc<Cell<Rect>>,
    metrics: Option<(Metrics, u32)>,
    painted: HashMap<u64, Rc<RowPaint>>,
    /// Projection row of each row key, for measuring rows by key.
    row_index: HashMap<u64, u32>,
    frame: Option<Rc<ViewFrame>>,
    prepared: Option<PrepareKey>,
    content_w: [f32; 2],
    syntax: Option<HighlightWorker>,
    highlights: Vec<[Option<Arc<[HighlightSpan]>>; 2]>,
    /// Revision of each held highlight, which a newer result of the same
    /// document must exceed (grammars arriving recolor it).
    highlight_rev: Vec<[Option<u32>; 2]>,
    highlight_gen: Vec<u32>,
    revision: u64,
    /// Bumped with every frame `prepare` builds.
    frame_id: u64,
}

impl DiffViewState {
    /// `id` names cache entries and accessibility ids (unique in the
    /// window); `focus` is the view's keyboard focus target.
    pub fn new(id: &'static str, focus: FocusId, doc: DiffDocument) -> Self {
        let doc = Rc::new(doc);
        let expansion = Expansion::new(&doc);
        let projection = Projection::new(&doc, Mode::Unified, &expansion);
        let mut state = Self {
            id,
            label: "Diff",
            focus,
            doc,
            doc_generation: 0,
            expansion,
            projection,
            list: VariableList::new(1.0, 0.0),
            style: DiffStyle::default(),
            viewport: (0.0, 0.0),
            hscroll: [ScrollHandle::new(), ScrollHandle::new()],
            scrollbar_auto_hide: false,
            scrollbar: ScrollbarVisibility::new(),
            selection: None,
            selection_side: Side::New,
            drag: None,
            bounds: Rc::default(),
            metrics: None,
            painted: HashMap::new(),
            row_index: HashMap::new(),
            frame: None,
            prepared: None,
            content_w: [0.0; 2],
            syntax: None,
            highlights: Vec::new(),
            highlight_rev: Vec::new(),
            highlight_gen: Vec::new(),
            revision: 0,
            frame_id: 0,
        };
        state.reset_highlights();
        state.rebuild_rows(None);
        state
    }

    pub fn with_label(mut self, label: &'static str) -> Self {
        self.label = label;
        self
    }

    pub fn with_style(mut self, style: DiffStyle) -> Self {
        self.set_style(style);
        self
    }

    /// Show the vertical scrollbar only while the pointer is over the view,
    /// its thumb is held, or briefly after the view scrolls or gains focus
    /// (see [`ScrollbarVisibility`]). Without it that scrollbar always
    /// shows.
    /// The sideways scrollbars of unwrapped lines always auto-hide.
    pub fn with_scrollbar_auto_hide(mut self) -> Self {
        self.scrollbar_auto_hide = true;
        self
    }

    // ---- Queries -------------------------------------------------------

    pub fn document(&self) -> &DiffDocument {
        &self.doc
    }

    pub fn projection(&self) -> &Projection {
        &self.projection
    }

    pub fn mode(&self) -> Mode {
        self.projection.mode
    }

    pub fn style(&self) -> DiffStyle {
        self.style
    }

    pub fn scroll_offset(&self) -> f32 {
        self.list.scroll_offset()
    }

    pub fn selection(&self) -> Option<Selection> {
        self.selection
    }

    /// Rows the last [`Self::prepare`] materialized: the window plus
    /// overscan.
    pub fn window(&self) -> Range<u32> {
        let range = self.list.window(self.overscan()).range;
        range.start as u32..range.end as u32
    }

    /// The row at the top of the viewport.
    pub fn top_row(&self) -> Option<u32> {
        self.list
            .rows()
            .row_at(self.list.scroll_offset())
            .map(|i| i as u32)
    }

    pub fn horizontal_scroll(&self, side: Side) -> &ScrollHandle {
        &self.hscroll[side as usize]
    }

    // ---- Changes -------------------------------------------------------

    /// Shows another document; expansion, selection, and scroll reset.
    pub fn set_document(&mut self, doc: DiffDocument) {
        self.doc = Rc::new(doc);
        self.doc_generation += 1;
        self.expansion = Expansion::new(&self.doc);
        self.selection = None;
        self.painted.clear();
        self.content_w = [0.0; 2];
        self.reset_highlights();
        let mode = self.projection.mode;
        self.projection.rebuild(&self.doc, mode, &self.expansion);
        self.rebuild_rows(None);
        if self.syntax.is_some() {
            self.request_highlights();
        }
    }

    pub fn set_mode(&mut self, mode: Mode) {
        if mode == self.projection.mode {
            return;
        }
        let anchor = self.anchor();
        self.projection.rebuild(&self.doc, mode, &self.expansion);
        self.rebuild_rows(anchor);
    }

    pub fn set_style(&mut self, style: DiffStyle) {
        if style == self.style {
            return;
        }
        let anchor = self.anchor();
        self.style = style;
        self.metrics = None;
        self.painted.clear();
        self.content_w = [0.0; 2];
        self.rebuild_rows(anchor);
    }

    /// The view's size in points.
    pub fn set_viewport(&mut self, width: f32, height: f32) {
        if self.viewport != (width, height) {
            if self.style.wrap && self.viewport.0 != width {
                self.painted.clear();
            }
            self.viewport = (width.max(0.0), height.max(0.0));
            self.list.set_viewport_height(self.viewport.1);
            self.revision += 1;
        }
    }

    /// Highlights every file on a background thread, by file extension,
    /// with `store`'s grammars. Files whose language has no grammar (or
    /// whose grammar is still downloading) stay plain until it arrives.
    pub fn enable_syntax(&mut self, store: GrammarStore) {
        self.syntax = Some(HighlightWorker::new(store));
        self.request_highlights();
    }

    pub fn set_selection(&mut self, selection: Option<Selection>, side: Side) {
        self.selection = selection;
        self.selection_side = side;
        self.revision += 1;
    }

    /// Reveals lines of a collapsed gap; see [`Expansion::reveal`].
    pub fn reveal(&mut self, gap: GapId, reveal: Reveal, amount: u32) -> bool {
        if !self.expansion.reveal(&self.doc, gap, reveal, amount) {
            return false;
        }
        let anchor = self.anchor();
        let mode = self.projection.mode;
        self.projection.rebuild(&self.doc, mode, &self.expansion);
        self.rebuild_rows(anchor);
        true
    }

    /// Scrolls so `row` is at the top.
    pub fn scroll_to_row(&mut self, row: u32) {
        let top = self.list.rows().offset_of_index(row as usize);
        self.set_scroll(top);
    }

    fn set_scroll(&mut self, offset: f32) -> bool {
        let before = self.list.scroll_offset();
        if self.list.set_scroll_offset(offset) == before {
            return false;
        }
        self.revision += 1;
        true
    }

    // ---- Input ---------------------------------------------------------

    /// Applies an event from [`diff_view`].
    pub fn handle(&mut self, event: DiffEvent) -> DiffOutcome {
        let changed = match event {
            DiffEvent::Press { x, y } => {
                let (x, y) = self.local(x, y);
                self.selection_side = self.side_at(x);
                self.drag = Some(Drag {
                    pointer: (x, y),
                    last_ms: None,
                });
                self.selection = self.point_at(x, y).map(Selection::collapsed);
                self.revision += 1;
                true
            }
            DiffEvent::Drag { x, y } => {
                let (x, y) = self.local(x, y);
                if let Some(drag) = &mut self.drag {
                    drag.pointer = (x, y);
                }
                match (self.selection, self.point_at(x, y)) {
                    (Some(s), Some(point)) if s.focus != point => {
                        self.selection = Some(Selection::new(s.anchor, point));
                        self.revision += 1;
                        true
                    }
                    _ => false,
                }
            }
            DiffEvent::Release => {
                self.drag = None;
                false
            }
            DiffEvent::Scroll(lines) => {
                self.set_scroll(self.list.scroll_offset() + lines as f32 * WHEEL_LINE_PX)
            }
            DiffEvent::ScrollTo(offset) => self.set_scroll(offset),
            DiffEvent::Expand(gap, reveal) => self.reveal(gap, reveal, REVEAL_STEP),
            DiffEvent::Key(key) => return self.key(key),
        };
        if changed {
            DiffOutcome::Changed
        } else {
            DiffOutcome::Unchanged
        }
    }

    fn key(&mut self, key: DiffKey) -> DiffOutcome {
        let line = self.metrics().line_h;
        let page = (self.viewport.1 - line * 2.0).max(line);
        let offset = self.list.scroll_offset();
        let moved = match key {
            DiffKey::NextHunk => self.jump(true, true),
            DiffKey::PrevHunk => self.jump(false, true),
            DiffKey::NextFile => self.jump(true, false),
            DiffKey::PrevFile => self.jump(false, false),
            DiffKey::LineUp => self.set_scroll(offset - line),
            DiffKey::LineDown => self.set_scroll(offset + line),
            DiffKey::PageUp => self.set_scroll(offset - page),
            DiffKey::PageDown => self.set_scroll(offset + page),
            DiffKey::Home => self.set_scroll(0.0),
            DiffKey::End => self.set_scroll(f32::MAX),
            DiffKey::Copy => return DiffOutcome::Copy(self.selected_text()),
            DiffKey::SelectAll => {
                self.select_all();
                true
            }
        };
        if moved {
            DiffOutcome::Changed
        } else {
            DiffOutcome::Unchanged
        }
    }

    /// Scrolls to the next or previous hunk or file start: the header or
    /// gap row above a hunk's first line when there is one.
    fn jump(&mut self, forward: bool, hunks: bool) -> bool {
        let p = &self.projection;
        let starts = if hunks { &p.hunk_rows } else { &p.file_rows };
        let rows = self.list.rows();
        let target_of = |&start: &u32| {
            let above = start.checked_sub(1).filter(|&r| {
                hunks && matches!(p.kind[r as usize], RowKind::HunkHeader | RowKind::Gap)
            });
            above.unwrap_or(start)
        };
        let current = self.list.scroll_offset();
        // Half a point of slack: offsets are rounded to Fenwick units.
        let target = if forward {
            starts
                .iter()
                .map(target_of)
                .find(|&r| rows.offset_of_index(r as usize) > current + 0.5)
        } else {
            starts
                .iter()
                .rev()
                .map(target_of)
                .find(|&r| rows.offset_of_index(r as usize) < current - 0.5)
        };
        match target {
            Some(row) => {
                let top = rows.offset_of_index(row as usize);
                self.set_scroll(top)
            }
            None => false,
        }
    }

    /// Selects every line: the new side unless a side-by-side selection
    /// already chose one.
    pub fn select_all(&mut self) {
        let side = match self.projection.mode {
            Mode::Split => self.selection_side,
            Mode::Unified => Side::New,
        };
        let p = &self.projection;
        let point = |row: u32, byte: usize| {
            let (side, index) = self.shown_line(row, side)?;
            let key = block_key(side, p.file[row as usize], index);
            Some(SelectionPoint::new(key, byte))
        };
        let first = (0..p.len()).find_map(|r| point(r, 0));
        let last = (0..p.len()).rev().find_map(|r| point(r, usize::MAX));
        if let (Some(first), Some(last)) = (first, last) {
            self.selection = Some(Selection::new(first, last));
            self.selection_side = side;
            self.revision += 1;
        }
    }

    // ---- Selection -----------------------------------------------------

    /// The selected text: lines joined by `\n`, from the side the
    /// selection copies. Empty without a selection.
    pub fn selected_text(&self) -> String {
        let mut out = String::new();
        let Some(((start_row, start_byte), (end_row, end_byte))) = self.ordered_selection() else {
            return out;
        };
        let side = self.copy_side();
        let mut first = true;
        for row in start_row..=end_row {
            let Some((side, index)) = self.copied_line(row, side) else {
                continue;
            };
            let file = self.projection.file[row as usize];
            let text = self.doc.text(file, side).display_line(index).unwrap_or("");
            let from = if row == start_row {
                floor_boundary(text, start_byte)
            } else {
                0
            };
            let to = if row == end_row {
                floor_boundary(text, end_byte)
            } else {
                text.len()
            };
            if !first {
                out.push('\n');
            }
            first = false;
            out.push_str(&text[from..to.max(from)]);
        }
        out
    }

    /// Which side copy reads: the selection's side by side, else the
    /// style's choice (`None` for lines as shown).
    fn copy_side(&self) -> Option<Side> {
        match (self.projection.mode, self.style.copy_side) {
            (Mode::Split, _) => Some(self.selection_side),
            (Mode::Unified, CopySide::New) => Some(Side::New),
            (Mode::Unified, CopySide::Old) => Some(Side::Old),
            (Mode::Unified, CopySide::Shown) => None,
        }
    }

    /// The line of `row` that copy takes, or `None` when the row has none
    /// on that side.
    fn copied_line(&self, row: u32, side: Option<Side>) -> Option<(Side, u32)> {
        let kind = self.projection.kind[row as usize];
        let side = side.unwrap_or(if kind == RowKind::Removed {
            Side::Old
        } else {
            Side::New
        });
        self.projection.line(row, side).map(|i| (side, i))
    }

    /// The line a point on `row` keys to: `side`'s line when side by
    /// side, else the line the row shows.
    fn shown_line(&self, row: u32, side: Side) -> Option<(Side, u32)> {
        let p = &self.projection;
        match p.mode {
            Mode::Split => p.line(row, side).map(|i| (side, i)),
            Mode::Unified => {
                let side = if p.kind[row as usize] == RowKind::Removed {
                    Side::Old
                } else {
                    Side::New
                };
                p.line(row, side).map(|i| (side, i))
            }
        }
    }

    /// Row and byte of each endpoint, start first. `None` without a
    /// selection or when an endpoint's line is not shown.
    fn ordered_selection(&self) -> Option<((u32, usize), (u32, usize))> {
        let s = self.selection?;
        let at = |point: SelectionPoint| {
            let (side, file, index) = decode_key(point.block);
            Some((self.projection.row_of(file, side, index)?, point.byte))
        };
        let (a, b) = (at(s.anchor)?, at(s.focus)?);
        Some(if a <= b { (a, b) } else { (b, a) })
    }

    /// Selected byte range of `row`'s line on `side`, as painted.
    fn row_selection(
        &self,
        ordered: Option<((u32, usize), (u32, usize))>,
        row: u32,
        side: Side,
        len: usize,
    ) -> Option<(usize, usize)> {
        let ((start_row, start_byte), (end_row, end_byte)) = ordered?;
        if row < start_row || row > end_row {
            return None;
        }
        // Only lines copy takes are highlighted, and side by side only on
        // the selection's side.
        self.copied_line(row, self.copy_side())?;
        if self.projection.mode == Mode::Split && side != self.selection_side {
            return None;
        }
        let lo = if row == start_row {
            start_byte.min(len)
        } else {
            0
        };
        let hi = if row == end_row {
            end_byte.min(len)
        } else {
            len
        };
        (lo < hi).then_some((lo, hi))
    }

    fn local(&self, x: f32, y: f32) -> (f32, f32) {
        let b = self.bounds.get();
        (x - b.x, y - b.y)
    }

    /// The side a press at view-local `x` selects.
    fn side_at(&self, x: f32) -> Side {
        let m = self.metrics();
        let columns = Columns::new(self.projection.mode, self.viewport.0, &m);
        match self.projection.mode {
            Mode::Unified => Side::New,
            Mode::Split if x < columns.of(Side::New).gutter_x => Side::Old,
            Mode::Split => Side::New,
        }
    }

    /// The selection point under view-local `(x, y)`, on the selection's
    /// side when side by side. Points above or below the window clamp to
    /// its first or last line; rows without a line on the side (headers,
    /// gaps, padding) give the start of the next line, or the end of the
    /// previous one at the bottom.
    pub fn point_at(&self, x: f32, y: f32) -> Option<SelectionPoint> {
        let frame = self.frame.as_ref()?;
        let side = self.selection_side;
        let rows = &frame.rows;
        let hit = rows
            .iter()
            .position(|r| y >= r.top && y < r.top + r.height)
            .unwrap_or(if y < rows.first()?.top {
                0
            } else {
                rows.len() - 1
            });
        let line_of = |r: &FrameRow| self.shown_line(r.index, side);
        let row = &rows[hit];
        if let Some((line_side, index)) = line_of(row) {
            let paint = row.paint.sides[line_side as usize].as_ref()?;
            let column = frame.columns.of(line_side);
            let scroll = if frame.wrap {
                0.0
            } else {
                self.hscroll[column_slot(frame.columns.mode, line_side)]
                    .offset()
                    .0
            };
            let tx = x - column.text_x - frame.metrics.text_pad + scroll;
            let byte = if x < column.text_x {
                0
            } else {
                paint.layout.hit(tx, y - row.top).get()
            };
            return Some(SelectionPoint::new(
                block_key(line_side, row.paint.file, index),
                byte,
            ));
        }
        if let Some(next) = rows[hit..].iter().find_map(|r| Some((r, line_of(r)?))) {
            let (r, (s, i)) = next;
            return Some(SelectionPoint::new(block_key(s, r.paint.file, i), 0));
        }
        let (r, (s, i)) = rows[..hit]
            .iter()
            .rev()
            .find_map(|r| Some((r, line_of(r)?)))?;
        Some(SelectionPoint::new(
            block_key(s, r.paint.file, i),
            usize::MAX,
        ))
    }

    // ---- Autoscroll ----------------------------------------------------

    /// A drag is held past the top or bottom edge: the view scrolls each
    /// frame, so it needs another one.
    pub fn wants_frame(&self) -> bool {
        self.drag
            .is_some_and(|drag| self.autoscroll_velocity(drag.pointer.1) != 0.0)
    }

    /// Signed autoscroll speed in points per millisecond for a pointer at
    /// view-local `y`: zero away from the edges, growing with the distance
    /// into or past a one-line band at the top or bottom.
    fn autoscroll_velocity(&self, y: f32) -> f32 {
        let (edge, height) = (self.metrics().line_h, self.viewport.1);
        let past = if y < edge {
            y - edge
        } else if y > height - edge {
            y - (height - edge)
        } else {
            0.0
        };
        (past * AUTOSCROLL_GAIN).clamp(-AUTOSCROLL_MAX, AUTOSCROLL_MAX)
    }

    /// Scroll a held drag past an edge by the time since its last step.
    /// Returns whether the view moved.
    fn autoscroll(&mut self, now_ms: u64) -> bool {
        let Some(mut drag) = self.drag else {
            return false;
        };
        let velocity = self.autoscroll_velocity(drag.pointer.1);
        let mut moved = false;
        if velocity == 0.0 {
            drag.last_ms = None;
        } else {
            if let Some(last) = drag.last_ms {
                let dt = now_ms.saturating_sub(last).min(AUTOSCROLL_MAX_DT_MS);
                let offset = self.list.scroll_offset() + velocity * dt as f32;
                moved = self.set_scroll(offset);
            }
            drag.last_ms = Some(now_ms);
        }
        self.drag = Some(drag);
        moved
    }

    /// Move the selection's focus to whatever now sits under the held
    /// pointer. Returns whether it changed.
    fn follow_pointer(&mut self) -> bool {
        let (Some(drag), Some(selection)) = (self.drag, self.selection) else {
            return false;
        };
        match self.point_at(drag.pointer.0, drag.pointer.1) {
            Some(point) if point != selection.focus => {
                self.selection = Some(Selection::new(selection.anchor, point));
                self.revision += 1;
                true
            }
            _ => false,
        }
    }

    // ---- Frame ---------------------------------------------------------

    fn overscan(&self) -> f32 {
        self.metrics().line_h * 8.0
    }

    pub(crate) fn metrics(&self) -> Metrics {
        self.metrics
            .map_or_else(|| self.estimated_metrics(), |(m, _)| m)
    }

    /// Metrics before the font has been measured.
    fn estimated_metrics(&self) -> Metrics {
        let font_size = self.style.font_size;
        metrics_for(
            font_size,
            self.style.line_height,
            font_size * 0.6,
            &self.doc,
        )
    }

    /// Advances a drag's autoscroll to `now_ms`, shapes the rows entering
    /// the window, and builds the frame the view paints. Does nothing (and
    /// allocates nothing) when the window, the state, and the scale are
    /// what the last call saw. `scale` must be the frame's scale factor.
    pub fn prepare(
        &mut self,
        text: &mut TextSystem,
        layouts: &mut LayoutCache,
        scale: f32,
        now_ms: u64,
    ) {
        self.poll_highlights();
        let scrolled = self.autoscroll(now_ms);
        self.build_frame(text, layouts, scale);
        // Rows moved under the held pointer: its selection end follows.
        if scrolled && self.follow_pointer() {
            self.build_frame(text, layouts, scale);
        }
    }

    fn build_frame(&mut self, text: &mut TextSystem, layouts: &mut LayoutCache, scale: f32) {
        if self.metrics.is_none_or(|(_, s)| s != scale.to_bits()) {
            self.measure_font(text, layouts, scale);
        }
        let m = self.metrics();
        let columns = Columns::new(self.projection.mode, self.viewport.0, &m);
        if self.style.wrap {
            self.measure_window(text, layouts, scale, &columns);
        }
        let window = self.list.window(self.overscan()).range;
        let key = PrepareKey {
            window: (window.start, window.end),
            scroll: self.list.scroll_offset().to_bits(),
            revision: self.revision,
            scale: scale.to_bits(),
        };
        if self.prepared == Some(key) && self.frame.is_some() {
            return;
        }
        self.prepared = Some(key);
        self.frame_id += 1;

        let scroll = self.list.scroll_offset();
        let ordered = self.ordered_selection();
        let mut kept = HashMap::with_capacity(window.len());
        let mut rows = Vec::with_capacity(window.len());
        for index in window {
            let row = index as u32;
            let key = self.projection.row_key(row);
            let stamp = self.stamp(row, &columns, scale);
            let paint = match self.painted.remove(&key) {
                Some(p) if p.stamp == stamp => p,
                _ => Rc::new(self.build_row(row, stamp, text, layouts, scale, &columns)),
            };
            for (side, line) in [Side::Old, Side::New].into_iter().zip(&paint.sides) {
                let Some(line) = line else {
                    continue;
                };
                let slot = column_slot(self.projection.mode, side);
                self.content_w[slot] = self.content_w[slot]
                    .max((line.layout.size().0 + m.text_pad * 2.0 + m.char_w).ceil());
            }
            let rows_table = self.list.rows();
            let selected = [Side::Old, Side::New].map(|side| {
                let len = paint.sides[side as usize]
                    .as_ref()
                    .map_or(0, |l| l.layout.text().len());
                paint.sides[side as usize].as_ref()?;
                self.row_selection(ordered, row, side, len)
            });
            rows.push(FrameRow {
                key,
                index: row,
                top: rows_table.offset_of_index(index) - scroll,
                height: rows_table.height_of(RowKey(key)).unwrap_or(m.line_h),
                paint: paint.clone(),
                selected,
            });
            kept.insert(key, paint);
        }
        self.painted = kept;
        self.frame = Some(Rc::new(ViewFrame {
            id: self.id,
            label: self.label,
            focus: self.focus,
            viewport: self.viewport,
            metrics: m,
            columns,
            wrap: self.style.wrap,
            rows,
            content_w: self.content_w,
            scroll,
            total: self.list.rows().total_extent(),
            row_count: self.projection.len(),
            hscroll: self.hscroll.clone(),
            scrollbar_auto_hide: self.scrollbar_auto_hide,
            scrollbar: self.scrollbar.clone(),
        }));
    }

    /// Heights of rows entering the window, from their wrapped layouts.
    fn measure_window(
        &mut self,
        text: &mut TextSystem,
        layouts: &mut LayoutCache,
        scale: f32,
        columns: &Columns,
    ) {
        let m = self.metrics();
        let overscan = self.overscan();
        let mut list = std::mem::replace(&mut self.list, VariableList::new(1.0, 0.0));
        let mut painted = std::mem::take(&mut self.painted);
        let this = &*self;
        list.measure_visible(this.viewport.0, overscan, |key, _| {
            let Some(&row) = this.row_index.get(&key) else {
                return m.line_h;
            };
            let stamp = this.stamp(row, columns, scale);
            match painted.get(&key) {
                Some(p) if p.stamp == stamp => p.height(&m),
                _ => {
                    let p = this.build_row(row, stamp, text, layouts, scale, columns);
                    let height = p.height(&m);
                    painted.insert(key, Rc::new(p));
                    height
                }
            }
        });
        self.list = list;
        self.painted = painted;
    }

    /// Identifies everything a row's paint depends on besides its key.
    fn stamp(&self, row: u32, columns: &Columns, scale: f32) -> u64 {
        let mut h = std::hash::DefaultHasher::new();
        let file = self.projection.file[row as usize];
        (
            self.doc_generation,
            self.highlight_gen.get(file as usize),
            scale.to_bits(),
            self.style.font_size.to_bits(),
            self.style.line_height.to_bits(),
            self.projection.mode,
        )
            .hash(&mut h);
        if self.style.wrap {
            let m = self.metrics();
            for side in [Side::Old, Side::New] {
                columns.wrap_width(side, &m).to_bits().hash(&mut h);
            }
        }
        if self.projection.kind[row as usize] == RowKind::Gap {
            // Its count changes as lines are revealed.
            self.projection.gap(row).map(|g| g.hidden).hash(&mut h);
        }
        h.finish()
    }

    fn measure_font(&mut self, text: &mut TextSystem, layouts: &mut LayoutCache, scale: f32) {
        let style = self.text_style();
        let params = TextParams::new("0000000000", style).scale_factor(scale);
        let char_w = layouts
            .layout(text, &params)
            .map_or(self.style.font_size * 0.6, |l| l.size().0 / 10.0);
        let m = metrics_for(
            self.style.font_size,
            self.style.line_height,
            char_w,
            &self.doc,
        );
        let changed = self.metrics.is_none_or(|(old, _)| old != m);
        self.metrics = Some((m, scale.to_bits()));
        if changed {
            let anchor = self.anchor();
            self.rebuild_rows(anchor);
        }
    }

    fn text_style(&self) -> TextStyle {
        TextStyle::new(self.style.font_size)
            .kind(FontKind::Mono)
            .line_height((self.style.font_size * self.style.line_height).round())
    }

    fn build_row(
        &self,
        row: u32,
        stamp: u64,
        text: &mut TextSystem,
        layouts: &mut LayoutCache,
        scale: f32,
        columns: &Columns,
    ) -> RowPaint {
        let p = &self.projection;
        let r = row as usize;
        let (kind, file) = (p.kind[r], p.file[r]);
        let doc = &self.doc;
        let meta = &doc.files().meta[file as usize];
        let mut paint = RowPaint {
            stamp,
            kind,
            file,
            sides: [None, None],
            numbers: [0, 0],
            title: Arc::from(""),
            gap: None,
            status: meta.status,
            stats: (
                doc.files().additions[file as usize],
                doc.files().deletions[file as usize],
            ),
            binary: meta.binary,
        };
        match kind {
            RowKind::FileHeader => {
                paint.title = match (&meta.old_path, &meta.new_path) {
                    (Some(old), Some(new)) if old != new => format!("{old} \u{2192} {new}").into(),
                    _ => doc.path(file).into(),
                };
            }
            RowKind::HunkHeader => paint.title = hunk_title(doc, p.hunk[r]).into(),
            RowKind::Gap => {
                let gap = p.gap(row).expect("gap row");
                paint.gap = Some(gap.id);
                let lines = if gap.hidden == 1 { "line" } else { "lines" };
                let mut title = format!("{} unchanged {lines}", gap.hidden);
                if let Some(h) = gap.id.hunk {
                    title.push_str("    ");
                    title.push_str(&hunk_title(doc, h));
                }
                paint.title = title.into();
            }
            _ => {
                let m = self.metrics();
                let words = self.word_ranges(row);
                for side in [Side::Old, Side::New] {
                    let Some(index) = p.line(row, side) else {
                        continue;
                    };
                    paint.numbers[side as usize] = line_number(doc, p, row, side, index);
                    if p.mode == Mode::Unified && kind == RowKind::Context && side == Side::Old {
                        // Unified context shows the new side's text.
                        continue;
                    }
                    let store = doc.text(file, side);
                    let line = store.display_line(index).unwrap_or("");
                    let range = store.display_range(index).unwrap_or(0..0);
                    let (spans, tones) = self.syntax_spans(file, side, range);
                    let params = TextParams::new(line, self.text_style())
                        .spans(spans)
                        .scale_factor(scale)
                        .wrap_width(self.style.wrap.then(|| columns.wrap_width(side, &m)));
                    let Ok(layout) = layouts.layout(text, &params) else {
                        continue;
                    };
                    paint.sides[side as usize] = Some(LinePaint {
                        layout,
                        tones,
                        words: words[side as usize].clone(),
                    });
                }
            }
        }
        paint
    }

    /// Changed words of `row`'s lines, by side.
    fn word_ranges(&self, row: u32) -> [Vec<Range<usize>>; 2] {
        let p = &self.projection;
        let r = row as usize;
        let (old, new) = match p.kind[r] {
            RowKind::Modified => (p.old[r], p.new[r]),
            RowKind::Removed => (p.old[r], p.pair[r]),
            RowKind::Added => (p.pair[r], p.new[r]),
            _ => return Default::default(),
        };
        if old == quark_diff::NONE || new == quark_diff::NONE {
            return Default::default();
        }
        let file = p.file[r];
        let line = |side, i| self.doc.text(file, side).display_line(i).unwrap_or("");
        let d = inline_diff(line(Side::Old, old), line(Side::New, new));
        let bytes = |v: Vec<Range<u32>>| -> Vec<Range<usize>> {
            v.into_iter()
                .map(|r| r.start as usize..r.end as usize)
                .collect()
        };
        [bytes(d.old), bytes(d.new)]
    }

    /// Layout spans splitting `range` of a store at highlight boundaries,
    /// with each span's kind.
    fn syntax_spans(
        &self,
        file: u32,
        side: Side,
        range: Range<usize>,
    ) -> (Vec<TextSpan>, Arc<[HighlightKind]>) {
        let Some(spans) = self
            .highlights
            .get(file as usize)
            .and_then(|h| h[side as usize].as_ref())
        else {
            return (Vec::new(), Arc::from([]));
        };
        let mut out = Vec::new();
        let mut tones = Vec::new();
        let base = range.start;
        let mut push = |from: usize, to: usize, kind: HighlightKind| {
            if from < to {
                out.push(TextSpan {
                    range: from - base..to - base,
                    weight: None,
                    style: None,
                    kind: None,
                });
                tones.push(kind);
            }
        };
        let mut at = range.start;
        let first = spans.partition_point(|s| s.range().end <= range.start);
        for span in &spans[first..] {
            let r = span.range();
            if r.start >= range.end {
                break;
            }
            let (from, to) = (r.start.max(range.start), r.end.min(range.end));
            push(at, from, HighlightKind::Normal);
            push(from, to, span.kind);
            at = to;
        }
        if at > range.start {
            push(at, range.end, HighlightKind::Normal);
        }
        (out, tones.into())
    }

    // ---- Rows ----------------------------------------------------------

    /// The first visible row and its offset into the viewport, to restore
    /// after the rows change.
    fn anchor(&self) -> Option<(u64, f32)> {
        let index = self.list.rows().row_at(self.list.scroll_offset())?;
        let rows = self.list.rows();
        Some((
            rows.keys()[index].0,
            self.list.scroll_offset() - rows.offset_of_index(index),
        ))
    }

    /// Rebuilds the row table from the projection, keeping `anchor` where
    /// it was when the row still exists.
    fn rebuild_rows(&mut self, anchor: Option<(u64, f32)>) {
        let m = self.metrics();
        let keys: Vec<RowKey> = (0..self.projection.len())
            .map(|r| RowKey(self.projection.row_key(r)))
            .collect();
        self.row_index = keys.iter().zip(0..).map(|(k, r)| (k.0, r)).collect();
        let mut list = VariableList::new(m.line_h, self.viewport.1);
        // A fresh list is pinned to the bottom; a diff opens at the top.
        list.set_scroll_offset(0.0);
        let _ = list.extend(&keys);
        for (row, key) in keys.iter().enumerate() {
            let kind = self.projection.kind[row];
            if !kind.is_line() {
                let _ = list.set_height(*key, row_height(kind, &m));
            }
        }
        let offset = anchor
            .and_then(|(key, delta)| Some(list.rows().offset_of(RowKey(key))? + delta))
            .unwrap_or_else(|| self.list.scroll_offset());
        list.set_scroll_offset(offset);
        self.list = list;
        self.revision += 1;
    }

    // ---- Syntax --------------------------------------------------------

    fn reset_highlights(&mut self) {
        let files = self.doc.file_count() as usize;
        self.highlights = vec![[None, None]; files];
        self.highlight_rev = vec![[None, None]; files];
        self.highlight_gen = vec![0; files];
    }

    fn request_highlights(&self) {
        let Some(worker) = &self.syntax else {
            return;
        };
        for file in 0..self.doc.file_count() {
            let Some(language) = LanguageId::from_path(self.doc.path(file)) else {
                continue;
            };
            for side in [Side::Old, Side::New] {
                let source = self.doc.text(file, side).shared().clone();
                if !source.is_empty() {
                    let slot = u64::from(file) * 2 + side as u64;
                    worker.request(slot, self.doc_generation, language.clone(), source);
                }
            }
        }
    }

    fn poll_highlights(&mut self) {
        let Some(worker) = &self.syntax else {
            return;
        };
        let mut results = Vec::new();
        while let Ok(Some(done)) = worker.try_recv() {
            results.push(done);
        }
        for done in results {
            self.take_highlight(done);
        }
    }

    /// Keeps a result for the current document unless the file side holds
    /// the same or a later revision; results for earlier documents are
    /// dropped.
    fn take_highlight(&mut self, done: Highlighted) {
        let (file, side) = ((done.slot / 2) as usize, (done.slot % 2) as usize);
        if done.generation != self.doc_generation || file >= self.highlights.len() {
            return;
        }
        if self.highlight_rev[file][side].is_some_and(|held| held >= done.revision) {
            return;
        }
        self.highlight_rev[file][side] = Some(done.revision);
        self.highlights[file][side] = Some(done.spans.into());
        self.highlight_gen[file] += 1;
        self.revision += 1;
    }
}

/// Sizes for a font of `font_size` points whose digits are `char_w` wide.
fn metrics_for(font_size: f32, line_height: f32, char_w: f32, doc: &DiffDocument) -> Metrics {
    let lines = (0..doc.file_count())
        .flat_map(|f| [doc.text(f, Side::Old), doc.text(f, Side::New)])
        .map(|t| t.line_count())
        .max()
        .unwrap_or(0)
        .max(1);
    // Partial stores hold fewer lines than the files; leave room for a
    // few more digits than they need.
    let digits = (lines.ilog10() + 1).max(3) as f32;
    Metrics {
        font_size,
        line_h: (font_size * line_height).round(),
        char_w,
        number_w: (digits * char_w + char_w * 2.0).ceil(),
        sign_w: (char_w * 2.0).ceil(),
        text_pad: (char_w * 0.5).ceil(),
    }
}

/// Height of a row of `kind` before its text is measured.
fn row_height(kind: RowKind, m: &Metrics) -> f32 {
    match kind {
        RowKind::FileHeader => (m.line_h * 2.0).round(),
        RowKind::HunkHeader | RowKind::Gap => (m.line_h * 1.4).round(),
        _ => m.line_h,
    }
}

/// The scroll handle a side's text column uses: unified has one column.
pub(crate) fn column_slot(mode: Mode, side: Side) -> usize {
    match mode {
        Mode::Unified => Side::New as usize,
        Mode::Split => side as usize,
    }
}

/// `@@ -a,b +c,d @@ section` of a hunk.
fn hunk_title(doc: &DiffDocument, hunk: u32) -> String {
    let (h, i) = (doc.hunks(), hunk as usize);
    let section = &h.section[i];
    format!(
        "@@ -{},{} +{},{} @@{}{section}",
        h.old_start[i],
        h.old_len[i],
        h.new_start[i],
        h.new_len[i],
        if section.is_empty() { "" } else { " " }
    )
}

/// One-based line number of store line `index` shown on `row`.
fn line_number(doc: &DiffDocument, p: &Projection, row: u32, side: Side, index: u32) -> u32 {
    let file = p.file[row as usize];
    if !doc.files().partial[file as usize] {
        return index + 1;
    }
    let hunk = p.hunk[row as usize];
    let (h, b) = (doc.hunks(), doc.blocks());
    for block in h.blocks[hunk as usize].clone() {
        let bi = block as usize;
        let (store, len, line) = match side {
            Side::Old => (b.old_store[bi], b.old_len[bi], b.old_line[bi]),
            Side::New => (b.new_store[bi], b.new_len[bi], b.new_line[bi]),
        };
        if (store..store + len).contains(&index) {
            debug_assert!(b.kind[bi] == BlockKind::Context || len > 0);
            return line + index - store;
        }
    }
    index + 1
}

const SIDE_BIT: u64 = 1 << 63;

/// The selection key of a line: side, file, and store index.
fn block_key(side: Side, file: u32, index: u32) -> BlockKey {
    let side = if side == Side::New { SIDE_BIT } else { 0 };
    BlockKey(side | (u64::from(file) << 32) | u64::from(index))
}

fn decode_key(key: BlockKey) -> (Side, u32, u32) {
    let side = if key.0 & SIDE_BIT != 0 {
        Side::New
    } else {
        Side::Old
    };
    (side, ((key.0 & !SIDE_BIT) >> 32) as u32, key.0 as u32)
}

/// Largest char boundary at or below `byte`, clamped to the text.
fn floor_boundary(text: &str, byte: usize) -> usize {
    let mut at = byte.min(text.len());
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATCH: &str = "diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1 +1 @@
-fn a() {}
+fn b() {}
";

    // Catches progressive highlights being dropped or stale ones winning:
    // later revisions for the shown document recolor it, while earlier
    // revisions and results for a replaced document do not.
    #[test]
    fn highlights_replace_only_when_newer_for_the_shown_document() {
        let doc = || quark_diff::parse_unified(PATCH).unwrap();
        let mut state = DiffViewState::new("test.diff", FocusId::new(1), doc());
        state.set_document(doc());
        let slot = Side::New as u64;
        let mut held = Vec::new();
        // (document generation, revision, highlighted length)
        for (generation, revision, length) in
            [(1, 0, 2), (1, 1, 5), (1, 0, 1), (0, 2, 9), (1, 1, 7)]
        {
            state.take_highlight(Highlighted {
                slot,
                generation,
                revision,
                source: Arc::from("fn b() {}\n"),
                spans: vec![HighlightSpan {
                    offset: 0,
                    length,
                    kind: HighlightKind::Keyword,
                }],
                pending: false,
                unresolved: Vec::new(),
            });
            let (spans, _) = state.syntax_spans(0, Side::New, 0..9);
            held.push(spans.first().map(|span| span.range.clone()));
        }

        assert_eq!(
            held,
            [Some(0..2), Some(0..5), Some(0..5), Some(0..5), Some(0..5)]
        );
    }
}
