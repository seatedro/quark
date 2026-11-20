//! Keys: bindings along the focus path, Enter and Space clicking the
//! focused node, the context menu key, and Tab order.

use super::*;

impl InputRouter {
    /// Shift+F10 or the Menu key: the context menu of the focused element,
    /// or of its nearest ancestor with one, at the focused element's bottom
    /// left corner. Other keys and an unfocused window deliver nothing.
    pub fn context_menu_key(&self, pressed: &Binding, focus: Option<FocusId>) -> Delivery {
        let m = pressed.mods;
        let menu_key = match pressed.key.as_str() {
            "contextmenu" => !(m.cmd || m.ctrl || m.alt || m.shift),
            "f10" => m.shift && !(m.cmd || m.ctrl || m.alt),
            _ => false,
        };
        let semantic = &self.frame.semantic;
        let Some(focused) = focus
            .filter(|_| menu_key)
            .and_then(|focus| semantic.node_for_focus(focus))
        else {
            return Delivery::default();
        };
        let Some(b) = semantic.nodes().get(focused).map(|n| n.bounds) else {
            return Delivery::default();
        };
        let event = ContextMenuEvent {
            x: b.x,
            y: b.y + b.height,
            keyboard: true,
        };
        let handlers = &self.frame.handlers;
        self.walk(focused, UiEventKind::ContextMenu, |node| {
            handlers
                .context_menu(node)
                .map(|handler| vec![handler.invoke(event)])
        })
    }

    /// Key press: the focused node first, then its ancestors. With nothing
    /// focused, the top-level nodes get it, as a page's body would.
    pub fn key_down(&self, pressed: &Binding, focus: Option<FocusId>) -> Delivery {
        let semantic = &self.frame.semantic;
        let handlers = &self.frame.handlers;
        let route = |target| {
            self.walk(target, UiEventKind::KeyDown, |node| {
                handlers
                    .key(node, pressed)
                    .map(|action| vec![action.clone()])
            })
        };
        if let Some(target) = focus.and_then(|focus| semantic.node_for_focus(focus)) {
            return route(target);
        }
        (0..semantic.nodes().len())
            .filter(|&node| semantic.nodes()[node].parent.is_none())
            .map(route)
            .find(|delivery| delivery.node.is_some())
            .unwrap_or_default()
    }

    /// Enter or Space with no modifiers on the focused node clicks it, at
    /// its center, when it has a click handler. Ancestors are not tried: a
    /// key on a focused field must not press the card around it.
    pub fn activate(&self, pressed: &Binding, focus: Option<FocusId>) -> Delivery {
        let m = pressed.mods;
        if !matches!(pressed.key.as_str(), "enter" | "space") || m.cmd || m.ctrl || m.alt || m.shift
        {
            return Delivery::default();
        }
        match focus.and_then(|focus| self.frame.semantic.node_for_focus(focus)) {
            Some(node) => self.click_node(node),
            None => Delivery::default(),
        }
    }

    /// Click `node` at its center, as Enter on it when focused does.
    /// Nothing for a disabled node or one without a click handler.
    pub fn click_node(&self, node: usize) -> Delivery {
        let (Some(handler), Some(target)) = (
            self.frame.handlers.click(node),
            self.frame.semantic.nodes().get(node),
        ) else {
            return Delivery::default();
        };
        if target.state.disabled {
            return Delivery::default();
        }
        let b = target.bounds;
        let center = ClickEvent::at(b.x + b.width / 2.0, b.y + b.height / 2.0);
        Delivery::handled(node, handler.invoke(center))
    }

    /// Next focus along the tab order, wrapping at either end. Inside a
    /// modal scope, only that scope's targets are in the order.
    pub fn traverse_focus(&self, focus: Option<FocusId>, backwards: bool) -> Option<FocusId> {
        let order: Vec<FocusId> = self
            .focus_tree
            .tab_order(None)
            .into_iter()
            .map(|node| node.id)
            .collect();
        if order.is_empty() {
            return focus;
        }
        let current = focus.and_then(|focus| order.iter().position(|id| *id == focus));
        let next = match (current, backwards) {
            (None, false) => 0,
            (None, true) => order.len() - 1,
            (Some(i), false) => (i + 1) % order.len(),
            (Some(i), true) => (i + order.len() - 1) % order.len(),
        };
        Some(order[next])
    }
}
