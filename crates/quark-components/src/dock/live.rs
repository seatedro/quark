//! Live tear-off: a dragged tab or group leaves its window for a floating
//! host of its own while the drag is still going, and the drag then
//! decides where it ends up.
//!
//! [`DockState::begin_live_detach`] moves the payload into a new floating
//! host at once, through the same checks as every other move, and keeps
//! an exact way back. Then one of:
//!
//! - [`DockState::drop_live_detach`]: released over a group, the host's
//!   panels move there like any transfer, and the emptied host closes.
//! - [`DockState::end_live_detach`]: released over no target, it stays a
//!   floating window.
//! - [`DockState::cancel_live_detach`]: Escape, a lost capture, or a window
//!   that could not be made, and the panels go back exactly where they
//!   were, the host left empty to close.
//!
//! Platforms without live tear-off use the two phase reservation instead
//! ([`DockState::reserve_host`]).

use super::workspace::{FloatingHost, Target};
use super::{
    DockEffects, DockLocation, DockRegion, DockState, HostId, MovePayload, PanelId, TransferRefusal,
};
use crate::pane_tree::{PaneId, PaneNode, TabGroup};
use crate::split::Axis;

/// A live tear-off in progress, and how to undo it.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct LiveDetach {
    pub(super) host: HostId,
    /// The panels torn off, in order.
    panels: Vec<PanelId>,
    /// The group they left, and the first one's tab index there.
    from: DockLocation,
    index: usize,
    /// That group's active panel before.
    active: Option<PanelId>,
    /// When the group went with them: the group to put a new one beside.
    beside: Option<(PaneId, Axis, bool)>,
    /// The source area's tree before and after the tear-off: when it is
    /// still as it was after, cancelling puts back the tree before, split
    /// positions and ids included.
    before: PaneNode,
    after: PaneNode,
    /// The source area's recent group, and whether a side region was
    /// hidden.
    recent: PaneId,
    hidden: bool,
}

impl DockState {
    /// Whether `payload` may tear off into a window of its own: it may
    /// leave its area, and is not already alone in a floating host.
    /// Confined panels and [`super::TabPolicy::SEALED`] areas never tear
    /// off.
    pub fn can_live_detach(&self, payload: MovePayload) -> bool {
        self.prepare_to(payload, Target::NewHost(HostId(self.next_host)))
            .is_ok()
    }

    /// Tear `payload` off into a new floating host now, mid drag: the
    /// first step of a live tear-off. The effects name the host to open a
    /// window for ([`DockEffects::open_host`]); nothing is announced or
    /// saved until the drag ends.
    pub fn begin_live_detach(
        &mut self,
        payload: MovePayload,
    ) -> Result<DockEffects, TransferRefusal> {
        let host = HostId(self.next_host);
        let transfer = self.prepare_to(payload, Target::NewHost(host))?;
        let from = transfer.source();
        let panels = transfer.panels();
        let index = self.location(panels[0]).map_or(0, |(_, i)| i);
        let before = self
            .area_root(from.host, from.region)
            .expect("source area exists")
            .clone();
        let removes_group = self
            .group(from.pane)
            .is_some_and(|g| g.panels.len() == panels.len());
        let record = LiveDetach {
            host,
            panels,
            from,
            index,
            active: self.group(from.pane).and_then(TabGroup::active_panel),
            beside: removes_group.then(|| before.neighbor(from.pane)).flatten(),
            recent: self.area_recent(from.host, from.region),
            hidden: self.is_hidden(from.region) && from.host == HostId::MAIN,
            after: before.clone(),
            before,
        };
        self.next_host += 1;
        let mut effects = self.apply_transfer(transfer);
        let after = self
            .area_root(from.host, from.region)
            .expect("a host is never torn off whole")
            .clone();
        self.live.push(LiveDetach { after, ..record });
        effects.announce = false;
        effects.persist = false;
        effects.open_host = Some(host);
        Ok(effects)
    }

    /// Whether `host` is a live tear-off whose drag has not ended.
    pub fn is_live(&self, host: HostId) -> bool {
        self.live.iter().any(|l| l.host == host)
    }

    /// The torn off host was released over a group: move its panels there,
    /// under the usual checks, and close the emptied host
    /// ([`DockEffects::closed_hosts`]). Refused, the host stays live; end
    /// or cancel the drag then.
    pub fn drop_live_detach(
        &mut self,
        host: HostId,
        destination: super::DockDestination,
    ) -> Result<DockEffects, TransferRefusal> {
        if !self.is_live(host) {
            return Err(TransferRefusal::MissingHost(host));
        }
        let root = self.host_root(host).expect("live hosts exist");
        let PaneNode::Tabs(group) = root else {
            return Err(TransferRefusal::Stale);
        };
        self.transfer(MovePayload::Group(group.id), destination)
    }

    /// The torn off host was released over no target: it stays a floating
    /// window, and the move is announced and saved. Its panels return to
    /// where they left from when it is closed.
    pub fn end_live_detach(&mut self, host: HostId) -> Result<DockEffects, TransferRefusal> {
        let at = self
            .live
            .iter()
            .position(|l| l.host == host)
            .ok_or(TransferRefusal::MissingHost(host))?;
        let record = self.live.remove(at);
        let root = self.host_root(host).expect("live hosts exist");
        let pane = root.groups()[0].id;
        self.changed();
        Ok(DockEffects {
            moved: record.panels,
            destination: Some(DockLocation {
                host,
                region: DockRegion::Center,
                pane,
            }),
            announce: true,
            persist: true,
            ..DockEffects::default()
        })
    }

    /// Escape, a lost capture, or a window that could not be made: put the
    /// torn off panels back exactly where they were (the tree as it was,
    /// when nothing else changed it since; else their group and tab index,
    /// a new group where theirs was, or their area) and remove the host,
    /// reported in [`DockEffects::closed_hosts`] to close its window.
    /// Silent: nothing to announce or save. Refused as stale when the host
    /// no longer holds just those panels.
    pub fn cancel_live_detach(&mut self, host: HostId) -> Result<DockEffects, TransferRefusal> {
        let at = self
            .live
            .iter()
            .position(|l| l.host == host)
            .ok_or(TransferRefusal::MissingHost(host))?;
        let groups = self.host_root(host).map(PaneNode::groups);
        let Some([group]) = groups.as_deref() else {
            return Err(TransferRefusal::Stale);
        };
        if group.panels != self.live[at].panels {
            return Err(TransferRefusal::Stale);
        }
        let active = group.active;
        let record = self.live.remove(at);
        self.floating.retain(|h| h.id != host);
        let from = record.from;
        let area = (from.host, from.region);
        if self.area_root(area.0, area.1) == Some(&record.after) {
            *self.area_root_mut(area.0, area.1).expect("area exists") = record.before;
            self.set_recent(area.0, area.1, record.recent);
            if area.0 == HostId::MAIN {
                self.set_hidden(area.1, record.hidden);
            }
        } else {
            self.put_back(&record, active);
        }
        for panel in &record.panels {
            if self
                .location_in_trees(*panel)
                .is_some_and(|h| h == HostId::MAIN)
            {
                self.returns.remove(panel);
            }
        }
        self.changed();
        let shown = self.group_of(record.panels[0]);
        Ok(DockEffects {
            moved: record.panels,
            destination: shown,
            closed_hosts: vec![host],
            ..DockEffects::default()
        })
    }

    /// Put a cancelled tear-off's panels back when their area changed
    /// since: into their group at their index, beside where their group
    /// was, into their area's recent group, or, their floating host gone,
    /// the main host's center.
    fn put_back(&mut self, record: &LiveDetach, active: usize) {
        let from = record.from;
        let area_exists = self.area_root(from.host, from.region).is_some();
        let in_area = |state: &Self, pane: PaneId| {
            state.area_of(pane) == Some((from.host, from.region)) && state.group(pane).is_some()
        };
        let (host, region) = if area_exists {
            (from.host, from.region)
        } else {
            (HostId::MAIN, DockRegion::Center)
        };
        let pane = if in_area(self, from.pane) {
            let group = self.group_mut(from.pane).expect("group exists");
            group.insert_all(record.index, &record.panels, active);
            if let Some(i) = record
                .active
                .and_then(|a| group.panels.iter().position(|p| *p == a))
            {
                group.active = i;
            }
            from.pane
        } else if let Some((beside, axis, first)) =
            record.beside.filter(|(pane, ..)| in_area(self, *pane))
        {
            let group = TabGroup {
                id: self.fresh_id(),
                panels: record.panels.clone(),
                active,
            };
            let pane = group.id;
            let split = self.fresh_id();
            self.area_root_mut(host, region)
                .expect("area exists")
                .insert_beside(beside, axis, first, group, split);
            pane
        } else {
            let pane = self.area_recent(host, region);
            let group = self.group_mut(pane).expect("recent group exists");
            let end = group.panels.len();
            group.insert_all(end, &record.panels, active);
            pane
        };
        // Beside an empty root group, the new split needs pruning.
        self.settle(host, region);
        let pane = if self.group(pane).is_some() {
            pane
        } else {
            self.area_recent(host, region)
        };
        self.set_recent(host, region, pane);
        if host == HostId::MAIN {
            self.set_hidden(region, false);
        }
    }

    /// The group an area adds to: its recent one, else its first.
    fn area_recent(&self, host: HostId, region: DockRegion) -> PaneId {
        if host == HostId::MAIN {
            return self.recent_group(region);
        }
        let h: &FloatingHost = self.floating_host(host).expect("host exists");
        if h.root.group(h.recent).is_some() {
            h.recent
        } else {
            h.root.groups()[0].id
        }
    }

    fn is_hidden(&self, region: DockRegion) -> bool {
        let (split, pane) = region.place();
        region != DockRegion::Center && self.split(split).is_collapsed(pane)
    }

    /// The host whose tree holds `panel`, read from the trees: for use
    /// before the lookup is rebuilt.
    fn location_in_trees(&self, panel: PanelId) -> Option<HostId> {
        self.areas()
            .find(|(_, _, root)| root.groups().iter().any(|g| g.panels.contains(&panel)))
            .map(|(host, ..)| host)
    }

    fn group_of(&self, panel: PanelId) -> Option<DockLocation> {
        self.location(panel).map(|(at, _)| at)
    }
}
