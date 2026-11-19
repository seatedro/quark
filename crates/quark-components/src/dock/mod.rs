//! Docked regions of tabbed panels around a center, built on [`Split`].
//!
//! A [`DockState`] is a workspace of hosts. Its main host has four regions:
//! left, right, and bottom docks that can be resized and hidden, and a
//! center that fills the rest. Each region holds a [`PaneNode`] tree of tab
//! groups, each an ordered list of app-defined [`PanelId`]s with one active.
//! Floating hosts (one per extra window the app opens) each hold a single
//! tree. Click a tab to select and focus it, close it with its close button
//! or a middle click, or move between tabs with the arrow keys. The dividers
//! between split groups move with the pointer, the arrow keys (Shift for
//! larger steps), or assistive tech setting their value.
//!
//! Tabs drag anywhere: into a group's tab strip to reorder or move them,
//! onto a group's body to join it, or onto one of its edges to split the
//! group and open the tab beside it. While a tab is dragged it follows the
//! pointer and the dock shows where it would land; a cancelled drag
//! (Escape through the app, or the window losing focus) leaves it where it
//! was. Apps constrain this per area, a region of the main host or a whole
//! floating host, with a [`TabPolicy`] (an area whose tabs stay in it, or
//! that takes no tabs from elsewhere) and per panel with
//! [`DockState::confine`].
//!
//! Every move, whether a drop, a keyboard move, a group move, a move to a
//! new window, or re-docking a closed window's panels, goes through one
//! checked operation ([`DockState::prepare`] and [`DockState::commit`]) and
//! reports what the app should do next as [`DockEffects`]. A drag can
//! also tear a tab or group off live into a window of its own
//! ([`DockState::begin_live_detach`]) and then drop, keep, or cancel it.
//! The dock knows
//! nothing about windows: the app binds each floating [`HostId`] to one and
//! renders it with [`Dock::host`].
//!
//! The dock knows nothing about what panels are: the app gives each one a
//! title and builds the active ones' content. Like [`Split`], it emits
//! [`DockEvent`]s through a caller supplied mapping, and the app passes them
//! back to [`DockState::apply`]. [`DockState::workspace_snapshot`] persists
//! sizes, visibility, the tab groups, and floating hosts.

use std::borrow::Cow;
use std::collections::HashMap;
use std::rc::Rc;

use quark_ui::element::ElementHandle;
use quark_ui::{Action, FocusId};
use serde::{Deserialize, Serialize};

use crate::pane_tree::{DropZone, PaneDrop, PaneId, PaneNode};
use crate::split::{Pane, SplitEvent, SplitSnapshot, SplitState};

mod drag;
mod events;
mod gesture;
mod integrity;
mod live;
mod persist;
mod state;
mod tabs;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_workspace;
mod view;
mod workspace;

use gesture::Gesture;
pub use persist::{
    FloatingSnapshot, ReturnSnapshot, StoredDock, WORKSPACE_VERSION, WorkspaceSnapshot,
};
use tabs::StripMemo;
pub use tabs::Title;
use workspace::FloatingHost;
pub use workspace::{
    Boundary, DockDestination, DockEffects, DockLocation, HostId, MovePayload, MoveTarget,
    Transfer, TransferRefusal,
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

/// Which tab moves an area (a main host region, or a floating host)
/// allows across its boundary. Moves inside an area, splits included, are
/// always allowed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TabPolicy {
    /// Its tabs may be dragged into other areas.
    pub can_leave: bool,
    /// Tabs from other areas may be dropped into it.
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
    /// Move a panel or a whole group to `destination`, in any host: a
    /// "Move group to" command, or a drop routed between windows. Checked
    /// when applied; see [`DockState::transfer`].
    Transfer {
        payload: MovePayload,
        destination: DockDestination,
    },
    /// "Move to new window" and "Move group to new window": reserve a
    /// floating host for the payload ([`DockState::reserve_host`]). The
    /// outcome's [`DockEffects::create_host`] names the host to open a
    /// window for; the panels stay where they are until
    /// [`DockState::commit_host`].
    MoveToNewHost(MovePayload),
    /// A dragged tab or group grip moved past the drag threshold, the
    /// pointer at `at` in the window's coordinates. An app that follows
    /// drags across windows hands the drag off to a drag session here
    /// (`UiContext::hand_off_drag`); the drag then carries `payload`. Others
    /// ignore it, and the drag stays inside the window.
    DragOut {
        payload: MovePayload,
        at: (f32, f32),
    },
    /// A group dragged by its grip is over `target`, or over nowhere it
    /// can land; see [`DockState::set_drag_hover`].
    Hover {
        payload: MovePayload,
        target: Option<DockDestination>,
    },
}

/// What [`DockState::apply_event`] did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DockOutcome {
    /// The change is settled and worth persisting.
    pub settled: bool,
    /// Where a [`DockEvent::MoveTab`] or [`DockEvent::Transfer`] put its
    /// tab (a group's active one); `None` when it was refused or the event
    /// was another kind.
    pub moved: Option<TabMove>,
    /// The group a [`DockEvent::Select`] (a tab clicked, pressed to drag,
    /// or activated by assistive tech) made a tab active in.
    pub selected: Option<PaneId>,
    /// What a move, close, or new host reservation asks of the app.
    pub effects: DockEffects,
    /// Why a [`DockEvent::Transfer`] or [`DockEvent::MoveToNewHost`] was
    /// refused.
    pub refused: Option<TransferRefusal>,
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
    pub host: HostId,
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

/// The persisted part of a [`DockState`]'s main host: the format before
/// floating hosts, and the main host inside a [`WorkspaceSnapshot`].
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
    /// A panel shows in more than one tab, in any host.
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
    /// A floating host is the main host's id, shares another's id, or is
    /// not below the next fresh host id; or a reservation names an existing
    /// host.
    HostIds(HostId),
    /// A floating host has no panels.
    EmptyHost(HostId),
    /// The panel lookup disagrees with the trees about this panel.
    Index(PanelId),
    /// A return location kept for a panel that is not in a floating host.
    ReturnLocation(PanelId),
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

/// Stable id of the root of a dock given no [`Dock::handle`].
const DOCK_ID: &str = "dock";

/// Why tabs may not cross from an area with policy `leave` to one with
/// `enter`; `confined` when one of them is [`DockState::confine`]d. The
/// single boundary rule for drops, keyboard moves, group moves, new hosts,
/// and re-docking.
fn crossing_refusal(leave: TabPolicy, enter: TabPolicy, confined: bool) -> Option<Boundary> {
    if confined {
        Some(Boundary::Confined)
    } else if !leave.can_leave {
        Some(Boundary::CannotLeave)
    } else if !enter.accepts {
        Some(Boundary::NotAccepted)
    } else {
        None
    }
}

/// Whether dropping a tab from `from`'s group, `count` tabs with it at
/// `index`, on `target` leaves the layout as it is.
fn drop_is_noop(pane: PaneId, index: usize, count: usize, target: PaneDrop) -> bool {
    if pane != target.pane {
        return false;
    }
    match target.zone {
        DropZone::Center => true,
        DropZone::Tabs(i) => i.min(count - 1) == index,
        // Splitting a group off its only tab would leave it empty.
        _ => count <= 1,
    }
}

/// Panels, sizes, and visibility of a dock workspace, owned by the app.
#[derive(Debug, Clone, PartialEq)]
pub struct DockState {
    columns: SplitState,
    rows: SplitState,
    /// The main host's regions.
    roots: [PaneNode; 4],
    labels: [&'static str; 4],
    policies: [TabPolicy; 4],
    /// Floating hosts in the order they were made.
    floating: Vec<FloatingHost>,
    /// Hosts reserved for a new window, with the move each waits on.
    reservations: Vec<(HostId, Transfer)>,
    confined: Vec<PanelId>,
    /// Per main host region, the group last selected or dropped into: where
    /// [`Self::open`] adds panels.
    recent: [PaneId; 4],
    /// Where each docked panel is, and its tab index: the one lookup every
    /// query goes through. Rebuilt from the trees after each change.
    index: HashMap<PanelId, (DockLocation, usize)>,
    /// For panels in floating hosts, the main host region, group, and tab
    /// index they left from: where closing their host puts them back.
    returns: HashMap<PanelId, (DockRegion, PaneId, usize)>,
    next_id: u32,
    next_host: u64,
    revision: u64,
    /// The pointer gesture under way: a divider or tab drag, or a drag
    /// the app routes.
    gesture: Gesture,
    /// What each group's tab strip last showed, for [`Dock`]'s cached strips.
    strips: StripMemo,
}

type EventMap = Rc<dyn Fn(DockEvent) -> Action>;

/// Builds the element for a [`DockState`].
pub struct Dock<'a> {
    state: &'a DockState,
    host: HostId,
    handle: Option<ElementHandle>,
    size: (f32, f32),
    map: EventMap,
    toggle_keys: Vec<(DockRegion, String)>,
    /// Borrowed for the defaults, so building a dock allocates no keys.
    move_keys: Option<(Cow<'static, str>, Cow<'static, str>)>,
    always_tabs: [bool; 4],
    tab_width: f32,
    grips: bool,
    /// Measured title widths, when tabs fit their titles.
    fit: Option<&'a dyn Fn(PanelId) -> f32>,
    close_on_hover: bool,
    indicator: bool,
}
