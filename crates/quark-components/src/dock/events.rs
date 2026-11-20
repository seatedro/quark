//! Moves and the events a dock's element emits: drops, keyboard moves,
//! divider drags, and the preview of the drag under way.

use super::{
    DockDestination, DockEvent, DockOutcome, DockRegion, DockState, Gesture, HostId, MIN_GROUP,
    MovePayload, PaneDividerEvent, PanelId, TREE_ORDER, TabMove,
};
use crate::pane_tree::{
    DropZone, PaneDrop, PaneId, child_sizes, divider_min, divider_span, push_divider,
};

/// The drop a [`DockEvent::MoveTab`] makes.
fn move_drop(pane: PaneId, index: Option<usize>) -> PaneDrop {
    PaneDrop {
        pane,
        zone: index.map_or(DropZone::Center, DropZone::Tabs),
    }
}

impl DockState {
    /// The destination a drop on `target` names: its group's host.
    fn drop_destination(&self, target: PaneDrop) -> Option<DockDestination> {
        let host = self.host_of(target.pane)?;
        Some(DockDestination {
            host,
            pane: target.pane,
            zone: target.zone,
        })
    }

    /// Whether dropping `panel` on `target` would move it, under the
    /// areas' [`TabPolicy`]s and [`Self::confine`].
    pub fn can_drop(&self, panel: PanelId, target: PaneDrop) -> bool {
        self.drop_destination(target)
            .is_some_and(|d| self.prepare(MovePayload::Panel(panel), d).is_ok())
    }

    /// Move `panel` to `target`: reorder it in its strip, move it into
    /// another group, or split `target`'s group and put it in a new group
    /// on that side. Returns false, changing nothing, when
    /// [`Self::can_drop`] refuses.
    pub fn drop_panel(&mut self, panel: PanelId, target: PaneDrop) -> bool {
        self.drop_destination(target)
            .is_some_and(|d| self.transfer(MovePayload::Panel(panel), d).is_ok())
    }

    /// Every group of `host` that is shown, in tree order: for the main
    /// host left, center, bottom, right, and within a region its groups in
    /// reading order.
    pub(super) fn shown_groups(&self, host: HostId) -> Vec<PaneId> {
        if host != HostId::MAIN {
            return self
                .floating_host(host)
                .map(|h| h.root.groups().iter().map(|g| g.id).collect())
                .unwrap_or_default();
        }
        TREE_ORDER
            .into_iter()
            .filter(|r| self.takes_moves(*r))
            .flat_map(|r| self.roots[r.index()].groups().into_iter().map(|g| g.id))
            .collect()
    }

    fn takes_moves(&self, region: DockRegion) -> bool {
        self.is_visible(region) || self.panels(region).is_empty()
    }

    /// Whether [`Self::move_tab`] would move `panel`: `destination` is in
    /// a shown or empty main host region or a floating host, and
    /// [`Self::can_drop`] allows it.
    pub fn can_move(&self, panel: PanelId, destination: PaneId, index: Option<usize>) -> bool {
        self.area_of(destination).is_some_and(|(h, r)| {
            if h == HostId::MAIN {
                self.takes_moves(r)
            } else {
                self.area_visible(h, r)
            }
        }) && self.can_drop(panel, move_drop(destination, index))
    }

    /// The groups `panel` can move into whole, in tree order, the main
    /// host's then each floating host's: every shown group but its own
    /// that takes it. What a "Move to group" menu lists;
    /// [`Self::move_options`] adds new windows and group moves.
    pub fn move_targets(&self, panel: PanelId) -> Vec<PaneId> {
        std::iter::once(HostId::MAIN)
            .chain(self.hosts())
            .flat_map(|host| self.shown_groups(host))
            .filter(|&pane| self.can_move(panel, pane, None))
            .collect()
    }

    /// The nearest group before (or with `forward`, after) `panel`'s own
    /// in its host's tree order that it can move into. Does not wrap
    /// around, or leave the host.
    pub fn move_target(&self, panel: PanelId, forward: bool) -> Option<PaneId> {
        let (at, _) = self.location(panel)?;
        let groups = self.shown_groups(at.host);
        let at = groups.iter().position(|g| *g == at.pane)?;
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

    /// The tab being dragged and where it would land, while a drag is over
    /// a target it may drop on.
    pub fn drop_preview(&self) -> Option<(PanelId, PaneDrop)> {
        let Gesture::Tab {
            panel,
            target: Some(target),
        } = self.gesture
        else {
            return None;
        };
        self.can_drop(panel, target).then_some((panel, target))
    }

    /// Show where a drag of `payload` the app routes itself would land, or
    /// nothing: a group dragged by its grip, or a drag that left its window
    /// for a drag session. Shown only while the move is allowed. Changes
    /// no layout, so drop targets painted before stay current.
    pub fn set_drag_hover(&mut self, hover: Option<(MovePayload, DockDestination)>) {
        let hover = hover.map(|(payload, d)| {
            let target = PaneDrop {
                pane: d.pane,
                zone: d.zone,
            };
            (payload, target)
        });
        self.step(|gesture| gesture.route(hover));
    }

    /// Where the drag under way would land while the move is allowed,
    /// shown as the drop preview.
    pub(super) fn shown_target(&self) -> Option<PaneDrop> {
        let (payload, target) = self.gesture.landing()?;
        let destination = self.drop_destination(target)?;
        self.prepare(payload, destination).is_ok().then_some(target)
    }

    fn step(&mut self, transition: impl FnOnce(Gesture) -> Gesture) {
        self.gesture = transition(std::mem::take(&mut self.gesture));
    }

    fn split_weights(&self, split: PaneId) -> Option<Vec<f32>> {
        self.areas()
            .find_map(|(_, _, r)| r.split(split))
            .map(|s| s.weights.clone())
    }

    fn drag_divider(&mut self, split: PaneId, divider: usize, event: PaneDividerEvent) {
        match event {
            PaneDividerEvent::Press => match self.split_weights(split) {
                Some(origin) => {
                    self.gesture = Gesture::Divider {
                        split,
                        divider,
                        origin,
                    };
                }
                None => self.step(Gesture::release_divider),
            },
            PaneDividerEvent::Drag { delta, extent } => {
                // Taken out while the split moves, so its weights need no
                // copy on each move.
                let gesture = std::mem::take(&mut self.gesture);
                if let Some(origin) = gesture.divider_origin(split, divider) {
                    self.move_divider(split, divider, origin, extent, |_| delta);
                }
                self.gesture = gesture;
            }
            PaneDividerEvent::Release => self.step(Gesture::release_divider),
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
        if let Some(s) = self.trees_mut().find_map(|r| r.split_mut(split)) {
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
    /// the destination; after any move, act on [`DockOutcome::effects`].
    pub fn apply_event(&mut self, event: DockEvent, now_ms: u64) -> DockOutcome {
        let mut outcome = DockOutcome::default();
        outcome.settled = match event {
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
                outcome.selected = self.group(pane).map(|g| g.id);
                true
            }
            DockEvent::Close { pane, index } => match self.close_tab(pane, index) {
                Some((_, emptied)) => {
                    outcome.effects.persist = true;
                    outcome.effects.closed_hosts.extend(emptied);
                    true
                }
                None => false,
            },
            DockEvent::Toggle(region) => {
                self.toggle(region);
                true
            }
            DockEvent::TabHover { panel, target } => {
                self.gesture = Gesture::Tab { panel, target };
                false
            }
            DockEvent::Hover { payload, target } => {
                self.set_drag_hover(target.map(|d| (payload, d)));
                false
            }
            DockEvent::DragOut { .. } => false,
            DockEvent::TabDrop { panel, target } => {
                self.step(Gesture::end_tab);
                let moved = target
                    .and_then(|t| self.drop_destination(t))
                    .and_then(|d| self.transfer(MovePayload::Panel(panel), d).ok());
                if let Some(effects) = moved {
                    outcome.effects = effects;
                    true
                } else {
                    false
                }
            }
            DockEvent::MoveTab {
                panel,
                destination,
                index,
            } => {
                if self.can_move(panel, destination, index)
                    && let Some(d) = self.drop_destination(move_drop(destination, index))
                    && let Ok(effects) = self.transfer(MovePayload::Panel(panel), d)
                {
                    outcome.moved = self.tab_move(panel);
                    outcome.effects = effects;
                    true
                } else {
                    false
                }
            }
            DockEvent::Transfer {
                payload,
                destination,
            } => match self.transfer(payload, destination) {
                Ok(effects) => {
                    outcome.moved = effects
                        .destination
                        .and_then(|d| self.group(d.pane)?.active_panel())
                        .and_then(|p| self.tab_move(p));
                    outcome.effects = effects;
                    true
                }
                Err(refusal) => {
                    outcome.refused = Some(refusal);
                    false
                }
            },
            DockEvent::MoveToNewHost(payload) => {
                match self.reserve_host(payload) {
                    Ok(host) => outcome.effects.create_host = Some(host),
                    Err(refusal) => outcome.refused = Some(refusal),
                }
                false
            }
        };
        self.debug_verify();
        outcome
    }

    fn tab_move(&self, panel: PanelId) -> Option<TabMove> {
        let (at, _) = self.location(panel)?;
        Some(TabMove {
            panel,
            host: at.host,
            region: at.region,
            pane: at.pane,
        })
    }
}
