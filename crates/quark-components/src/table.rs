//! A virtualized data table with a sticky header.
//!
//! The app owns its rows behind [`TableData`] (row count, cell text, sort
//! comparison, and an optional cell renderer) in an `Rc`, and a
//! [`TableState`] holding the column table (parallel columns of title,
//! width, minimum width, and sortability, plus the display order) and the
//! row view: the sorted permutation of data rows and its inverse,
//! selection, cursor, and scroll offsets.
//!
//! [`table_view`] builds only the rows and columns inside the viewport.
//! Each row is its own cache boundary inside one boundary for the whole
//! view, so an unchanged frame replays without building or allocating.
//!
//! The body scrolls both ways under shared scrollbars: wheel (Shift+wheel
//! or sideways trackpad motion for horizontal), thumb drags, and track
//! presses. Keyboard navigation scrolls the cursor column into view, and
//! apps can call [`TableState::scroll_x_by`].
//!
//! Like [`crate::tree_view`], the view emits [`TableEvent`]s through a
//! `fn(TableEvent) -> Action`; the app passes them to
//! [`TableState::handle`] with the modifier keys held.

use std::borrow::Cow;
use std::cmp::Ordering;
use std::fmt;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use accesskit::Role;
use quark::view;
use quark_ui::accessibility::SortDirection;
use quark_ui::element::{
    AnyElement, CacheKey, ClickEvent, CursorHint, DragHandler, DragReleaseResult, IntoAnyElement,
    ScrollActionBuilder, ScrollbarVisibility, WHEEL_LINE_PX, cached, div, inputs_hash, svg_icon,
    text,
};
use quark_ui::icons::lucide;
use quark_ui::style::Styled;
use quark_ui::theme::{Color, Theme};
use quark_ui::virtual_list::virtual_list_window;
use quark_ui::{Action, FocusId};

use crate::tree::{CollectionEnv, SelectMods};

const OVERSCAN: usize = 2;
const DRAG_THRESHOLD_PX: f32 = 4.0;
/// Width of the grab area at a header cell's right edge.
const RESIZE_HANDLE_PX: f32 = 6.0;
const CELL_PAD_PX: f32 = 8.0;
const NONE: u32 = u32::MAX;

/// Rows of a table, addressed by data row and column (the index
/// [`TableState::add_column`] returned).
pub trait TableData: 'static {
    fn row_count(&self) -> usize;

    /// The text of a cell: what the default renderer draws, what sorting
    /// compares by default, and the cell's accessible name.
    fn cell_text(&self, row: usize, column: usize) -> Cow<'_, str>;

    /// Order of rows `a` and `b` by `column`, ascending.
    fn compare(&self, a: usize, b: usize, column: usize) -> Ordering {
        self.cell_text(a, column).cmp(&self.cell_text(b, column))
    }

    /// The element drawn in a cell. Defaults to the cell text.
    fn render_cell(&self, row: usize, column: usize, style: &CellStyle) -> AnyElement {
        view! {
            <text class="text-sm" color={style.text} class="truncate">
                {self.cell_text(row, column).into_owned()}
            </text>
        }
    }

    /// Changes whenever any cell does, so cached rows rebuild and the
    /// sort reruns. Constant data can keep the default.
    fn revision(&self) -> u64 {
        0
    }
}

/// What a cell renderer may paint with.
#[derive(Debug, Clone, Copy)]
pub struct CellStyle {
    pub text: Color,
    pub muted: Color,
    pub accent: Color,
    pub selected: bool,
}

/// Input from [`table_view`], to pass to [`TableState::handle`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TableEvent {
    /// A data row was clicked.
    PressRow(usize),
    /// The pointer moved `dx` points since pressing on `column`'s header.
    HeaderDrag {
        column: u32,
        dx: f32,
    },
    /// The header press on `column` ended: a sort when it never moved, a
    /// column move otherwise.
    HeaderRelease {
        column: u32,
    },
    /// The resize handle of `column` was dragged to `width`.
    Resize {
        column: u32,
        width: f32,
    },
    Key(TableKey),
    Scroll(i32),
    ScrollTo(f32),
    /// Horizontal wheel lines; positive scrolls right.
    ScrollX(i32),
    ScrollXTo(f32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TableKey {
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    ExtendUp,
    ExtendDown,
    ToggleSelect,
    SelectAll,
    Activate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableOutcome {
    Unchanged,
    Changed,
    /// Enter on the cursor row (a data row).
    Activated(usize),
}

/// A header press in progress.
#[derive(Debug, Clone, Copy)]
struct HeaderPress {
    column: u32,
    /// Display position the column would move to, once dragged.
    target: Option<usize>,
}

#[derive(Clone)]
struct TableInner {
    id: &'static str,
    label: &'static str,
    focus: FocusId,
    row_height: f32,
    header_height: f32,
    viewport: (f32, f32),

    // Column table, by column.
    titles: Vec<Arc<str>>,
    widths: Vec<f32>,
    min_widths: Vec<f32>,
    sortable: Vec<bool>,
    /// Display position to column.
    order: Vec<u32>,

    // Row view.
    /// Display row to data row.
    view: Vec<u32>,
    /// Data row to display row.
    pos_of: Vec<u32>,
    data_revision: u64,
    sort: Option<(u32, SortDirection)>,
    selected_flags: Vec<bool>,
    selected: Vec<u32>,
    /// Cursor and anchor as data rows, so sorting keeps them.
    cursor: u32,
    anchor: u32,
    /// Cursor column as a display position.
    cursor_col: u32,
    scroll_y: f32,
    scroll_x: f32,
    header: Option<HeaderPress>,
    revision: u64,
    /// Bumped when column widths, order, or the horizontal offset change:
    /// every row's cache input.
    layout_revision: u64,
    /// The body's scrollbars show only on demand, as `scrollbar` decides.
    scrollbar_auto_hide: bool,
    scrollbar: ScrollbarVisibility,
}

/// App-owned column table and row view. See the [module docs](self).
#[derive(Clone)]
pub struct TableState {
    inner: Rc<TableInner>,
}

impl TableState {
    /// `id` names cache entries and accessibility ids (unique in the
    /// window); `focus` is the table's keyboard focus target.
    pub fn new(id: &'static str, focus: FocusId) -> Self {
        Self {
            inner: Rc::new(TableInner {
                id,
                label: id,
                focus,
                row_height: 28.0,
                header_height: 30.0,
                viewport: (640.0, 320.0),
                titles: Vec::new(),
                widths: Vec::new(),
                min_widths: Vec::new(),
                sortable: Vec::new(),
                order: Vec::new(),
                view: Vec::new(),
                pos_of: Vec::new(),
                data_revision: 0,
                sort: None,
                selected_flags: Vec::new(),
                selected: Vec::new(),
                cursor: NONE,
                anchor: NONE,
                cursor_col: 0,
                scroll_y: 0.0,
                scroll_x: 0.0,
                header: None,
                revision: 0,
                layout_revision: 0,
                scrollbar_auto_hide: false,
                scrollbar: ScrollbarVisibility::new(),
            }),
        }
    }

    pub fn with_label(mut self, label: &'static str) -> Self {
        self.m().label = label;
        self
    }

    pub fn with_row_height(mut self, row_height: f32) -> Self {
        self.m().row_height = row_height.max(1.0);
        self
    }

    /// Show the body's scrollbars only while the pointer is over the body,
    /// a thumb is held, or briefly after the body scrolls (see
    /// [`ScrollbarVisibility`]). Without it they always show.
    pub fn with_scrollbar_auto_hide(mut self) -> Self {
        self.m().scrollbar_auto_hide = true;
        self
    }

    fn m(&mut self) -> &mut TableInner {
        let t = Rc::make_mut(&mut self.inner);
        t.revision += 1;
        t
    }

    fn m_layout(&mut self) -> &mut TableInner {
        let t = self.m();
        t.layout_revision += 1;
        t
    }

    /// The view's size in points, header included.
    pub fn set_viewport(&mut self, width: f32, height: f32) {
        if self.inner.viewport != (width, height) {
            let t = self.m_layout();
            t.viewport = (width.max(0.0), height.max(0.0));
            t.clamp_scroll();
        }
    }

    /// Append a column; returns its index, which [`TableData`] receives.
    pub fn add_column(&mut self, title: impl Into<Arc<str>>, width: f32, sortable: bool) -> u32 {
        let t = self.m_layout();
        let column = t.titles.len() as u32;
        t.titles.push(title.into());
        t.min_widths.push(32.0);
        t.widths.push(width.max(32.0));
        t.sortable.push(sortable);
        t.order.push(column);
        t.debug_check();
        column
    }

    // ---- Queries -------------------------------------------------------

    pub fn column_title(&self, column: u32) -> &str {
        &self.inner.titles[column as usize]
    }

    pub fn column_width(&self, column: u32) -> f32 {
        self.inner.widths[column as usize]
    }

    /// Columns in display order.
    pub fn column_order(&self) -> &[u32] {
        &self.inner.order
    }

    pub fn sort(&self) -> Option<(u32, SortDirection)> {
        self.inner.sort
    }

    /// Data rows in display order.
    pub fn view_rows(&self) -> &[u32] {
        &self.inner.view
    }

    /// Selected data rows, in selection order.
    pub fn selected_rows(&self) -> impl Iterator<Item = usize> + '_ {
        self.inner.selected.iter().map(|&r| r as usize)
    }

    /// The cursor's data row and column.
    pub fn cursor(&self) -> Option<(usize, u32)> {
        let t = &*self.inner;
        (t.cursor != NONE).then(|| (t.cursor as usize, t.order[t.cursor_col as usize]))
    }

    pub fn scroll_offset(&self) -> (f32, f32) {
        (self.inner.scroll_x, self.inner.scroll_y)
    }

    /// Display rows [`table_view`] builds: the viewport plus overscan.
    pub fn window(&self) -> Range<usize> {
        self.inner.row_window().range
    }

    /// Display positions of the columns [`table_view`] builds.
    pub fn visible_columns(&self) -> Range<usize> {
        self.inner.column_window()
    }

    // ---- Changes -------------------------------------------------------

    /// Bring the row view up to date with `data`: a new row count resets
    /// it, and a new revision reruns the sort. [`table_view`] calls this.
    pub fn sync<D: TableData>(&mut self, data: &D) {
        let rows = data.row_count();
        let t = &*self.inner;
        if t.view.len() == rows && t.data_revision == data.revision() {
            return;
        }
        let t = self.m_layout();
        if t.view.len() != rows {
            t.view.clear();
            t.view.extend(0..rows as u32);
            t.selected_flags.clear();
            t.selected_flags.resize(rows, false);
            t.selected.clear();
            if t.cursor as usize >= rows {
                t.cursor = NONE;
                t.anchor = NONE;
            }
        }
        t.data_revision = data.revision();
        t.resort(data);
        t.clamp_scroll();
    }

    /// Sort by `column`, or restore data order with `None`.
    pub fn sort_by<D: TableData>(&mut self, data: &D, sort: Option<(u32, SortDirection)>) {
        self.sync(data);
        let t = self.m_layout();
        t.sort = sort;
        if sort.is_none() {
            t.view.clear();
            t.view.extend(0..t.pos_of.len() as u32);
        }
        t.resort(data);
        t.reveal_cursor();
    }

    /// Set a column's width, kept at or above its minimum.
    pub fn resize_column(&mut self, column: u32, width: f32) {
        let t = self.m_layout();
        let c = column as usize;
        t.widths[c] = width.max(t.min_widths[c]);
        t.clamp_scroll();
    }

    /// Move the column at display position `from` to position `to`.
    pub fn move_column(&mut self, from: usize, to: usize) {
        let t = self.m_layout();
        // The cursor stays on its column wherever that moves.
        let cursor_column = t.order.get(t.cursor_col as usize).copied();
        let column = t.order.remove(from);
        t.order.insert(to.min(t.order.len()), column);
        if let Some(column) = cursor_column {
            t.cursor_col = t.position_of(column) as u32;
        }
        t.debug_check();
    }

    /// Scroll horizontally by `dx` points, clamped to the content.
    pub fn scroll_x_by(&mut self, dx: f32) {
        let t = self.m_layout();
        t.scroll_x += dx;
        t.clamp_scroll();
    }

    /// Apply an event from [`table_view`]. `mods` are the modifier keys
    /// held now; they matter for row presses.
    pub fn handle<D: TableData>(
        &mut self,
        data: &D,
        event: TableEvent,
        mods: SelectMods,
    ) -> TableOutcome {
        self.sync(data);
        let t = &*self.inner;
        match event {
            TableEvent::PressRow(row) => {
                let t = self.m();
                let row = row as u32;
                if mods.extend && t.anchor != NONE {
                    let (a, b) = (t.pos_of[t.anchor as usize], t.pos_of[row as usize]);
                    t.select_display_range(a as usize, b as usize);
                } else if mods.toggle {
                    let on = !t.selected_flags[row as usize];
                    t.set_selected(row, on);
                    t.anchor = row;
                } else {
                    t.select_only(row);
                    t.anchor = row;
                }
                t.cursor = row;
                TableOutcome::Changed
            }
            TableEvent::HeaderDrag { column, dx } => {
                let active = t.header.is_some_and(|h| h.target.is_some());
                if !active && dx.abs() < DRAG_THRESHOLD_PX {
                    self.m().header = Some(HeaderPress {
                        column,
                        target: None,
                    });
                    return TableOutcome::Unchanged;
                }
                let target = t.reorder_target(column, dx);
                if t.header.and_then(|h| h.target) == Some(target) {
                    return TableOutcome::Unchanged;
                }
                self.m().header = Some(HeaderPress {
                    column,
                    target: Some(target),
                });
                TableOutcome::Changed
            }
            TableEvent::HeaderRelease { column } => {
                let target = t.header.and_then(|h| h.target);
                let (sortable, sort) = (t.sortable[column as usize], t.sort);
                self.m().header = None;
                match target {
                    Some(to) => {
                        let from = self.inner.position_of(column);
                        self.move_column(from, to);
                    }
                    None if sortable => {
                        let direction = match sort {
                            Some((c, SortDirection::Ascending)) if c == column => {
                                SortDirection::Descending
                            }
                            _ => SortDirection::Ascending,
                        };
                        self.sort_by(data, Some((column, direction)));
                    }
                    None => {}
                }
                TableOutcome::Changed
            }
            TableEvent::Resize { column, width } => {
                self.resize_column(column, width);
                TableOutcome::Changed
            }
            TableEvent::Key(key) => self.key(key),
            TableEvent::Scroll(lines) => {
                let before = t.scroll_y;
                let t = self.m();
                t.scroll_y += lines as f32 * WHEEL_LINE_PX;
                t.clamp_scroll();
                if t.scroll_y == before {
                    TableOutcome::Unchanged
                } else {
                    TableOutcome::Changed
                }
            }
            TableEvent::ScrollTo(offset) => {
                let t = self.m();
                t.scroll_y = offset;
                t.clamp_scroll();
                TableOutcome::Changed
            }
            TableEvent::ScrollX(lines) => {
                let before = t.scroll_x;
                self.scroll_x_by(lines as f32 * WHEEL_LINE_PX);
                if self.inner.scroll_x == before {
                    TableOutcome::Unchanged
                } else {
                    TableOutcome::Changed
                }
            }
            TableEvent::ScrollXTo(offset) => {
                let t = self.m_layout();
                t.scroll_x = offset;
                t.clamp_scroll();
                TableOutcome::Changed
            }
        }
    }

    fn key(&mut self, key: TableKey) -> TableOutcome {
        let t = &*self.inner;
        let rows = t.view.len();
        if rows == 0 {
            return TableOutcome::Unchanged;
        }
        let last = rows - 1;
        let current = (t.cursor != NONE).then(|| t.pos_of[t.cursor as usize] as usize);
        let page = (t.body_height() / t.row_height).max(1.0) as usize;
        let step = |delta: isize| match current {
            Some(r) => r.saturating_add_signed(delta).min(last),
            None => 0,
        };
        let columns = t.order.len().max(1) as u32;
        let t = self.m();
        match key {
            TableKey::Up => t.select_display(step(-1)),
            TableKey::Down => t.select_display(step(1)),
            TableKey::PageUp => t.select_display(step(-(page as isize))),
            TableKey::PageDown => t.select_display(step(page as isize)),
            TableKey::Home => t.select_display(0),
            TableKey::End => t.select_display(last),
            TableKey::Left => t.cursor_col = t.cursor_col.saturating_sub(1),
            TableKey::Right => t.cursor_col = (t.cursor_col + 1).min(columns - 1),
            TableKey::ExtendUp | TableKey::ExtendDown => {
                let row = step(if key == TableKey::ExtendUp { -1 } else { 1 });
                let anchor = match t.anchor {
                    NONE => row,
                    a => t.pos_of[a as usize] as usize,
                };
                t.select_display_range(anchor, row);
                t.cursor = t.view[row];
                if t.anchor == NONE {
                    t.anchor = t.cursor;
                }
            }
            TableKey::ToggleSelect => {
                let Some(row) = current else {
                    return TableOutcome::Unchanged;
                };
                let data_row = t.view[row];
                let on = !t.selected_flags[data_row as usize];
                t.set_selected(data_row, on);
                t.anchor = data_row;
            }
            TableKey::SelectAll => t.select_display_range(0, last),
            TableKey::Activate => {
                return match current {
                    Some(row) => TableOutcome::Activated(t.view[row] as usize),
                    None => TableOutcome::Unchanged,
                };
            }
        }
        if matches!(key, TableKey::Left | TableKey::Right) {
            t.layout_revision += 1;
            if t.cursor == NONE {
                t.select_display(0);
            }
        }
        t.reveal_cursor();
        TableOutcome::Changed
    }

    /// Checks the column table, the row permutation, and the selection.
    /// O(rows + columns).
    pub fn verify_integrity(&self) -> Result<(), TableIntegrityError> {
        self.inner.verify_integrity()
    }

    /// Like [`Self::verify_integrity`], plus that the view is sorted by
    /// the current sort column of `data`.
    pub fn verify_sorted<D: TableData>(&self, data: &D) -> Result<(), TableIntegrityError> {
        self.inner.verify_integrity()?;
        self.inner.verify_sorted(data)
    }

    /// The header and the rows in the build window as text: cells in
    /// display order separated by ` | `, the sorted header marked `^` or
    /// `v`, selected rows prefixed `*`. For tests and logs.
    pub fn dump<D: TableData>(&self, data: &D) -> String {
        let t = &*self.inner;
        let mut out = String::new();
        let header: Vec<String> = t
            .order
            .iter()
            .map(|&c| {
                let mark = match t.sort {
                    Some((s, SortDirection::Ascending)) if s == c => " ^",
                    Some((s, SortDirection::Descending)) if s == c => " v",
                    _ => "",
                };
                format!("{}{mark}", t.titles[c as usize])
            })
            .collect();
        out.push_str(&header.join(" | "));
        out.push('\n');
        for display in t.row_window().range {
            let row = t.view[display] as usize;
            let cells: Vec<Cow<'_, str>> = t
                .order
                .iter()
                .map(|&c| data.cell_text(row, c as usize))
                .collect();
            if t.selected_flags[row] {
                out.push_str("* ");
            }
            out.push_str(&cells.join(" | "));
            out.push('\n');
        }
        out
    }
}

impl TableInner {
    fn body_height(&self) -> f32 {
        (self.viewport.1 - self.header_height).max(0.0)
    }

    fn total_width(&self) -> f32 {
        self.widths.iter().sum()
    }

    fn row_window(&self) -> quark_ui::virtual_list::VirtualListWindow {
        virtual_list_window(
            self.view.len(),
            self.scroll_y,
            self.body_height(),
            self.row_height,
            0.0,
            OVERSCAN,
        )
    }

    /// Display positions of columns overlapping the horizontal viewport.
    fn column_window(&self) -> Range<usize> {
        let (left, right) = (self.scroll_x, self.scroll_x + self.viewport.0);
        let mut x = 0.0;
        let mut start = self.order.len();
        let mut end = 0;
        for (pos, &c) in self.order.iter().enumerate() {
            let w = self.widths[c as usize];
            if x + w > left && x < right {
                start = start.min(pos);
                end = pos + 1;
            }
            x += w;
        }
        if start > end { 0..0 } else { start..end }
    }

    fn column_left(&self, pos: usize) -> f32 {
        self.order[..pos]
            .iter()
            .map(|&c| self.widths[c as usize])
            .sum()
    }

    fn position_of(&self, column: u32) -> usize {
        self.order.iter().position(|&c| c == column).unwrap_or(0)
    }

    /// The display position a header dragged `dx` points moves to: the
    /// column under the dragged column's center.
    fn reorder_target(&self, column: u32, dx: f32) -> usize {
        let pos = self.position_of(column);
        let center = self.column_left(pos) + self.widths[column as usize] / 2.0 + dx;
        let mut x = 0.0;
        for (p, &c) in self.order.iter().enumerate() {
            x += self.widths[c as usize];
            if center < x {
                return p;
            }
        }
        self.order.len().saturating_sub(1)
    }

    fn clamp_scroll(&mut self) {
        let max_y = (self.view.len() as f32 * self.row_height - self.body_height()).max(0.0);
        self.scroll_y = self.scroll_y.clamp(0.0, max_y);
        let max_x = (self.total_width() - self.viewport.0).max(0.0);
        self.scroll_x = self.scroll_x.clamp(0.0, max_x);
    }

    fn resort<D: TableData>(&mut self, data: &D) {
        if let Some((column, direction)) = self.sort {
            let c = column as usize;
            match direction {
                SortDirection::Descending => self
                    .view
                    .sort_by(|&a, &b| data.compare(b as usize, a as usize, c)),
                _ => self
                    .view
                    .sort_by(|&a, &b| data.compare(a as usize, b as usize, c)),
            }
        }
        self.pos_of.clear();
        self.pos_of.resize(self.view.len(), 0);
        for (display, &row) in self.view.iter().enumerate() {
            self.pos_of[row as usize] = display as u32;
        }
        self.debug_check();
    }

    fn set_selected(&mut self, row: u32, on: bool) {
        if self.selected_flags[row as usize] == on {
            return;
        }
        self.selected_flags[row as usize] = on;
        if on {
            self.selected.push(row);
        } else if let Some(at) = self.selected.iter().position(|&r| r == row) {
            self.selected.remove(at);
        }
    }

    fn select_only(&mut self, row: u32) {
        for &r in &self.selected {
            self.selected_flags[r as usize] = false;
        }
        self.selected.clear();
        self.set_selected(row, true);
    }

    fn select_display(&mut self, display: usize) {
        let row = self.view[display];
        self.select_only(row);
        self.cursor = row;
        self.anchor = row;
    }

    fn select_display_range(&mut self, a: usize, b: usize) {
        for &r in &self.selected {
            self.selected_flags[r as usize] = false;
        }
        self.selected.clear();
        for display in a.min(b)..=a.max(b) {
            self.set_selected(self.view[display], true);
        }
    }

    /// Scroll so the cursor cell is inside the viewport.
    fn reveal_cursor(&mut self) {
        if self.cursor == NONE {
            return;
        }
        let top = self.pos_of[self.cursor as usize] as f32 * self.row_height;
        let body = self.body_height();
        if top < self.scroll_y {
            self.scroll_y = top;
        } else if top + self.row_height > self.scroll_y + body {
            self.scroll_y = top + self.row_height - body;
        }
        let pos = self.cursor_col as usize;
        if pos < self.order.len() {
            let left = self.column_left(pos);
            let right = left + self.widths[self.order[pos] as usize];
            if left < self.scroll_x {
                self.scroll_x = left;
            } else if right > self.scroll_x + self.viewport.0 {
                self.scroll_x = right - self.viewport.0;
            }
        }
        self.clamp_scroll();
    }

    fn debug_check(&self) {
        debug_assert_eq!(self.verify_integrity(), Ok(()));
    }

    fn verify_integrity(&self) -> Result<(), TableIntegrityError> {
        quark::selection::count_integrity_steps(self.view.len() + self.titles.len());
        let columns = self.titles.len();
        if self.widths.len() != columns
            || self.min_widths.len() != columns
            || self.sortable.len() != columns
            || self.order.len() != columns
        {
            return Err(TableIntegrityError::ColumnLength);
        }
        let mut seen = vec![false; columns];
        for &c in &self.order {
            if c as usize >= columns || std::mem::replace(&mut seen[c as usize], true) {
                return Err(TableIntegrityError::ColumnOrder { column: c });
            }
        }
        if let Some(c) = (0..columns).find(|&c| self.widths[c] < self.min_widths[c]) {
            return Err(TableIntegrityError::Width { column: c as u32 });
        }
        let rows = self.view.len();
        if self.pos_of.len() != rows || self.selected_flags.len() != rows {
            return Err(TableIntegrityError::RowLength);
        }
        for (display, &row) in self.view.iter().enumerate() {
            if row as usize >= rows || self.pos_of[row as usize] != display as u32 {
                return Err(TableIntegrityError::RowMap { display });
            }
        }
        let flagged = self.selected_flags.iter().filter(|&&s| s).count();
        if flagged != self.selected.len()
            || self
                .selected
                .iter()
                .any(|&r| !self.selected_flags[r as usize])
        {
            return Err(TableIntegrityError::Selection);
        }
        Ok(())
    }

    fn verify_sorted<D: TableData>(&self, data: &D) -> Result<(), TableIntegrityError> {
        let Some((column, direction)) = self.sort else {
            return Ok(());
        };
        let wrong = match direction {
            SortDirection::Descending => Ordering::Less,
            _ => Ordering::Greater,
        };
        match self.view.windows(2).position(|pair| {
            data.compare(pair[0] as usize, pair[1] as usize, column as usize) == wrong
        }) {
            Some(display) => Err(TableIntegrityError::Unsorted { display }),
            None => Ok(()),
        }
    }
}

/// A broken invariant of a [`TableState`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TableIntegrityError {
    ColumnLength,
    /// `order` is not a permutation of the columns.
    ColumnOrder {
        column: u32,
    },
    Width {
        column: u32,
    },
    RowLength,
    /// `view` and `pos_of` are not inverse permutations.
    RowMap {
        display: usize,
    },
    Selection,
    Unsorted {
        display: usize,
    },
}

impl fmt::Display for TableIntegrityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "table integrity: {self:?}")
    }
}

impl std::error::Error for TableIntegrityError {}

// ---------------------------------------------------------------------------
// View
// ---------------------------------------------------------------------------

const KEYS: &[(&str, TableKey)] = &[
    ("arrowup", TableKey::Up),
    ("arrowdown", TableKey::Down),
    ("arrowleft", TableKey::Left),
    ("arrowright", TableKey::Right),
    ("home", TableKey::Home),
    ("end", TableKey::End),
    ("pageup", TableKey::PageUp),
    ("pagedown", TableKey::PageDown),
    ("shift+arrowup", TableKey::ExtendUp),
    ("shift+arrowdown", TableKey::ExtendDown),
    ("space", TableKey::ToggleSelect),
    ("mod+a", TableKey::SelectAll),
    ("enter", TableKey::Activate),
];

#[derive(Debug, Clone, Copy)]
struct TableColors {
    header: Color,
    border: Color,
    selected: Color,
    hover: Color,
    text: Color,
    muted: Color,
    accent: Color,
    focus: Color,
}

impl TableColors {
    fn of(theme: &Theme) -> Self {
        let c = &theme.colors;
        Self {
            header: c.panel_strong,
            border: c.border_variant,
            selected: c.sidebar_row_selected,
            hover: c.sidebar_row_hover,
            text: c.text,
            muted: c.text_muted,
            accent: c.accent,
            focus: c.focus_border,
        }
    }
}

/// The table in `state` over `data`, at the state's viewport size.
pub fn table_view<D: TableData>(
    state: &mut TableState,
    data: &Rc<D>,
    theme: &Theme,
    env: CollectionEnv,
    on_event: fn(TableEvent) -> Action,
) -> AnyElement {
    state.sync(&**data);
    let inner = state.inner.clone();
    let data = data.clone();
    let colors = TableColors::of(theme);
    let (width, height) = inner.viewport;
    let hash = inputs_hash(&(inner.revision, data.revision(), env, on_event as usize));
    view! {
        <cached(inner.id, hash, move || build_table(&inner, &data, colors, env, on_event))
                w={width} h={height} />
    }
}

fn build_table<D: TableData>(
    t: &Rc<TableInner>,
    data: &Rc<D>,
    colors: TableColors,
    env: CollectionEnv,
    on_event: fn(TableEvent) -> Action,
) -> AnyElement {
    let (width, height) = t.viewport;
    let window = t.row_window();
    let columns = t.column_window();
    let reorder_target = t.header.and_then(|h| h.target.map(|to| (h.column, to)));
    view! {
        <div w={width} h={height} class="flex-col overflow-clip" track_focus={t.focus}
             @when {env.accessible} {
                 accessibility_id={t.id} accessibility_role={Role::Table} aria-label={t.label}
                 accessibility_table_size={(t.view.len() + 1, t.order.len())}
                 aria-multiselectable={true}
             }
             @for &(binding, key) in KEYS { on_key={(binding, on_event(TableEvent::Key(key)))} }>
            <div class="w-full" h={t.header_height} class="shrink-0 overflow-clip">
                // The header stays put vertically and follows the body
                // horizontally.
                <div w={t.total_width()} h={t.header_height} class="flex-row shrink-0"
                     translate={(-t.scroll_x, 0.0)} bg={colors.header} border_b={colors.border}
                     @when {env.accessible} {
                         accessibility_id={format!("{}.header", t.id)}
                         accessibility_role={Role::Row} aria-rowindex={0}
                     }>
                    <div w={t.column_left(columns.start)} class="shrink-0" />
                    for pos in columns.clone() {
                        {header_cell(t, pos, reorder_target, colors, env, on_event)}
                    }
                </div>
            </div>
            <div class="w-full" h={t.body_height()} class="flex-col" scroll_y={t.scroll_y}
                 scroll_total={window.total_extent}
                 on:scroll={ScrollActionBuilder::new(move |lines| on_event(TableEvent::Scroll(lines)))
                     .with_to_px(move |px| on_event(TableEvent::ScrollTo(px as f32)))}
                 scroll_x={t.scroll_x} scroll_total_x={t.total_width()}
                 on:scroll_x={ScrollActionBuilder::new(move |lines| on_event(TableEvent::ScrollX(lines)))
                     .with_to_px(move |px| on_event(TableEvent::ScrollXTo(px as f32)))}
                 @when {t.scrollbar_auto_hide} {
                     scrollbar_visibility={&t.scrollbar} class="scrollbar-auto-hide"
                 }>
                <div class="w-full shrink-0" h={window.top_spacer} />
                for display in window.range {
                    {table_row(t, data, display, columns.clone(), colors, env, on_event)}
                }
                <div class="w-full shrink-0" h={window.bottom_spacer} />
            </div>
        </div>
    }
}

fn header_cell(
    t: &TableInner,
    pos: usize,
    reorder_target: Option<(u32, usize)>,
    colors: TableColors,
    env: CollectionEnv,
    on_event: fn(TableEvent) -> Action,
) -> AnyElement {
    let column = t.order[pos];
    let c = column as usize;
    let width = t.widths[c];
    let sort = t.sort.filter(|(s, _)| *s == column).map(|(_, d)| d);
    let start_width = width;
    view! {
        <div w={width} h={t.header_height} class="shrink-0 relative flex-row items-center"
             gap={4.0} px={CELL_PAD_PX} border_r={colors.border} class="cursor-pointer"
             on:drag={move |press: ClickEvent| {
                 Box::new(HeaderDrag {
                     column,
                     press_x: press.x,
                     on_event,
                 }) as Box<dyn DragHandler>
             }}
             @when {env.accessible} {
                 accessibility_id={format!("{}.col.{column}", t.id)}
                 accessibility_role={Role::ColumnHeader} aria-label={&*t.titles[c]}
                 aria-colindex={pos} aria-rowindex={0}
                 @when {let Some(direction) = sort} { aria-sort={direction} }
             }>
            <text class="text-sm font-semibold" color={colors.text} class="truncate">
                {&*t.titles[c]}
            </text>
            if let Some(direction) = sort {
                <icon svg={if direction == SortDirection::Descending {
                          lucide::ARROW_DOWN
                      } else {
                          lucide::ARROW_UP
                      }}
                      size={12.0} color={colors.muted} />
            }
            // Where a dragged column would land.
            if let Some((dragged, to)) = reorder_target
                && to == pos
                && dragged != column
            {
                <div class="absolute top-0" w={2.0} h={t.header_height} bg={colors.accent}
                     @when {t.position_of(dragged) < to} { class="right-0" }
                     @when {t.position_of(dragged) >= to} { class="left-0" } />
            }
            <div class="absolute top-0 right-0" w={RESIZE_HANDLE_PX} h={t.header_height}
                 class="cursor-col-resize"
                 on:drag={move |press: ClickEvent| {
                     Box::new(ResizeDrag {
                         column,
                         press_x: press.x,
                         start_width,
                         on_event,
                     }) as Box<dyn DragHandler>
                 }} />
        </div>
    }
}

fn table_row<D: TableData>(
    t: &Rc<TableInner>,
    data: &Rc<D>,
    display: usize,
    columns: Range<usize>,
    colors: TableColors,
    env: CollectionEnv,
    on_event: fn(TableEvent) -> Action,
) -> AnyElement {
    let row = t.view[display] as usize;
    let selected = t.selected_flags[row];
    let cursor_col = (env.focused && t.cursor as usize == row).then_some(t.cursor_col);
    let key = quark::stable_hash(t.id) ^ (row as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let hash = inputs_hash(&(
        t.layout_revision,
        data.revision(),
        display,
        selected,
        cursor_col,
        env,
        on_event as usize,
    ));
    let t = t.clone();
    let data = data.clone();
    let height = t.row_height;
    let build = move || {
        let style = CellStyle {
            text: colors.text,
            muted: colors.muted,
            accent: colors.accent,
            selected,
        };
        view! {
            <div w={t.total_width()} h={t.row_height} class="shrink-0 flex-row"
                 border_b={colors.border} on:click={on_event(TableEvent::PressRow(row))}
                 @when {selected} { bg={colors.selected} }
                 @when {!selected} { hover_bg={colors.hover} }
                 @when {env.accessible} {
                     accessibility_id={format!("{}.row.{row}", t.id)}
                     accessibility_role={Role::Row} aria-rowindex={display + 1}
                     aria-selected={selected}
                 }>
                <div w={t.column_left(columns.start)} class="shrink-0" />
                for pos in columns {
                    <div w={t.widths[t.order[pos] as usize]} h={t.row_height}
                         class="shrink-0 flex-row items-center" px={CELL_PAD_PX}
                         @when {cursor_col == Some(pos as u32)} { border={colors.focus} }
                         @when {env.accessible} {
                             accessibility_id={format!("{}.cell.{row}.{}", t.id, t.order[pos])}
                             accessibility_role={Role::Cell}
                             aria-label={data.cell_text(row, t.order[pos] as usize).into_owned()}
                             aria-rowindex={display + 1} aria-colindex={pos}
                         }>
                        {data.render_cell(row, t.order[pos] as usize, &style)}
                    </div>
                }
            </div>
        }
    };
    view! { <cached(CacheKey(key), hash, build) class="w-full" h={height} /> }
}

/// A press on a header cell: a click sorts, a drag moves the column.
struct HeaderDrag {
    column: u32,
    press_x: f32,
    on_event: fn(TableEvent) -> Action,
}

impl DragHandler for HeaderDrag {
    fn on_move(&mut self, x: f32, _y: f32) -> Vec<Action> {
        vec![(self.on_event)(TableEvent::HeaderDrag {
            column: self.column,
            dx: x - self.press_x,
        })]
    }

    fn on_release(&mut self) -> DragReleaseResult {
        DragReleaseResult {
            actions: vec![(self.on_event)(TableEvent::HeaderRelease {
                column: self.column,
            })],
        }
    }
}

struct ResizeDrag {
    column: u32,
    press_x: f32,
    start_width: f32,
    on_event: fn(TableEvent) -> Action,
}

impl DragHandler for ResizeDrag {
    fn on_move(&mut self, x: f32, _y: f32) -> Vec<Action> {
        vec![(self.on_event)(TableEvent::Resize {
            column: self.column,
            width: self.start_width + x - self.press_x,
        })]
    }

    fn on_release(&mut self) -> DragReleaseResult {
        DragReleaseResult::empty()
    }

    fn cursor(&self) -> CursorHint {
        CursorHint::ResizeCol
    }
}
