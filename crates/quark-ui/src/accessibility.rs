use std::collections::{HashMap, HashSet};

use accesskit::{
    Action as AxAction, Node, NodeId, Rect as AxRect, Role, Toggled, Tree, TreeId, TreeUpdate,
};

use crate::action::{Action, FocusId};
use crate::element::ScrollActionBuilder;
use quark_render::Rect;

pub const ROOT_ID: NodeId = NodeId(1);

#[derive(Debug, Clone)]
pub enum AccessibilityAction {
    Click(Action),
    Focus(FocusId),
    TextValue(FocusId),
    Scroll(ScrollActionBuilder),
    EditorViewport {
        focus: FocusId,
        scroll: ScrollActionBuilder,
    },
}

#[derive(Debug, Clone)]
pub struct AccessibilityNode {
    id: NodeId,
    role: Role,
    bounds: Rect,
    label: Option<String>,
    value: Option<String>,
    description: Option<String>,
    disabled: bool,
    selected: Option<bool>,
    toggled: Option<bool>,
    expanded: Option<bool>,
    action: Option<AccessibilityAction>,
    author_id: String,
    parent: Option<NodeId>,
}

impl AccessibilityNode {
    pub fn new(key: impl AsRef<str>, role: Role, bounds: Rect) -> Self {
        let key = key.as_ref();
        Self {
            id: stable_node_id(key),
            role,
            bounds,
            label: None,
            value: None,
            description: None,
            disabled: false,
            selected: None,
            toggled: None,
            expanded: None,
            action: None,
            author_id: key.to_owned(),
            parent: None,
        }
    }

    pub fn button(key: impl AsRef<str>, label: impl Into<String>, bounds: Rect) -> Self {
        Self::new(key, Role::Button, bounds).label(label)
    }

    pub fn label(mut self, label: impl Into<String>) -> Self {
        let label = label.into();
        if !label.is_empty() {
            self.label = Some(label);
        }
        self
    }

    pub fn value(mut self, value: impl Into<String>) -> Self {
        let value = value.into();
        self.value = Some(value);
        self
    }

    pub fn description(mut self, description: impl Into<String>) -> Self {
        let description = description.into();
        if !description.is_empty() {
            self.description = Some(description);
        }
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = Some(selected);
        self
    }

    pub fn toggled(mut self, toggled: bool) -> Self {
        self.toggled = Some(toggled);
        self
    }

    pub fn expanded(mut self, expanded: bool) -> Self {
        self.expanded = Some(expanded);
        self
    }

    pub fn action(mut self, action: AccessibilityAction) -> Self {
        self.action = Some(action);
        self
    }

    fn to_accesskit_node(&self) -> Node {
        let mut node = Node::new(self.role);
        node.set_bounds(ax_rect(self.bounds));
        node.set_author_id(self.author_id.clone());
        if let Some(label) = &self.label {
            node.set_label(label.clone());
        }
        // accesskit reads a Label node's name from its value, so static text
        // with only a label would reach screen readers unnamed.
        match (&self.value, &self.label) {
            (Some(value), _) => node.set_value(value.clone()),
            (None, Some(label)) if self.role == Role::Label => node.set_value(label.clone()),
            (None, _) => {}
        }
        if let Some(description) = &self.description {
            node.set_description(description.clone());
        }
        if self.disabled {
            node.set_disabled();
        }
        if let Some(selected) = self.selected {
            node.set_selected(selected);
        }
        if let Some(toggled) = self.toggled {
            node.set_toggled(Toggled::from(toggled));
        }
        if let Some(expanded) = self.expanded {
            node.set_expanded(expanded);
        }
        match &self.action {
            Some(AccessibilityAction::Click(_)) => node.add_action(AxAction::Click),
            Some(AccessibilityAction::Focus(_)) => node.add_action(AxAction::Focus),
            Some(AccessibilityAction::TextValue(_)) => {
                node.add_action(AxAction::Focus);
                node.add_action(AxAction::SetValue);
                node.add_action(AxAction::ReplaceSelectedText);
            }
            Some(AccessibilityAction::Scroll(_)) => {
                node.add_action(AxAction::ScrollUp);
                node.add_action(AxAction::ScrollDown);
            }
            Some(AccessibilityAction::EditorViewport { .. }) => {
                node.add_action(AxAction::Focus);
                node.add_action(AxAction::ScrollUp);
                node.add_action(AxAction::ScrollDown);
            }
            None => {}
        }
        node
    }
}

#[derive(Debug, Clone, Default)]
pub struct AccessibilityFrame {
    nodes: Vec<AccessibilityNode>,
    node_ids: HashSet<NodeId>,
    actions: HashMap<NodeId, AccessibilityAction>,
    /// Semantic node index (in the frame's `SemanticFrame`) to the
    /// accessibility node that represents it.
    semantic_owners: HashMap<usize, NodeId>,
    focused: Option<NodeId>,
    root_bounds: Rect,
}

impl AccessibilityFrame {
    pub fn new(width: f32, height: f32) -> Self {
        Self {
            root_bounds: Rect {
                x: 0.0,
                y: 0.0,
                width,
                height,
            },
            ..Self::default()
        }
    }

    /// Push a direct child of the window. Returns the node's final id, which
    /// differs from the key hash when the key collided.
    pub fn push(&mut self, node: AccessibilityNode) -> NodeId {
        self.push_child(node, None)
    }

    /// Push a node under `parent`, an id previously returned by this frame,
    /// or under the window when `None`. Parents must be pushed first.
    pub fn push_child(&mut self, mut node: AccessibilityNode, parent: Option<NodeId>) -> NodeId {
        self.ensure_unique_id(&mut node);
        node.parent = parent.filter(|id| self.node_ids.contains(id) && *id != node.id);
        let id = node.id;
        if let Some(action) = node.action.clone() {
            self.actions.insert(node.id, action);
        }
        if matches!(
            node.action,
            Some(
                AccessibilityAction::Focus(_)
                    | AccessibilityAction::TextValue(_)
                    | AccessibilityAction::EditorViewport { .. }
            )
        ) {
            self.focused.get_or_insert(node.id);
        }
        self.nodes.push(node);
        id
    }

    /// Record that `id` represents semantic node `semantic_index`, so nodes
    /// emitted under that semantic node nest beneath it.
    pub fn bind_semantic(&mut self, semantic_index: usize, id: NodeId) {
        self.semantic_owners.insert(semantic_index, id);
    }

    pub fn semantic_owner(&self, semantic_index: usize) -> Option<NodeId> {
        self.semantic_owners.get(&semantic_index).copied()
    }

    fn ensure_unique_id(&mut self, node: &mut AccessibilityNode) {
        if self.node_ids.insert(node.id) {
            return;
        }

        let base_author_id = node.author_id.clone();
        let mut suffix = 2usize;
        loop {
            let author_id = format!("{base_author_id}#{suffix}");
            let id = stable_node_id(&author_id);
            if self.node_ids.insert(id) {
                node.id = id;
                node.author_id = author_id;
                return;
            }
            suffix += 1;
        }
    }

    pub fn action_for(&self, id: NodeId) -> Option<&AccessibilityAction> {
        self.actions.get(&id)
    }

    /// `app_name` labels the root window node.
    pub fn tree_update(&self, app_name: &str, focus: Option<FocusId>) -> TreeUpdate {
        let mut children: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        for node in &self.nodes {
            children
                .entry(node.parent.unwrap_or(ROOT_ID))
                .or_default()
                .push(node.id);
        }

        let mut root = Node::new(Role::Window);
        root.set_bounds(ax_rect(self.root_bounds));
        root.set_label(app_name);
        root.set_children(children.remove(&ROOT_ID).unwrap_or_default());

        let mut nodes = Vec::with_capacity(self.nodes.len() + 1);
        nodes.push((ROOT_ID, root));

        let mut focused = ROOT_ID;
        for node in &self.nodes {
            if let Some(target) = focus {
                let node_focus = match node.action {
                    Some(AccessibilityAction::Focus(t) | AccessibilityAction::TextValue(t)) => {
                        t == target
                    }
                    Some(AccessibilityAction::EditorViewport { focus, .. }) => focus == target,
                    _ => false,
                };
                if node_focus {
                    focused = node.id;
                }
            }
            let mut ax_node = node.to_accesskit_node();
            if let Some(kids) = children.remove(&node.id) {
                ax_node.set_children(kids);
            }
            nodes.push((node.id, ax_node));
        }

        TreeUpdate {
            nodes,
            tree: Some(Tree {
                root: ROOT_ID,
                toolkit_name: Some("Quark".to_owned()),
                toolkit_version: Some(env!("CARGO_PKG_VERSION").to_owned()),
            }),
            tree_id: TreeId::ROOT,
            focus: focused,
        }
    }
}

pub fn empty_tree_update(app_name: &str) -> TreeUpdate {
    AccessibilityFrame::new(1.0, 1.0).tree_update(app_name, None)
}

/// One stable line per node, in tree order:
/// `author_id | role | label | value | x,y,w,h` (bounds rounded to ints, `-` for None).
/// Used by the devtools harness to snapshot the accessibility "DOM".
pub fn dump_accessibility(frame: &AccessibilityFrame) -> String {
    let mut out = String::new();
    for node in &frame.nodes {
        let label = node.label.as_deref().unwrap_or("-");
        let value = node.value.as_deref().unwrap_or("-");
        let b = node.bounds;
        out.push_str(&format!(
            "{} | {:?} | {} | {} | {},{},{},{}\n",
            node.author_id,
            node.role,
            label,
            value,
            b.x.round() as i64,
            b.y.round() as i64,
            b.width.round() as i64,
            b.height.round() as i64,
        ));
    }
    out
}

/// The tree shape: one line per node, `  ` per depth level, as
/// `author_id | role | label`. Children follow their parent in push order.
pub fn dump_accessibility_tree(frame: &AccessibilityFrame) -> String {
    let mut children: HashMap<Option<NodeId>, Vec<usize>> = HashMap::new();
    for (index, node) in frame.nodes.iter().enumerate() {
        children.entry(node.parent).or_default().push(index);
    }
    let mut out = String::new();
    // Depth-first; parents always precede children, so this terminates.
    let mut stack: Vec<(usize, usize)> = children
        .get(&None)
        .map(|roots| roots.iter().rev().map(|&index| (index, 0)).collect())
        .unwrap_or_default();
    while let Some((index, depth)) = stack.pop() {
        let node = &frame.nodes[index];
        out.push_str(&format!(
            "{}{} | {:?} | {}\n",
            "  ".repeat(depth),
            node.author_id,
            node.role,
            node.label.as_deref().unwrap_or("-"),
        ));
        if let Some(kids) = children.get(&Some(node.id)) {
            stack.extend(kids.iter().rev().map(|&kid| (kid, depth + 1)));
        }
    }
    out
}

fn ax_rect(rect: Rect) -> AxRect {
    AxRect::new(
        f64::from(rect.x),
        f64::from(rect.y),
        f64::from(rect.x + rect.width),
        f64::from(rect.y + rect.height),
    )
}

/// Ids 0 and 1 are reserved for the runner's placeholder root and [`ROOT_ID`].
fn stable_node_id(key: &str) -> NodeId {
    NodeId(quark::stable_hash(key).max(2))
}

#[cfg(test)]
mod tests {
    use accesskit::Role;

    use super::*;

    fn rect() -> Rect {
        Rect {
            x: 0.0,
            y: 0.0,
            width: 10.0,
            height: 10.0,
        }
    }

    #[test]
    fn tree_update_publishes_children_under_their_parent() {
        let mut frame = AccessibilityFrame::new(100.0, 100.0);
        let dialog = frame.push(AccessibilityNode::new("dialog", Role::Dialog, rect()));
        let ok = frame.push_child(AccessibilityNode::button("ok", "OK", rect()), Some(dialog));
        let update = frame.tree_update("Test", None);
        let children = |id: NodeId| {
            update
                .nodes
                .iter()
                .find(|(node_id, _)| *node_id == id)
                .map(|(_, node)| node.children().to_vec())
                .expect("node in update")
        };
        assert_eq!(children(ROOT_ID), vec![dialog]);
        assert_eq!(children(dialog), vec![ok]);
    }

    #[test]
    fn label_node_publishes_its_text_as_value() {
        let mut frame = AccessibilityFrame::new(100.0, 100.0);
        let id = frame.push(AccessibilityNode::new("text", Role::Label, rect()).label("Hello"));
        let update = frame.tree_update("Test", None);
        let node = update.nodes.iter().find(|(node_id, _)| *node_id == id);
        assert_eq!(node.and_then(|(_, node)| node.value()), Some("Hello"));
    }

    #[test]
    fn duplicate_node_keys_are_disambiguated() {
        let mut frame = AccessibilityFrame::new(100.0, 100.0);
        frame.push(AccessibilityNode::new("same", Role::Label, rect()).label("One"));
        frame.push(AccessibilityNode::new("same", Role::Label, rect()).label("Two"));

        let update = frame.tree_update("Test", None);
        let root = update
            .nodes
            .iter()
            .find(|(id, _)| *id == ROOT_ID)
            .map(|(_, node)| node)
            .expect("root node");
        let children = root.children();
        assert_eq!(children.len(), 2);
        assert_ne!(children[0], children[1]);
    }
}
