//! The saved form of a dock workspace: the main host's layout, floating
//! host trees, and where floating panels return to. Windows, their
//! placement, and anything transient (drags, reservations) are not part of
//! it; an app saves placement beside it, keyed by [`HostId`].

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::workspace::FloatingHost;
use super::{DockRegion, DockSnapshot, DockState, HostId, PanelId};
use crate::pane_tree::{PaneId, PaneNode, TabGroup};

/// The [`WorkspaceSnapshot::version`] this build writes.
pub const WORKSPACE_VERSION: u32 = 1;

/// The persisted part of a [`DockState`] and its floating hosts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceSnapshot {
    pub version: u32,
    pub main: DockSnapshot,
    #[serde(default)]
    pub floating: Vec<FloatingSnapshot>,
    #[serde(default)]
    pub returns: Vec<ReturnSnapshot>,
    /// Below this, host ids are taken, so a restored workspace never
    /// reuses one an app may still have saved placement under.
    #[serde(default)]
    pub next_host: u64,
}

/// A floating host's tree. Its policy and label are the app's to set
/// again after a restore.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FloatingSnapshot {
    pub host: HostId,
    pub root: PaneNode,
}

/// Where a panel in a floating host returns to in the main host: `pane`
/// is a group id of [`WorkspaceSnapshot::main`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReturnSnapshot {
    pub panel: PanelId,
    pub region: DockRegion,
    pub pane: PaneId,
    pub index: usize,
}

impl From<DockSnapshot> for WorkspaceSnapshot {
    /// A main host layout saved before floating hosts: a workspace with
    /// none.
    fn from(main: DockSnapshot) -> Self {
        Self {
            version: WORKSPACE_VERSION,
            main,
            floating: Vec::new(),
            returns: Vec::new(),
            next_host: 1,
        }
    }
}

/// Either saved form: deserialize into this to read files written before
/// and after floating hosts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StoredDock {
    Workspace(WorkspaceSnapshot),
    Legacy(DockSnapshot),
}

impl StoredDock {
    pub fn into_workspace(self) -> WorkspaceSnapshot {
        match self {
            Self::Workspace(snapshot) => snapshot,
            Self::Legacy(main) => main.into(),
        }
    }
}

impl DockState {
    /// The whole workspace, to save. Save it once more on quit, before
    /// windows close, so floating hosts are kept.
    pub fn workspace_snapshot(&self) -> WorkspaceSnapshot {
        let mut returns: Vec<ReturnSnapshot> = self
            .returns
            .iter()
            .map(|(&panel, &(region, pane, index))| ReturnSnapshot {
                panel,
                region,
                pane,
                index,
            })
            .collect();
        returns.sort_by_key(|r| r.panel.0);
        WorkspaceSnapshot {
            version: WORKSPACE_VERSION,
            main: self.snapshot(),
            floating: self
                .floating
                .iter()
                .map(|h| FloatingSnapshot {
                    host: h.id,
                    root: h.root.clone(),
                })
                .collect(),
            returns,
            next_host: self.next_host,
        }
    }

    /// Restore a workspace snapshot, returning its floating hosts to open
    /// windows for (recover any that cannot open with
    /// [`DockState::recover_host`]). Panels `keep` rejects are dropped, and
    /// so are repeats anywhere in the workspace, the main host's first;
    /// emptied groups are pruned, regions left empty hide, and floating
    /// hosts left empty, or repeating a host id, are dropped. Groups get
    /// fresh ids, floating hosts start [`super::TabPolicy::OPEN`], and
    /// reservations and live tear-offs are dropped.
    pub fn restore_workspace(
        &mut self,
        snapshot: &WorkspaceSnapshot,
        keep: impl Fn(PanelId) -> bool,
    ) -> Vec<HostId> {
        let main = &snapshot.main;
        self.columns.restore(&main.columns);
        self.rows.restore(&main.rows);
        let saved = [&main.left, &main.right, &main.bottom, &main.center];
        let mut seen: Vec<PanelId> = Vec::new();
        let mut filter = |root: &mut PaneNode| {
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
        };
        let mut next = 0;
        // Saved main host group ids, per region, to their new ids.
        let mut renamed: Vec<(DockRegion, PaneId, PaneId)> = Vec::new();
        for region in DockRegion::ALL {
            let mut root = saved[region.index()].clone();
            let mut map = Vec::new();
            root.renumber(&mut next, &mut map);
            renamed.extend(map.into_iter().map(|(old, new)| (region, old, new)));
            filter(&mut root);
            self.roots[region.index()] = root
                .prune()
                .unwrap_or_else(|| PaneNode::Tabs(TabGroup::new(PaneId(next))));
            next += 1;
        }
        self.floating.clear();
        self.reservations.clear();
        self.live.clear();
        for saved in &snapshot.floating {
            if saved.host == HostId::MAIN || self.floating_host(saved.host).is_some() {
                continue;
            }
            let mut root = saved.root.clone();
            root.renumber(&mut next, &mut Vec::new());
            filter(&mut root);
            if let Some(root) = root.prune() {
                self.floating.push(FloatingHost::new(saved.host, root));
            }
        }
        self.next_id = next;
        let hosts = self.hosts();
        self.next_host = hosts
            .iter()
            .map(|h| h.0 + 1)
            .chain([snapshot.next_host, 1])
            .max()
            .unwrap_or(1);
        self.recent = DockRegion::ALL.map(|r| self.roots[r.index()].groups()[0].id);

        let index = self.build_index();
        self.returns = HashMap::new();
        for r in &snapshot.returns {
            let floating = index
                .get(&r.panel)
                .is_some_and(|(at, _)| at.host != HostId::MAIN);
            if !floating || self.returns.contains_key(&r.panel) {
                continue;
            }
            // The group it left, when it survived the restore; else its
            // region, which re-docking falls back to.
            let root = &self.roots[r.region.index()];
            let pane = renamed
                .iter()
                .find(|(region, old, _)| *region == r.region && *old == r.pane)
                .map(|(_, _, new)| *new)
                .filter(|new| root.group(*new).is_some())
                .unwrap_or(root.groups()[0].id);
            self.returns.insert(r.panel, (r.region, pane, r.index));
        }
        self.confined.retain(|p| seen.contains(p));
        self.tab_drag = None;
        self.divider_drag = None;
        for region in DockRegion::ALL {
            if self.panels(region).is_empty() {
                self.set_hidden(region, true);
            }
        }
        self.changed();
        hosts
    }
}
