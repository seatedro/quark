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

pub(super) struct AccessibilityActivation {
    pub(super) latest_tree: Arc<Mutex<TreeUpdate>>,
}

impl ActivationHandler for AccessibilityActivation {
    fn request_initial_tree(&mut self) -> Option<TreeUpdate> {
        self.latest_tree.lock().ok().map(|tree| tree.clone())
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

pub(super) struct AccessibilityDeactivation;

impl DeactivationHandler for AccessibilityDeactivation {
    fn deactivate_accessibility(&mut self) {}
}
