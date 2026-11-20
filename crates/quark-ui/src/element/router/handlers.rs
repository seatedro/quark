//! The handlers a painted frame registered, in parallel columns keyed by
//! semantic node.

use super::binding::parsed_binding;
use super::scroll::AxisTarget;
use super::*;

/// Event handlers of one frame. Each kind is a pair of parallel columns:
/// the owning semantic node index and the handler.
#[derive(Default)]
pub struct InputHandlers {
    click_node: Vec<usize>,
    click: Vec<ClickHandler>,
    middle_click_node: Vec<usize>,
    middle_click: Vec<Action>,
    context_menu_node: Vec<usize>,
    context_menu: Vec<ContextMenuHandler>,
    drag_node: Vec<usize>,
    drag: Vec<DragStart>,
    scroll_node: Vec<usize>,
    scroll: Vec<ScrollTarget>,
    scroll_x_node: Vec<usize>,
    scroll_x: Vec<ScrollTarget>,
    handle_node: Vec<usize>,
    handle: Vec<HandleTarget>,
    scrollbar_node: Vec<usize>,
    scrollbar: Vec<(Scrollbar, ScrollSink)>,
    key_node: Vec<usize>,
    /// Shared so a replayed cache boundary re-registers its bindings
    /// without copying their key strings.
    key_binding: Vec<Rc<Binding>>,
    key_action: Vec<Action>,
}

impl InputHandlers {
    pub fn on_click(&mut self, node: usize, handler: ClickHandler) {
        self.click_node.push(node);
        self.click.push(handler);
    }

    /// A click of the middle (auxiliary) button: a press and release on the
    /// same handler.
    pub fn on_middle_click(&mut self, node: usize, action: Action) {
        self.middle_click_node.push(node);
        self.middle_click.push(action);
    }

    /// A secondary click (Ctrl-click on macOS), Shift+F10, or the Menu
    /// key on this node or a descendant without its own.
    pub fn on_context_menu(&mut self, node: usize, handler: ContextMenuHandler) {
        self.context_menu_node.push(node);
        self.context_menu.push(handler);
    }

    pub fn on_drag(&mut self, node: usize, start: DragStart) {
        self.drag_node.push(node);
        self.drag.push(start);
    }

    /// Vertical wheel input for an app-owned offset.
    pub fn on_scroll(&mut self, node: usize, target: ScrollTarget) {
        self.scroll_node.push(node);
        self.scroll.push(target);
    }

    /// Horizontal wheel input for an app-owned offset; `target.builder`
    /// gets the lines.
    pub fn on_scroll_x(&mut self, node: usize, target: ScrollTarget) {
        self.scroll_x_node.push(node);
        self.scroll_x.push(target);
    }

    /// Wheel and key input on `axes` move `handle`. Takes precedence over
    /// app-owned targets of the same node on those axes.
    pub fn on_scroll_handle(&mut self, node: usize, handle: ScrollHandle, axes: ScrollAxes) {
        self.handle_node.push(node);
        self.handle.push(HandleTarget { handle, axes });
    }

    /// A press on `bar` pages or drags its thumb; kept apart from
    /// [`Self::on_drag`] so a frame registers it without allocating.
    pub(crate) fn on_scrollbar(&mut self, node: usize, bar: Scrollbar, sink: ScrollSink) {
        self.scrollbar_node.push(node);
        self.scrollbar.push((bar, sink));
    }

    /// `binding` uses the keymap format, e.g. `"enter"` or `"mod+s"`. A
    /// binding that does not parse is a programming error: it panics in
    /// debug builds and never fires in release builds.
    pub fn on_key(&mut self, node: usize, binding: impl AsRef<str>, action: Action) {
        match parsed_binding(binding.as_ref()) {
            Ok(parsed) => {
                self.key_node.push(node);
                self.key_binding.push(parsed);
                self.key_action.push(action);
            }
            Err(error) => debug_assert!(false, "{error}"),
        }
    }

    pub fn clear(&mut self) {
        self.click_node.clear();
        self.click.clear();
        self.middle_click_node.clear();
        self.middle_click.clear();
        self.context_menu_node.clear();
        self.context_menu.clear();
        self.drag_node.clear();
        self.drag.clear();
        self.scroll_node.clear();
        self.scroll.clear();
        self.scroll_x_node.clear();
        self.scroll_x.clear();
        self.handle_node.clear();
        self.handle.clear();
        self.scrollbar_node.clear();
        self.scrollbar.clear();
        self.key_node.clear();
        self.key_binding.clear();
        self.key_action.clear();
    }

    pub(in crate::element) fn marks(&self) -> HandlerMarks {
        HandlerMarks {
            click: self.click.len(),
            middle_click: self.middle_click.len(),
            context_menu: self.context_menu.len(),
            drag: self.drag.len(),
            scroll: self.scroll.len(),
            scroll_x: self.scroll_x.len(),
            handle: self.handle.len(),
            scrollbar: self.scrollbar.len(),
            key: self.key_action.len(),
        }
    }

    /// Replace `out` with the handlers registered since `marks`, their nodes
    /// made relative to `node_base`. False when one belongs to a node
    /// before `node_base`, which a relative copy cannot express.
    pub(in crate::element) fn copy_since(
        &self,
        marks: HandlerMarks,
        node_base: usize,
        out: &mut InputHandlers,
    ) -> bool {
        fn nodes(from: &[usize], base: usize, out: &mut Vec<usize>) -> bool {
            out.clear();
            out.extend(from.iter().map(|node| node.wrapping_sub(base)));
            from.iter().all(|node| *node >= base)
        }
        let ok = nodes(
            &self.click_node[marks.click..],
            node_base,
            &mut out.click_node,
        ) & nodes(
            &self.middle_click_node[marks.middle_click..],
            node_base,
            &mut out.middle_click_node,
        ) & nodes(
            &self.context_menu_node[marks.context_menu..],
            node_base,
            &mut out.context_menu_node,
        ) & nodes(&self.drag_node[marks.drag..], node_base, &mut out.drag_node)
            & nodes(
                &self.scroll_node[marks.scroll..],
                node_base,
                &mut out.scroll_node,
            )
            & nodes(
                &self.scroll_x_node[marks.scroll_x..],
                node_base,
                &mut out.scroll_x_node,
            )
            & nodes(
                &self.handle_node[marks.handle..],
                node_base,
                &mut out.handle_node,
            )
            & nodes(
                &self.scrollbar_node[marks.scrollbar..],
                node_base,
                &mut out.scrollbar_node,
            )
            & nodes(&self.key_node[marks.key..], node_base, &mut out.key_node);
        fn copy<T: Clone>(from: &[T], out: &mut Vec<T>) {
            out.clear();
            out.extend_from_slice(from);
        }
        copy(&self.click[marks.click..], &mut out.click);
        copy(
            &self.middle_click[marks.middle_click..],
            &mut out.middle_click,
        );
        copy(
            &self.context_menu[marks.context_menu..],
            &mut out.context_menu,
        );
        copy(&self.drag[marks.drag..], &mut out.drag);
        copy(&self.scroll[marks.scroll..], &mut out.scroll);
        copy(&self.scroll_x[marks.scroll_x..], &mut out.scroll_x);
        copy(&self.handle[marks.handle..], &mut out.handle);
        copy(&self.scrollbar[marks.scrollbar..], &mut out.scrollbar);
        copy(&self.key_binding[marks.key..], &mut out.key_binding);
        copy(&self.key_action[marks.key..], &mut out.key_action);
        ok
    }

    /// Append `from`, whose nodes are relative, with nodes at `node_base`.
    pub(in crate::element) fn extend_shifted(&mut self, from: &InputHandlers, node_base: usize) {
        fn shift(nodes: &[usize], base: usize) -> impl Iterator<Item = usize> + '_ {
            nodes.iter().map(move |node| node + base)
        }
        self.click_node.extend(shift(&from.click_node, node_base));
        self.click.extend_from_slice(&from.click);
        self.middle_click_node
            .extend(shift(&from.middle_click_node, node_base));
        self.middle_click.extend_from_slice(&from.middle_click);
        self.context_menu_node
            .extend(shift(&from.context_menu_node, node_base));
        self.context_menu.extend_from_slice(&from.context_menu);
        self.drag_node.extend(shift(&from.drag_node, node_base));
        self.drag.extend_from_slice(&from.drag);
        self.scroll_node.extend(shift(&from.scroll_node, node_base));
        self.scroll.extend_from_slice(&from.scroll);
        self.scroll_x_node
            .extend(shift(&from.scroll_x_node, node_base));
        self.scroll_x.extend_from_slice(&from.scroll_x);
        self.handle_node.extend(shift(&from.handle_node, node_base));
        self.handle.extend_from_slice(&from.handle);
        self.scrollbar_node
            .extend(shift(&from.scrollbar_node, node_base));
        self.scrollbar.extend_from_slice(&from.scrollbar);
        self.key_node.extend(shift(&from.key_node, node_base));
        self.key_binding.extend_from_slice(&from.key_binding);
        self.key_action.extend_from_slice(&from.key_action);
    }

    pub(in crate::element) fn click(&self, node: usize) -> Option<&ClickHandler> {
        let i = self.click_node.iter().position(|n| *n == node)?;
        self.click.get(i)
    }

    pub(super) fn middle_click(&self, node: usize) -> Option<&Action> {
        let i = self.middle_click_node.iter().position(|n| *n == node)?;
        self.middle_click.get(i)
    }

    pub(super) fn context_menu(&self, node: usize) -> Option<&ContextMenuHandler> {
        let i = self.context_menu_node.iter().position(|n| *n == node)?;
        self.context_menu.get(i)
    }

    fn drag(&self, node: usize) -> Option<&DragStart> {
        let i = self.drag_node.iter().position(|n| *n == node)?;
        self.drag.get(i)
    }

    /// Whether a press on `node` starts a drag, its scrollbar's included,
    /// or clicks.
    pub(super) fn takes_press(&self, node: usize) -> bool {
        self.drag(node).is_some()
            || self.scrollbar_node.contains(&node)
            || self.click(node).is_some()
    }

    /// The drag `press` on `node` starts, if it has one.
    pub(super) fn start_drag(
        &self,
        node: usize,
        press: ClickEvent,
    ) -> Option<Box<dyn DragHandler>> {
        if let Some(start) = self.drag(node) {
            return Some(start.start(press));
        }
        let i = self.scrollbar_node.iter().position(|n| *n == node)?;
        let (bar, sink) = self.scrollbar.get(i)?;
        Some(Box::new(ScrollbarDrag::new(
            *bar,
            sink.clone(),
            press.x,
            press.y,
        )))
    }

    pub(super) fn axis_target(&self, node: usize, axis: Axis) -> Option<AxisTarget<'_>> {
        let handle = self
            .handle_node
            .iter()
            .zip(&self.handle)
            .find(|(n, target)| **n == node && target.axes.has(axis));
        if let Some((_, target)) = handle {
            return Some(AxisTarget::Handle(&target.handle));
        }
        let (nodes, targets) = match axis {
            Axis::X => (&self.scroll_x_node, &self.scroll_x),
            Axis::Y => (&self.scroll_node, &self.scroll),
        };
        let i = nodes.iter().position(|n| *n == node)?;
        targets.get(i).map(AxisTarget::Builder)
    }

    pub(super) fn key(&self, node: usize, pressed: &Binding) -> Option<&Action> {
        let i = (0..self.key_node.len())
            .find(|&i| self.key_node[i] == node && self.key_binding[i].matches(pressed))?;
        self.key_action.get(i)
    }
}

/// Column lengths of [`InputHandlers`]: where a cache boundary's
/// registrations start.
#[derive(Debug, Clone, Copy)]
pub(in crate::element) struct HandlerMarks {
    click: usize,
    middle_click: usize,
    context_menu: usize,
    drag: usize,
    scroll: usize,
    scroll_x: usize,
    handle: usize,
    scrollbar: usize,
    key: usize,
}
