//! Hosts of a dock workspace, and the one checked transfer every move goes
//! through.
//!
//! The main host is the four region layout; each floating host is one
//! [`PaneNode`] tree the app shows in a window of its own. A policy
//! boundary is an area: a `(host, region)` pair, where a floating host is
//! the single area `(host, Center)`. Moving a tab to another window crosses
//! a boundary like moving it to another region does, so
//! [`TabPolicy::SEALED`], `can_leave`, `accepts`, and
//! [`DockState::confine`] hold across windows.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::{Dock, DockRegion, DockState, PanelId, TabPolicy, crossing_refusal, drop_is_noop};
use crate::pane_tree::{DropZone, PaneDrop, PaneId, PaneNode, TabGroup};
use quark_ui::FocusId;

/// Identity of a dock host: [`HostId::MAIN`], or a floating host the app
/// binds to a window. Persisted, and never reused within a workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct HostId(pub u64);

impl HostId {
    /// The four region layout every dock has.
    pub const MAIN: Self = Self(0);
}

/// A tab group and the area it is in. A floating host's region is
/// [`DockRegion::Center`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DockLocation {
    pub host: HostId,
    pub region: DockRegion,
    pub pane: PaneId,
}

/// Where a move lands: in or beside the group `pane` of `host`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DockDestination {
    pub host: HostId,
    pub pane: PaneId,
    pub zone: DropZone,
}

/// What a move carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MovePayload {
    /// One tab.
    Panel(PanelId),
    /// Every tab of a group, in order, keeping its active tab.
    Group(PaneId),
}

/// An entry of a move menu ([`DockState::move_options`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoveTarget {
    /// Into the group, after its tabs.
    Group { host: HostId, pane: PaneId },
    /// "Move to new window" or, for a group, "Move group to new window".
    NewHost,
}

/// Which side of an area boundary refuses a move.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Boundary {
    /// The panel is [`DockState::confine`]d to its area.
    Confined,
    /// Its area's [`TabPolicy::can_leave`] is false.
    CannotLeave,
    /// The destination area's [`TabPolicy::accepts`] is false.
    NotAccepted,
}

/// Why a move was refused. A refused move changes nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferRefusal {
    /// The payload's panel is not docked, or its group does not exist or
    /// has no tabs.
    MissingSource,
    /// The payload is no longer where it was when the move was prepared.
    Stale,
    /// The destination group is gone, or not in the named host.
    MissingDestination,
    /// No such floating host, or reservation.
    MissingHost(HostId),
    /// The move would leave the layout as it is.
    NoMove,
    /// `panel` may not cross the boundary between the two areas. For a
    /// floating host's close, the first panel with no legal destination.
    Policy { panel: PanelId, boundary: Boundary },
}

/// What a settled move asks of the app. Moves report it; refused ones
/// return a [`TransferRefusal`] instead.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DockEffects {
    /// The panels moved, in order.
    pub moved: Vec<PanelId>,
    /// The group that received the move's active panel. Focus its tab
    /// ([`Self::focus`]) once its host's window is shown, and announce it.
    pub destination: Option<DockLocation>,
    /// Announce the move once, naming the moved panels and destination.
    pub announce: bool,
    /// The layout changed: save it.
    pub persist: bool,
    /// Floating hosts the change emptied and removed: close their windows
    /// (as emptied, not by the user) after the destination is shown.
    pub closed_hosts: Vec<HostId>,
    /// A floating host reserved for a new window: open one for it, then
    /// [`DockState::commit_host`] once it exists, or
    /// [`DockState::abort_host`] when it could not be made.
    pub create_host: Option<HostId>,
    /// A floating host a live tear-off made, already holding its panels:
    /// open a window for it now, following the pointer, and
    /// [`DockState::cancel_live_detach`] when it could not be made.
    pub open_host: Option<HostId>,
}

impl DockEffects {
    /// The destination tab's focus target, and the host whose window takes
    /// it.
    pub fn focus(&self) -> Option<(HostId, FocusId)> {
        self.destination.map(|d| (d.host, Dock::tab_focus(d.pane)))
    }
}

/// Where a prepared transfer goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Target {
    Pane(DockDestination),
    /// A floating host made by the commit.
    NewHost(HostId),
}

/// A move checked against the layout by [`DockState::prepare`], to apply
/// with [`DockState::commit`].
#[derive(Debug, Clone, PartialEq)]
pub struct Transfer {
    payload: MovePayload,
    to: Target,
    /// Each moving panel, in order, and where it was.
    sources: Vec<(PanelId, DockLocation)>,
}

impl Transfer {
    pub fn payload(&self) -> MovePayload {
        self.payload
    }

    /// The panels that move, in order.
    pub fn panels(&self) -> Vec<PanelId> {
        self.sources.iter().map(|(p, _)| *p).collect()
    }

    /// The group the panels leave.
    pub(super) fn source(&self) -> DockLocation {
        self.sources[0].1
    }
}

/// A floating host: one tree in a window of its own.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct FloatingHost {
    pub(super) id: HostId,
    pub(super) root: PaneNode,
    pub(super) policy: TabPolicy,
    /// The group last selected or dropped into.
    pub(super) recent: PaneId,
    pub(super) label: Option<String>,
}

impl FloatingHost {
    pub(super) fn new(id: HostId, root: PaneNode) -> Self {
        let recent = root.groups()[0].id;
        Self {
            id,
            root,
            policy: TabPolicy::OPEN,
            recent,
            label: None,
        }
    }
}

/// One panel's place in a re-dock: the main host group and, for a
/// recorded return location, the tab index to insert at.
type Redock = (PanelId, DockRegion, PaneId, Option<usize>);

impl DockState {
    pub(super) fn floating_host(&self, host: HostId) -> Option<&FloatingHost> {
        self.floating.iter().find(|h| h.id == host)
    }

    /// The floating hosts, in the order they were made.
    pub fn hosts(&self) -> Vec<HostId> {
        self.floating.iter().map(|h| h.id).collect()
    }

    /// A floating host's tree.
    pub fn host_root(&self, host: HostId) -> Option<&PaneNode> {
        self.floating_host(host).map(|h| &h.root)
    }

    /// Constrain which tab moves cross a floating host's boundary. Hosts
    /// start [`TabPolicy::OPEN`].
    pub fn set_host_policy(&mut self, host: HostId, policy: TabPolicy) {
        if let Some(h) = self.floating.iter_mut().find(|h| h.id == host) {
            h.policy = policy;
        }
    }

    /// Name a floating host's tab lists for assistive tech. Not persisted;
    /// unnamed hosts use the center's label.
    pub fn set_host_label(&mut self, host: HostId, label: impl Into<String>) {
        if let Some(h) = self.floating.iter_mut().find(|h| h.id == host) {
            h.label = Some(label.into());
        }
    }

    /// The label of a floating host's tab lists.
    pub fn host_label(&self, host: HostId) -> &str {
        self.floating_host(host)
            .and_then(|h| h.label.as_deref())
            .unwrap_or(self.label(DockRegion::Center))
    }

    /// Where closing `panel`'s floating host returns it: the main host
    /// group and tab index it left from. The group may since have gone,
    /// when re-docking falls back to its region.
    pub fn return_location(&self, panel: PanelId) -> Option<(DockLocation, usize)> {
        let &(region, pane, index) = self.returns.get(&panel)?;
        Some((
            DockLocation {
                host: HostId::MAIN,
                region,
                pane,
            },
            index,
        ))
    }

    /// Check moving `payload` to `destination` under every rule: the
    /// payload and destination exist, crossing an area boundary is allowed
    /// for every moving panel, and the move changes something. Changes
    /// nothing.
    pub fn prepare(
        &self,
        payload: MovePayload,
        destination: DockDestination,
    ) -> Result<Transfer, TransferRefusal> {
        self.prepare_to(payload, Target::Pane(destination))
    }

    pub(super) fn prepare_to(
        &self,
        payload: MovePayload,
        to: Target,
    ) -> Result<Transfer, TransferRefusal> {
        let sources = self.sources(payload)?;
        let from = sources[0].1;
        let (to_host, to_region, enter) = match to {
            Target::Pane(d) => {
                let (host, region) = self
                    .area_of(d.pane)
                    .filter(|(host, _)| *host == d.host)
                    .ok_or(TransferRefusal::MissingDestination)?;
                if self.group(d.pane).is_none() {
                    return Err(TransferRefusal::MissingDestination);
                }
                let noop = match payload {
                    MovePayload::Group(g) => g == d.pane,
                    MovePayload::Panel(panel) => {
                        let (at, index) = self.location(panel).expect("source is docked");
                        let count = self.group(at.pane).map_or(0, |g| g.panels.len());
                        drop_is_noop(
                            at.pane,
                            index,
                            count,
                            PaneDrop {
                                pane: d.pane,
                                zone: d.zone,
                            },
                        )
                    }
                };
                if noop {
                    return Err(TransferRefusal::NoMove);
                }
                (host, region, self.area_policy(host, region))
            }
            Target::NewHost(host) => {
                if self.floating_host(host).is_some() {
                    return Err(TransferRefusal::MissingDestination);
                }
                // Everything a floating host has, already alone in it.
                if from.host != HostId::MAIN
                    && self
                        .host_root(from.host)
                        .is_some_and(|root| Self::tree_panels(root).len() == sources.len())
                {
                    return Err(TransferRefusal::NoMove);
                }
                (host, DockRegion::Center, TabPolicy::OPEN)
            }
        };
        if (from.host, from.region) != (to_host, to_region) {
            let leave = self.area_policy(from.host, from.region);
            for &(panel, _) in &sources {
                if let Some(boundary) =
                    crossing_refusal(leave, enter, self.confined.contains(&panel))
                {
                    return Err(TransferRefusal::Policy { panel, boundary });
                }
            }
        }
        Ok(Transfer {
            payload,
            to,
            sources,
        })
    }

    /// The panels `payload` moves, in order, and where each is.
    fn sources(
        &self,
        payload: MovePayload,
    ) -> Result<Vec<(PanelId, DockLocation)>, TransferRefusal> {
        let sources: Vec<_> = match payload {
            MovePayload::Panel(panel) => self
                .location(panel)
                .map(|(at, _)| (panel, at))
                .into_iter()
                .collect(),
            MovePayload::Group(pane) => self
                .group(pane)
                .into_iter()
                .flat_map(|g| g.panels.iter())
                .filter_map(|&p| Some((p, self.location(p)?.0)))
                .collect(),
        };
        if sources.is_empty() {
            return Err(TransferRefusal::MissingSource);
        }
        Ok(sources)
    }

    /// Apply a prepared transfer, checking it again against the layout as
    /// it is now: a payload that has since moved is [`TransferRefusal::Stale`],
    /// and the rules may since refuse it. Atomic: it moves every panel of
    /// the payload or none.
    pub fn commit(&mut self, transfer: Transfer) -> Result<DockEffects, TransferRefusal> {
        let fresh = self.prepare_to(transfer.payload, transfer.to)?;
        if fresh.sources != transfer.sources {
            return Err(TransferRefusal::Stale);
        }
        Ok(self.apply_transfer(fresh))
    }

    /// [`Self::prepare`] then [`Self::commit`]: what drops, keyboard moves,
    /// and group moves call.
    pub fn transfer(
        &mut self,
        payload: MovePayload,
        destination: DockDestination,
    ) -> Result<DockEffects, TransferRefusal> {
        let transfer = self.prepare(payload, destination)?;
        self.commit(transfer)
    }

    /// Move a checked transfer's panels: out of their group, then into the
    /// destination group, beside it, or into a new floating host. Prunes
    /// emptied groups and hosts, and records where panels leaving the main
    /// host came from.
    pub(super) fn apply_transfer(&mut self, t: Transfer) -> DockEffects {
        let from = t.sources[0].1;
        let returns: Vec<(PanelId, usize)> = t
            .sources
            .iter()
            .map(|&(p, _)| (p, self.location(p).map_or(0, |(_, i)| i)))
            .collect();
        let source = self.group_mut(from.pane).expect("prepared source exists");
        let (panels, active) = match t.payload {
            MovePayload::Panel(panel) => {
                if let Some(i) = source.panels.iter().position(|p| *p == panel) {
                    source.remove(i);
                }
                (vec![panel], 0)
            }
            MovePayload::Group(_) => {
                let active = source.active;
                source.active = 0;
                (std::mem::take(&mut source.panels), active)
            }
        };
        let (to_host, to_region, pane) = match t.to {
            Target::Pane(d) => {
                let (host, region) = self.area_of(d.pane).expect("prepared destination exists");
                match d.zone.edge() {
                    None => {
                        let index = match d.zone {
                            DropZone::Tabs(i) => i,
                            _ => usize::MAX,
                        };
                        let group = self.group_mut(d.pane).expect("destination exists");
                        group.insert_all(index, &panels, active);
                        (host, region, d.pane)
                    }
                    Some((axis, first)) => {
                        let group = TabGroup {
                            id: self.fresh_id(),
                            panels: panels.clone(),
                            active,
                        };
                        let new_group = group.id;
                        let split_id = self.fresh_id();
                        self.area_root_mut(host, region)
                            .expect("destination area exists")
                            .insert_beside(d.pane, axis, first, group, split_id);
                        (host, region, new_group)
                    }
                }
            }
            Target::NewHost(host) => {
                let group = TabGroup {
                    id: self.fresh_id(),
                    panels: panels.clone(),
                    active,
                };
                let pane = group.id;
                self.floating
                    .push(FloatingHost::new(host, PaneNode::Tabs(group)));
                (host, DockRegion::Center, pane)
            }
        };
        let mut closed_hosts = Vec::new();
        if self.settle(from.host, from.region) {
            closed_hosts.push(from.host);
        }
        if (to_host, to_region) != (from.host, from.region) {
            self.settle(to_host, to_region);
        }
        self.set_recent(to_host, to_region, pane);
        if to_host == HostId::MAIN {
            self.set_hidden(to_region, false);
            for (panel, _) in &returns {
                self.returns.remove(panel);
            }
        } else if from.host == HostId::MAIN {
            for (panel, index) in returns {
                self.returns.insert(panel, (from.region, from.pane, index));
            }
        }
        self.changed();
        DockEffects {
            moved: panels,
            destination: Some(DockLocation {
                host: to_host,
                region: to_region,
                pane,
            }),
            announce: true,
            persist: true,
            closed_hosts,
            ..DockEffects::default()
        }
    }

    /// Reserve a floating host for moving `payload` to a new window, the
    /// first phase of "Move to new window": checks the move as if the host
    /// existed (a new host is [`TabPolicy::OPEN`]), and leaves the panels
    /// where they are. Open a window for the returned host, then
    /// [`Self::commit_host`] or [`Self::abort_host`].
    pub fn reserve_host(&mut self, payload: MovePayload) -> Result<HostId, TransferRefusal> {
        let host = HostId(self.next_host);
        let transfer = self.prepare_to(payload, Target::NewHost(host))?;
        self.next_host += 1;
        self.reservations.push((host, transfer));
        Ok(host)
    }

    /// The window for a reserved host exists: move the payload into it.
    /// Refused, dropping the reservation, when the payload has since moved
    /// or the rules now refuse it; close the window then.
    pub fn commit_host(&mut self, host: HostId) -> Result<DockEffects, TransferRefusal> {
        let at = self
            .reservations
            .iter()
            .position(|(h, _)| *h == host)
            .ok_or(TransferRefusal::MissingHost(host))?;
        let (_, transfer) = self.reservations.remove(at);
        self.commit(transfer)
    }

    /// The window for a reserved host could not be made: drop the
    /// reservation. The panels never left. Returns false for a host that
    /// was not reserved.
    pub fn abort_host(&mut self, host: HostId) -> bool {
        let before = self.reservations.len();
        self.reservations.retain(|(h, _)| *h != host);
        self.reservations.len() != before
    }

    /// Whether [`Self::close_host`] would re-dock every panel of `host`.
    pub fn can_close_host(&self, host: HostId) -> Result<(), TransferRefusal> {
        self.redock_plan(host, false).map(|_| ())
    }

    /// The user closed a floating host's window: put all its panels back
    /// in the main host, each at its return location when that area still
    /// takes it, else in the first main host region that does (center,
    /// then left, bottom, right). Atomic: refused, changing nothing, when
    /// any panel has no legal destination; keep the window open then and
    /// explain why.
    ///
    /// Not for quitting the app, which keeps floating hosts to persist
    /// them, or for a host whose window could not be made
    /// ([`Self::recover_host`]).
    pub fn close_host(&mut self, host: HostId) -> Result<DockEffects, TransferRefusal> {
        let plan = self.redock_plan(host, false)?;
        Ok(self.apply_redock(host, plan))
    }

    /// A floating host's window could not be opened (after a restore, say):
    /// re-dock its panels like [`Self::close_host`], but put those with no
    /// legal destination in the center's recent group rather than lose
    /// them.
    pub fn recover_host(&mut self, host: HostId) -> Result<DockEffects, TransferRefusal> {
        let plan = self.redock_plan(host, true)?;
        Ok(self.apply_redock(host, plan))
    }

    /// Where each of `host`'s panels goes when it closes, in reading order;
    /// with `force`, past the rules instead of refusing.
    fn redock_plan(&self, host: HostId, force: bool) -> Result<Vec<Redock>, TransferRefusal> {
        let h = self
            .floating_host(host)
            .ok_or(TransferRefusal::MissingHost(host))?;
        let center = (
            DockRegion::Center,
            self.recent_group(DockRegion::Center),
            None,
        );
        let mut plan = Vec::new();
        for panel in Self::tree_panels(&h.root) {
            match self.redock_destination(h.policy, panel) {
                Ok((region, pane, index)) => plan.push((panel, region, pane, index)),
                Err(_) if force => plan.push((panel, center.0, center.1, center.2)),
                Err(boundary) => return Err(TransferRefusal::Policy { panel, boundary }),
            }
        }
        Ok(plan)
    }

    /// The main host group `panel` re-docks into from a floating host
    /// with policy `leave`: its return location, its return region's
    /// recent group, or the first region that takes it.
    fn redock_destination(
        &self,
        leave: TabPolicy,
        panel: PanelId,
    ) -> Result<(DockRegion, PaneId, Option<usize>), Boundary> {
        let candidates = self
            .returns
            .get(&panel)
            .map(|&(region, pane, index)| {
                if self.roots[region.index()].group(pane).is_some() {
                    (region, pane, Some(index))
                } else {
                    (region, self.recent_group(region), None)
                }
            })
            .into_iter()
            .chain(
                [
                    DockRegion::Center,
                    DockRegion::Left,
                    DockRegion::Bottom,
                    DockRegion::Right,
                ]
                .map(|r| (r, self.recent_group(r), None)),
            );
        let confined = self.confined.contains(&panel);
        let mut refusal = Boundary::NotAccepted;
        for (region, pane, index) in candidates {
            match crossing_refusal(leave, self.policies[region.index()], confined) {
                None => return Ok((region, pane, index)),
                Some(b) => refusal = b,
            }
        }
        Err(refusal)
    }

    fn apply_redock(&mut self, host: HostId, plan: Vec<Redock>) -> DockEffects {
        let at = self
            .floating
            .iter()
            .position(|h| h.id == host)
            .expect("planned host exists");
        let closing = self.floating.remove(at);
        let focus = closing
            .root
            .group(closing.recent)
            .or(closing.root.groups().first().copied())
            .and_then(TabGroup::active_panel);
        // Each destination group shows the last panel it took, except the
        // closing host's active one, which shows wherever it lands.
        let mut shown: HashMap<PaneId, PanelId> = HashMap::new();
        for &(panel, region, pane, index) in &plan {
            if let Some(group) = self.group_mut(pane) {
                group.insert(index.unwrap_or(usize::MAX), panel);
            }
            self.set_hidden(region, false);
            self.returns.remove(&panel);
            if !focus.is_some_and(|f| shown.get(&pane) == Some(&f)) {
                shown.insert(pane, panel);
            }
        }
        for (&pane, &panel) in &shown {
            if let Some(group) = self.group_mut(pane)
                && let Some(i) = group.panels.iter().position(|p| *p == panel)
            {
                group.active = i;
            }
        }
        let destination =
            focus
                .and_then(|f| plan.iter().find(|(p, ..)| *p == f))
                .map(|&(_, region, pane, _)| DockLocation {
                    host: HostId::MAIN,
                    region,
                    pane,
                });
        if let Some(d) = destination {
            self.recent[d.region.index()] = d.pane;
        }
        self.changed();
        DockEffects {
            moved: plan.iter().map(|(p, ..)| *p).collect(),
            destination,
            announce: true,
            persist: true,
            closed_hosts: vec![host],
            ..DockEffects::default()
        }
    }

    /// What a "Move…" menu for `payload` offers, in order: every shown
    /// group in any host it can move into, then a new window when it can
    /// move to one.
    pub fn move_options(&self, payload: MovePayload) -> Vec<MoveTarget> {
        let mut options: Vec<MoveTarget> = std::iter::once(HostId::MAIN)
            .chain(self.hosts())
            .flat_map(|host| {
                self.shown_groups(host)
                    .into_iter()
                    .map(move |pane| (host, pane))
            })
            .filter(|&(host, pane)| {
                let d = DockDestination {
                    host,
                    pane,
                    zone: DropZone::Center,
                };
                self.prepare(payload, d).is_ok()
            })
            .map(|(host, pane)| MoveTarget::Group { host, pane })
            .collect();
        if self
            .prepare_to(payload, Target::NewHost(HostId(self.next_host)))
            .is_ok()
        {
            options.push(MoveTarget::NewHost);
        }
        options
    }
}
