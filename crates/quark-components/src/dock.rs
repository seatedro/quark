//! Docked regions of tabbed panels around a center, built on [`Split`].
//!
//! A [`DockState`] has four regions: left, right, and bottom docks that can
//! be resized and hidden, and a center that fills the rest. Each region
//! holds a [`PaneNode`] tree of tab groups, each an ordered list of
//! app-defined [`PanelId`]s with one active. Click a tab to select and
//! focus it, close it with its close button or a middle click, or move
//! between tabs with the arrow keys. The dividers between split groups
//! move with the pointer, the arrow keys (Shift for larger steps), or
//! assistive tech setting their value.
//!
//! Tabs drag anywhere: into a group's tab strip to reorder or move them,
//! onto a group's body to join it, or onto one of its edges to split the
//! group and open the tab beside it. While a tab is dragged it follows the
//! pointer and the dock shows where it would land; a cancelled drag
//! (Escape through the app, or the window losing focus) leaves it where it
//! was. Apps constrain this per region with a [`TabPolicy`] (a region
//! whose tabs stay in it, or that takes no tabs from elsewhere) and per
//! panel with [`DockState::confine`].
//!
//! The dock knows nothing about what panels are: the app gives each one a
//! title and builds the active ones' content. Like [`Split`], it emits
//! [`DockEvent`]s through a caller supplied mapping, and the app passes them
//! back to [`DockState::apply`]. [`DockState::snapshot`] persists sizes,
//! visibility, and the tab groups.

use std::rc::Rc;

use accesskit::Role;
use quark::{TabStop, view};
use quark_ui::accessibility::{NumericActions, NumericValue, Orientation};
use quark_ui::design::Shadow;
use quark_ui::element::{
    AnyElement, ClickEvent, CursorHint, DragHandler, DragPreview, DragReleaseResult,
    ElementGeometry, ElementHandle, IntoAnyElement, LayoutSnapshot, div, svg_icon, text,
};
use quark_ui::icons::lucide;
use quark_ui::style::Styled;
use quark_ui::theme::{Color, Theme};
use quark_ui::{Action, FocusId};
use serde::{Deserialize, Serialize};

use crate::pane_tree::{
    DropZone, PaneDrop, PaneId, PaneNode, PaneSplit, Rect, TabGroup, child_sizes, divider_min,
    divider_span, push_divider,
};
use crate::split::{
    Axis, DIVIDER_THICKNESS, NUDGE_STEP, NUDGE_STEP_LARGE, Pane, Split, SplitEvent, SplitSnapshot,
    SplitState,
};

/// An app-chosen panel identity. Persisted, so keep values stable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PanelId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DockRegion {
    Left,
    Right,
    Bottom,
    Center,
}

impl DockRegion {
    pub const ALL: [DockRegion; 4] = [Self::Left, Self::Right, Self::Bottom, Self::Center];

    fn index(self) -> usize {
        self as usize
    }

    /// Which split holds the region, and its pane there.
    fn place(self) -> (DockSplit, usize) {
        match self {
            Self::Left => (DockSplit::Columns, 0),
            Self::Right => (DockSplit::Columns, 2),
            Self::Center => (DockSplit::Rows, 0),
            Self::Bottom => (DockSplit::Rows, 1),
        }
    }
}

/// The dock's two splits: left | middle | right, and inside the middle,
/// center over bottom.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DockSplit {
    Columns,
    Rows,
}

/// Which tab moves a region allows across its boundary. Moves inside a
/// region, splits included, are always allowed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TabPolicy {
    /// Its tabs may be dragged into other regions.
    pub can_leave: bool,
    /// Tabs from other regions may be dropped into it.
    pub accepts: bool,
}

impl TabPolicy {
    pub const OPEN: Self = Self {
        can_leave: true,
        accepts: true,
    };
    /// Tabs neither leave nor enter.
    pub const SEALED: Self = Self {
        can_leave: false,
        accepts: false,
    };
}

impl Default for TabPolicy {
    fn default() -> Self {
        Self::OPEN
    }
}

/// A divider between two children of a split inside a region. Every move
/// shrinks the groups ahead of the divider, nearest first, down to a
/// minimum of 100 points (less when the split cannot give each group that
/// much), and grows the one behind it by as much.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PaneDividerEvent {
    Press,
    /// Pointer moved `delta` points along the split's axis since the press;
    /// `extent` is the split's length.
    Drag {
        delta: f32,
        extent: f32,
    },
    Release,
    /// Keyboard resize by `delta` points along the axis, right or down
    /// positive.
    Nudge {
        delta: f32,
        extent: f32,
    },
    /// Move the divider to `position` points from the split's start, as
    /// near as the groups' minimums allow: assistive tech setting the
    /// divider's value.
    SetPosition {
        position: f32,
        extent: f32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DockEvent {
    Split(DockSplit, SplitEvent),
    PaneDivider {
        split: PaneId,
        divider: usize,
        event: PaneDividerEvent,
    },
    Select {
        pane: PaneId,
        index: usize,
    },
    Close {
        pane: PaneId,
        index: usize,
    },
    /// Hide or show a side region.
    Toggle(DockRegion),
    /// A dragged tab is over `target`, or over nowhere it can land.
    TabHover {
        panel: PanelId,
        target: Option<PaneDrop>,
    },
    /// A dragged tab was released over `target`.
    TabDrop {
        panel: PanelId,
        target: Option<PaneDrop>,
    },
    /// Move `panel` into the group `destination`, before tab `index`, or
    /// at the end for `None`. Checked again when applied, under the same
    /// rules as a drop; see [`DockState::move_tab`].
    MoveTab {
        panel: PanelId,
        destination: PaneId,
        index: Option<usize>,
    },
}

/// What [`DockState::apply_event`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DockOutcome {
    /// The change is settled and worth persisting.
    pub settled: bool,
    /// Where a [`DockEvent::MoveTab`] put its tab; `None` when it was
    /// refused or the event was another kind.
    pub moved: Option<TabMove>,
    /// The group a [`DockEvent::Select`] (a tab clicked, pressed to drag,
    /// or activated by assistive tech) made a tab active in.
    pub selected: Option<PaneId>,
}

impl DockOutcome {
    /// Where keyboard focus belongs after the event: on a moved tab, in its
    /// new group, or on a selected one. Pass it to the app's focus when it
    /// is `Some`.
    pub fn focus(&self) -> Option<FocusId> {
        self.moved
            .map(|m| m.pane)
            .or(self.selected)
            .map(Dock::tab_focus)
    }
}

/// A tab [`DockEvent::MoveTab`] moved, and the group it is now active in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TabMove {
    pub panel: PanelId,
    pub region: DockRegion,
    pub pane: PaneId,
}

/// Sizes of the three side regions and the center's minimum. Labels name
/// the regions for assistive tech.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DockLayout {
    pub left: Pane,
    pub right: Pane,
    pub bottom: Pane,
    pub center_label: &'static str,
    pub center_min_width: f32,
    pub center_min_height: f32,
}

impl Default for DockLayout {
    fn default() -> Self {
        Self {
            left: Pane::fixed("Left dock", 260.0).min(160.0).max(520.0),
            right: Pane::fixed("Right dock", 420.0).min(240.0).max(900.0),
            bottom: Pane::fixed("Bottom dock", 240.0).min(100.0).max(700.0),
            center_label: "Center",
            center_min_width: 320.0,
            center_min_height: 160.0,
        }
    }
}

/// The persisted part of a [`DockState`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DockSnapshot {
    pub columns: SplitSnapshot,
    pub rows: SplitSnapshot,
    pub left: PaneNode,
    pub right: PaneNode,
    pub bottom: PaneNode,
    pub center: PaneNode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DockIntegrityError {
    /// A panel shows in more than one tab.
    DuplicatePanel(PanelId),
    /// Two nodes share an id, or one is not below the next fresh id.
    PaneIds,
    /// An empty group below a region's root.
    EmptyGroup(PaneId),
    /// A split with fewer than two children, weights that do not match
    /// them or do not sum to one, or a child split of the same axis.
    SplitShape(PaneId),
    /// A group's active index is past its tabs.
    Active(PaneId),
}

/// Smallest a group gets when a divider between groups is dragged.
const MIN_GROUP: f32 = 100.0;

/// The regions in the dock's tree order: its columns split holds the left
/// region, the middle (center over bottom), and the right region.
const TREE_ORDER: [DockRegion; 4] = [
    DockRegion::Left,
    DockRegion::Center,
    DockRegion::Bottom,
    DockRegion::Right,
];

/// Default [`Dock::move_tab_keys`].
const MOVE_TAB_KEYS: (&str, &str) = ("mod+shift+pageup", "mod+shift+pagedown");

/// Where a tab being dragged came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Origin {
    region: DockRegion,
    pane: PaneId,
    index: usize,
    count: usize,
}

/// Whether a tab from `from` may land on `target` in `to`. The single rule
/// both [`DockState::can_drop`] and a drag in progress use.
fn drop_allowed(
    policies: &[TabPolicy; 4],
    confined: bool,
    from: Origin,
    to: DockRegion,
    target: PaneDrop,
) -> bool {
    if from.region != to
        && (confined || !policies[from.region.index()].can_leave || !policies[to.index()].accepts)
    {
        return false;
    }
    if from.pane != target.pane {
        return true;
    }
    match target.zone {
        DropZone::Center => false,
        DropZone::Tabs(i) => i.min(from.count - 1) != from.index,
        // Splitting a group off its only tab would leave it empty.
        _ => from.count > 1,
    }
}

/// The drop a [`DockEvent::MoveTab`] makes.
fn move_drop(pane: PaneId, index: Option<usize>) -> PaneDrop {
    PaneDrop {
        pane,
        zone: index.map_or(DropZone::Center, DropZone::Tabs),
    }
}

/// Panels, sizes, and visibility of a dock, owned by the app.
#[derive(Debug, Clone, PartialEq)]
pub struct DockState {
    columns: SplitState,
    rows: SplitState,
    roots: [PaneNode; 4],
    labels: [&'static str; 4],
    policies: [TabPolicy; 4],
    confined: Vec<PanelId>,
    /// Per region, the group last selected or dropped into: where
    /// [`Self::open`] adds panels.
    recent: [PaneId; 4],
    next_id: u32,
    tab_drag: Option<(PanelId, Option<PaneDrop>)>,
    divider_drag: Option<(PaneId, usize, Vec<f32>)>,
}

impl DockState {
    pub fn new(layout: DockLayout) -> Self {
        let columns = SplitState::new(
            Axis::Horizontal,
            vec![
                layout.left.collapsible(),
                Pane::flex(layout.center_label).min(layout.center_min_width),
                layout.right.collapsible(),
            ],
        )
        .hide_collapsed_dividers(true);
        let rows = SplitState::new(
            Axis::Vertical,
            vec![
                Pane::flex(layout.center_label).min(layout.center_min_height),
                layout.bottom.collapsible(),
            ],
        )
        .hide_collapsed_dividers(true);
        let group = |id| PaneNode::Tabs(TabGroup::new(PaneId(id)));
        let mut state = Self {
            columns,
            rows,
            roots: [group(0), group(1), group(2), group(3)],
            labels: [
                layout.left.label,
                layout.right.label,
                layout.bottom.label,
                layout.center_label,
            ],
            policies: [TabPolicy::OPEN; 4],
            confined: Vec::new(),
            recent: [PaneId(0), PaneId(1), PaneId(2), PaneId(3)],
            next_id: 4,
            tab_drag: None,
            divider_drag: None,
        };
        // Empty side regions take no space until a panel arrives.
        for region in [DockRegion::Left, DockRegion::Right, DockRegion::Bottom] {
            state.set_hidden(region, true);
        }
        state
    }

    fn split(&self, which: DockSplit) -> &SplitState {
        match which {
            DockSplit::Columns => &self.columns,
            DockSplit::Rows => &self.rows,
        }
    }

    fn split_mut(&mut self, which: DockSplit) -> &mut SplitState {
        match which {
            DockSplit::Columns => &mut self.columns,
            DockSplit::Rows => &mut self.rows,
        }
    }

    fn set_hidden(&mut self, region: DockRegion, hidden: bool) {
        if region != DockRegion::Center {
            let (split, pane) = region.place();
            self.split_mut(split).set_collapsed(pane, hidden);
        }
    }

    fn fresh_id(&mut self) -> PaneId {
        self.next_id += 1;
        PaneId(self.next_id - 1)
    }

    pub fn label(&self, region: DockRegion) -> &'static str {
        self.labels[region.index()]
    }

    /// Constrain which tab moves cross `region`'s boundary.
    pub fn set_policy(&mut self, region: DockRegion, policy: TabPolicy) {
        self.policies[region.index()] = policy;
    }

    pub fn policy(&self, region: DockRegion) -> TabPolicy {
        self.policies[region.index()]
    }

    /// Keep `panel` in whichever region it is in: it can still be
    /// reordered and split off inside it.
    pub fn confine(&mut self, panel: PanelId, confined: bool) {
        self.confined.retain(|p| *p != panel);
        if confined {
            self.confined.push(panel);
        }
    }

    /// The region's tree of tab groups.
    pub fn root(&self, region: DockRegion) -> &PaneNode {
        &self.roots[region.index()]
    }

    pub fn group(&self, pane: PaneId) -> Option<&TabGroup> {
        self.roots.iter().find_map(|root| root.group(pane))
    }

    fn group_mut(&mut self, pane: PaneId) -> Option<&mut TabGroup> {
        self.roots.iter_mut().find_map(|root| root.group_mut(pane))
    }

    fn region_of(&self, pane: PaneId) -> Option<DockRegion> {
        DockRegion::ALL
            .into_iter()
            .find(|r| self.roots[r.index()].group(pane).is_some())
    }

    /// Where `panel` is: its region, group, and tab index.
    pub fn locate(&self, panel: PanelId) -> Option<(DockRegion, PaneId, usize)> {
        DockRegion::ALL.into_iter().find_map(|region| {
            self.roots[region.index()]
                .groups()
                .into_iter()
                .find_map(|g| {
                    let index = g.panels.iter().position(|p| *p == panel)?;
                    Some((region, g.id, index))
                })
        })
    }

    /// Every panel in a region, group by group.
    pub fn panels(&self, region: DockRegion) -> Vec<PanelId> {
        self.roots[region.index()]
            .groups()
            .into_iter()
            .flat_map(|g| g.panels.iter().copied())
            .collect()
    }

    /// The group [`Self::open`] adds to: the one last used, else the first.
    pub fn recent_group(&self, region: DockRegion) -> PaneId {
        let root = &self.roots[region.index()];
        let recent = self.recent[region.index()];
        if root.group(recent).is_some() {
            recent
        } else {
            root.groups()[0].id
        }
    }

    /// The active panel of the region's most recently used group.
    pub fn active(&self, region: DockRegion) -> Option<PanelId> {
        self.group(self.recent_group(region))
            .and_then(TabGroup::active_panel)
    }

    /// Shown with its content: has panels, and is not hidden.
    pub fn is_visible(&self, region: DockRegion) -> bool {
        if self.panels(region).is_empty() {
            return false;
        }
        let (split, pane) = region.place();
        region == DockRegion::Center || !self.split(split).is_collapsed(pane)
    }

    /// Size of a side region while shown, kept while hidden.
    pub fn size(&self, region: DockRegion) -> f32 {
        let (split, pane) = region.place();
        self.split(split).size(pane)
    }

    /// Show or hide a side region that has panels.
    pub fn set_visible(&mut self, region: DockRegion, visible: bool) {
        if !self.panels(region).is_empty() {
            self.set_hidden(region, !visible);
        }
    }

    pub fn toggle(&mut self, region: DockRegion) {
        self.set_visible(region, !self.is_visible(region));
    }

    /// Make `panel` active where it is, or add it at the end of the
    /// region's most recently used group. Shows its region.
    pub fn open(&mut self, region: DockRegion, panel: PanelId) {
        let (region, pane, index) = match self.locate(panel) {
            Some(found) => found,
            None => {
                let pane = self.recent_group(region);
                let group = self.group_mut(pane).expect("recent group exists");
                group.panels.push(panel);
                (region, pane, group.panels.len() - 1)
            }
        };
        self.select(pane, index);
        self.set_hidden(region, false);
        self.debug_verify();
    }

    pub fn select(&mut self, pane: PaneId, index: usize) {
        let Some(region) = self.region_of(pane) else {
            return;
        };
        if let Some(group) = self.group_mut(pane)
            && index < group.panels.len()
        {
            group.active = index;
            self.recent[region.index()] = pane;
        }
    }

    /// Remove a panel. The neighbor after it becomes active (or before it,
    /// at the end); an emptied group is removed, and a side region left
    /// empty hides.
    pub fn close(&mut self, pane: PaneId, index: usize) -> Option<PanelId> {
        let region = self.region_of(pane)?;
        let removed = self.group_mut(pane)?.remove(index)?;
        self.confined.retain(|p| *p != removed);
        self.settle(region);
        Some(removed)
    }

    /// Prune the region's tree after a removal and hide it when empty.
    fn settle(&mut self, region: DockRegion) {
        let i = region.index();
        let placeholder = PaneNode::Tabs(TabGroup::new(self.roots[i].groups()[0].id));
        let root = std::mem::replace(&mut self.roots[i], placeholder);
        if let Some(pruned) = root.prune() {
            self.roots[i] = pruned;
        }
        if self.panels(region).is_empty() {
            self.set_hidden(region, true);
        }
        self.debug_verify();
    }

    fn origin(&self, panel: PanelId) -> Option<Origin> {
        let (region, pane, index) = self.locate(panel)?;
        let count = self.group(pane)?.panels.len();
        Some(Origin {
            region,
            pane,
            index,
            count,
        })
    }

    /// Whether dropping `panel` on `target` would move it, under the
    /// regions' [`TabPolicy`]s and [`Self::confine`].
    pub fn can_drop(&self, panel: PanelId, target: PaneDrop) -> bool {
        let (Some(from), Some(to)) = (self.origin(panel), self.region_of(target.pane)) else {
            return false;
        };
        drop_allowed(
            &self.policies,
            self.confined.contains(&panel),
            from,
            to,
            target,
        )
    }

    /// Move `panel` to `target`: reorder it in its strip, move it into
    /// another group, or split `target`'s group and put it in a new group
    /// on that side. Returns false, changing nothing, when
    /// [`Self::can_drop`] refuses.
    pub fn drop_panel(&mut self, panel: PanelId, target: PaneDrop) -> bool {
        if !self.can_drop(panel, target) {
            return false;
        }
        let (Some(from), Some(to)) = (self.origin(panel), self.region_of(target.pane)) else {
            return false;
        };
        if let Some(group) = self.group_mut(from.pane) {
            group.remove(from.index);
        }
        match target.zone.edge() {
            None => {
                let index = match target.zone {
                    DropZone::Tabs(i) => i,
                    _ => usize::MAX,
                };
                if let Some(group) = self.group_mut(target.pane) {
                    group.insert(index, panel);
                }
                self.recent[to.index()] = target.pane;
            }
            Some((axis, first)) => {
                let mut group = TabGroup::new(self.fresh_id());
                group.panels.push(panel);
                let new_group = group.id;
                let split_id = self.fresh_id();
                self.roots[to.index()].insert_beside(target.pane, axis, first, group, split_id);
                self.recent[to.index()] = new_group;
            }
        }
        self.settle(from.region);
        if to != from.region {
            self.settle(to);
        }
        true
    }

    /// Every group of the shown regions, in tree order: left, center,
    /// bottom, right, and within a region its groups in reading order.
    fn shown_groups(&self) -> Vec<PaneId> {
        TREE_ORDER
            .into_iter()
            .filter(|r| self.is_visible(*r))
            .flat_map(|r| self.roots[r.index()].groups().into_iter().map(|g| g.id))
            .collect()
    }

    /// Whether [`Self::move_tab`] would move `panel`: `destination` is in
    /// a shown region and [`Self::can_drop`] allows it.
    pub fn can_move(&self, panel: PanelId, destination: PaneId, index: Option<usize>) -> bool {
        self.region_of(destination)
            .is_some_and(|r| self.is_visible(r))
            && self.can_drop(panel, move_drop(destination, index))
    }

    /// The groups `panel` can move into whole, in tree order: every shown
    /// group but its own that takes it. What a "Move to group" menu lists.
    pub fn move_targets(&self, panel: PanelId) -> Vec<PaneId> {
        self.shown_groups()
            .into_iter()
            .filter(|&pane| self.can_move(panel, pane, None))
            .collect()
    }

    /// The nearest group before (or with `forward`, after) `panel`'s own
    /// in tree order that it can move into. Does not wrap around.
    pub fn move_target(&self, panel: PanelId, forward: bool) -> Option<PaneId> {
        let own = self.locate(panel)?.1;
        let groups = self.shown_groups();
        let at = groups.iter().position(|g| *g == own)?;
        let eligible = |pane: &&PaneId| self.can_move(panel, **pane, None);
        if forward {
            groups[at + 1..].iter().find(eligible).copied()
        } else {
            groups[..at].iter().rev().find(eligible).copied()
        }
    }

    /// Move `panel` into the group `destination`, before tab `index` or at
    /// its end, and make it active there. Returns false, changing nothing,
    /// when [`Self::can_move`] refuses.
    pub fn move_tab(&mut self, panel: PanelId, destination: PaneId, index: Option<usize>) -> bool {
        self.can_move(panel, destination, index)
            && self.drop_panel(panel, move_drop(destination, index))
    }

    /// The panel whose tab has `focus`, a [`Dock::tab_focus`] target: the
    /// active panel of that group.
    pub fn focused_panel(&self, focus: Option<FocusId>) -> Option<PanelId> {
        let focus = focus?;
        self.roots
            .iter()
            .flat_map(PaneNode::groups)
            .find(|g| Dock::tab_focus(g.id) == focus)?
            .active_panel()
    }

    /// The tab being dragged and where it would land, while a drag is over
    /// a target it may drop on.
    pub fn drop_preview(&self) -> Option<(PanelId, PaneDrop)> {
        let (panel, target) = self.tab_drag?;
        let target = target?;
        self.can_drop(panel, target).then_some((panel, target))
    }

    fn split_weights(&self, split: PaneId) -> Option<Vec<f32>> {
        self.roots
            .iter()
            .find_map(|r| r.split(split))
            .map(|s| s.weights.clone())
    }

    fn drag_divider(&mut self, split: PaneId, divider: usize, event: PaneDividerEvent) {
        match event {
            PaneDividerEvent::Press => {
                self.divider_drag = self.split_weights(split).map(|w| (split, divider, w));
            }
            PaneDividerEvent::Drag { delta, extent } => {
                let Some((id, d, origin)) = self.divider_drag.clone() else {
                    return;
                };
                if id == split && d == divider {
                    self.move_divider(split, divider, &origin, extent, |_| delta);
                }
            }
            PaneDividerEvent::Release => self.divider_drag = None,
            PaneDividerEvent::Nudge { delta, extent } => {
                if let Some(weights) = self.split_weights(split) {
                    self.move_divider(split, divider, &weights, extent, |_| delta);
                }
            }
            PaneDividerEvent::SetPosition { position, extent } => {
                if let Some(weights) = self.split_weights(split) {
                    self.move_divider(split, divider, &weights, extent, |at| position - at);
                }
            }
        }
    }

    /// Lay `split` out from `weights` in `extent` and move `divider` by
    /// `delta` of where it is there.
    fn move_divider(
        &mut self,
        split: PaneId,
        divider: usize,
        weights: &[f32],
        extent: f32,
        delta: impl FnOnce(f32) -> f32,
    ) {
        let mut sizes = child_sizes(weights, extent);
        let avail: f32 = sizes.iter().sum();
        if avail <= 0.0 {
            return;
        }
        let min = divider_min(MIN_GROUP, avail, sizes.len());
        let (at, _, _) = divider_span(&sizes, min, divider);
        push_divider(&mut sizes, min, divider, delta(at));
        if let Some(s) = self.roots.iter_mut().find_map(|r| r.split_mut(split)) {
            s.weights = sizes.iter().map(|size| size / avail).collect();
        }
    }

    /// Apply an event from the dock's element. `now_ms` dates divider
    /// presses for double click. Returns true when the change is settled
    /// and worth persisting. [`Self::apply_event`] also reports where a
    /// moved tab went.
    pub fn apply(&mut self, event: DockEvent, now_ms: u64) -> bool {
        self.apply_event(event, now_ms).settled
    }

    /// Apply an event from the dock's element, like [`Self::apply`]. After
    /// a [`DockEvent::MoveTab`], focus [`DockOutcome::focus`] and announce
    /// the destination.
    pub fn apply_event(&mut self, event: DockEvent, now_ms: u64) -> DockOutcome {
        let mut moved = None;
        let mut selected = None;
        let settled = match event {
            DockEvent::Split(which, event) => self.split_mut(which).apply(event, now_ms),
            DockEvent::PaneDivider {
                split,
                divider,
                event,
            } => {
                self.drag_divider(split, divider, event);
                matches!(
                    event,
                    PaneDividerEvent::Release
                        | PaneDividerEvent::Nudge { .. }
                        | PaneDividerEvent::SetPosition { .. }
                )
            }
            DockEvent::Select { pane, index } => {
                self.select(pane, index);
                selected = self.group(pane).map(|g| g.id);
                true
            }
            DockEvent::Close { pane, index } => self.close(pane, index).is_some(),
            DockEvent::Toggle(region) => {
                self.toggle(region);
                true
            }
            DockEvent::TabHover { panel, target } => {
                self.tab_drag = Some((panel, target));
                false
            }
            DockEvent::TabDrop { panel, target } => {
                self.tab_drag = None;
                target.is_some_and(|t| self.drop_panel(panel, t))
            }
            DockEvent::MoveTab {
                panel,
                destination,
                index,
            } => {
                let ok = self.move_tab(panel, destination, index);
                if ok && let Some((region, pane, _)) = self.locate(panel) {
                    moved = Some(TabMove {
                        panel,
                        region,
                        pane,
                    });
                }
                ok
            }
        };
        self.debug_verify();
        DockOutcome {
            settled,
            moved,
            selected,
        }
    }

    pub fn snapshot(&self) -> DockSnapshot {
        let [left, right, bottom, center] = self.roots.clone();
        DockSnapshot {
            columns: self.columns.snapshot(),
            rows: self.rows.snapshot(),
            left,
            right,
            bottom,
            center,
        }
    }

    /// Restore a snapshot. Panels `keep` rejects (ones the app no longer
    /// has) are dropped, and so are repeats; emptied groups are pruned and
    /// regions left empty hide. Groups get fresh ids.
    pub fn restore(&mut self, snapshot: &DockSnapshot, keep: impl Fn(PanelId) -> bool) {
        self.columns.restore(&snapshot.columns);
        self.rows.restore(&snapshot.rows);
        let saved = [
            &snapshot.left,
            &snapshot.right,
            &snapshot.bottom,
            &snapshot.center,
        ];
        let mut seen: Vec<PanelId> = Vec::new();
        let mut next = 0;
        for region in DockRegion::ALL {
            let mut root = saved[region.index()].clone();
            root.renumber(&mut next);
            let ids: Vec<PaneId> = root.groups().iter().map(|g| g.id).collect();
            for id in ids {
                let Some(group) = root.group_mut(id) else {
                    continue;
                };
                let active = group.active_panel();
                let mut kept = Vec::with_capacity(group.panels.len());
                for &p in &group.panels {
                    if keep(p) && !seen.contains(&p) {
                        seen.push(p);
                        kept.push(p);
                    }
                }
                group.panels = kept;
                group.active = active
                    .and_then(|a| group.panels.iter().position(|p| *p == a))
                    .unwrap_or(0);
            }
            self.roots[region.index()] = root
                .prune()
                .unwrap_or_else(|| PaneNode::Tabs(TabGroup::new(PaneId(next))));
            next += 1;
        }
        self.next_id = next;
        self.recent = DockRegion::ALL.map(|r| self.roots[r.index()].groups()[0].id);
        self.confined.retain(|p| seen.contains(p));
        self.tab_drag = None;
        self.divider_drag = None;
        for region in DockRegion::ALL {
            if self.panels(region).is_empty() {
                self.set_hidden(region, true);
            }
        }
        self.debug_verify();
    }

    pub fn verify_integrity(&self) -> Result<(), DockIntegrityError> {
        fn walk(
            node: &PaneNode,
            root: bool,
            next_id: u32,
            ids: &mut Vec<PaneId>,
            panels: &mut Vec<PanelId>,
        ) -> Result<(), DockIntegrityError> {
            let id = match node {
                PaneNode::Tabs(g) => g.id,
                PaneNode::Split(s) => s.id,
            };
            if id.0 >= next_id || ids.contains(&id) {
                return Err(DockIntegrityError::PaneIds);
            }
            ids.push(id);
            match node {
                PaneNode::Tabs(g) => {
                    if g.panels.is_empty() && !root {
                        return Err(DockIntegrityError::EmptyGroup(g.id));
                    }
                    if g.active >= g.panels.len().max(1) {
                        return Err(DockIntegrityError::Active(g.id));
                    }
                    for p in &g.panels {
                        if panels.contains(p) {
                            return Err(DockIntegrityError::DuplicatePanel(*p));
                        }
                        panels.push(*p);
                    }
                }
                PaneNode::Split(PaneSplit {
                    id,
                    axis,
                    children,
                    weights,
                }) => {
                    let sum: f32 = weights.iter().sum();
                    let nested_same_axis = children
                        .iter()
                        .any(|c| matches!(c, PaneNode::Split(s) if s.axis == *axis));
                    if children.len() < 2
                        || weights.len() != children.len()
                        || weights.iter().any(|w| !(w.is_finite() && *w > 0.0))
                        || (sum - 1.0).abs() > 1e-3
                        || nested_same_axis
                    {
                        return Err(DockIntegrityError::SplitShape(*id));
                    }
                    for child in children {
                        walk(child, false, next_id, ids, panels)?;
                    }
                }
            }
            Ok(())
        }
        let mut ids = Vec::new();
        let mut panels = Vec::new();
        for root in &self.roots {
            walk(root, true, self.next_id, &mut ids, &mut panels)?;
        }
        Ok(())
    }

    fn debug_verify(&self) {
        debug_assert_eq!(self.verify_integrity(), Ok(()));
    }

    /// Each region's tree as text, one line per region with panels:
    /// `right: row([10* 11] | col([12*] | [13*]))`, `*` marking the active
    /// tab.
    #[cfg(test)]
    fn dump(&self) -> String {
        fn node(n: &PaneNode, out: &mut String) {
            match n {
                PaneNode::Tabs(g) => {
                    let tabs: Vec<String> = g
                        .panels
                        .iter()
                        .enumerate()
                        .map(|(i, p)| format!("{}{}", p.0, if i == g.active { "*" } else { "" }))
                        .collect();
                    out.push_str(&format!("[{}]", tabs.join(" ")));
                }
                PaneNode::Split(s) => {
                    out.push_str(match s.axis {
                        Axis::Horizontal => "row(",
                        Axis::Vertical => "col(",
                    });
                    for (i, c) in s.children.iter().enumerate() {
                        if i > 0 {
                            out.push_str(" | ");
                        }
                        node(c, out);
                    }
                    out.push(')');
                }
            }
        }
        let mut out = String::new();
        for (region, name) in DockRegion::ALL
            .into_iter()
            .zip(["left", "right", "bottom", "center"])
        {
            if self.panels(region).is_empty() {
                continue;
            }
            out.push_str(name);
            out.push_str(": ");
            node(self.root(region), &mut out);
            out.push('\n');
        }
        out
    }
}

type EventMap = Rc<dyn Fn(DockEvent) -> Action>;

/// One tab group as laid out this frame, for finding drop targets.
#[derive(Debug, Clone, Copy)]
struct GroupHit {
    pane: PaneId,
    region: DockRegion,
    rect: Rect,
    /// Height of its tab strip; zero when it shows none.
    strip: f32,
    tab_width: f32,
}

/// Stable id of the root of a dock given no [`Dock::handle`].
const DOCK_ID: &str = "dock";

/// How a tab drag finds its dock's root in a frame's geometry.
#[derive(Debug, Clone, Copy)]
enum DockRoot {
    Handle(ElementHandle),
    Id,
}

impl DockRoot {
    fn find(self, geometry: &LayoutSnapshot) -> Option<ElementGeometry> {
        match self {
            Self::Handle(handle) => geometry.by_handle(handle),
            Self::Id => geometry.by_id(DOCK_ID),
        }
        .ok()
    }
}

/// What a tab drag needs from the frame it started in. Its rects are in
/// the dock's own coordinates.
struct DragContext {
    hits: Vec<GroupHit>,
    policies: [TabPolicy; 4],
    root: DockRoot,
}

impl DragContext {
    fn target_at(&self, x: f32, y: f32) -> Option<(DockRegion, PaneDrop)> {
        let hit = self.hits.iter().find(|h| h.rect.contains(x, y))?;
        let (lx, ly) = (x - hit.rect.x, y - hit.rect.y);
        let zone = if ly < hit.strip {
            DropZone::Tabs((lx / hit.tab_width).floor().max(0.0) as usize)
        } else {
            DropZone::in_body(
                hit.rect.width,
                hit.rect.height - hit.strip,
                lx,
                ly - hit.strip,
            )
        };
        Some((
            hit.region,
            PaneDrop {
                pane: hit.pane,
                zone,
            },
        ))
    }
}

/// Builds the element for a [`DockState`].
pub struct Dock<'a> {
    state: &'a DockState,
    handle: Option<ElementHandle>,
    size: (f32, f32),
    map: EventMap,
    toggle_keys: Vec<(DockRegion, String)>,
    move_keys: Option<(String, String)>,
    always_tabs: [bool; 4],
    tab_width: f32,
}

impl<'a> Dock<'a> {
    /// A dock filling `size` points. `on_event` wraps its events in the
    /// app's action type.
    pub fn new(
        state: &'a DockState,
        size: (f32, f32),
        on_event: impl Fn(DockEvent) -> Action + 'static,
    ) -> Self {
        Self {
            state,
            handle: None,
            size,
            map: Rc::new(on_event),
            toggle_keys: Vec::new(),
            move_keys: Some((MOVE_TAB_KEYS.0.into(), MOVE_TAB_KEYS.1.into())),
            always_tabs: [false; 4],
            tab_width: 120.0,
        }
    }

    /// Name the dock's root element by `handle`. A tab drag finds the dock
    /// in the frame's geometry to map the pointer onto its groups, by the
    /// handle or else by the stable id `"dock"`, so a window with more than
    /// one dock gives each a handle.
    pub fn handle(mut self, handle: ElementHandle) -> Self {
        self.handle = Some(handle);
        self
    }

    /// Stable id of `panel`'s tab, wherever it is docked: for finding it
    /// in the frame's geometry (anchoring a menu to it) or in the
    /// accessibility tree.
    pub fn tab_id(panel: PanelId) -> String {
        format!("dock:tab:{}", panel.0)
    }

    /// Toggle a side region with `binding` (keymap format, `"mod+b"`) from
    /// anywhere inside the dock, or anywhere at all when nothing is
    /// focused.
    pub fn toggle_key(mut self, region: DockRegion, binding: impl Into<String>) -> Self {
        self.toggle_keys.push((region, binding.into()));
        self
    }

    /// Keys (keymap format) that move the focused tab into the previous and
    /// next group that takes it, in tree order ([`DockState::move_target`]);
    /// `None` binds none. Defaults to Mod+Shift+Page Up and Page Down.
    pub fn move_tab_keys(mut self, keys: Option<(&str, &str)>) -> Self {
        self.move_keys = keys.map(|(previous, next)| (previous.into(), next.into()));
        self
    }

    /// Show the region's tab strip even for a single panel.
    pub fn always_show_tabs(mut self, region: DockRegion) -> Self {
        self.always_tabs[region.index()] = true;
        self
    }

    /// Width of every tab, shrunk so all tabs fit the strip. Tabs share one
    /// width so a drag maps to a slot without measuring titles.
    pub fn tab_width(mut self, width: f32) -> Self {
        self.tab_width = width;
        self
    }

    /// Focus target of a group's active tab. Arrow keys move it.
    pub fn tab_focus(pane: PaneId) -> FocusId {
        FocusId::new(
            FocusId::from_key("dock:tab")
                .0
                .wrapping_add(u64::from(pane.0) + 1),
        )
    }

    fn strip_height(theme: &Theme) -> f32 {
        (theme.metrics.ui_font_size * 2.5).round()
    }

    /// A group shows tabs with more than one panel, when its region asks,
    /// or when its region is split, so every group's tabs can be dragged.
    fn shows_tabs(&self, region: DockRegion, group: &TabGroup) -> bool {
        group.panels.len() > 1
            || self.always_tabs[region.index()]
            || matches!(self.state.root(region), PaneNode::Split(_))
    }

    fn tab_width_for(&self, width: f32, count: usize) -> f32 {
        self.tab_width.min(width / count.max(1) as f32).floor()
    }

    /// Each shown region's rect inside the dock.
    fn region_rects(&self) -> [Rect; 4] {
        let state = self.state;
        let (width, height) = self.size;
        let columns = state.columns.resolve(width);
        let columns = columns.as_slice();
        let rows = state.rows.resolve(height);
        let rows = rows.as_slice();
        let gap = |split: &SplitState, divider: usize| {
            if split.divider_shown(divider) {
                DIVIDER_THICKNESS
            } else {
                0.0
            }
        };
        let middle_x = columns[0] + gap(&state.columns, 0);
        let right_x = middle_x + columns[1] + gap(&state.columns, 1);
        let bottom_y = rows[0] + gap(&state.rows, 0);
        [
            Rect::new(0.0, 0.0, columns[0], height),
            Rect::new(right_x, 0.0, columns[2], height),
            Rect::new(middle_x, bottom_y, columns[1], rows[1]),
            Rect::new(middle_x, 0.0, columns[1], rows[0]),
        ]
    }

    /// `title` names each panel's tab; `content` builds an active panel's
    /// content for the size it gets.
    pub fn build(
        self,
        theme: &Theme,
        title: impl Fn(PanelId) -> String,
        mut content: impl FnMut(PanelId, (f32, f32)) -> AnyElement,
    ) -> AnyElement {
        let state = self.state;
        let (width, height) = self.size;
        let rects = self.region_rects();

        let strip = Self::strip_height(theme);
        let mut hits = Vec::new();
        for region in DockRegion::ALL {
            if region != DockRegion::Center && !state.is_visible(region) {
                continue;
            }
            let mut groups = Vec::new();
            state
                .root(region)
                .layout(rects[region.index()], &mut groups);
            for (pane, rect) in groups {
                let Some(group) = state.group(pane) else {
                    continue;
                };
                let tabs = self.shows_tabs(region, group);
                hits.push(GroupHit {
                    pane,
                    region,
                    rect,
                    strip: if tabs { strip } else { 0.0 },
                    tab_width: self.tab_width_for(rect.width, group.panels.len()),
                });
            }
        }
        let drag = Rc::new(DragContext {
            hits,
            policies: state.policies,
            root: self.handle.map_or(DockRoot::Id, DockRoot::Handle),
        });

        let mut region = |r: DockRegion| {
            let rect = rects[r.index()];
            self.region(
                theme,
                r,
                (rect.width, rect.height),
                &drag,
                &title,
                &mut content,
            )
        };
        let left = region(DockRegion::Left);
        let right = region(DockRegion::Right);
        let center = region(DockRegion::Center);
        let bottom = region(DockRegion::Bottom);

        let rows_map = self.map.clone();
        let middle = Split::new("dock:rows", &state.rows, height, move |e| {
            rows_map(DockEvent::Split(DockSplit::Rows, e))
        })
        .child(center)
        .child(bottom)
        .build(theme);
        let columns_map = self.map.clone();
        let body = Split::new("dock:columns", &state.columns, width, move |e| {
            columns_map(DockEvent::Split(DockSplit::Columns, e))
        })
        .child(left)
        .child(middle)
        .child(right)
        .build(theme);

        view! {
            <div w={width} h={height} bg={theme.colors.background}
                 @when {let Some(handle) = self.handle} { element_handle={handle} }
                 @when {self.handle.is_none()} { id={DOCK_ID} }
                 @for (r, binding) in &self.toggle_keys {
                     on_key={(binding.clone(), (self.map)(DockEvent::Toggle(*r)))}
                 }>
                {body}
            </div>
        }
    }

    fn region(
        &self,
        theme: &Theme,
        region: DockRegion,
        size: (f32, f32),
        drag: &Rc<DragContext>,
        title: &impl Fn(PanelId) -> String,
        content: &mut impl FnMut(PanelId, (f32, f32)) -> AnyElement,
    ) -> AnyElement {
        if region != DockRegion::Center && !self.state.is_visible(region) {
            return view! { <div w={size.0} h={size.1} bg={theme.colors.surface} /> };
        }
        self.node(
            theme,
            region,
            self.state.root(region),
            size,
            drag,
            title,
            content,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn node(
        &self,
        theme: &Theme,
        region: DockRegion,
        node: &PaneNode,
        (width, height): (f32, f32),
        drag: &Rc<DragContext>,
        title: &impl Fn(PanelId) -> String,
        content: &mut impl FnMut(PanelId, (f32, f32)) -> AnyElement,
    ) -> AnyElement {
        let split = match node {
            PaneNode::Tabs(group) => {
                return self.group(theme, region, group, (width, height), drag, title, content);
            }
            PaneNode::Split(split) => split,
        };
        let horizontal = split.axis == Axis::Horizontal;
        let extent = if horizontal { width } else { height };
        let sizes = child_sizes(&split.weights, extent);
        view! {
            <div @when {horizontal} { class="flex-row" } @when {!horizontal} { class="flex-col" }
                 w={width} h={height}>
                for (i, (child, &size)) in split.children.iter().zip(&sizes).enumerate() {
                    if i > 0 {
                        {self.pane_divider(theme, region, split, i - 1, &sizes, extent)}
                    }
                    <div class="flex-none overflow-clip" w={if horizontal { size } else { width }}
                         h={if horizontal { height } else { size }}>
                        {self.node(
                            theme,
                            region,
                            child,
                            if horizontal { (size, height) } else { (width, size) },
                            drag,
                            title,
                            content,
                        )}
                    </div>
                }
            </div>
        }
    }

    /// Focus target of divider `divider` of the split `split` inside a
    /// region; arrow keys move it. Stays with the divider while it is
    /// resized; a split that gains or loses groups renumbers its dividers.
    pub fn divider_focus(split: PaneId, divider: usize) -> FocusId {
        let key = (u64::from(split.0) << 16) | (divider as u64 & 0xffff);
        FocusId::new(FocusId::from_key("dock:divider").0.wrapping_add(key + 1))
    }

    /// The divider's value for assistive tech is its position in points
    /// from the split's start, with the range [`DockState`] lets it move
    /// in; its text is that position as a percentage of the split.
    fn pane_divider(
        &self,
        theme: &Theme,
        region: DockRegion,
        split: &PaneSplit,
        divider: usize,
        sizes: &[f32],
        extent: f32,
    ) -> AnyElement {
        let colors = &theme.colors;
        let horizontal = split.axis == Axis::Horizontal;
        let cursor = if horizontal {
            CursorHint::ResizeCol
        } else {
            CursorHint::ResizeRow
        };
        let (back, forward) = if horizontal {
            ("left", "right")
        } else {
            ("up", "down")
        };
        let map = self.map.clone();
        let id = split.id;
        let event = move |event| {
            (map)(DockEvent::PaneDivider {
                split: id,
                divider,
                event,
            })
        };
        let nudge = |delta: f32| event(PaneDividerEvent::Nudge { delta, extent });
        let set_position = event.clone();
        let numeric_actions = NumericActions::new(move |position| {
            set_position(PaneDividerEvent::SetPosition {
                position: position as f32,
                extent,
            })
        })
        .steps(nudge(-NUDGE_STEP), nudge(NUDGE_STEP));
        let avail: f32 = sizes.iter().sum();
        let (at, lo, hi) = divider_span(sizes, divider_min(MIN_GROUP, avail, sizes.len()), divider);
        let percent = if extent > 0.0 {
            at / extent * 100.0
        } else {
            0.0
        };
        let drag_map = self.map.clone();
        // The same wide invisible grip as a `Split` divider.
        let grip = 8.0;
        let offset = -(grip - DIVIDER_THICKNESS) / 2.0;
        view! {
            <div class="flex-none relative" bg={colors.border_variant}
                 @when {horizontal} { w={DIVIDER_THICKNESS} class="h-full" }
                 @when {!horizontal} { h={DIVIDER_THICKNESS} class="w-full" }>
                <div class="absolute" z_index={1}
                     accessibility_id={format!("dock:split:{}:{divider}", id.0)}
                     accessibility_role={Role::Splitter} role="separator"
                     aria-label={quark_ui::i18n::tr_args(
                         "quark-resize-named",
                         [("name", self.state.label(region).into())],
                     )}
                     aria-valuetext={format!("{percent:.0}%")}
                     accessibility_numeric={NumericValue {
                         value: f64::from(at),
                         min: f64::from(lo),
                         max: f64::from(hi),
                         step: Some(f64::from(NUDGE_STEP)),
                     }}
                     accessibility_numeric_actions={numeric_actions}
                     // A line between side by side groups stands upright.
                     accessibility_orientation={if horizontal {
                         Orientation::Vertical
                     } else {
                         Orientation::Horizontal
                     }}
                     focus_ring={Self::divider_focus(id, divider)}
                     on_key={(back, nudge(-NUDGE_STEP))}
                     on_key={(forward, nudge(NUDGE_STEP))}
                     on_key={(format!("shift+{back}"), nudge(-NUDGE_STEP_LARGE))}
                     on_key={(format!("shift+{forward}"), nudge(NUDGE_STEP_LARGE))}
                     test_id="dock-pane-divider" cursor={cursor} hover_bg={colors.accent}
                     on:drag={move |press: ClickEvent| {
                         Box::new(PaneDividerDrag {
                             map: drag_map.clone(),
                             split: id,
                             divider,
                             horizontal,
                             origin: if horizontal { press.x } else { press.y },
                             extent,
                             cursor,
                         }) as Box<dyn DragHandler>
                     }}
                     @when {horizontal} { class="top-0 bottom-0" left={offset} w={grip} }
                     @when {!horizontal} { class="left-0 right-0" top={offset} h={grip} } />
            </div>
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn group(
        &self,
        theme: &Theme,
        region: DockRegion,
        group: &TabGroup,
        (width, height): (f32, f32),
        drag: &Rc<DragContext>,
        title: &impl Fn(PanelId) -> String,
        content: &mut impl FnMut(PanelId, (f32, f32)) -> AnyElement,
    ) -> AnyElement {
        let colors = &theme.colors;
        let show_tabs = self.shows_tabs(region, group);
        let strip_height = if show_tabs {
            Self::strip_height(theme)
        } else {
            0.0
        };
        let body_height = (height - strip_height).max(0.0);
        // Where a dragged tab would land in this group.
        let preview = self
            .state
            .drop_preview()
            .filter(|(_, target)| target.pane == group.id)
            .map(|(_, target)| match target.zone {
                DropZone::Tabs(i) => {
                    let tab = self.tab_width_for(width, group.panels.len());
                    let x = (i.min(group.panels.len()) as f32 * tab - 1.0).max(0.0);
                    (x, 0.0, 2.0, strip_height)
                }
                zone => {
                    let (x, y, w, h) = zone.preview(width, body_height);
                    (x, y + strip_height, w, h)
                }
            });
        view! {
            <div class="relative flex-col" w={width} h={height} bg={colors.surface}>
                if show_tabs && !group.panels.is_empty() {
                    {self.tab_strip(theme, region, group, (width, strip_height), drag, title)}
                }
                if let Some(active) = group.active_panel() {
                    <div w={width} h={body_height} class="overflow-clip"
                         accessibility_id={format!("dock:pane:{}:panel", group.id.0)}
                         accessibility_role={Role::TabPanel} role="tabpanel"
                         aria-label={title(active)}>
                        {content(active, (width, body_height))}
                    </div>
                }
                if let Some((x, y, w, h)) = preview {
                    <div class="absolute" z_index={10} left={x} top={y} w={w} h={h}
                         bg={colors.accent.with_alpha(if w > 2.0 { 56 } else { 255 })}
                         test_id="dock-drop-preview" />
                }
            </div>
        }
    }

    fn tab_strip(
        &self,
        theme: &Theme,
        region: DockRegion,
        group: &TabGroup,
        (width, height): (f32, f32),
        drag: &Rc<DragContext>,
        title: &impl Fn(PanelId) -> String,
    ) -> AnyElement {
        let colors = &theme.colors;
        let tab_width = self.tab_width_for(width, group.panels.len());
        view! {
            <div class="flex-row w-full" h={height} class="flex-none overflow-clip"
                 border_b={colors.border_variant}
                 accessibility_id={format!("dock:pane:{}:tabs", group.id.0)}
                 accessibility_role={Role::TabList} role="tablist"
                 aria-label={self.state.label(region)} test_id="dock-tabs">
                for (index, &panel) in group.panels.iter().enumerate() {
                    {self.tab(theme, region, group, index, panel, (tab_width, height), drag, title)}
                }
                <div class="flex-1 h-full" bg={Color::TRANSPARENT} />
            </div>
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn tab(
        &self,
        theme: &Theme,
        region: DockRegion,
        group: &TabGroup,
        index: usize,
        panel: PanelId,
        (tab_width, tab_height): (f32, f32),
        drag: &Rc<DragContext>,
        title: &impl Fn(PanelId) -> String,
    ) -> AnyElement {
        let colors = &theme.colors;
        let m = &theme.metrics;
        let pane = group.id;
        let count = group.panels.len();
        let selected = index == group.active;
        let name = title(panel);
        let map = self.map.clone();
        let ctx = drag.clone();
        let confined = self.state.confined.contains(&panel);
        let close = (self.map)(DockEvent::Close { pane, index });
        // Roving focus: only the active tab is a focus target, so selecting
        // a neighbor by arrow key moves focus with it.
        let prev = index.checked_sub(1).unwrap_or(count - 1);
        let next = (index + 1) % count;
        let select = |index| (self.map)(DockEvent::Select { pane, index });
        // Only the focusable active tab takes the move keys, bound only
        // toward a group that takes it, so a refused move leaves the key
        // to the app.
        let moves: Vec<(String, Action)> = match (&self.move_keys, selected) {
            (Some((previous, next)), true) => [(previous, false), (next, true)]
                .into_iter()
                .filter_map(|(key, forward)| {
                    let destination = self.state.move_target(panel, forward)?;
                    let event = DockEvent::MoveTab {
                        panel,
                        destination,
                        index: None,
                    };
                    Some((key.clone(), (self.map)(event)))
                })
                .collect(),
            _ => Vec::new(),
        };
        let label_color = if selected {
            colors.text_strong
        } else {
            colors.text_muted
        };
        let close_label = quark_ui::i18n::tr_args(
            "quark-close-named",
            [("name", quark_ui::i18n::Arg::Text(&name))],
        );
        let drag_title = name.clone();
        view! {
            <div class="flex-row flex-none items-center" gap={m.spacing_xs} px={m.spacing_sm}
                 w={tab_width} class="h-full" border_r={colors.border_variant}
                 accessibility_id={Self::tab_id(panel)} accessibility_role={Role::Tab}
                 role="tab" aria-label={name.clone()} aria-selected={selected} test_id="dock-tab"
                 // For assistive tech and Enter: a pointer press starts the
                 // drag below, which selects the tab itself.
                 on:click={select(index)} cursor={CursorHint::Default}
                 on:middle_click={close.clone()}
                 on:drag={move |press: ClickEvent| {
                     Box::new(TabDrag {
                         map: map.clone(),
                         ctx: ctx.clone(),
                         panel,
                         confined,
                         from: Origin {
                             region,
                             pane,
                             index,
                             count,
                         },
                         target: None,
                         allowed: true,
                         press: (press.x, press.y),
                         dock: None,
                         held: false,
                         preview: tab_preview(panel, &drag_title, (tab_width, tab_height)),
                     }) as Box<dyn DragHandler>
                 }}
                 @when {selected} {
                     bg={colors.background} focus_ring={Self::tab_focus(pane)}
                     on_key={("left", select(prev))} on_key={("right", select(next))}
                     on_key={("home", select(0))} on_key={("end", select(count - 1))}
                     on_key={("delete", close.clone())}
                 }
                 @for (key, action) in &moves {
                     on_key={(key.clone(), action.clone())}
                 }
                 // A click makes an inactive tab focusable without making it
                 // a Tab stop: a press focuses it, and the selection then
                 // hands focus to the group (`DockOutcome::focus`).
                 @when {!selected} {
                     hover_bg={colors.ghost_element_hover} tab_stop={TabStop::disabled(0)}
                 }>
                // Let a long title shrink and truncate instead of pushing the
                // close button out of the tab.
                <div class="flex-1 min-w-0 overflow-clip">
                    <text class="text-sm" color={label_color} class="truncate">{name}</text>
                </div>
                <div class="flex-none items-center justify-center" rounded={m.control_radius * 0.5}
                     p={2.0} hover_bg={colors.ghost_element_hover}
                     accessibility_id={format!("dock:close:{}", panel.0)}
                     accessibility_role={Role::Button} aria-label={close_label} on:click={close}>
                    <icon svg={lucide::X} size={m.ui_small_font_size} color={colors.text_muted} />
                </div>
            </div>
        }
    }
}

/// The picture of a dragged tab: its title on a raised tab of its size,
/// which the drag replaces with the tab's measured size.
fn tab_preview(panel: PanelId, title: &str, size: (f32, f32)) -> DragPreview {
    use std::hash::{Hash, Hasher};
    let mut key = std::hash::DefaultHasher::new();
    (panel, title).hash(&mut key);
    let title = title.to_owned();
    DragPreview::new(key.finish(), size, move |theme, (width, height)| {
        let colors = &theme.colors;
        let m = &theme.metrics;
        view! {
            <div class="flex-row items-center" w={width} h={height} px={m.spacing_sm}
                 bg={colors.elevated_surface} rounded={m.control_radius}
                 shadow_preset={Shadow::POPOVER}>
                <div class="flex-1 min-w-0 overflow-clip">
                    <text class="text-sm truncate" color={colors.text_strong}>
                        {title.clone()}
                    </text>
                </div>
            </div>
        }
    })
    .hotspot(size.0 / 2.0, size.1 / 2.0)
}

/// Selects a tab on press, reports the drop target under the pointer as it
/// moves, and drops the tab there on release. Shows the tab under the
/// pointer, held where it was pressed.
struct TabDrag {
    map: EventMap,
    ctx: Rc<DragContext>,
    panel: PanelId,
    confined: bool,
    from: Origin,
    target: Option<PaneDrop>,
    allowed: bool,
    /// Window point of the press.
    press: (f32, f32),
    /// The dock's root in the frame the drag routes through, to map the
    /// pointer into the dock's coordinates.
    dock: Option<ElementGeometry>,
    /// The preview has the tab's measured size and the press's place on it.
    held: bool,
    preview: DragPreview,
}

impl DragHandler for TabDrag {
    fn set_geometry(&mut self, geometry: &LayoutSnapshot) {
        self.dock = self.ctx.root.find(geometry);
        // Once, from the frame pressed in: later frames may have moved the
        // tab (selected, or dropped somewhere) while it stays held.
        if !self.held
            && let Ok(tab) = geometry.by_id(&Dock::tab_id(self.panel))
        {
            let b = tab.bounds;
            self.preview.set_size(b.width, b.height);
            self.preview
                .set_hotspot(self.press.0 - b.x, self.press.1 - b.y);
            self.held = true;
        }
    }

    fn on_press(&mut self) -> Vec<Action> {
        vec![(self.map)(DockEvent::Select {
            pane: self.from.pane,
            index: self.from.index,
        })]
    }

    fn on_move(&mut self, x: f32, y: f32) -> Vec<Action> {
        // Without the dock in the frame, as if it filled the window.
        let (x, y) = self.dock.and_then(|g| g.to_local(x, y)).unwrap_or((x, y));
        let mut allowed = true;
        let target = self.ctx.target_at(x, y).and_then(|(region, target)| {
            allowed = drop_allowed(&self.ctx.policies, self.confined, self.from, region, target);
            // Over its own slot, or a split its group cannot give it, the
            // tab is simply not moving: no preview and no refusal.
            (allowed || target.pane != self.from.pane).then_some(target)
        });
        self.allowed = allowed || target.is_none();
        if target == self.target {
            return Vec::new();
        }
        self.target = target;
        vec![(self.map)(DockEvent::TabHover {
            panel: self.panel,
            target,
        })]
    }

    fn on_release(&mut self) -> DragReleaseResult {
        DragReleaseResult {
            actions: vec![(self.map)(DockEvent::TabDrop {
                panel: self.panel,
                target: self.target.filter(|_| self.allowed),
            })],
        }
    }

    /// Drop nowhere: the tab stays where it was.
    fn on_cancel(&mut self) -> Vec<Action> {
        vec![(self.map)(DockEvent::TabDrop {
            panel: self.panel,
            target: None,
        })]
    }

    fn preview(&self) -> Option<&DragPreview> {
        Some(&self.preview)
    }

    fn cursor(&self) -> CursorHint {
        match (self.target, self.allowed) {
            (Some(_), false) => CursorHint::NotAllowed,
            (Some(_), true) => CursorHint::Grabbing,
            (None, _) => CursorHint::Default,
        }
    }
}

struct PaneDividerDrag {
    map: EventMap,
    split: PaneId,
    divider: usize,
    horizontal: bool,
    origin: f32,
    extent: f32,
    cursor: CursorHint,
}

impl PaneDividerDrag {
    fn event(&self, event: PaneDividerEvent) -> Action {
        (self.map)(DockEvent::PaneDivider {
            split: self.split,
            divider: self.divider,
            event,
        })
    }
}

impl DragHandler for PaneDividerDrag {
    fn on_press(&mut self) -> Vec<Action> {
        vec![self.event(PaneDividerEvent::Press)]
    }

    fn on_move(&mut self, x: f32, y: f32) -> Vec<Action> {
        let at = if self.horizontal { x } else { y };
        vec![self.event(PaneDividerEvent::Drag {
            delta: at - self.origin,
            extent: self.extent,
        })]
    }

    fn on_release(&mut self) -> DragReleaseResult {
        DragReleaseResult {
            actions: vec![self.event(PaneDividerEvent::Release)],
        }
    }

    /// The panes follow the pointer as it moves, so cancelling keeps the
    /// weights dragged to, like a release.
    fn on_cancel(&mut self) -> Vec<Action> {
        self.on_release().actions
    }

    fn cursor(&self) -> CursorHint {
        self.cursor
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEFT: PanelId = PanelId(1);
    const CHAT: PanelId = PanelId(2);
    const A: PanelId = PanelId(10);
    const B: PanelId = PanelId(11);
    const C: PanelId = PanelId(12);

    /// Threads on the left, chat in the center, A B C on the right.
    fn dock() -> DockState {
        let mut dock = DockState::new(DockLayout::default());
        dock.open(DockRegion::Left, LEFT);
        dock.open(DockRegion::Center, CHAT);
        for panel in [A, B, C] {
            dock.open(DockRegion::Right, panel);
        }
        dock
    }

    fn pane_of(dock: &DockState, panel: PanelId) -> PaneId {
        dock.locate(panel).expect("panel is docked").1
    }

    /// Drop `panel` on the group holding `onto`, in `zone`.
    fn drop(dock: &mut DockState, panel: PanelId, onto: PanelId, zone: DropZone) -> bool {
        let pane = pane_of(dock, onto);
        dock.apply(
            DockEvent::TabDrop {
                panel,
                target: Some(PaneDrop { pane, zone }),
            },
            0,
        )
    }

    #[test]
    fn dropping_a_tab_moves_reorders_and_splits() {
        // (panel, onto the group of, zone, dock after)
        let cases: &[(PanelId, PanelId, DropZone, &str)] = &[
            (
                A,
                B,
                DropZone::Tabs(2),
                "left: [1*]\nright: [11 12 10*]\ncenter: [2*]\n",
            ),
            (
                C,
                B,
                DropZone::Tabs(0),
                "left: [1*]\nright: [12* 10 11]\ncenter: [2*]\n",
            ),
            (
                A,
                CHAT,
                DropZone::Center,
                "left: [1*]\nright: [11 12*]\ncenter: [2 10*]\n",
            ),
            (
                A,
                CHAT,
                DropZone::Tabs(0),
                "left: [1*]\nright: [11 12*]\ncenter: [10* 2]\n",
            ),
            (
                A,
                B,
                DropZone::Bottom,
                "left: [1*]\nright: col([11 12*] | [10*])\ncenter: [2*]\n",
            ),
            (
                C,
                CHAT,
                DropZone::Left,
                "left: [1*]\nright: [10 11*]\ncenter: row([12*] | [2*])\n",
            ),
        ];
        for &(panel, onto, zone, expected) in cases {
            let mut dock = dock();
            assert!(drop(&mut dock, panel, onto, zone), "{panel:?} {zone:?}");
            assert_eq!(dock.dump(), expected, "{panel:?} onto {onto:?} {zone:?}");
        }
    }

    #[test]
    fn splits_of_one_axis_share_a_parent() {
        let mut dock = dock();
        drop(&mut dock, A, CHAT, DropZone::Right);
        drop(&mut dock, B, A, DropZone::Right);
        drop(&mut dock, C, A, DropZone::Bottom);
        assert_eq!(
            dock.dump(),
            "left: [1*]\ncenter: row([2*] | col([10*] | [12*]) | [11*])\n"
        );
    }

    #[test]
    fn emptied_groups_collapse_out_of_the_tree() {
        let mut dock = dock();
        drop(&mut dock, A, CHAT, DropZone::Right);
        drop(&mut dock, B, A, DropZone::Bottom);
        // Moving A out leaves B's column with one child, which unwraps.
        drop(&mut dock, A, CHAT, DropZone::Center);
        assert_eq!(
            dock.dump(),
            "left: [1*]\nright: [12*]\ncenter: row([2 10*] | [11*])\n"
        );
        // Closing B's only tab leaves the center a single group again.
        dock.close(pane_of(&dock, B), 0);
        assert_eq!(dock.dump(), "left: [1*]\nright: [12*]\ncenter: [2 10*]\n");
    }

    #[test]
    fn moving_the_last_tab_out_of_a_side_region_hides_it() {
        let mut dock = dock();
        drop(&mut dock, LEFT, CHAT, DropZone::Center);
        assert!(!dock.is_visible(DockRegion::Left));
        assert_eq!(dock.dump(), "right: [10 11 12*]\ncenter: [2 1*]\n");
    }

    #[test]
    fn policies_and_confinement_refuse_moves_across_regions() {
        // (policy for the right region, confined panel, drop, allowed)
        let sealed = TabPolicy::SEALED;
        let keeps = TabPolicy {
            can_leave: false,
            accepts: true,
        };
        type Case = (
            TabPolicy,
            Option<PanelId>,
            (PanelId, PanelId, DropZone),
            bool,
        );
        let cases: &[Case] = &[
            (keeps, None, (A, CHAT, DropZone::Center), false),
            (keeps, None, (LEFT, A, DropZone::Center), true),
            (sealed, None, (LEFT, A, DropZone::Center), false),
            (sealed, None, (LEFT, A, DropZone::Left), false),
            // Inside a sealed region, tabs still reorder and split.
            (sealed, None, (A, B, DropZone::Tabs(2)), true),
            (sealed, None, (A, B, DropZone::Bottom), true),
            (TabPolicy::OPEN, Some(A), (A, CHAT, DropZone::Center), false),
            (TabPolicy::OPEN, Some(A), (B, CHAT, DropZone::Center), true),
        ];
        for &(policy, confined, (panel, onto, zone), allowed) in cases {
            let mut dock = dock();
            dock.set_policy(DockRegion::Right, policy);
            if let Some(p) = confined {
                dock.confine(p, true);
            }
            let before = dock.dump();
            assert_eq!(
                drop(&mut dock, panel, onto, zone),
                allowed,
                "{policy:?} {panel:?} {zone:?}"
            );
            if !allowed {
                assert_eq!(dock.dump(), before);
            }
        }
    }

    #[test]
    fn a_lone_tab_cannot_split_its_own_group() {
        let mut dock = dock();
        assert!(!drop(&mut dock, CHAT, CHAT, DropZone::Right));
        assert!(drop(&mut dock, A, A, DropZone::Right));
        assert_eq!(
            dock.dump(),
            "left: [1*]\nright: row([11 12*] | [10*])\ncenter: [2*]\n"
        );
    }

    /// Apply a [`DockEvent::MoveTab`] of `panel` to the end of the group
    /// holding `onto`.
    fn move_onto(dock: &mut DockState, panel: PanelId, onto: PanelId) -> DockOutcome {
        let destination = pane_of(dock, onto);
        dock.apply_event(
            DockEvent::MoveTab {
                panel,
                destination,
                index: None,
            },
            0,
        )
    }

    #[test]
    fn move_targets_follow_tree_order_and_skip_hidden_regions() {
        let mut dock = dock();
        // The right region split in two: [A] beside [B C].
        drop(&mut dock, A, B, DropZone::Left);
        const D: PanelId = PanelId(30);
        dock.open(DockRegion::Bottom, D);
        // (panel, forward, target holding)
        let cases: &[(PanelId, bool, Option<PanelId>)] = &[
            (LEFT, false, None),
            (LEFT, true, Some(CHAT)),
            (CHAT, false, Some(LEFT)),
            (CHAT, true, Some(D)),
            (D, true, Some(A)),
            (A, true, Some(B)),
            (B, false, Some(A)),
            (B, true, None),
        ];
        for &(panel, forward, expected) in cases {
            assert_eq!(
                dock.move_target(panel, forward),
                expected.map(|p| pane_of(&dock, p)),
                "{panel:?} forward {forward}"
            );
        }
        dock.set_visible(DockRegion::Bottom, false);
        assert_eq!(dock.move_target(CHAT, true), Some(pane_of(&dock, A)));
    }

    #[test]
    fn move_targets_skip_groups_policy_refuses() {
        let mut dock = dock();
        dock.set_policy(DockRegion::Center, TabPolicy::SEALED);
        // Past the sealed center, to the right region.
        assert_eq!(dock.move_target(LEFT, true), Some(pane_of(&dock, A)));
        dock.confine(LEFT, true);
        assert_eq!(dock.move_target(LEFT, true), None);
        assert_eq!(dock.move_targets(LEFT), []);
    }

    #[test]
    fn a_refused_move_changes_nothing() {
        let mut dock = dock();
        dock.set_policy(DockRegion::Right, TabPolicy::SEALED);
        let before = dock.dump();
        assert_eq!(move_onto(&mut dock, A, CHAT), DockOutcome::default());
        // Its own group is no destination either.
        assert_eq!(move_onto(&mut dock, A, B), DockOutcome::default());
        assert_eq!(dock.dump(), before);
    }

    #[test]
    fn a_moved_tab_lands_at_the_end_of_its_destination_and_is_active() {
        let mut dock = dock();
        let outcome = move_onto(&mut dock, B, CHAT);
        assert_eq!(
            dock.dump(),
            "left: [1*]\nright: [10 12*]\ncenter: [2 11*]\n"
        );
        assert_eq!(
            outcome.moved,
            Some(TabMove {
                panel: B,
                region: DockRegion::Center,
                pane: pane_of(&dock, CHAT),
            })
        );
        assert!(outcome.settled);
    }

    #[test]
    fn moving_a_groups_last_tab_prunes_the_group_and_keeps_the_destination() {
        let mut dock = dock();
        drop(&mut dock, A, CHAT, DropZone::Right);
        let destination = pane_of(&dock, CHAT);
        let outcome = move_onto(&mut dock, A, CHAT);
        assert_eq!(
            dock.dump(),
            "left: [1*]\nright: [11 12*]\ncenter: [2 10*]\n"
        );
        // The destination group keeps its identity, so focus can follow.
        assert_eq!(outcome.moved.map(|m| m.pane), Some(destination));
        assert_eq!(dock.focused_panel(outcome.focus()), Some(A));
    }

    #[test]
    fn a_snapshot_restores_split_groups() {
        let mut dock = dock();
        drop(&mut dock, A, CHAT, DropZone::Bottom);
        drop(&mut dock, B, A, DropZone::Right);
        let mut restored = DockState::new(DockLayout::default());
        restored.restore(&dock.snapshot(), |p| p != C);
        assert_eq!(
            restored.dump(),
            "left: [1*]\ncenter: col([2*] | row([10*] | [11*]))\n"
        );
    }
}
