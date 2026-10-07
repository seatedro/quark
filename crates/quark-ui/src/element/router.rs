use super::*;

// ---------------------------------------------------------------------------
// Input routing: one hit table, handlers keyed by semantic node, and
// capture/bubble dispatch along SemanticFrame routes.
// ---------------------------------------------------------------------------

/// A node's wheel handler plus its position in the scroll range, so wheel
/// input can chain to the parent once this node is at its limit.
#[derive(Debug, Clone)]
pub struct ScrollTarget {
    pub builder: ScrollActionBuilder,
    pub offset: f32,
    /// Largest offset, or `None` when the element does not know its
    /// content size (it then never reports a limit when scrolling down).
    pub max: Option<f32>,
}

impl ScrollTarget {
    fn at_limit(&self, lines: i32) -> bool {
        if lines < 0 {
            self.offset <= 0.0
        } else {
            self.max.is_some_and(|max| self.offset >= max)
        }
    }
}

/// Event handlers of one frame. Each kind is a pair of parallel columns:
/// the owning semantic node index and the handler.
#[derive(Default)]
pub struct InputHandlers {
    click_node: Vec<usize>,
    click: Vec<ClickHandler>,
    drag_node: Vec<usize>,
    drag: Vec<DragStart>,
    scroll_node: Vec<usize>,
    scroll: Vec<ScrollTarget>,
    key_node: Vec<usize>,
    key_binding: Vec<String>,
    key_action: Vec<Action>,
}

impl InputHandlers {
    pub fn on_click(&mut self, node: usize, handler: ClickHandler) {
        self.click_node.push(node);
        self.click.push(handler);
    }

    pub fn on_drag(&mut self, node: usize, start: DragStart) {
        self.drag_node.push(node);
        self.drag.push(start);
    }

    pub fn on_scroll(&mut self, node: usize, target: ScrollTarget) {
        self.scroll_node.push(node);
        self.scroll.push(target);
    }

    /// `binding` uses the keymap format, e.g. `"enter"` or `"cmd+s"`.
    pub fn on_key(&mut self, node: usize, binding: impl Into<String>, action: Action) {
        self.key_node.push(node);
        self.key_binding.push(binding.into());
        self.key_action.push(action);
    }

    fn click(&self, node: usize) -> Option<&ClickHandler> {
        let i = self.click_node.iter().position(|n| *n == node)?;
        self.click.get(i)
    }

    fn drag(&self, node: usize) -> Option<&DragStart> {
        let i = self.drag_node.iter().position(|n| *n == node)?;
        self.drag.get(i)
    }

    fn scroll(&self, node: usize) -> Option<&ScrollTarget> {
        let i = self.scroll_node.iter().position(|n| *n == node)?;
        self.scroll.get(i)
    }

    fn key(&self, node: usize, binding: &str) -> Option<&Action> {
        let i = (0..self.key_node.len()).find(|&i| {
            self.key_node[i] == node && self.key_binding[i].eq_ignore_ascii_case(binding)
        })?;
        self.key_action.get(i)
    }
}

/// What one painted frame registered for input.
#[derive(Default)]
pub struct InputFrame {
    pub hits: HitTable,
    pub handlers: InputHandlers,
    pub semantic: SemanticFrame,
}

/// The outcome of routing one event: the semantic node that handled it
/// and the actions it emitted.
#[derive(Debug, Default)]
pub struct Delivery {
    pub node: Option<usize>,
    pub actions: Vec<Action>,
}

struct Capture {
    node: usize,
    drag: Box<dyn DragHandler>,
}

/// Routes pointer, wheel, and key input through the last painted frame.
#[derive(Default)]
pub struct InputRouter {
    frame: InputFrame,
    focus_tree: FocusTree,
    capture: Option<Capture>,
}

impl InputRouter {
    pub fn set_frame(&mut self, frame: InputFrame) {
        self.focus_tree = frame.semantic.focus_tree();
        self.frame = frame;
    }

    pub fn frame(&self) -> &InputFrame {
        &self.frame
    }

    pub fn is_capturing(&self) -> bool {
        self.capture.is_some()
    }

    /// Topmost semantic node under the point, honoring clips, z, and
    /// `BLOCKS_MOUSE`.
    pub fn target_at(&self, x: f32, y: f32) -> Option<usize> {
        self.frame
            .hits
            .stack_at(x, y)
            .into_iter()
            .find_map(|id| self.frame.hits.node(id))
    }

    pub fn cursor_at(&self, x: f32, y: f32) -> CursorHint {
        if let Some(capture) = &self.capture {
            return capture.drag.cursor();
        }
        self.frame
            .hits
            .stack_at(x, y)
            .into_iter()
            .map(|id| self.frame.hits.cursor(id))
            .find(|cursor| *cursor != CursorHint::Default)
            .unwrap_or_default()
    }

    /// Press: update `focus`, then start a drag (capturing the pointer and
    /// delivering its press actions) or deliver a click to the first handler
    /// on the route.
    pub fn pointer_down(&mut self, x: f32, y: f32, focus: &mut Option<FocusId>) -> Delivery {
        let target = self.target_at(x, y);
        if let Some(next) = self.focus_after_press(target) {
            *focus = next;
        }
        let Some(target) = target else {
            return Delivery::default();
        };
        let handlers = &self.frame.handlers;
        // The innermost node with a drag or click handler owns the press, so
        // a button inside a draggable panel still clicks.
        let drag = self.walk(target, UiEventKind::PointerDown, |node| {
            (handlers.drag(node).is_some() || handlers.click(node).is_some()).then(Vec::new)
        });
        if let Some(node) = drag.node
            && let Some(start) = handlers.drag(node)
        {
            let mut drag = start.start(ClickEvent { x, y });
            let actions = drag.on_press();
            self.capture = Some(Capture { node, drag });
            return Delivery {
                node: Some(node),
                actions,
            };
        }
        self.walk(target, UiEventKind::Click, |node| {
            handlers
                .click(node)
                .map(|handler| handler.invoke(ClickEvent { x, y }))
        })
    }

    /// Moves go to the capturing drag, wherever the pointer is.
    pub fn pointer_move(&mut self, x: f32, y: f32) -> Delivery {
        match &mut self.capture {
            Some(capture) => Delivery {
                node: Some(capture.node),
                actions: capture.drag.on_move(x, y),
            },
            None => Delivery::default(),
        }
    }

    /// Release ends pointer capture.
    pub fn pointer_up(&mut self) -> Delivery {
        match self.capture.take() {
            Some(mut capture) => Delivery {
                node: Some(capture.node),
                actions: capture.drag.on_release().actions,
            },
            None => Delivery::default(),
        }
    }

    /// Wheel goes to the innermost scrollable under the pointer. A node at
    /// its limit in the wheel's direction passes the input to the next
    /// scrollable ancestor; when every one is at its limit, the innermost
    /// still gets it so the app can clamp or rubber-band.
    pub fn wheel(&self, x: f32, y: f32, lines: i32) -> Delivery {
        let Some(target) = self.target_at(x, y) else {
            return Delivery::default();
        };
        let handlers = &self.frame.handlers;
        let mut innermost = None;
        let delivery = self.walk(target, UiEventKind::Wheel, |node| {
            let scroll = handlers.scroll(node)?;
            innermost.get_or_insert(node);
            (!scroll.at_limit(lines)).then(|| vec![scroll.builder.build(lines)])
        });
        if delivery.node.is_some() {
            return delivery;
        }
        match innermost.and_then(|node| Some((node, handlers.scroll(node)?))) {
            Some((node, scroll)) => Delivery {
                node: Some(node),
                actions: vec![scroll.builder.build(lines)],
            },
            None => delivery,
        }
    }

    /// Key press: the focused node first, then its ancestors.
    pub fn key_down(&self, binding: &str, focus: Option<FocusId>) -> Delivery {
        let Some(target) = focus.and_then(|focus| self.frame.semantic.node_for_focus(focus)) else {
            return Delivery::default();
        };
        let handlers = &self.frame.handlers;
        self.walk(target, UiEventKind::KeyDown, |node| {
            handlers
                .key(node, binding)
                .map(|action| vec![action.clone()])
        })
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

    /// Focus after a press on `target`: the nearest text field on its path,
    /// or none. `None` means focus stays, which is the case for presses
    /// outside an open modal.
    fn focus_after_press(&self, target: Option<usize>) -> Option<Option<FocusId>> {
        let semantic = &self.frame.semantic;
        if let Some(modal) = semantic.modal_root()
            && !target.is_some_and(|node| semantic.is_within(node, modal))
        {
            return None;
        }
        let focus = target.and_then(|target| {
            semantic
                .ancestors_inclusive(target)
                .filter(|i| semantic.nodes()[*i].actions.text_value)
                .find_map(|i| semantic.focus_id(i))
        });
        Some(focus)
    }

    /// Capture then bubble along `target`'s route. `handle` runs at the
    /// target and bubble steps; the first `Some` handles the event. An event
    /// binding whose default result stops propagation ends the walk at its
    /// node.
    fn walk(
        &self,
        target: usize,
        kind: UiEventKind,
        mut handle: impl FnMut(usize) -> Option<Vec<Action>>,
    ) -> Delivery {
        let Some(steps) = self.frame.semantic.route_indices(target) else {
            return Delivery::default();
        };
        for (node, phase) in steps {
            if phase != UiEventPhase::Capture
                && let Some(actions) = handle(node)
            {
                return Delivery {
                    node: Some(node),
                    actions,
                };
            }
            if self.binding_stops(node, kind, phase) {
                return Delivery {
                    node: Some(node),
                    actions: Vec::new(),
                };
            }
        }
        Delivery::default()
    }

    fn binding_stops(&self, node: usize, kind: UiEventKind, phase: UiEventPhase) -> bool {
        let capture = phase == UiEventPhase::Capture;
        self.frame.semantic.nodes()[node]
            .event_bindings
            .iter()
            .any(|binding| {
                binding.kind == kind
                    && (binding.phase == UiEventPhase::Capture) == capture
                    && !binding.default_result.should_continue()
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::Styled;
    use crate::theme::Theme;

    #[derive(Debug, Clone, PartialEq)]
    enum Msg {
        Click(&'static str),
        Scroll(&'static str, i32),
        Move(i32, i32),
        Release,
    }

    impl From<Msg> for Action {
        fn from(msg: Msg) -> Self {
            Action::new(msg)
        }
    }

    /// Paint `root` into a `w`x`h` window and route input through the result.
    fn routed(root: impl IntoAnyElement, w: f32, h: f32) -> InputRouter {
        // Vendored fonts only, so the tests do not depend on the host's fonts.
        let mut text = TextSystem::vendored_only(&Default::default());
        let mut layouts = LayoutCache::default();
        let theme = Theme::default_dark();
        let signals = SignalStore::new();
        let mut cx = ElementContext::new(&theme, 1.0, &mut text, &mut layouts, None, &signals);
        cx.semantic = SemanticFrame::new(w, h);
        let mut root = root.into_any();
        render_element(&mut root, &mut Scene::default(), &mut cx, w, h);
        let mut router = InputRouter::default();
        router.set_frame(cx.take_input_frame());
        router
    }

    /// `"<node test id> <actions>"`, or `"-"` when nothing handled the event.
    fn dump(router: &InputRouter, delivery: Delivery) -> String {
        let Some(node) = delivery.node else {
            return "-".to_owned();
        };
        let name = router.frame().semantic.nodes()[node]
            .test_id
            .as_ref()
            .map_or("?", |id| id.as_str())
            .to_owned();
        format!("{name} {:?}", delivery.actions)
    }

    fn click(router: &mut InputRouter, x: f32, y: f32) -> String {
        let delivery = router.pointer_down(x, y, &mut None);
        router.pointer_up();
        dump(router, delivery)
    }

    fn button(name: &'static str, w: f32, h: f32) -> Div {
        div().w(w).h(h).test_id(name).on_click(Msg::Click(name))
    }

    #[test]
    fn clipped_out_row_cannot_take_clicks_outside_its_clip() {
        // The list starts at y=40 and is scrolled by 100, so row-1 spans
        // y=-10..40: inside its bounds at y=20, but outside the list's clip.
        let rows = ["row-0", "row-1", "row-2", "row-3", "row-4"]
            .map(|name| button(name, 200.0, 50.0).flex_shrink_0());
        let root = div()
            .w(200.0)
            .h(140.0)
            .flex_col()
            .child(button("header", 200.0, 40.0))
            .child(
                div()
                    .w(200.0)
                    .h(100.0)
                    .flex_col()
                    .scroll_y(100.0)
                    .children_from(rows),
            );
        let mut router = routed(root, 200.0, 140.0);

        let got = [
            click(&mut router, 10.0, 20.0),
            click(&mut router, 10.0, 60.0),
        ];

        assert_eq!(
            got,
            [r#"header [Click("header")]"#, r#"row-2 [Click("row-2")]"#]
        );
    }

    #[test]
    fn higher_z_wins_over_later_paint_order() {
        let root = div()
            .w(100.0)
            .h(100.0)
            .child(
                button("raised", 100.0, 100.0)
                    .absolute()
                    .top(0.0)
                    .left(0.0)
                    .z_index(10),
            )
            .child(button("later", 100.0, 100.0).absolute().top(0.0).left(0.0));
        let mut router = routed(root, 100.0, 100.0);

        assert_eq!(
            click(&mut router, 50.0, 50.0),
            r#"raised [Click("raised")]"#
        );
    }

    #[test]
    fn wheel_chains_to_parent_only_at_the_scroll_limit() {
        // The inner list is scrolled to its end (300 content - 100 viewport).
        let inner = div()
            .w(200.0)
            .h(100.0)
            .test_id("inner")
            .scroll_y(200.0)
            .scroll_total(300.0)
            .on_scroll(ScrollActionBuilder::new(|lines| {
                Msg::Scroll("inner", lines).into()
            }))
            .child(div().w(200.0).h(300.0));
        let outer = div()
            .w(200.0)
            .h(200.0)
            .test_id("outer")
            .flex_col()
            .scroll_y(0.0)
            .scroll_total(1000.0)
            .on_scroll(ScrollActionBuilder::new(|lines| {
                Msg::Scroll("outer", lines).into()
            }))
            .child(inner);
        let router = routed(outer, 200.0, 200.0);

        let got = [3, -3].map(|lines| dump(&router, router.wheel(50.0, 50.0, lines)));

        assert_eq!(
            got,
            [
                r#"outer [Scroll("outer", 3)]"#,
                r#"inner [Scroll("inner", -3)]"#
            ]
        );
    }

    struct RecordDrag;

    impl DragHandler for RecordDrag {
        fn on_move(&mut self, x: f32, y: f32) -> Vec<Action> {
            vec![Msg::Move(x as i32, y as i32).into()]
        }

        fn on_release(&mut self) -> DragReleaseResult {
            DragReleaseResult {
                actions: vec![Msg::Release.into()],
            }
        }
    }

    #[test]
    fn drag_capture_keeps_delivering_after_leaving_bounds() {
        let root = div().w(400.0).h(400.0).child(
            div()
                .w(50.0)
                .h(50.0)
                .test_id("handle")
                .on_drag(|_| Box::new(RecordDrag)),
        );
        let mut router = routed(root, 400.0, 400.0);

        router.pointer_down(10.0, 10.0, &mut None);
        let moved = router.pointer_move(300.0, 300.0);
        let moved = dump(&router, moved);
        let released = router.pointer_up();
        let released = dump(&router, released);

        assert_eq!(
            [moved, released],
            ["handle [Move(300, 300)]", "handle [Release]"]
        );
    }

    mod streaming {
        use std::collections::HashMap;

        use quark::selection::BlockKey;

        use super::*;
        use crate::transcript::{
            TextMeasurer, Transcript, TranscriptBlock, TranscriptEvent, TranscriptMessage,
            TranscriptRole, TranscriptStyle,
        };
        use crate::virtual_list::RowKey;

        #[derive(Debug, Clone, PartialEq)]
        struct Ev(TranscriptEvent);

        impl From<Ev> for Action {
            fn from(ev: Ev) -> Self {
                Action::new(ev)
            }
        }

        type Messages = HashMap<RowKey, TranscriptMessage>;

        fn message(i: u64, text: &str) -> TranscriptMessage {
            TranscriptMessage {
                key: RowKey(i),
                role: TranscriptRole::Assistant,
                author: format!("author {i}").into(),
                blocks: vec![TranscriptBlock::plain(BlockKey(i), text)],
            }
        }

        /// A 400x300 transcript stuck to the bottom, with shared text state
        /// so each frame is painted the way the adapter paints it.
        struct Chat {
            messages: Messages,
            transcript: Transcript,
            text: TextSystem,
            layouts: LayoutCache,
            router: InputRouter,
        }

        impl Chat {
            fn new(n: u64) -> Self {
                let messages: Messages = (0..n)
                    .map(|i| (RowKey(i), message(i, &format!("Message {i} text."))))
                    .collect();
                let mut transcript = Transcript::new(TranscriptStyle::for_font_size(14.0));
                transcript
                    .extend((0..n).map(|i| &messages[&RowKey(i)]))
                    .unwrap();
                let mut chat = Self {
                    messages,
                    transcript,
                    text: TextSystem::vendored_only(&Default::default()),
                    layouts: LayoutCache::default(),
                    router: InputRouter::default(),
                };
                chat.frame();
                chat.transcript.jump_to_latest();
                chat.frame();
                chat
            }

            /// Prepare and paint a frame; route input through it from now on.
            fn frame(&mut self) {
                let (w, h) = (400.0, 300.0);
                self.transcript.prepare(
                    w,
                    h,
                    0,
                    &self.messages,
                    &mut TextMeasurer::new(&mut self.text, &mut self.layouts, 14.0, 1.0),
                );
                let theme = Theme::default_dark();
                let signals = SignalStore::new();
                let mut root = self
                    .transcript
                    .element(&self.messages, &theme, |ev| Ev(ev).into())
                    .into_any();
                let mut cx = ElementContext::new(
                    &theme,
                    1.0,
                    &mut self.text,
                    &mut self.layouts,
                    None,
                    &signals,
                );
                cx.semantic = SemanticFrame::new(w, h);
                render_element(&mut root, &mut Scene::default(), &mut cx, w, h);
                self.router.set_frame(cx.take_input_frame());
            }

            /// Apply delivered actions the way an app would, right away.
            fn apply(&mut self, delivery: Delivery) {
                for action in delivery.actions {
                    if let Some(Ev(event)) = action.downcast_ref::<Ev>() {
                        self.transcript.handle(*event);
                    }
                }
            }

            fn block_rect(&self, key: u64) -> Rect {
                self.transcript
                    .visible_blocks()
                    .iter()
                    .find(|b| b.key == BlockKey(key))
                    .map(|b| b.rect)
                    .expect("block is visible")
            }

            /// Grow the last message, as a streaming reply does.
            fn stream_into_last(&mut self, text: &str) {
                let last = RowKey(self.messages.len() as u64 - 1);
                let message = message(last.0, text);
                self.transcript.update(&message).unwrap();
                self.messages.insert(last, message);
            }
        }

        // Regression: a drag reported its press only with the first move, so
        // text streaming in between shifted the anchor onto other text.
        #[test]
        fn press_then_stream_keeps_the_anchor_at_the_pressed_text() {
            let mut chat = Chat::new(8);
            let from = chat.block_rect(5);

            let pressed = chat
                .router
                .pointer_down(from.x - 5.0, from.y + 5.0, &mut None);
            chat.apply(pressed);
            chat.stream_into_last(&"streamed words ".repeat(15));
            chat.frame();
            let to = chat.block_rect(5);
            let moved = chat
                .router
                .pointer_move(to.x + to.width + 40.0, to.y + to.height - 2.0);
            chat.apply(moved);
            let released = chat.router.pointer_up();
            chat.apply(released);

            assert!(to.y < from.y, "streaming moved the pressed block up");
            assert_eq!(
                chat.transcript.selected_text(&chat.messages),
                "Message 5 text."
            );
        }
    }

    fn focusable(name: &'static str) -> Div {
        div()
            .w(50.0)
            .h(20.0)
            .test_id(name)
            .focus_ring(FocusId::from_key(name))
    }

    #[test]
    fn tab_cycles_only_inside_a_modal_scope() {
        let root = div()
            .w(400.0)
            .h(300.0)
            .flex_col()
            .child(focusable("outside"))
            .child(
                div()
                    .flex_col()
                    .focus_scope("dialog")
                    .trap_focus(true)
                    .child(focusable("ok"))
                    .child(focusable("cancel")),
            );
        let router = routed(root, 400.0, 300.0);

        let mut focus = None;
        let visited: Vec<&str> = (0..3)
            .map(|_| {
                focus = router.traverse_focus(focus, false);
                ["outside", "ok", "cancel"]
                    .into_iter()
                    .find(|name| focus == Some(FocusId::from_key(name)))
                    .unwrap_or("?")
            })
            .collect();

        assert_eq!(visited, ["ok", "cancel", "ok"]);
    }

    #[test]
    fn click_outside_a_modal_scope_keeps_focus() {
        let inside = FocusId::from_key("inside");
        let root = div()
            .w(400.0)
            .h(300.0)
            .flex_col()
            .child(
                text_input("Outside", "")
                    .focus_target(FocusId::from_key("outside"))
                    .w(200.0)
                    .h(40.0),
            )
            .child(
                div().focus_scope("dialog").trap_focus(true).child(
                    text_input("Inside", "")
                        .focus_target(inside)
                        .w(200.0)
                        .h(40.0),
                ),
            );
        let mut router = routed(root, 400.0, 300.0);

        let mut focus = Some(inside);
        router.pointer_down(10.0, 10.0, &mut focus);

        assert_eq!(focus, Some(inside));
    }
}
