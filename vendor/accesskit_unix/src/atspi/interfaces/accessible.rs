// Copyright 2022 The AccessKit Authors. All rights reserved.
// Licensed under the Apache License, Version 2.0 (found in
// the LICENSE-APACHE file) or the MIT license (found in
// the LICENSE-MIT file), at your option.

use std::{
    collections::HashMap,
    sync::{Arc, OnceLock},
};

use accesskit_atspi_common::{NodeIdOrRoot, PlatformNode, PlatformRoot};
use atspi::{InterfaceSet, ObjectRefOwned, RelationType, Role, StateSet};
use zbus::{fdo, interface, names::OwnedUniqueName};

use super::map_root_error;
use crate::atspi::{ObjectId, OwnedObjectAddress};

pub(crate) struct NodeAccessibleInterface {
    bus_name: OwnedUniqueName,
    node: PlatformNode,
}

impl NodeAccessibleInterface {
    pub fn new(bus_name: OwnedUniqueName, node: PlatformNode) -> Self {
        Self { bus_name, node }
    }

    fn map_error(&self) -> impl '_ + FnOnce(accesskit_atspi_common::Error) -> fdo::Error {
        |error| crate::util::map_error_from_node(&self.node, error)
    }
}

#[interface(name = "org.a11y.atspi.Accessible")]
impl NodeAccessibleInterface {
    #[zbus(property)]
    fn name(&self) -> fdo::Result<String> {
        self.node.name().map_err(self.map_error())
    }

    #[zbus(property)]
    fn description(&self) -> fdo::Result<String> {
        self.node.description().map_err(self.map_error())
    }

    #[zbus(property)]
    fn parent(&self) -> fdo::Result<OwnedObjectAddress> {
        self.node.parent().map_err(self.map_error()).map(|parent| {
            match parent {
                NodeIdOrRoot::Node(node) => ObjectId::Node {
                    adapter: self.node.adapter_id(),
                    node,
                },
                NodeIdOrRoot::Root => ObjectId::Root,
            }
            .to_address(self.bus_name.inner())
        })
    }

    #[zbus(property)]
    fn child_count(&self) -> fdo::Result<i32> {
        self.node.child_count().map_err(self.map_error())
    }

    #[zbus(property)]
    fn locale(&self) -> &str {
        ""
    }

    #[zbus(property)]
    fn accessible_id(&self) -> fdo::Result<String> {
        self.node.accessible_id().map_err(self.map_error())
    }

    fn get_child_at_index(&self, index: i32) -> fdo::Result<(OwnedObjectAddress,)> {
        let index = index
            .try_into()
            .map_err(|_| fdo::Error::InvalidArgs("Index can't be negative.".into()))?;
        let child = self
            .node
            .child_at_index(index)
            .map_err(self.map_error())?
            .map(|child| ObjectId::Node {
                adapter: self.node.adapter_id(),
                node: child,
            });
        Ok(super::optional_object_address(&self.bus_name, child))
    }

    fn get_children(&self) -> fdo::Result<Vec<OwnedObjectAddress>> {
        self.node
            .map_children(|child| {
                ObjectId::Node {
                    adapter: self.node.adapter_id(),
                    node: child,
                }
                .to_address(self.bus_name.inner())
            })
            .map_err(self.map_error())
    }

    fn get_index_in_parent(&self) -> fdo::Result<i32> {
        self.node.index_in_parent().map_err(self.map_error())
    }

    fn get_relation_set(&self) -> fdo::Result<Vec<(RelationType, Vec<OwnedObjectAddress>)>> {
        self.node
            .relation_set(|relation| {
                ObjectId::Node {
                    adapter: self.node.adapter_id(),
                    node: relation,
                }
                .to_address(self.bus_name.inner())
            })
            .map(|set| {
                set.into_iter()
                    .collect::<Vec<(RelationType, Vec<OwnedObjectAddress>)>>()
            })
            .map_err(self.map_error())
    }

    fn get_role(&self) -> fdo::Result<Role> {
        self.node.role().map_err(self.map_error())
    }

    fn get_role_name(&self) -> fdo::Result<&'static str> {
        role_name(&self.node).map_err(self.map_error())
    }

    fn get_localized_role_name(&self) -> fdo::Result<String> {
        localized_role_name(&self.node).map_err(self.map_error())
    }

    fn get_state(&self) -> StateSet {
        self.node.state()
    }

    fn get_attributes(&self) -> fdo::Result<HashMap<&str, String>> {
        attributes(&self.node).map_err(self.map_error())
    }

    fn get_application(&self) -> (OwnedObjectAddress,) {
        (ObjectId::Root.to_address(self.bus_name.inner()),)
    }

    fn get_interfaces(&self) -> fdo::Result<InterfaceSet> {
        self.node.interfaces().map_err(self.map_error())
    }
}

/// The English name of the node's role, as libatspi's
/// `atspi_role_get_name` gives it: "push button", "check box", ...
fn role_name(node: &PlatformNode) -> accesskit_atspi_common::Result<&'static str> {
    node.role().map(atspi_role_name)
}

fn atspi_role_name(role: Role) -> &'static str {
    match role {
        // atspi's table says "button"; libatspi derives names from the
        // AtspiRole enum nicks, where this one is ATSPI_ROLE_PUSH_BUTTON.
        Role::Button => "push button",
        role => role.name(),
    }
}

/// The author's role description when there is one, else the role name.
/// AccessKit has no translations, so the name stays in English.
fn localized_role_name(node: &PlatformNode) -> accesskit_atspi_common::Result<String> {
    let description = node.localized_role_name()?;
    if description.is_empty() {
        role_name(node).map(str::to_string)
    } else {
        Ok(description)
    }
}

/// The node's object attributes, plus `id` for its author id. Chromium and
/// Gecko expose the DOM `id` the same way, which is what tools that only
/// read attributes (and not the newer `AccessibleId` property) look for.
fn attributes(
    node: &PlatformNode,
) -> accesskit_atspi_common::Result<HashMap<&'static str, String>> {
    let mut attributes = node.attributes()?;
    let id = node.accessible_id()?;
    if !id.is_empty() {
        attributes.insert("id", id);
    }
    Ok(attributes)
}

pub(crate) struct RootAccessibleInterface {
    bus_name: OwnedUniqueName,
    root: PlatformRoot,
    desktop: Arc<OnceLock<ObjectRefOwned>>,
}

impl RootAccessibleInterface {
    pub fn new(
        bus_name: OwnedUniqueName,
        root: PlatformRoot,
        desktop: Arc<OnceLock<ObjectRefOwned>>,
    ) -> Self {
        Self {
            bus_name,
            root,
            desktop,
        }
    }
}

#[interface(name = "org.a11y.atspi.Accessible")]
impl RootAccessibleInterface {
    #[zbus(property)]
    fn name(&self) -> fdo::Result<String> {
        self.root.name().map_err(map_root_error)
    }

    #[zbus(property)]
    fn description(&self) -> fdo::Result<String> {
        self.root.description().map_err(map_root_error)
    }

    #[zbus(property)]
    fn parent(&self) -> OwnedObjectAddress {
        self.desktop
            .get()
            .cloned()
            .unwrap_or_default()
            .into_inner()
            .into()
    }

    #[zbus(property)]
    fn child_count(&self) -> fdo::Result<i32> {
        self.root.child_count().map_err(map_root_error)
    }

    #[zbus(property)]
    fn locale(&self) -> &str {
        ""
    }

    #[zbus(property)]
    fn accessible_id(&self) -> &str {
        ""
    }

    fn get_child_at_index(&self, index: i32) -> fdo::Result<(OwnedObjectAddress,)> {
        let index = index
            .try_into()
            .map_err(|_| fdo::Error::InvalidArgs("Index can't be negative.".into()))?;
        let child = self
            .root
            .child_id_at_index(index)
            .map_err(map_root_error)?
            .map(|(adapter, node)| ObjectId::Node { adapter, node });
        Ok(super::optional_object_address(&self.bus_name, child))
    }

    fn get_children(&self) -> fdo::Result<Vec<OwnedObjectAddress>> {
        self.root
            .map_child_ids(|(adapter, node)| {
                ObjectId::Node { adapter, node }.to_address(self.bus_name.inner())
            })
            .map_err(map_root_error)
    }

    fn get_index_in_parent(&self) -> i32 {
        self.root.index_in_parent()
    }

    fn get_relation_set(&self) -> Vec<(RelationType, Vec<OwnedObjectAddress>)> {
        Vec::new()
    }

    fn get_role(&self) -> Role {
        self.root.role()
    }

    fn get_role_name(&self) -> &'static str {
        atspi_role_name(self.root.role())
    }

    fn get_localized_role_name(&self) -> &'static str {
        atspi_role_name(self.root.role())
    }

    fn get_attributes(&self) -> HashMap<&str, String> {
        HashMap::new()
    }

    fn get_state(&self) -> StateSet {
        self.root.state()
    }

    fn get_application(&self) -> (OwnedObjectAddress,) {
        (ObjectId::Root.to_address(self.bus_name.inner()),)
    }

    fn get_interfaces(&self) -> InterfaceSet {
        self.root.interfaces()
    }
}

#[cfg(test)]
mod tests {
    use super::NodeAccessibleInterface;
    use accesskit::{
        ActionHandler, ActionRequest, Node, NodeId as LocalNodeId, Role, Tree, TreeId, TreeUpdate,
    };
    use accesskit_atspi_common::{
        Adapter, AdapterCallback, AppContext, Event, NodeId, NodeIdOrRoot, WindowBounds,
    };
    use std::sync::{Arc, Mutex};
    use zbus::names::OwnedUniqueName;

    struct NoOpActionHandler;
    impl ActionHandler for NoOpActionHandler {
        fn do_action(&mut self, _request: ActionRequest) {}
    }

    /// Records the nodes the adapter registers, the way the real adapter
    /// learns which objects to serve on the bus.
    #[derive(Default)]
    struct Registered(Arc<Mutex<Vec<NodeId>>>);
    impl AdapterCallback for Registered {
        fn register_interfaces(&self, _: &Adapter, id: NodeId, _: atspi::InterfaceSet) {
            self.0.lock().unwrap().push(id);
        }
        fn unregister_interfaces(&self, _: &Adapter, _: NodeId, _: atspi::InterfaceSet) {}
        fn emit_event(&self, _: &Adapter, _: Event) {}
    }

    /// A window holding `children`; returns the adapter and the Accessible
    /// interface of each child, in order.
    fn window_with(children: Vec<Node>) -> (Adapter, Vec<NodeAccessibleInterface>) {
        let ids: Vec<_> = (1..=children.len() as u64).map(LocalNodeId).collect();
        let mut window = Node::new(Role::Window);
        window.set_children(ids.clone());
        let mut nodes = vec![(LocalNodeId(0), window)];
        nodes.extend(ids.iter().copied().zip(children));
        let update = TreeUpdate {
            nodes,
            tree: Some(Tree::new(LocalNodeId(0))),
            tree_id: TreeId::ROOT,
            focus: LocalNodeId(0),
        };
        let registered = Registered::default();
        let seen = Arc::clone(&registered.0);
        let adapter = Adapter::new(
            &AppContext::new(None),
            registered,
            update,
            true,
            WindowBounds::default(),
            NoOpActionHandler,
        );
        let mut children: Vec<_> = seen
            .lock()
            .unwrap()
            .iter()
            .map(|&id| adapter.platform_node(id))
            .filter(|node| matches!(node.parent(), Ok(NodeIdOrRoot::Node(_))))
            .collect();
        children.sort_by_key(|node| node.index_in_parent().unwrap());
        let bus_name = OwnedUniqueName::try_from(":1.0").unwrap();
        let interfaces = children
            .into_iter()
            .map(|node| NodeAccessibleInterface::new(bus_name.clone(), node))
            .collect();
        (adapter, interfaces)
    }

    #[test]
    fn role_names_follow_libatspi() {
        let cases = [
            (Role::Button, "push button"),
            (Role::CheckBox, "check box"),
            (Role::TextInput, "entry"),
            (Role::Heading, "heading"),
            (Role::ListItem, "list item"),
        ];
        let (_adapter, nodes) =
            window_with(cases.iter().map(|(role, _)| Node::new(*role)).collect());
        assert_eq!(nodes.len(), cases.len());
        for ((role, expected), node) in cases.iter().zip(&nodes) {
            assert_eq!(node.get_role_name().unwrap(), *expected, "{role:?}");
            assert_eq!(
                node.get_localized_role_name().unwrap(),
                *expected,
                "{role:?}"
            );
        }
    }

    #[test]
    fn localized_role_name_prefers_role_description() {
        let mut node = Node::new(Role::Button);
        node.set_role_description("toolbar toggle");
        let (_adapter, nodes) = window_with(vec![node]);
        assert_eq!(
            nodes[0].get_localized_role_name().unwrap(),
            "toolbar toggle"
        );
    }

    #[test]
    fn attributes_carry_author_id_beside_object_attributes() {
        let mut with_id = Node::new(Role::ListItem);
        with_id.set_author_id("row-7");
        // AccessKit counts from 0; AT-SPI's posinset from 1.
        with_id.set_position_in_set(6);
        let (_adapter, nodes) = window_with(vec![with_id, Node::new(Role::Button)]);

        let attributes = nodes[0].get_attributes().unwrap();
        let mut pairs: Vec<_> = attributes.iter().map(|(k, v)| format!("{k}={v}")).collect();
        pairs.sort();
        assert_eq!(pairs, ["id=row-7", "posinset=7"]);
        assert!(!nodes[1].get_attributes().unwrap().contains_key("id"));
    }
}
