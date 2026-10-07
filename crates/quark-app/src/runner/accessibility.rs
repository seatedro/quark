use std::sync::atomic::{AtomicBool, Ordering};

use super::*;

/// accesskit requires a tree before the app has produced one.
pub(super) fn empty_tree_update() -> TreeUpdate {
    let root = NodeId(0);
    TreeUpdate {
        nodes: vec![(root, Node::new(Role::Window))],
        tree: Some(Tree::new(root)),
        tree_id: TreeId::ROOT,
        focus: root,
    }
}

/// Whether assistive tech is listening to one window, and the last tree
/// published to it. Building a tree costs a walk of every node per frame,
/// so it is skipped while nothing listens. accesskit flips the flags from
/// its own threads.
pub(super) struct AccessibilityState {
    active: AtomicBool,
    /// Set on activation until the runner has scheduled a frame for it.
    activated: AtomicBool,
    latest_tree: Mutex<TreeUpdate>,
}

impl Default for AccessibilityState {
    fn default() -> Self {
        Self {
            active: AtomicBool::new(false),
            activated: AtomicBool::new(false),
            latest_tree: Mutex::new(empty_tree_update()),
        }
    }
}

impl AccessibilityState {
    fn activate(&self) -> TreeUpdate {
        self.active.store(true, Ordering::Release);
        self.activated.store(true, Ordering::Release);
        self.latest_tree
            .lock()
            .map_or_else(|_| empty_tree_update(), |tree| tree.clone())
    }

    fn deactivate(&self) {
        self.active.store(false, Ordering::Release);
    }

    /// Whether assistive tech connected since the last call. The tree it
    /// got on connecting may be stale, so the window must draw again.
    pub(super) fn take_activation(&self) -> bool {
        self.activated.swap(false, Ordering::AcqRel)
    }

    /// The tree to send after a frame: `build`'s, kept as the latest, or
    /// `None` without calling `build` while nothing listens.
    pub(super) fn publish(&self, build: impl FnOnce() -> Option<TreeUpdate>) -> Option<TreeUpdate> {
        if !self.active.load(Ordering::Acquire) {
            return None;
        }
        let update = build()?;
        if let Ok(mut latest) = self.latest_tree.lock() {
            *latest = update.clone();
        }
        Some(update)
    }
}

pub(super) struct AccessibilityActivation {
    pub(super) state: Arc<AccessibilityState>,
    pub(super) waker: Waker,
}

impl ActivationHandler for AccessibilityActivation {
    fn request_initial_tree(&mut self) -> Option<TreeUpdate> {
        let tree = self.state.activate();
        self.waker.wake();
        Some(tree)
    }
}

/// accesskit calls this off the main thread; queue the request and wake the
/// loop so the app handles it with an `EventContext`.
pub(super) struct AccessibilityActions {
    pub(super) window: WindowId,
    pub(super) sender: Sender<(WindowId, ActionRequest)>,
    pub(super) waker: Waker,
}

impl ActionHandler for AccessibilityActions {
    fn do_action(&mut self, request: ActionRequest) {
        if self.sender.send((self.window, request)).is_ok() {
            self.waker.wake();
        }
    }
}

pub(super) struct AccessibilityDeactivation {
    pub(super) state: Arc<AccessibilityState>,
}

impl DeactivationHandler for AccessibilityDeactivation {
    fn deactivate_accessibility(&mut self) {
        self.state.deactivate();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree_with(nodes: u64) -> TreeUpdate {
        let mut update = empty_tree_update();
        update
            .nodes
            .extend((1..=nodes).map(|id| (NodeId(id), Node::new(Role::Button))));
        update
    }

    // Regression: the full tree was built and cloned every frame even with
    // no assistive tech running.
    #[test]
    fn tree_is_published_only_while_assistive_tech_listens() {
        let state = AccessibilityState::default();
        let frame = |nodes| state.publish(|| Some(tree_with(nodes)));

        assert_eq!(frame(1), None, "inactive");
        let initial = state.activate();
        assert!(state.take_activation(), "activation asks for a frame");
        assert_eq!(initial.nodes.len(), 1, "nothing was kept while inactive");
        assert_eq!(frame(2).map(|tree| tree.nodes.len()), Some(3));
        state.deactivate();
        assert_eq!(frame(3), None, "deactivated");
        assert_eq!(
            state.activate().nodes.len(),
            3,
            "reconnecting gets the last tree"
        );
    }
}
