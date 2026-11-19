//! The workspace's invariants, checked after every change in debug
//! builds.

#[cfg(test)]
use super::DockRegion;
use super::{DockIntegrityError, DockState, HostId, PanelId};
use crate::pane_tree::{PaneId, PaneNode, PaneSplit};
#[cfg(test)]
use crate::split::Axis;

impl DockState {
    pub fn verify_integrity(&self) -> Result<(), DockIntegrityError> {
        fn walk(
            node: &PaneNode,
            root: bool,
            next_id: u32,
            ids: &mut Vec<PaneId>,
            panels: &mut Vec<PanelId>,
        ) -> Result<(), DockIntegrityError> {
            let id = node.id();
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
        // Pane ids and panels are unique across every host.
        let mut ids = Vec::new();
        let mut panels = Vec::new();
        for root in &self.roots {
            walk(root, true, self.next_id, &mut ids, &mut panels)?;
        }
        let mut hosts = Vec::new();
        for host in &self.floating {
            if host.id == HostId::MAIN || host.id.0 >= self.next_host || hosts.contains(&host.id) {
                return Err(DockIntegrityError::HostIds(host.id));
            }
            hosts.push(host.id);
            if Self::tree_panels(&host.root).is_empty() {
                return Err(DockIntegrityError::EmptyHost(host.id));
            }
            walk(&host.root, true, self.next_id, &mut ids, &mut panels)?;
        }
        for (host, _) in &self.reservations {
            if *host == HostId::MAIN || host.0 >= self.next_host || hosts.contains(host) {
                return Err(DockIntegrityError::HostIds(*host));
            }
        }
        let fresh = self.build_index();
        if let Some(p) = panels
            .iter()
            .chain(self.index.keys())
            .find(|p| fresh.get(p) != self.index.get(p))
        {
            return Err(DockIntegrityError::Index(*p));
        }
        // A return location's group may since have gone; re-docking then
        // falls back to its region.
        for &panel in self.returns.keys() {
            if !fresh
                .get(&panel)
                .is_some_and(|(at, _)| at.host != HostId::MAIN)
            {
                return Err(DockIntegrityError::ReturnLocation(panel));
            }
        }
        Ok(())
    }

    pub(super) fn debug_verify(&self) {
        debug_assert_eq!(self.verify_integrity(), Ok(()));
    }

    /// Each area's tree as text, one line per area with panels:
    /// `right: row([10* 11] | col([12*] | [13*]))`, `*` marking the active
    /// tab, then floating hosts as `host 1: [14*]`.
    #[cfg(test)]
    pub(super) fn dump(&self) -> String {
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
        for host in &self.floating {
            out.push_str(&format!("host {}: ", host.id.0));
            node(&host.root, &mut out);
            out.push('\n');
        }
        out
    }
}
