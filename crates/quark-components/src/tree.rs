//! A virtualized tree view over a flat node table.
//!
//! [`TreeState`] is app owned. Nodes live in parallel columns indexed by
//! [`NodeId`]: parent and sibling links, child count, label, icon, and
//! flags. The rows on screen are a second set of columns (the depth-first
//! order of expanded nodes with each row's depth and position among its
//! siblings), rebuilt after structural changes.
//!
//! [`tree_view`] paints only the rows in the scroll window, each in its own
//! cache boundary, inside one boundary for the whole view, so a frame where
//! nothing changed replays without building or allocating.
//!
//! The view emits [`TreeEvent`]s through the caller's `fn(TreeEvent) ->
//! Action`. The app hands each one back to [`TreeState::handle`] with the
//! modifier keys held at the time and the clock, and gets a [`TreeOutcome`]
//! naming anything it must act on: children to load, an item opened, a
//! node moved.

use std::fmt;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use accesskit::Role;
use quark::selection::{FULL_INTEGRITY_CHECKS, count_integrity_steps};
use quark::view;
use quark_ui::element::{
    AnyElement, CacheKey, ClickEvent, DragHandler, DragReleaseResult, IntoAnyElement,
    ScrollActionBuilder, ScrollbarVisibility, WHEEL_LINE_PX, cached, div, inputs_hash, svg_icon,
    text,
};
use quark_ui::icons::lucide;
use quark_ui::style::Styled;
use quark_ui::theme::{Color, Theme};
use quark_ui::virtual_list::virtual_list_window;
use quark_ui::{Action, FocusId};

/// Rows built beyond each edge of the viewport.
const OVERSCAN: usize = 2;

/// Pointer travel before a press on a row becomes a drag.
const DRAG_THRESHOLD_PX: f32 = 4.0;

/// Pause after which type-ahead starts a new prefix.
const TYPE_AHEAD_RESET_MS: u64 = 1_000;

/// Keys that feed type-ahead, one binding each.
const TYPE_AHEAD_KEYS: &str = "abcdefghijklmnopqrstuvwxyz0123456789";

const NONE: u32 = u32::MAX;
/// The hidden root: parent of every top-level node.
const ROOT: u32 = 0;

const EXPANDED: u8 = 1;
/// Has children the app has not loaded yet.
const LAZY: u8 = 1 << 1;
const SELECTED: u8 = 1 << 2;

/// A node of a [`TreeState`]. Stable for the node's life: moves keep it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId(u32);

impl NodeId {
    pub fn index(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum SelectionMode {
    Single,
    #[default]
    Multi,
}

/// Modifier keys held during a press: `extend` (Shift) selects a range
/// from the anchor, `toggle` (Cmd or Ctrl) adds or removes one item.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SelectMods {
    pub extend: bool,
    pub toggle: bool,
}

/// What the view needs to know about its surroundings, shared by
/// [`tree_view`] and [`crate::table_view`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct CollectionEnv {
    /// The view holds keyboard focus; the cursor row is outlined.
    pub focused: bool,
    /// Assistive tech is listening. Without it the view skips building
    /// accessible names, so replayed frames copy no strings.
    pub accessible: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DropPosition {
    Before,
    Inside,
    After,
}

/// Where a dragged node lands: before, inside (as last child), or after
/// `node`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DropTarget {
    pub node: NodeId,
    pub position: DropPosition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TreeNav {
    Up,
    Down,
    /// Collapse, or go to the parent.
    Left,
    /// Expand, or go to the first child.
    Right,
    Home,
    End,
    PageUp,
    PageDown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TreeKey {
    /// Move the cursor and select only its row.
    Nav(TreeNav),
    /// Move the cursor and select the range from the anchor (Shift).
    Extend(TreeNav),
    /// Move the cursor without changing the selection (Cmd or Ctrl).
    Move(TreeNav),
    /// Add or remove the cursor row (Space).
    ToggleSelect,
    SelectAll,
    /// Open the cursor row (Enter).
    Activate,
}

/// Input from [`tree_view`], to pass to [`TreeState::handle`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TreeEvent {
    /// Pointer pressed on a row (or assistive tech clicked it).
    Press(NodeId),
    /// The disclosure chevron was clicked.
    Toggle(NodeId),
    /// The pointer moved `dy` points since the press on a row.
    DragMove {
        dy: f32,
    },
    DragEnd,
    Key(TreeKey),
    TypeAhead(char),
    /// Wheel lines; positive scrolls down.
    Scroll(i32),
    /// Scrollbar drag to an absolute offset in points.
    ScrollTo(f32),
}

/// What [`TreeState::handle`] did that the app may need to act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeOutcome {
    Unchanged,
    /// Selection, cursor, expansion, or scroll changed; the view redraws.
    Changed,
    /// A lazy node was expanded: add its children, then call
    /// [`TreeState::finish_loading`].
    LoadChildren(NodeId),
    Activated(NodeId),
    /// A drag moved `node` to `index` among the children of `parent`
    /// (`None` for the top level).
    Moved {
        node: NodeId,
        parent: Option<NodeId>,
        index: usize,
    },
}

/// A drag that started with a press on `source`.
#[derive(Debug, Clone, Copy)]
struct Drag {
    source: u32,
    /// Scroll offset at the press, so wheel scrolling mid-drag moves the
    /// drop point with the content.
    scroll_at_press: f32,
    active: bool,
}

#[derive(Clone)]
struct TreeData {
    id: &'static str,
    label: &'static str,
    focus: FocusId,
    mode: SelectionMode,
    row_height: f32,
    indent: f32,
    viewport_height: f32,

    // Node columns; index 0 is the hidden root.
    parent: Vec<u32>,
    first_child: Vec<u32>,
    last_child: Vec<u32>,
    next_sibling: Vec<u32>,
    prev_sibling: Vec<u32>,
    child_count: Vec<u32>,
    labels: Vec<Arc<str>>,
    icons: Vec<Option<&'static str>>,
    flags: Vec<u8>,

    // Row columns: expanded nodes in depth-first order.
    rows: Vec<u32>,
    row_depth: Vec<u16>,
    /// 1-based position among siblings.
    row_pos: Vec<u32>,
    /// Row of each node, `NONE` when hidden under a collapsed ancestor.
    row_of: Vec<u32>,
    rows_dirty: bool,
    scratch: Vec<u32>,

    selected: Vec<u32>,
    cursor: u32,
    anchor: u32,
    scroll: f32,
    drag: Option<Drag>,
    drop: Option<DropTarget>,
    type_ahead: String,
    type_ahead_at: u64,
    /// Bumped by every change the view can show; the view's cache input.
    revision: u64,
    /// The scrollbar shows only on demand, as `scrollbar` decides.
    scrollbar_auto_hide: bool,
    scrollbar: ScrollbarVisibility,
}

/// App-owned tree model and view state. See the [module docs](self).
///
/// Cloning is cheap (the columns are shared until the next change).
#[derive(Clone)]
pub struct TreeState {
    data: Rc<TreeData>,
}

impl TreeState {
    /// An empty tree. `id` names its cache entries and accessibility ids
    /// and must be unique in the window; `focus` is its keyboard focus
    /// target.
    pub fn new(id: &'static str, focus: FocusId) -> Self {
        Self {
            data: Rc::new(TreeData {
                id,
                label: id,
                focus,
                mode: SelectionMode::Multi,
                row_height: 24.0,
                indent: 16.0,
                viewport_height: 320.0,
                parent: vec![NONE],
                first_child: vec![NONE],
                last_child: vec![NONE],
                next_sibling: vec![NONE],
                prev_sibling: vec![NONE],
                child_count: vec![0],
                labels: vec![Arc::from("")],
                icons: vec![None],
                flags: vec![EXPANDED],
                rows: Vec::new(),
                row_depth: Vec::new(),
                row_pos: Vec::new(),
                row_of: vec![NONE],
                rows_dirty: false,
                scratch: Vec::new(),
                selected: Vec::new(),
                cursor: NONE,
                anchor: NONE,
                scroll: 0.0,
                drag: None,
                drop: None,
                type_ahead: String::new(),
                type_ahead_at: 0,
                revision: 0,
                scrollbar_auto_hide: false,
                scrollbar: ScrollbarVisibility::new(),
            }),
        }
    }

    /// The accessible name of the tree.
    pub fn with_label(mut self, label: &'static str) -> Self {
        self.m().label = label;
        self
    }

    pub fn with_mode(mut self, mode: SelectionMode) -> Self {
        self.m().mode = mode;
        self
    }

    pub fn with_row_height(mut self, row_height: f32) -> Self {
        self.m().row_height = row_height.max(1.0);
        self
    }

    /// Show the scrollbar only while the pointer is over the view, its
    /// thumb is held, or briefly after the view scrolls or gains focus (see
    /// [`ScrollbarVisibility`]). Without it the scrollbar always shows.
    pub fn with_scrollbar_auto_hide(mut self) -> Self {
        self.m().scrollbar_auto_hide = true;
        self
    }

    /// Height of the view in points; the scroll window covers this much.
    pub fn set_viewport_height(&mut self, height: f32) {
        if self.data.viewport_height != height {
            let d = self.m();
            d.viewport_height = height.max(0.0);
            d.clamp_scroll();
        }
    }

    /// The data for a change the view shows: copied first if a frame still
    /// shares it, and with the revision bumped.
    fn m(&mut self) -> &mut TreeData {
        let d = Rc::make_mut(&mut self.data);
        d.revision += 1;
        d
    }

    // ---- Building ------------------------------------------------------

    /// Append a node with `label` as the last child of `parent` (`None` for
    /// the top level).
    pub fn add(&mut self, parent: Option<NodeId>, label: impl Into<Arc<str>>) -> NodeId {
        let d = self.m();
        let id = d.push_node(parent.map_or(ROOT, |p| p.0), label.into());
        if FULL_INTEGRITY_CHECKS {
            d.debug_check();
        } else {
            debug_assert_eq!(d.verify_node(id), Ok(()));
        }
        NodeId(id)
    }

    /// Append many children of `parent` at once, with one integrity check.
    /// Returns the ids of the new nodes.
    pub fn extend<L: Into<Arc<str>>>(
        &mut self,
        parent: Option<NodeId>,
        labels: impl IntoIterator<Item = L>,
    ) -> Range<u32> {
        let d = self.m();
        let start = d.parent.len() as u32;
        let parent = parent.map_or(ROOT, |p| p.0);
        for label in labels {
            d.push_node(parent, label.into());
        }
        d.debug_check();
        start..d.parent.len() as u32
    }

    pub fn set_icon(&mut self, node: NodeId, svg: Option<&'static str>) {
        self.m().icons[node.0 as usize] = svg;
    }

    /// Mark `node` as having children not loaded yet: it shows a chevron,
    /// and expanding it returns [`TreeOutcome::LoadChildren`].
    pub fn set_lazy(&mut self, node: NodeId, lazy: bool) {
        let d = self.m();
        d.set_flag(node.0, LAZY, lazy);
        d.rows_dirty = true;
    }

    /// The app finished adding the children of a lazy node.
    pub fn finish_loading(&mut self, node: NodeId) {
        self.set_lazy(node, false);
    }

    // ---- Queries -------------------------------------------------------

    /// Nodes, not counting the hidden root.
    pub fn len(&self) -> usize {
        self.data.parent.len() - 1
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn node(&self, index: u32) -> Option<NodeId> {
        (index != ROOT && (index as usize) < self.data.parent.len()).then_some(NodeId(index))
    }

    pub fn label(&self, node: NodeId) -> &str {
        &self.data.labels[node.0 as usize]
    }

    pub fn parent(&self, node: NodeId) -> Option<NodeId> {
        let parent = self.data.parent[node.0 as usize];
        (parent != ROOT && parent != NONE).then_some(NodeId(parent))
    }

    /// Children of `parent` (`None` for the top level), in order.
    pub fn children(&self, parent: Option<NodeId>) -> impl Iterator<Item = NodeId> + '_ {
        let d = &*self.data;
        let first = d.first_child[parent.map_or(ROOT, |p| p.0) as usize];
        std::iter::successors((first != NONE).then_some(first), move |&n| {
            let next = d.next_sibling[n as usize];
            (next != NONE).then_some(next)
        })
        .map(NodeId)
    }

    pub fn is_expanded(&self, node: NodeId) -> bool {
        self.data.has(node.0, EXPANDED)
    }

    pub fn is_selected(&self, node: NodeId) -> bool {
        self.data.has(node.0, SELECTED)
    }

    /// Selected nodes in the order they were selected.
    pub fn selected(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.data.selected.iter().map(|&n| NodeId(n))
    }

    /// The keyboard cursor.
    pub fn cursor(&self) -> Option<NodeId> {
        (self.data.cursor != NONE).then_some(NodeId(self.data.cursor))
    }

    /// Where the current drag would drop, while one is over a valid target.
    pub fn drop_target(&self) -> Option<DropTarget> {
        self.data.drop
    }

    pub fn scroll_offset(&self) -> f32 {
        self.data.scroll
    }

    /// Visible rows (expanded nodes in depth-first order).
    pub fn row_count(&mut self) -> usize {
        self.ensure_rows();
        self.data.rows.len()
    }

    /// Rows [`tree_view`] builds at the current scroll offset: the
    /// viewport plus overscan.
    pub fn window(&mut self) -> Range<usize> {
        self.ensure_rows();
        let d = &*self.data;
        virtual_list_window(
            d.rows.len(),
            d.scroll,
            d.viewport_height,
            d.row_height,
            0.0,
            OVERSCAN,
        )
        .range
    }

    /// The visible rows as text, one per line: two spaces per level, a
    /// disclosure mark (`v` expanded, `>` collapsed), the label, then `*`
    /// when selected and `<` on the cursor row. For tests and logs.
    pub fn dump_rows(&mut self) -> String {
        self.ensure_rows();
        let d = &*self.data;
        let mut out = String::new();
        for (row, &node) in d.rows.iter().enumerate() {
            let n = node as usize;
            for _ in 0..d.row_depth[row] {
                out.push_str("  ");
            }
            if d.expandable(node) {
                out.push_str(if d.has(node, EXPANDED) { "v " } else { "> " });
            }
            out.push_str(&d.labels[n]);
            if d.has(node, SELECTED) {
                out.push_str(" *");
            }
            if d.cursor == node {
                out.push_str(" <");
            }
            out.push('\n');
        }
        out
    }

    // ---- Changes -------------------------------------------------------

    /// Expand `node`. A lazy node also asks the app for its children.
    pub fn expand(&mut self, node: NodeId) -> TreeOutcome {
        let d = &*self.data;
        if !d.expandable(node.0) || d.has(node.0, EXPANDED) {
            return TreeOutcome::Unchanged;
        }
        let lazy = d.has(node.0, LAZY);
        let d = self.m();
        d.set_flag(node.0, EXPANDED, true);
        d.rows_dirty = true;
        if lazy {
            TreeOutcome::LoadChildren(node)
        } else {
            TreeOutcome::Changed
        }
    }

    /// Collapse `node`; a cursor inside it moves to it.
    pub fn collapse(&mut self, node: NodeId) -> TreeOutcome {
        if !self.data.has(node.0, EXPANDED) {
            return TreeOutcome::Unchanged;
        }
        let d = self.m();
        d.set_flag(node.0, EXPANDED, false);
        if d.cursor != NONE && d.is_descendant(d.cursor, node.0) {
            d.cursor = node.0;
            d.anchor = node.0;
        }
        d.rows_dirty = true;
        TreeOutcome::Changed
    }

    pub fn toggle(&mut self, node: NodeId) -> TreeOutcome {
        if self.is_expanded(node) {
            self.collapse(node)
        } else {
            self.expand(node)
        }
    }

    /// Select only `node` and put the cursor on it.
    pub fn select_only(&mut self, node: NodeId) {
        let d = self.m();
        d.select_only(node.0);
        d.cursor = node.0;
        d.anchor = node.0;
    }

    /// Move `node` to `target`. False, changing nothing, when the target is
    /// the node itself or inside it.
    pub fn move_node(&mut self, node: NodeId, target: DropTarget) -> bool {
        if !self.data.can_drop(node.0, target) {
            return false;
        }
        let d = self.m();
        d.move_node(node.0, target);
        if FULL_INTEGRITY_CHECKS {
            d.debug_check();
        } else {
            debug_assert_eq!(d.verify_node(node.0), Ok(()));
        }
        true
    }

    fn ensure_rows(&mut self) {
        if self.data.rows_dirty {
            self.m().flatten();
        }
    }

    /// Apply an event from [`tree_view`]. `mods` are the modifier keys
    /// held now (they only matter for presses); `now_ms` is the app clock,
    /// for type-ahead.
    pub fn handle(&mut self, event: TreeEvent, mods: SelectMods, now_ms: u64) -> TreeOutcome {
        self.ensure_rows();
        let outcome = match event {
            TreeEvent::Press(node) => self.press(node.0, mods),
            TreeEvent::Toggle(node) => self.toggle(node),
            TreeEvent::DragMove { dy } => self.drag_move(dy),
            TreeEvent::DragEnd => self.drag_end(),
            TreeEvent::Key(key) => self.key(key),
            TreeEvent::TypeAhead(ch) => self.type_ahead(ch, now_ms),
            TreeEvent::Scroll(lines) => {
                self.scroll_to(self.data.scroll + lines as f32 * WHEEL_LINE_PX)
            }
            TreeEvent::ScrollTo(offset) => self.scroll_to(offset),
        };
        // Helpers change view state in place; any visible change bumps the
        // revision so the cached view rebuilds.
        if outcome != TreeOutcome::Unchanged {
            self.m();
        }
        outcome
    }

    fn scroll_to(&mut self, offset: f32) -> TreeOutcome {
        let before = self.data.scroll;
        let d = Rc::make_mut(&mut self.data);
        d.scroll = offset;
        d.clamp_scroll();
        changed(d.scroll != before)
    }

    fn press(&mut self, node: u32, mods: SelectMods) -> TreeOutcome {
        let d = Rc::make_mut(&mut self.data);
        let multi = d.mode == SelectionMode::Multi;
        if multi && mods.extend && d.anchor != NONE && d.row_of[d.anchor as usize] != NONE {
            let anchor = d.row_of[d.anchor as usize] as usize;
            d.select_rows(anchor, d.row_of[node as usize] as usize);
        } else if multi && mods.toggle {
            let on = !d.has(node, SELECTED);
            d.set_selected(node, on);
            d.anchor = node;
        } else {
            d.select_only(node);
            d.anchor = node;
        }
        d.cursor = node;
        d.drag = Some(Drag {
            source: node,
            scroll_at_press: d.scroll,
            active: false,
        });
        TreeOutcome::Changed
    }

    fn drag_move(&mut self, dy: f32) -> TreeOutcome {
        let d = Rc::make_mut(&mut self.data);
        let Some(drag) = d.drag.as_mut() else {
            return TreeOutcome::Unchanged;
        };
        if !drag.active && dy.abs() < DRAG_THRESHOLD_PX {
            return TreeOutcome::Unchanged;
        }
        drag.active = true;
        let drag = *drag;
        let target = d.drop_at(drag, dy);
        changed(std::mem::replace(&mut d.drop, target) != target)
    }

    fn drag_end(&mut self) -> TreeOutcome {
        let d = Rc::make_mut(&mut self.data);
        let drag = d.drag.take();
        let Some(target) = d.drop.take() else {
            return changed(drag.is_some_and(|drag| drag.active));
        };
        let Some(drag) = drag else {
            return TreeOutcome::Changed;
        };
        let node = NodeId(drag.source);
        if !self.move_node(node, target) {
            return TreeOutcome::Changed;
        }
        let d = &*self.data;
        let parent = d.parent[drag.source as usize];
        TreeOutcome::Moved {
            node,
            parent: (parent != ROOT).then_some(NodeId(parent)),
            index: d.sibling_index(drag.source),
        }
    }

    fn key(&mut self, key: TreeKey) -> TreeOutcome {
        let d = &*self.data;
        if d.rows.is_empty() {
            return TreeOutcome::Unchanged;
        }
        let multi = d.mode == SelectionMode::Multi;
        let cursor_row = (d.cursor != NONE)
            .then(|| d.row_of[d.cursor as usize])
            .filter(|&r| r != NONE)
            .map(|r| r as usize);
        let key = match key {
            TreeKey::Extend(nav) | TreeKey::Move(nav) if !multi => TreeKey::Nav(nav),
            TreeKey::SelectAll if !multi => return TreeOutcome::Unchanged,
            key => key,
        };
        let outcome = match key {
            TreeKey::Nav(TreeNav::Left) => {
                let Some(row) = cursor_row else {
                    return self.key(TreeKey::Nav(TreeNav::Home));
                };
                let node = d.rows[row];
                if d.has(node, EXPANDED) && d.expandable(node) {
                    return self.collapse(NodeId(node));
                }
                let parent = d.parent[node as usize];
                if parent == ROOT {
                    return TreeOutcome::Unchanged;
                }
                self.select_only(NodeId(parent));
                TreeOutcome::Changed
            }
            TreeKey::Nav(TreeNav::Right) => {
                let Some(row) = cursor_row else {
                    return self.key(TreeKey::Nav(TreeNav::Home));
                };
                let node = d.rows[row];
                if !d.expandable(node) {
                    return TreeOutcome::Unchanged;
                }
                if !d.has(node, EXPANDED) {
                    return self.expand(NodeId(node));
                }
                let child = d.first_child[node as usize];
                if child == NONE {
                    return TreeOutcome::Unchanged;
                }
                self.select_only(NodeId(child));
                TreeOutcome::Changed
            }
            TreeKey::Nav(nav) => {
                let row = d.nav_row(nav, cursor_row);
                let node = d.rows[row];
                self.select_only(NodeId(node));
                TreeOutcome::Changed
            }
            TreeKey::Extend(nav) => {
                let row = d.nav_row(nav, cursor_row);
                let anchor = (d.anchor != NONE)
                    .then(|| d.row_of[d.anchor as usize])
                    .filter(|&r| r != NONE)
                    .map_or(row, |r| r as usize);
                let d = self.m();
                d.select_rows(anchor, row);
                d.cursor = d.rows[row];
                if d.anchor == NONE {
                    d.anchor = d.cursor;
                }
                TreeOutcome::Changed
            }
            TreeKey::Move(nav) => {
                let row = d.nav_row(nav, cursor_row);
                let d = self.m();
                d.cursor = d.rows[row];
                TreeOutcome::Changed
            }
            TreeKey::ToggleSelect => {
                let Some(row) = cursor_row else {
                    return TreeOutcome::Unchanged;
                };
                let node = d.rows[row];
                let d = self.m();
                if multi {
                    let on = !d.has(node, SELECTED);
                    d.set_selected(node, on);
                } else {
                    d.select_only(node);
                }
                d.anchor = node;
                TreeOutcome::Changed
            }
            TreeKey::SelectAll => {
                let last = d.rows.len() - 1;
                self.m().select_rows(0, last);
                TreeOutcome::Changed
            }
            TreeKey::Activate => {
                return match cursor_row {
                    Some(row) => TreeOutcome::Activated(NodeId(d.rows[row])),
                    None => TreeOutcome::Unchanged,
                };
            }
        };
        self.reveal_cursor();
        outcome
    }

    /// Scroll so the cursor row is inside the viewport.
    fn reveal_cursor(&mut self) {
        self.ensure_rows();
        let d = &*self.data;
        if d.cursor == NONE || d.row_of[d.cursor as usize] == NONE {
            return;
        }
        let top = d.row_of[d.cursor as usize] as f32 * d.row_height;
        let bottom = top + d.row_height;
        let scroll = if top < d.scroll {
            top
        } else if bottom > d.scroll + d.viewport_height {
            bottom - d.viewport_height
        } else {
            return;
        };
        let d = self.m();
        d.scroll = scroll;
        d.clamp_scroll();
    }

    fn type_ahead(&mut self, ch: char, now_ms: u64) -> TreeOutcome {
        let d = Rc::make_mut(&mut self.data);
        if now_ms.saturating_sub(d.type_ahead_at) > TYPE_AHEAD_RESET_MS {
            d.type_ahead.clear();
        }
        d.type_ahead_at = now_ms;
        d.type_ahead.push(ch.to_ascii_lowercase());
        let len = d.rows.len();
        let cursor_row = (d.cursor != NONE)
            .then(|| d.row_of[d.cursor as usize])
            .filter(|&r| r != NONE)
            .map(|r| r as usize);
        // A fresh prefix looks past the cursor so repeating a letter walks
        // through the matches; a longer one may stay on the cursor row.
        let start = match cursor_row {
            Some(row) if d.type_ahead.len() == 1 => row + 1,
            Some(row) => row,
            None => 0,
        };
        let prefix = d.type_ahead.as_bytes();
        let found = (0..len).map(|i| (start + i) % len).find(|&row| {
            let label = d.labels[d.rows[row] as usize].as_bytes();
            label.len() >= prefix.len() && label[..prefix.len()].eq_ignore_ascii_case(prefix)
        });
        let Some(row) = found else {
            return TreeOutcome::Unchanged;
        };
        let node = d.rows[row];
        d.select_only(node);
        d.cursor = node;
        d.anchor = node;
        self.reveal_cursor();
        TreeOutcome::Changed
    }

    /// Checks every link, count, flag, and row. O(n).
    pub fn verify_integrity(&self) -> Result<(), TreeIntegrityError> {
        self.data.verify_integrity()
    }
}

fn changed(changed: bool) -> TreeOutcome {
    if changed {
        TreeOutcome::Changed
    } else {
        TreeOutcome::Unchanged
    }
}

impl TreeData {
    fn has(&self, node: u32, flag: u8) -> bool {
        self.flags[node as usize] & flag != 0
    }

    fn set_flag(&mut self, node: u32, flag: u8, on: bool) {
        let flags = &mut self.flags[node as usize];
        if on {
            *flags |= flag;
        } else {
            *flags &= !flag;
        }
    }

    fn expandable(&self, node: u32) -> bool {
        self.child_count[node as usize] > 0 || self.has(node, LAZY)
    }

    fn push_node(&mut self, parent: u32, label: Arc<str>) -> u32 {
        let id = self.parent.len() as u32;
        self.parent.push(NONE);
        self.first_child.push(NONE);
        self.last_child.push(NONE);
        self.next_sibling.push(NONE);
        self.prev_sibling.push(NONE);
        self.child_count.push(0);
        self.labels.push(label);
        self.icons.push(None);
        self.flags.push(0);
        self.row_of.push(NONE);
        self.link_last(parent, id);
        self.rows_dirty = true;
        id
    }

    fn unlink(&mut self, n: u32) {
        let (p, prev, next) = (
            self.parent[n as usize],
            self.prev_sibling[n as usize],
            self.next_sibling[n as usize],
        );
        if prev != NONE {
            self.next_sibling[prev as usize] = next;
        } else {
            self.first_child[p as usize] = next;
        }
        if next != NONE {
            self.prev_sibling[next as usize] = prev;
        } else {
            self.last_child[p as usize] = prev;
        }
        self.child_count[p as usize] -= 1;
        self.parent[n as usize] = NONE;
        self.prev_sibling[n as usize] = NONE;
        self.next_sibling[n as usize] = NONE;
    }

    fn link_last(&mut self, p: u32, n: u32) {
        let prev = self.last_child[p as usize];
        self.parent[n as usize] = p;
        self.prev_sibling[n as usize] = prev;
        self.next_sibling[n as usize] = NONE;
        if prev != NONE {
            self.next_sibling[prev as usize] = n;
        } else {
            self.first_child[p as usize] = n;
        }
        self.last_child[p as usize] = n;
        self.child_count[p as usize] += 1;
    }

    fn link_before(&mut self, target: u32, n: u32) {
        let p = self.parent[target as usize];
        let prev = self.prev_sibling[target as usize];
        self.parent[n as usize] = p;
        self.prev_sibling[n as usize] = prev;
        self.next_sibling[n as usize] = target;
        self.prev_sibling[target as usize] = n;
        if prev != NONE {
            self.next_sibling[prev as usize] = n;
        } else {
            self.first_child[p as usize] = n;
        }
        self.child_count[p as usize] += 1;
    }

    fn sibling_index(&self, n: u32) -> usize {
        std::iter::successors(Some(n), |&s| {
            let prev = self.prev_sibling[s as usize];
            (prev != NONE).then_some(prev)
        })
        .count()
            - 1
    }

    /// Whether `node` is strictly inside `ancestor`.
    fn is_descendant(&self, node: u32, ancestor: u32) -> bool {
        let mut at = self.parent[node as usize];
        while at != NONE {
            if at == ancestor {
                return true;
            }
            at = self.parent[at as usize];
        }
        false
    }

    fn can_drop(&self, node: u32, target: DropTarget) -> bool {
        let len = self.parent.len() as u32;
        let t = target.node.0;
        node != ROOT
            && t != ROOT
            && node < len
            && t < len
            && node != t
            && !self.is_descendant(t, node)
    }

    fn move_node(&mut self, node: u32, target: DropTarget) {
        let t = target.node.0;
        self.unlink(node);
        match target.position {
            DropPosition::Before => self.link_before(t, node),
            DropPosition::After => match self.next_sibling[t as usize] {
                NONE => self.link_last(self.parent[t as usize], node),
                next => self.link_before(next, node),
            },
            DropPosition::Inside => {
                self.link_last(t, node);
                self.set_flag(t, EXPANDED, true);
            }
        }
        self.rows_dirty = true;
    }

    /// Where a drag from `drag.source` lands once the pointer moved `dy`:
    /// the row under the dragged row's center, split into before, inside,
    /// and after bands.
    fn drop_at(&self, drag: Drag, dy: f32) -> Option<DropTarget> {
        let source_row = self.row_of[drag.source as usize];
        if source_row == NONE || self.rows.is_empty() {
            return None;
        }
        let h = self.row_height;
        let center = source_row as f32 * h + h / 2.0 + dy + (self.scroll - drag.scroll_at_press);
        let last = self.rows.len() - 1;
        let (row, position) = if center < 0.0 {
            (0, DropPosition::Before)
        } else if center >= self.rows.len() as f32 * h {
            (last, DropPosition::After)
        } else {
            let row = ((center / h) as usize).min(last);
            let frac = center / h - row as f32;
            let position = if frac < 0.25 {
                DropPosition::Before
            } else if frac > 0.75 {
                DropPosition::After
            } else {
                DropPosition::Inside
            };
            (row, position)
        };
        let target = DropTarget {
            node: NodeId(self.rows[row]),
            position,
        };
        self.can_drop(drag.source, target).then_some(target)
    }

    fn clamp_scroll(&mut self) {
        let total = self.rows.len() as f32 * self.row_height;
        let max = (total - self.viewport_height).max(0.0);
        self.scroll = self.scroll.clamp(0.0, max);
    }

    fn nav_row(&self, nav: TreeNav, from: Option<usize>) -> usize {
        let last = self.rows.len() - 1;
        let page = ((self.viewport_height / self.row_height) as usize).max(1);
        match (nav, from) {
            (TreeNav::End, _) => last,
            (TreeNav::Home, _) | (_, None) => 0,
            (TreeNav::Up | TreeNav::Left, Some(r)) => r.saturating_sub(1),
            (TreeNav::Down | TreeNav::Right, Some(r)) => (r + 1).min(last),
            (TreeNav::PageUp, Some(r)) => r.saturating_sub(page),
            (TreeNav::PageDown, Some(r)) => (r + page).min(last),
        }
    }

    fn set_selected(&mut self, node: u32, on: bool) {
        if self.has(node, SELECTED) == on {
            return;
        }
        self.set_flag(node, SELECTED, on);
        if on {
            self.selected.push(node);
        } else if let Some(at) = self.selected.iter().position(|&n| n == node) {
            self.selected.remove(at);
        }
    }

    fn clear_selection(&mut self) {
        for i in 0..self.selected.len() {
            let n = self.selected[i];
            self.set_flag(n, SELECTED, false);
        }
        self.selected.clear();
    }

    fn select_only(&mut self, node: u32) {
        self.clear_selection();
        self.set_selected(node, true);
    }

    fn select_rows(&mut self, a: usize, b: usize) {
        self.clear_selection();
        for row in a.min(b)..=a.max(b) {
            self.set_selected(self.rows[row], true);
        }
    }

    /// Rebuild the row columns from the expanded nodes.
    fn flatten(&mut self) {
        self.rows.clear();
        self.row_depth.clear();
        self.row_pos.clear();
        self.row_of.clear();
        self.row_of.resize(self.parent.len(), NONE);
        // Positions of the ancestors of `node` among their siblings.
        self.scratch.clear();
        let mut node = self.first_child[ROOT as usize];
        let mut depth: u16 = 0;
        let mut pos: u32 = 1;
        while node != NONE {
            self.row_of[node as usize] = self.rows.len() as u32;
            self.rows.push(node);
            self.row_depth.push(depth);
            self.row_pos.push(pos);
            let child = self.first_child[node as usize];
            if self.has(node, EXPANDED) && child != NONE {
                self.scratch.push(pos);
                node = child;
                depth += 1;
                pos = 1;
                continue;
            }
            loop {
                let next = self.next_sibling[node as usize];
                if next != NONE {
                    node = next;
                    pos += 1;
                    break;
                }
                let parent = self.parent[node as usize];
                if parent == ROOT {
                    node = NONE;
                    break;
                }
                node = parent;
                depth -= 1;
                pos = self.scratch.pop().expect("ancestor position");
            }
        }
        self.rows_dirty = false;
        self.drop = None;
        self.clamp_scroll();
        self.debug_check();
    }

    fn debug_check(&self) {
        debug_assert_eq!(self.verify_integrity(), Ok(()));
    }

    /// The links around one node agree with each other.
    fn verify_node(&self, n: u32) -> Result<(), TreeIntegrityError> {
        let i = n as usize;
        let link = |what| Err(TreeIntegrityError::Link { node: n, what });
        let p = self.parent[i];
        if p == NONE || p as usize >= self.parent.len() {
            return link("parent");
        }
        let prev = self.prev_sibling[i];
        let next = self.next_sibling[i];
        if prev == NONE {
            if self.first_child[p as usize] != n {
                return link("first child");
            }
        } else if self.next_sibling[prev as usize] != n || self.parent[prev as usize] != p {
            return link("previous sibling");
        }
        if next == NONE {
            if self.last_child[p as usize] != n {
                return link("last child");
            }
        } else if self.prev_sibling[next as usize] != n || self.parent[next as usize] != p {
            return link("next sibling");
        }
        Ok(())
    }

    fn verify_integrity(&self) -> Result<(), TreeIntegrityError> {
        let n = self.parent.len();
        count_integrity_steps(n);
        let lengths = [
            ("first_child", self.first_child.len()),
            ("last_child", self.last_child.len()),
            ("next_sibling", self.next_sibling.len()),
            ("prev_sibling", self.prev_sibling.len()),
            ("child_count", self.child_count.len()),
            ("labels", self.labels.len()),
            ("icons", self.icons.len()),
            ("flags", self.flags.len()),
            ("row_of", self.row_of.len()),
        ];
        if let Some(&(column, _)) = lengths.iter().find(|(_, len)| *len != n) {
            return Err(TreeIntegrityError::ColumnLength { column });
        }
        if self.parent[ROOT as usize] != NONE {
            return Err(TreeIntegrityError::Link {
                node: ROOT,
                what: "root parent",
            });
        }
        // Walk every child chain from the root. Reaching more nodes than
        // exist means a cycle; fewer means a detached node.
        let mut reached = 0usize;
        let mut stack = vec![ROOT];
        while let Some(p) = stack.pop() {
            let mut count = 0u32;
            let mut prev = NONE;
            let mut c = self.first_child[p as usize];
            while c != NONE {
                reached += 1;
                if reached >= n {
                    return Err(TreeIntegrityError::Unreachable { reached, nodes: n });
                }
                if self.parent[c as usize] != p {
                    return Err(TreeIntegrityError::Link {
                        node: c,
                        what: "parent",
                    });
                }
                if self.prev_sibling[c as usize] != prev {
                    return Err(TreeIntegrityError::Link {
                        node: c,
                        what: "previous sibling",
                    });
                }
                count += 1;
                stack.push(c);
                prev = c;
                c = self.next_sibling[c as usize];
            }
            if self.last_child[p as usize] != prev {
                return Err(TreeIntegrityError::Link {
                    node: p,
                    what: "last child",
                });
            }
            if self.child_count[p as usize] != count {
                return Err(TreeIntegrityError::ChildCount {
                    node: p,
                    counted: count,
                    stored: self.child_count[p as usize],
                });
            }
        }
        if reached != n - 1 {
            return Err(TreeIntegrityError::Unreachable { reached, nodes: n });
        }
        let flagged = self.flags.iter().filter(|f| *f & SELECTED != 0).count();
        if flagged != self.selected.len() || self.selected.iter().any(|&s| !self.has(s, SELECTED)) {
            return Err(TreeIntegrityError::Selection {
                flagged,
                listed: self.selected.len(),
            });
        }
        if !self.rows_dirty {
            self.verify_rows()?;
        }
        Ok(())
    }

    /// The row columns are the depth-first order of expanded nodes, with
    /// matching depths, positions, and inverse map.
    fn verify_rows(&self) -> Result<(), TreeIntegrityError> {
        let mut expected = Vec::new();
        // (node, depth, position) in reverse so pops come out in order.
        let mut stack: Vec<(u32, u16, u32)> = Vec::new();
        let push_children = |stack: &mut Vec<(u32, u16, u32)>, p: u32, depth: u16| {
            let mut kids = Vec::new();
            let mut c = self.first_child[p as usize];
            while c != NONE {
                kids.push(c);
                c = self.next_sibling[c as usize];
            }
            for (i, &k) in kids.iter().enumerate().rev() {
                stack.push((k, depth, i as u32 + 1));
            }
        };
        push_children(&mut stack, ROOT, 0);
        while let Some((node, depth, pos)) = stack.pop() {
            expected.push((node, depth, pos));
            if self.has(node, EXPANDED) {
                push_children(&mut stack, node, depth + 1);
            }
        }
        if expected.len() != self.rows.len()
            || self.row_depth.len() != self.rows.len()
            || self.row_pos.len() != self.rows.len()
        {
            return Err(TreeIntegrityError::Rows {
                row: self.rows.len().min(expected.len()),
                what: "count",
            });
        }
        for (row, &(node, depth, pos)) in expected.iter().enumerate() {
            let what = if self.rows[row] != node {
                "node"
            } else if self.row_depth[row] != depth {
                "depth"
            } else if self.row_pos[row] != pos {
                "position"
            } else if self.row_of[node as usize] != row as u32 {
                "row map"
            } else {
                continue;
            };
            return Err(TreeIntegrityError::Rows { row, what });
        }
        let mapped = self.row_of.iter().filter(|&&r| r != NONE).count();
        if mapped != self.rows.len() {
            return Err(TreeIntegrityError::Rows {
                row: mapped,
                what: "hidden node mapped to a row",
            });
        }
        Ok(())
    }
}

/// A broken invariant of a [`TreeState`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeIntegrityError {
    ColumnLength {
        column: &'static str,
    },
    Link {
        node: u32,
        what: &'static str,
    },
    ChildCount {
        node: u32,
        counted: u32,
        stored: u32,
    },
    /// Walking from the root reached `reached` of `nodes` (a cycle when
    /// more, a detached node when fewer).
    Unreachable {
        reached: usize,
        nodes: usize,
    },
    Selection {
        flagged: usize,
        listed: usize,
    },
    Rows {
        row: usize,
        what: &'static str,
    },
}

impl fmt::Display for TreeIntegrityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "tree integrity: {self:?}")
    }
}

impl std::error::Error for TreeIntegrityError {}

// ---------------------------------------------------------------------------
// View
// ---------------------------------------------------------------------------

/// Key bindings of a focused tree.
const KEYS: &[(&str, TreeKey)] = &[
    ("arrowup", TreeKey::Nav(TreeNav::Up)),
    ("arrowdown", TreeKey::Nav(TreeNav::Down)),
    ("arrowleft", TreeKey::Nav(TreeNav::Left)),
    ("arrowright", TreeKey::Nav(TreeNav::Right)),
    ("home", TreeKey::Nav(TreeNav::Home)),
    ("end", TreeKey::Nav(TreeNav::End)),
    ("pageup", TreeKey::Nav(TreeNav::PageUp)),
    ("pagedown", TreeKey::Nav(TreeNav::PageDown)),
    ("shift+arrowup", TreeKey::Extend(TreeNav::Up)),
    ("shift+arrowdown", TreeKey::Extend(TreeNav::Down)),
    ("shift+home", TreeKey::Extend(TreeNav::Home)),
    ("shift+end", TreeKey::Extend(TreeNav::End)),
    ("shift+pageup", TreeKey::Extend(TreeNav::PageUp)),
    ("shift+pagedown", TreeKey::Extend(TreeNav::PageDown)),
    ("mod+arrowup", TreeKey::Move(TreeNav::Up)),
    ("mod+arrowdown", TreeKey::Move(TreeNav::Down)),
    ("space", TreeKey::ToggleSelect),
    ("mod+space", TreeKey::ToggleSelect),
    ("mod+a", TreeKey::SelectAll),
    ("enter", TreeKey::Activate),
];

/// Theme colors a tree paints with, copied so cached closures own them.
#[derive(Debug, Clone, Copy)]
struct TreeColors {
    selected: Color,
    hover: Color,
    text: Color,
    icon: Color,
    guide: Color,
    accent: Color,
    focus: Color,
}

impl TreeColors {
    fn of(theme: &Theme) -> Self {
        let c = &theme.colors;
        Self {
            selected: c.sidebar_row_selected,
            hover: c.sidebar_row_hover,
            text: c.text,
            icon: c.icon,
            guide: c.border_variant,
            accent: c.accent,
            focus: c.focus_border,
        }
    }
}

/// Everything one row's build closure reads besides its label and icon.
#[derive(Debug, Clone, Copy, PartialEq, Hash)]
struct RowSpec {
    node: u32,
    depth: u16,
    pos: u32,
    set_size: u32,
    expandable: bool,
    expanded: bool,
    selected: bool,
    cursor: bool,
    drop: Option<DropPosition>,
    accessible: bool,
    row_height: u32,
    indent: u32,
}

/// The tree in `state` at the state's viewport height and the parent's
/// width. Emits events through `on_event`; see [`TreeState::handle`].
pub fn tree_view(
    state: &mut TreeState,
    theme: &Theme,
    env: CollectionEnv,
    on_event: fn(TreeEvent) -> Action,
) -> AnyElement {
    state.ensure_rows();
    let data = state.data.clone();
    let colors = TreeColors::of(theme);
    let height = data.viewport_height;
    let hash = inputs_hash(&(data.revision, env, on_event as usize));
    view! {
        <cached(data.id, hash, move || build_tree(&data, colors, env, on_event))
                class="w-full" h={height} />
    }
}

fn build_tree(
    d: &TreeData,
    colors: TreeColors,
    env: CollectionEnv,
    on_event: fn(TreeEvent) -> Action,
) -> AnyElement {
    let window = virtual_list_window(
        d.rows.len(),
        d.scroll,
        d.viewport_height,
        d.row_height,
        0.0,
        OVERSCAN,
    );
    view! {
        <div class="w-full" h={d.viewport_height} class="flex-col" track_focus={d.focus}
             scroll_y={d.scroll} scroll_total={window.total_extent}
             on:scroll={ScrollActionBuilder::new(move |lines| on_event(TreeEvent::Scroll(lines)))
                 .with_to_px(move |px| on_event(TreeEvent::ScrollTo(px as f32)))}
             @when {d.scrollbar_auto_hide} {
                 scrollbar_visibility={&d.scrollbar} class="scrollbar-auto-hide"
             }
             @when {env.accessible} {
                 accessibility_id={d.id} accessibility_role={Role::Tree} aria-label={d.label}
                 aria-multiselectable={d.mode == SelectionMode::Multi}
             }
             @for &(binding, key) in KEYS { on_key={(binding, on_event(TreeEvent::Key(key)))} }
             @for (i, ch) in TYPE_AHEAD_KEYS.char_indices() {
                 on_key={(&TYPE_AHEAD_KEYS[i..i + 1], on_event(TreeEvent::TypeAhead(ch)))}
             }>
            <div class="w-full shrink-0" h={window.top_spacer} />
            for row in window.range {
                {visible_row(d, row, colors, env, on_event)}
            }
            <div class="w-full shrink-0" h={window.bottom_spacer} />
        </div>
    }
}

/// Row `row` of the flattened tree.
fn visible_row(
    d: &TreeData,
    row: usize,
    colors: TreeColors,
    env: CollectionEnv,
    on_event: fn(TreeEvent) -> Action,
) -> AnyElement {
    let node = d.rows[row];
    let n = node as usize;
    let spec = RowSpec {
        node,
        depth: d.row_depth[row],
        pos: d.row_pos[row],
        set_size: d.child_count[d.parent[n] as usize],
        expandable: d.expandable(node),
        expanded: d.has(node, EXPANDED),
        selected: d.has(node, SELECTED),
        cursor: env.focused && d.cursor == node,
        drop: d
            .drop
            .filter(|t| t.node.0 == node)
            .map(|target| target.position),
        accessible: env.accessible,
        row_height: d.row_height.to_bits(),
        indent: d.indent.to_bits(),
    };
    tree_row(
        d.id,
        spec,
        d.labels[n].clone(),
        d.icons[n],
        colors,
        on_event,
    )
}

fn tree_row(
    id: &'static str,
    spec: RowSpec,
    label: Arc<str>,
    icon: Option<&'static str>,
    colors: TreeColors,
    on_event: fn(TreeEvent) -> Action,
) -> AnyElement {
    let key = quark::stable_hash(id) ^ u64::from(spec.node).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let hash = inputs_hash(&(
        spec,
        &*label,
        icon.map(|s| s.as_ptr() as usize),
        on_event as usize,
    ));
    let build = move || {
        let node = NodeId(spec.node);
        let h = f32::from_bits(spec.row_height);
        let indent = f32::from_bits(spec.indent);
        let depth = f32::from(spec.depth);
        view! {
            <div class="w-full" h={h} class="shrink-0 flex-row items-center relative" gap={4.0}
                 pr={8.0} on:click={on_event(TreeEvent::Press(node))}
                 on:drag={move |press: ClickEvent| {
                     Box::new(RowDrag {
                         node,
                         press_y: press.y,
                         on_event,
                     }) as Box<dyn DragHandler>
                 }}
                 @when {spec.selected} { bg={colors.selected} }
                 @when {!spec.selected} { hover_bg={colors.hover} }
                 @when {spec.cursor} { border={colors.focus} }
                 @when {spec.accessible} {
                     accessibility_id={format!("{id}.item.{}", spec.node)}
                     accessibility_role={Role::TreeItem} aria-label={&*label}
                     aria-level={usize::from(spec.depth) + 1}
                     accessibility_position_in_set={(spec.pos as usize, spec.set_size as usize)}
                     aria-selected={spec.selected}
                     @when {spec.expandable} { aria-expanded={spec.expanded} }
                 }>
                // Indent guides: one hairline per ancestor level, through the
                // chevron column of that level.
                for level in 0..spec.depth {
                    <div class="absolute top-0" left={f32::from(level) * indent + 4.0 + 8.0}
                         w={1.0} h={h} bg={colors.guide} />
                }
                <div w={depth * indent} h={h} class="shrink-0" />
                <div w={16.0} h={16.0} class="shrink-0 items-center justify-center"
                     @when {spec.expandable} { on:click={on_event(TreeEvent::Toggle(node))} }>
                    if spec.expandable {
                        <icon svg={if spec.expanded { lucide::CHEVRON_DOWN } else { lucide::CHEVRON_RIGHT }}
                              size={12.0} color={colors.icon} />
                    }
                </div>
                if let Some(svg) = icon {
                    <icon svg={svg} size={14.0} color={colors.icon} />
                }
                <text class="text-sm" color={colors.text} class="truncate">{&*label}</text>
                match spec.drop {
                    Some(DropPosition::Before) => {
                        <div class="absolute" left={depth * indent + 4.0} class="right-0"
                             h={2.0} bg={colors.accent} class="top-0" />
                    }
                    Some(DropPosition::After) => {
                        <div class="absolute" left={depth * indent + 4.0} class="right-0"
                             h={2.0} bg={colors.accent} class="bottom-0" />
                    }
                    Some(DropPosition::Inside) => {
                        <div class="absolute inset-0" rounded={4.0} border={colors.accent} />
                    }
                    None => {}
                }
            </div>
        }
    };
    view! {
        <cached(CacheKey(key), hash, build) class="w-full" h={f32::from_bits(spec.row_height)} />
    }
}

/// A press on a row: selects on press, then reports pointer travel so
/// the state can pick a drop target once it passes the threshold.
struct RowDrag {
    node: NodeId,
    press_y: f32,
    on_event: fn(TreeEvent) -> Action,
}

impl DragHandler for RowDrag {
    fn on_press(&mut self) -> Vec<Action> {
        vec![(self.on_event)(TreeEvent::Press(self.node))]
    }

    fn on_move(&mut self, _x: f32, y: f32) -> Vec<Action> {
        vec![(self.on_event)(TreeEvent::DragMove {
            dy: y - self.press_y,
        })]
    }

    fn on_release(&mut self) -> DragReleaseResult {
        DragReleaseResult {
            actions: vec![(self.on_event)(TreeEvent::DragEnd)],
        }
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[derive(Debug, Clone)]
    enum Op {
        Add(u32),
        Move(u32, u32, u8),
        Toggle(u32),
        Key(u8),
    }

    fn op() -> impl Strategy<Value = Op> {
        prop_oneof![
            (0..64u32).prop_map(Op::Add),
            (0..64u32, 0..64u32, 0..3u8).prop_map(|(n, t, p)| Op::Move(n, t, p)),
            (0..64u32).prop_map(Op::Toggle),
            (0..6u8).prop_map(Op::Key),
        ]
    }

    proptest! {
        // Moves, expansion, and navigation in any order keep every link,
        // count, and row consistent, never lose a node, and refuse exactly
        // the moves into the node itself or its subtree.
        #[test]
        fn random_edits_keep_the_tree_consistent(ops in prop::collection::vec(op(), 1..80)) {
            let mut tree = TreeState::new("t", FocusId::from_key("t"));
            tree.add(None, "root");
            for op in ops {
                let pick = |i: u32| tree.node(1 + i % tree.len() as u32).unwrap();
                match op {
                    Op::Add(p) => {
                        let parent = pick(p);
                        tree.add(Some(parent), "n");
                    }
                    Op::Move(n, t, p) => {
                        let (node, target) = (pick(n), pick(t));
                        let into_itself = node == target || tree.data.is_descendant(target.0, node.0);
                        let position = [DropPosition::Before, DropPosition::Inside, DropPosition::After][p as usize];
                        let moved = tree.move_node(node, DropTarget { node: target, position });
                        prop_assert_eq!(moved, !into_itself);
                    }
                    Op::Toggle(n) => {
                        let node = pick(n);
                        tree.toggle(node);
                    }
                    Op::Key(k) => {
                        let nav = [TreeNav::Up, TreeNav::Down, TreeNav::Left, TreeNav::Right, TreeNav::Home, TreeNav::End][k as usize];
                        tree.handle(TreeEvent::Key(TreeKey::Extend(nav)), SelectMods::default(), 0);
                        tree.handle(TreeEvent::Key(TreeKey::Nav(nav)), SelectMods::default(), 0);
                    }
                }
                tree.ensure_rows();
                prop_assert_eq!(tree.verify_integrity(), Ok(()));
            }
        }
    }
}
