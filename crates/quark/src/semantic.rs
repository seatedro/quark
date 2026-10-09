use std::sync::Arc;

use crate::{
    FocusId, FocusNode, FocusScopeId, FocusTree, KeyContext, Rect, StyleState, TabStop, TestId,
    UiEventBinding, UiEventPhase, UiKey, UiNodeId,
};

/// What a node is, for routing, focus, devtools, and test queries. The
/// accessibility tree carries the exact platform role (any
/// `accesskit::Role`); this is the toolkit-level view of it, so related
/// platform roles share a variant (every single-line text field is
/// `TextInput`, a menu checkbox is a `CheckBox`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SemanticRole {
    Button,
    Link,
    Dialog,
    Alert,
    Status,
    CheckBox,
    Switch,
    RadioButton,
    RadioGroup,
    Tab,
    TabList,
    TabPanel,
    Tree,
    TreeItem,
    List,
    ListItem,
    ListBox,
    ListBoxOption,
    Menu,
    MenuBar,
    MenuItem,
    ComboBox,
    TextInput,
    Slider,
    SpinButton,
    ProgressIndicator,
    Heading,
    Image,
    Toolbar,
    Tooltip,
    Separator,
    Table,
    Grid,
    Row,
    Cell,
    Document,
    ScrollArea,
    Group,
    Label,
    /// ARIA landmarks: a navigation region (a sidebar of links) and a
    /// region that complements the main content (a side panel).
    Navigation,
    Complementary,
    /// A region where new entries are appended, such as a transcript.
    Log,
    AlertDialog,
    /// A window-like container inside the app's window. Not an ARIA role.
    Window,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct SemanticActions {
    pub click: bool,
    pub focus: bool,
    pub text_value: bool,
    pub scroll: bool,
    pub drag: bool,
    pub tooltip: bool,
    pub hit_test: bool,
}

impl SemanticActions {
    pub fn clickable(mut self) -> Self {
        self.click = true;
        self.hit_test = true;
        self
    }

    pub fn focusable(mut self) -> Self {
        self.focus = true;
        self
    }

    pub fn text_value(mut self) -> Self {
        self.text_value = true;
        self.focus = true;
        self
    }

    pub fn scrollable(mut self) -> Self {
        self.scroll = true;
        self.hit_test = true;
        self
    }

    pub fn draggable(mut self) -> Self {
        self.drag = true;
        self.hit_test = true;
        self
    }

    pub fn tooltip(mut self) -> Self {
        self.tooltip = true;
        self
    }

    pub fn hit_test(mut self) -> Self {
        self.hit_test = true;
        self
    }

    pub fn is_empty(self) -> bool {
        !self.click
            && !self.focus
            && !self.text_value
            && !self.scroll
            && !self.drag
            && !self.tooltip
            && !self.hit_test
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct SemanticNodeState {
    pub disabled: bool,
    pub selected: Option<bool>,
    pub toggled: Option<bool>,
    pub expanded: Option<bool>,
    /// The value fails validation (a form field with an error).
    pub invalid: bool,
    /// The form cannot be submitted without a value here.
    pub required: bool,
    pub style_state: StyleState,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SemanticNode {
    pub id: Option<UiNodeId>,
    pub key: Option<UiKey>,
    pub test_id: Option<TestId>,
    pub parent: Option<usize>,
    pub role: Option<SemanticRole>,
    pub label: Option<Arc<str>>,
    pub value: Option<Arc<str>>,
    pub description: Option<Arc<str>>,
    pub tooltip: Option<Arc<str>>,
    pub bounds: Rect,
    pub actions: SemanticActions,
    pub state: SemanticNodeState,
    /// Focus target this node represents. Falls back to a hash of the
    /// node's stable id when unset.
    pub focus: Option<FocusId>,
    pub focus_scope: Option<FocusScopeId>,
    /// With `focus_scope` set, traps Tab inside the scope and keeps clicks
    /// outside the node from moving focus.
    pub modal: bool,
    pub tab_stop: Option<TabStop>,
    pub key_context: Option<KeyContext>,
    pub event_bindings: Vec<UiEventBinding>,
}

impl SemanticNode {
    pub fn new(bounds: Rect) -> Self {
        Self {
            id: None,
            key: None,
            test_id: None,
            parent: None,
            role: None,
            label: None,
            value: None,
            description: None,
            tooltip: None,
            bounds,
            actions: SemanticActions::default(),
            state: SemanticNodeState::default(),
            focus: None,
            focus_scope: None,
            modal: false,
            tab_stop: None,
            key_context: None,
            event_bindings: Vec::new(),
        }
    }

    pub fn id(mut self, id: impl Into<UiNodeId>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn key(mut self, key: impl Into<UiKey>) -> Self {
        self.key = Some(key.into());
        self
    }

    pub fn test_id(mut self, test_id: impl Into<TestId>) -> Self {
        self.test_id = Some(test_id.into());
        self
    }

    pub fn role(mut self, role: SemanticRole) -> Self {
        self.role = Some(role);
        self
    }

    pub fn label(mut self, label: impl Into<Arc<str>>) -> Self {
        let label = label.into();
        if !label.is_empty() {
            self.label = Some(label);
        }
        self
    }

    pub fn value(mut self, value: impl Into<Arc<str>>) -> Self {
        self.value = Some(value.into());
        self
    }

    pub fn description(mut self, description: impl Into<Arc<str>>) -> Self {
        let description = description.into();
        if !description.is_empty() {
            self.description = Some(description);
        }
        self
    }

    pub fn tooltip(mut self, tooltip: impl Into<Arc<str>>) -> Self {
        let tooltip = tooltip.into();
        if !tooltip.is_empty() {
            self.tooltip = Some(tooltip);
            self.actions = self.actions.tooltip();
        }
        self
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SemanticFrame {
    root_bounds: Rect,
    nodes: Vec<SemanticNode>,
}

impl SemanticFrame {
    pub fn new(width: f32, height: f32) -> Self {
        Self {
            root_bounds: Rect {
                x: 0.0,
                y: 0.0,
                width,
                height,
            },
            nodes: Vec::new(),
        }
    }

    pub fn root_bounds(&self) -> Rect {
        self.root_bounds
    }

    pub fn push(&mut self, node: SemanticNode) -> usize {
        let index = self.nodes.len();
        self.nodes.push(node);
        index
    }

    pub fn nodes(&self) -> &[SemanticNode] {
        &self.nodes
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn clear(&mut self) {
        self.nodes.clear();
    }

    /// Empty the frame for a `width` x `height` window, keeping its buffer.
    pub fn reset(&mut self, width: f32, height: f32) {
        self.nodes.clear();
        self.root_bounds = Rect {
            x: 0.0,
            y: 0.0,
            width,
            height,
        };
    }

    /// Indices from the root down to `target`. Built from
    /// [`Self::ancestors_inclusive`], so a malformed parent link ends the
    /// path instead of looping.
    pub fn node_path(&self, target: usize) -> Option<Vec<usize>> {
        let mut path: Vec<usize> = self.ancestors_inclusive(target).collect();
        if path.is_empty() {
            return None;
        }
        path.reverse();
        Some(path)
    }

    /// Node indices an event visits on its way to `target` and back:
    /// ancestors root-first in capture, the target, then ancestors in bubble.
    pub fn route_indices(&self, target: usize) -> Option<Vec<(usize, UiEventPhase)>> {
        let path = self.node_path(target)?;
        let (&target, ancestors) = path.split_last()?;
        let mut steps = Vec::with_capacity(path.len() * 2 - 1);
        steps.extend(ancestors.iter().map(|i| (*i, UiEventPhase::Capture)));
        steps.push((target, UiEventPhase::Target));
        steps.extend(ancestors.iter().rev().map(|i| (*i, UiEventPhase::Bubble)));
        Some(steps)
    }

    /// `node` and its ancestors, innermost first.
    pub fn ancestors_inclusive(&self, node: usize) -> impl Iterator<Item = usize> + '_ {
        // Parents are pushed before children; requiring a smaller index
        // guarantees the walk ends even on a malformed frame.
        std::iter::successors(Some(node).filter(|i| *i < self.nodes.len()), |i| {
            self.nodes[*i].parent.filter(|parent| parent < i)
        })
    }

    pub fn is_within(&self, node: usize, ancestor: usize) -> bool {
        self.ancestors_inclusive(node).any(|i| i == ancestor)
    }

    /// The focus scope `node` belongs to: its own or its nearest ancestor's.
    pub fn scope_of(&self, node: usize) -> Option<&FocusScopeId> {
        self.ancestors_inclusive(node)
            .find_map(|i| self.nodes[i].focus_scope.as_ref())
    }

    /// The topmost (last painted) modal focus scope root, if any.
    pub fn modal_root(&self) -> Option<usize> {
        self.nodes
            .iter()
            .rposition(|node| node.modal && node.focus_scope.is_some())
    }

    /// Focus target of `node`, when it can take focus.
    pub fn focus_id(&self, node: usize) -> Option<FocusId> {
        let n = self.nodes.get(node)?;
        if !(n.actions.focus || n.actions.text_value || n.tab_stop.is_some()) {
            return None;
        }
        n.focus
            .or_else(|| self.stable_node_id(node).map(FocusId::from))
    }

    /// Index of the node that owns focus target `focus`.
    pub fn node_for_focus(&self, focus: FocusId) -> Option<usize> {
        (0..self.nodes.len()).rfind(|i| self.focus_id(*i) == Some(focus))
    }

    pub fn focus_tree(&self) -> FocusTree {
        let mut tree = FocusTree::default();
        self.fill_focus_tree(&mut tree);
        tree
    }

    /// [`Self::focus_tree`] into `tree`, reusing its memory.
    pub fn fill_focus_tree(&self, tree: &mut FocusTree) {
        tree.clear();
        for (index, node) in self.nodes.iter().enumerate() {
            let Some(id) = self.focus_id(index) else {
                continue;
            };
            tree.register(FocusNode {
                id,
                scope: self.scope_of(index).cloned(),
                tab_stop: node.tab_stop.unwrap_or_else(|| TabStop::new(index as i32)),
                key_context: node.key_context.clone(),
            });
        }
        if let Some(scope) = self
            .modal_root()
            .and_then(|root| self.nodes[root].focus_scope.clone())
        {
            tree.trap_modal_scope(scope);
        }
    }

    fn stable_node_id(&self, index: usize) -> Option<UiNodeId> {
        let node = self.nodes.get(index)?;
        node.id
            .clone()
            .or_else(|| node.test_id.as_ref().map(|id| UiNodeId::from(id.as_str())))
    }
}

pub fn dump_semantic(frame: &SemanticFrame) -> String {
    let mut out = String::new();
    for (i, node) in frame.nodes.iter().enumerate() {
        let id = node
            .id
            .as_ref()
            .map(UiNodeId::as_str)
            .or_else(|| node.test_id.as_ref().map(TestId::as_str))
            .unwrap_or("-");
        let role = node
            .role
            .map(|role| format!("{role:?}"))
            .unwrap_or_else(|| "-".to_owned());
        let label = node.label.as_deref().unwrap_or("");
        let test_id = node.test_id.as_ref().map(TestId::as_str).unwrap_or("-");
        out.push_str(&format!(
            "{i} | parent={:?} | {id} | test={test_id} | {role} | {label} | {:.0},{:.0},{:.0},{:.0} | actions={}{}{}{}{}{}{} | state={}\n",
            node.parent,
            node.bounds.x,
            node.bounds.y,
            node.bounds.width,
            node.bounds.height,
            if node.actions.click { " click" } else { "" },
            if node.actions.focus { " focus" } else { "" },
            if node.actions.text_value { " text" } else { "" },
            if node.actions.scroll { " scroll" } else { "" },
            if node.actions.drag { " drag" } else { "" },
            if node.actions.tooltip { " tooltip" } else { "" },
            if node.actions.hit_test { " hit" } else { "" },
            node.state.style_state.bits(),
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, parent: Option<usize>) -> SemanticNode {
        let mut node = SemanticNode::new(Rect {
            x: 0.0,
            y: 0.0,
            width: 10.0,
            height: 10.0,
        })
        .id(id);
        node.parent = parent;
        node
    }

    #[test]
    fn route_indices_capture_target_bubble() {
        let mut frame = SemanticFrame::new(400.0, 300.0);
        let root = frame.push(node("root", None));
        let dialog = frame.push(node("dialog", Some(root)));
        let button = frame.push(node("save", Some(dialog)));
        use UiEventPhase::*;
        assert_eq!(
            frame.route_indices(button),
            Some(vec![
                (root, Capture),
                (dialog, Capture),
                (button, Target),
                (dialog, Bubble),
                (root, Bubble),
            ])
        );
    }

    /// Regression: a parent link pointing at itself or forward used to spin
    /// `node_path` forever.
    #[test]
    fn node_path_ends_at_a_malformed_parent() {
        let mut frame = SemanticFrame::new(400.0, 300.0);
        frame.push(node("a", Some(1)));
        frame.push(node("b", Some(0)));
        frame.push(node("c", Some(2)));
        assert_eq!(frame.node_path(0), Some(vec![0]));
        assert_eq!(frame.node_path(1), Some(vec![0, 1]));
        assert_eq!(frame.node_path(2), Some(vec![2]));
        assert_eq!(frame.node_path(3), None);
    }

    #[test]
    fn focus_tree_scopes_nodes_under_their_ancestor_scope() {
        let mut frame = SemanticFrame::new(400.0, 300.0);
        let mut dialog = node("dialog", None);
        dialog.focus_scope = Some(FocusScopeId::from("dialog"));
        let dialog = frame.push(dialog);
        let mut button = node("save", Some(dialog));
        button.actions = SemanticActions::default().clickable().focusable();
        frame.push(button);
        let mut outside = node("outside", None);
        outside.actions = SemanticActions::default().focusable();
        frame.push(outside);

        let order: Vec<_> = frame
            .focus_tree()
            .tab_order(Some(&FocusScopeId::from("dialog")))
            .into_iter()
            .map(|node| node.id)
            .collect();
        assert_eq!(order, vec![FocusId::from_key("save")]);
    }
}
