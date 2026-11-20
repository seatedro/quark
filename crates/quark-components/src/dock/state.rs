//! A dock workspace's layout: its hosts and regions, the groups in them,
//! and the panel lookup every query goes through.

use std::collections::HashMap;

use quark_ui::FocusId;

use super::{
    Dock, DockLayout, DockLocation, DockRegion, DockSnapshot, DockSplit, DockState, Gesture,
    HostId, PanelId, StripMemo, TabPolicy, WorkspaceSnapshot,
};
use crate::pane_tree::{PaneId, PaneNode, TabGroup};
use crate::split::{Axis, Pane, SplitState};

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
            floating: Vec::new(),
            reservations: Vec::new(),
            confined: Vec::new(),
            recent: [PaneId(0), PaneId(1), PaneId(2), PaneId(3)],
            index: HashMap::new(),
            returns: HashMap::new(),
            next_id: 4,
            next_host: 1,
            revision: 0,
            gesture: Gesture::Idle,
            strips: StripMemo::default(),
        };
        // Empty side regions take no space until a panel arrives.
        for region in [DockRegion::Left, DockRegion::Right, DockRegion::Bottom] {
            state.set_hidden(region, true);
        }
        state
    }

    pub(super) fn split(&self, which: DockSplit) -> &SplitState {
        match which {
            DockSplit::Columns => &self.columns,
            DockSplit::Rows => &self.rows,
        }
    }

    pub(super) fn split_mut(&mut self, which: DockSplit) -> &mut SplitState {
        match which {
            DockSplit::Columns => &mut self.columns,
            DockSplit::Rows => &mut self.rows,
        }
    }

    pub(super) fn set_hidden(&mut self, region: DockRegion, hidden: bool) {
        if region != DockRegion::Center {
            let (split, pane) = region.place();
            self.split_mut(split).set_collapsed(pane, hidden);
        }
    }

    pub(super) fn fresh_id(&mut self) -> PaneId {
        self.next_id += 1;
        PaneId(self.next_id - 1)
    }

    pub fn label(&self, region: DockRegion) -> &'static str {
        self.labels[region.index()]
    }

    /// A group's name, which its tab list publishes: its area's label (a
    /// main host region or a floating host), numbered in reading order when
    /// the area is split, so no two groups share a name.
    pub fn group_label(&self, pane: PaneId) -> String {
        self.areas()
            .find_map(|(host, region, root)| {
                let groups = root.groups();
                let at = groups.iter().position(|g| g.id == pane)?;
                let label = if host == HostId::MAIN {
                    self.label(region)
                } else {
                    self.host_label(host)
                };
                Some(if groups.len() == 1 {
                    label.to_owned()
                } else {
                    format!("{label} {}", at + 1)
                })
            })
            .unwrap_or_default()
    }

    /// Constrain which tab moves cross the main host `region`'s boundary.
    pub fn set_policy(&mut self, region: DockRegion, policy: TabPolicy) {
        self.policies[region.index()] = policy;
    }

    pub fn policy(&self, region: DockRegion) -> TabPolicy {
        self.policies[region.index()]
    }

    /// Keep `panel` in whichever area (main host region, or floating host)
    /// it is in: it can still be reordered and split off inside it.
    pub fn confine(&mut self, panel: PanelId, confined: bool) {
        self.confined.retain(|p| *p != panel);
        if confined {
            self.confined.push(panel);
        }
    }

    /// The main host region's tree of tab groups.
    pub fn root(&self, region: DockRegion) -> &PaneNode {
        &self.roots[region.index()]
    }

    /// The tree of the area `(host, region)`: a main host region, or a
    /// floating host's one tree, whose region is always
    /// [`DockRegion::Center`].
    pub fn area_root(&self, host: HostId, region: DockRegion) -> Option<&PaneNode> {
        if host == HostId::MAIN {
            return Some(&self.roots[region.index()]);
        }
        if region != DockRegion::Center {
            return None;
        }
        self.floating_host(host).map(|h| &h.root)
    }

    pub(super) fn area_root_mut(
        &mut self,
        host: HostId,
        region: DockRegion,
    ) -> Option<&mut PaneNode> {
        if host == HostId::MAIN {
            return Some(&mut self.roots[region.index()]);
        }
        if region != DockRegion::Center {
            return None;
        }
        self.floating
            .iter_mut()
            .find(|h| h.id == host)
            .map(|h| &mut h.root)
    }

    /// Every area and its tree: the main host's regions, then each
    /// floating host in the order they were made.
    pub(super) fn areas(&self) -> impl Iterator<Item = (HostId, DockRegion, &PaneNode)> {
        DockRegion::ALL
            .into_iter()
            .map(|r| (HostId::MAIN, r, &self.roots[r.index()]))
            .chain(
                self.floating
                    .iter()
                    .map(|h| (h.id, DockRegion::Center, &h.root)),
            )
    }

    pub(super) fn trees_mut(&mut self) -> impl Iterator<Item = &mut PaneNode> {
        self.roots
            .iter_mut()
            .chain(self.floating.iter_mut().map(|h| &mut h.root))
    }

    /// The area holding the group or split `pane`.
    pub(super) fn area_of(&self, pane: PaneId) -> Option<(HostId, DockRegion)> {
        self.areas()
            .find(|(_, _, root)| root.group(pane).is_some() || root.split(pane).is_some())
            .map(|(host, region, _)| (host, region))
    }

    /// The host holding the group or split `pane`.
    pub fn host_of(&self, pane: PaneId) -> Option<HostId> {
        self.area_of(pane).map(|(host, _)| host)
    }

    /// The tab policy of an area. A floating host is one area.
    pub fn area_policy(&self, host: HostId, region: DockRegion) -> TabPolicy {
        if host == HostId::MAIN {
            self.policies[region.index()]
        } else {
            self.floating_host(host)
                .map_or(TabPolicy::OPEN, |h| h.policy)
        }
    }

    pub fn group(&self, pane: PaneId) -> Option<&TabGroup> {
        self.areas().find_map(|(_, _, root)| root.group(pane))
    }

    pub(super) fn group_mut(&mut self, pane: PaneId) -> Option<&mut TabGroup> {
        self.trees_mut().find_map(|root| root.group_mut(pane))
    }

    /// Where `panel` is: its region, group, and tab index. A panel in a
    /// floating host reports [`DockRegion::Center`]; [`Self::location`]
    /// names its host too.
    pub fn locate(&self, panel: PanelId) -> Option<(DockRegion, PaneId, usize)> {
        self.location(panel)
            .map(|(at, index)| (at.region, at.pane, index))
    }

    /// Where `panel` is, in any host, and its tab index.
    pub fn location(&self, panel: PanelId) -> Option<(DockLocation, usize)> {
        self.index.get(&panel).copied()
    }

    /// Changes whenever panels, groups, hosts, or region visibility do: a
    /// drop target computed from an older layout is stale.
    pub fn layout_revision(&self) -> u64 {
        self.revision
    }

    /// Every panel in a main host region, group by group.
    pub fn panels(&self, region: DockRegion) -> Vec<PanelId> {
        Self::tree_panels(&self.roots[region.index()])
    }

    pub(super) fn tree_panels(root: &PaneNode) -> Vec<PanelId> {
        root.groups()
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

    pub(super) fn set_recent(&mut self, host: HostId, region: DockRegion, pane: PaneId) {
        if host == HostId::MAIN {
            self.recent[region.index()] = pane;
        } else if let Some(h) = self.floating.iter_mut().find(|h| h.id == host) {
            h.recent = pane;
        }
    }

    /// The active panel of the region's most recently used group.
    pub fn active(&self, region: DockRegion) -> Option<PanelId> {
        self.group(self.recent_group(region))
            .and_then(TabGroup::active_panel)
    }

    /// Shown with its content: has panels, and is not hidden.
    pub fn is_visible(&self, region: DockRegion) -> bool {
        if !self.roots[region.index()].has_panels() {
            return false;
        }
        let (split, pane) = region.place();
        region == DockRegion::Center || !self.split(split).is_collapsed(pane)
    }

    /// A shown main host region, or an existing floating host.
    pub(super) fn area_visible(&self, host: HostId, region: DockRegion) -> bool {
        if host == HostId::MAIN {
            self.is_visible(region)
        } else {
            self.floating_host(host).is_some()
        }
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
            self.changed();
        }
    }

    pub fn toggle(&mut self, region: DockRegion) {
        self.set_visible(region, !self.is_visible(region));
    }

    /// Make `panel` active where it is, or add it at the end of the main
    /// host region's most recently used group. Shows its region.
    pub fn open(&mut self, region: DockRegion, panel: PanelId) {
        let (host, region, pane, index) = match self.location(panel) {
            Some((at, index)) => (at.host, at.region, at.pane, index),
            None => {
                let pane = self.recent_group(region);
                let group = self.group_mut(pane).expect("recent group exists");
                group.panels.push(panel);
                (HostId::MAIN, region, pane, group.panels.len() - 1)
            }
        };
        if host == HostId::MAIN {
            self.set_hidden(region, false);
        }
        self.changed();
        self.select(pane, index);
    }

    pub fn select(&mut self, pane: PaneId, index: usize) {
        let Some((host, region)) = self.area_of(pane) else {
            return;
        };
        if let Some(group) = self.group_mut(pane)
            && index < group.panels.len()
        {
            group.active = index;
            self.set_recent(host, region, pane);
        }
    }

    /// Remove a panel. The neighbor after it becomes active (or before it,
    /// at the end); an emptied group is removed, a side region left empty
    /// hides, and a floating host left empty is removed (see
    /// [`Self::hosts`]).
    pub fn close(&mut self, pane: PaneId, index: usize) -> Option<PanelId> {
        self.close_tab(pane, index).map(|(panel, _)| panel)
    }

    /// [`Self::close`], also naming a floating host it emptied.
    pub(super) fn close_tab(
        &mut self,
        pane: PaneId,
        index: usize,
    ) -> Option<(PanelId, Option<HostId>)> {
        let (host, region) = self.area_of(pane)?;
        let removed = self.group_mut(pane)?.remove(index)?;
        self.confined.retain(|p| *p != removed);
        self.returns.remove(&removed);
        let emptied = self.settle(host, region).then_some(host);
        self.changed();
        Some((removed, emptied))
    }

    /// Prune an area's tree after a removal: a side region left empty
    /// hides, and a floating host left empty is removed, returning true.
    pub(super) fn settle(&mut self, host: HostId, region: DockRegion) -> bool {
        if host == HostId::MAIN {
            let i = region.index();
            let placeholder = PaneNode::Tabs(TabGroup::new(self.roots[i].groups()[0].id));
            let root = std::mem::replace(&mut self.roots[i], placeholder);
            if let Some(pruned) = root.prune() {
                self.roots[i] = pruned;
            }
            if self.panels(region).is_empty() {
                self.set_hidden(region, true);
            }
            return false;
        }
        let Some(at) = self.floating.iter().position(|h| h.id == host) else {
            return false;
        };
        let placeholder = PaneNode::Tabs(TabGroup::new(self.floating[at].root.id()));
        let root = std::mem::replace(&mut self.floating[at].root, placeholder);
        match root.prune() {
            Some(pruned) => {
                self.floating[at].root = pruned;
                false
            }
            None => {
                self.floating.remove(at);
                true
            }
        }
    }

    /// After any change to panels, groups, hosts, or visibility: a new
    /// revision, the panel lookup rebuilt, and (in debug builds) every
    /// invariant checked.
    pub(super) fn changed(&mut self) {
        self.revision += 1;
        self.index = self.build_index();
        self.debug_verify();
    }

    pub(super) fn build_index(&self) -> HashMap<PanelId, (DockLocation, usize)> {
        let mut index = HashMap::new();
        for (host, region, root) in self.areas() {
            for group in root.groups() {
                for (i, &panel) in group.panels.iter().enumerate() {
                    let at = DockLocation {
                        host,
                        region,
                        pane: group.id,
                    };
                    index.entry(panel).or_insert((at, i));
                }
            }
        }
        index
    }

    /// The panel whose tab has `focus`, in any host: a group's
    /// [`Dock::tab_focus`] target, meaning its active panel, or an inactive
    /// tab's own target, which assistive technology can focus without
    /// selecting the tab.
    pub fn focused_panel(&self, focus: Option<FocusId>) -> Option<PanelId> {
        let focus = focus?;
        self.areas()
            .flat_map(|(_, _, root)| root.groups())
            .find_map(|g| {
                if Dock::tab_focus(g.id) == focus {
                    return g.active_panel();
                }
                g.panels
                    .iter()
                    .copied()
                    .find(|&p| FocusId::from_key(&Dock::tab_id(p)) == focus)
            })
    }

    /// The main host's layout, in the format from before floating hosts.
    /// [`Self::workspace_snapshot`] saves the whole workspace.
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

    /// Restore a main host snapshot, closing every floating host. Panels
    /// `keep` rejects (ones the app no longer has) are dropped, and so are
    /// repeats; emptied groups are pruned and regions left empty hide.
    /// Groups get fresh ids.
    pub fn restore(&mut self, snapshot: &DockSnapshot, keep: impl Fn(PanelId) -> bool) {
        self.restore_workspace(&WorkspaceSnapshot::from(snapshot.clone()), keep);
    }
}
