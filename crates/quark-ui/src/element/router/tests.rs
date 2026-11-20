use super::*;
use crate::style::Styled;
use crate::theme::Theme;

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Click(&'static str),
    Scroll(&'static str, i32),
    Move(i32, i32),
    Release,
    Cancel,
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
    let mut router = routed(outer, 200.0, 200.0);

    let got = [3.0, -3.0].map(|lines| {
        let delivery = router.wheel(50.0, 50.0, lines * WHEEL_LINE_PX);
        dump(&router, delivery)
    });

    assert_eq!(
        got,
        [
            r#"outer [Scroll("outer", 3)]"#,
            r#"inner [Scroll("inner", -3)]"#
        ]
    );
}

// A code block that scrolls sideways inside a page that scrolls both
// ways: each axis goes to the innermost container scrolling on it that
// is not at its limit.
#[test]
fn wheel_chains_per_axis() {
    let route = |inner_x: f32, dx: f32, dy: f32| {
        let (outer, inner) = (ScrollHandle::new(), ScrollHandle::new());
        inner.set_offset(inner_x, 0.0);
        let code = div()
            .w(200.0)
            .h(100.0)
            .flex_shrink_0()
            .track_scroll(&inner)
            .overflow_x_scroll()
            .child(div().w(600.0).h(100.0).flex_shrink_0());
        let page = div()
            .w(200.0)
            .h(200.0)
            .flex_col()
            .track_scroll(&outer)
            .overflow_scroll()
            .child(code)
            .child(div().w(400.0).h(1000.0).flex_shrink_0());
        let mut router = routed(page, 200.0, 200.0);
        router.scroll_wheel(50.0, 50.0, WheelEvent { dx, dy, now_ms: 0 });
        format!("outer {:?} inner {:?}", outer.offset(), inner.offset())
    };

    let got = [
        route(0.0, 0.0, 40.0),
        route(0.0, 40.0, 0.0),
        // The code block is scrolled to its end (600 - 200).
        route(400.0, 40.0, 0.0),
    ];

    assert_eq!(
        got,
        [
            "outer (0.0, 40.0) inner (0.0, 0.0)",
            "outer (0.0, 0.0) inner (40.0, 0.0)",
            "outer (40.0, 0.0) inner (400.0, 0.0)",
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

    fn on_cancel(&mut self) -> Vec<Action> {
        vec![Msg::Cancel.into()]
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

#[test]
fn clicks_and_drags_report_held_modifiers() {
    struct ModsDrag(Mods);
    fn name(m: Mods) -> &'static str {
        match (m.shift, m.alt) {
            (true, _) => "shift",
            (_, true) => "alt",
            _ => "none",
        }
    }
    impl DragHandler for ModsDrag {
        fn on_move(&mut self, _x: f32, _y: f32) -> Vec<Action> {
            vec![Msg::Click(name(self.0)).into()]
        }

        fn on_release(&mut self) -> DragReleaseResult {
            DragReleaseResult {
                actions: vec![Msg::Click(name(self.0)).into()],
            }
        }

        fn set_modifiers(&mut self, modifiers: Mods) {
            self.0 = modifiers;
        }

        fn on_cancel(&mut self) -> Vec<Action> {
            Vec::new()
        }
    }
    let root = div()
        .w(400.0)
        .h(400.0)
        .flex_row()
        .child(
            div()
                .w(50.0)
                .h(50.0)
                .test_id("button")
                .on_click_handler(ClickHandler::new(|e| {
                    vec![Msg::Click(name(e.modifiers)).into()]
                })),
        )
        .child(
            div()
                .w(50.0)
                .h(50.0)
                .test_id("handle")
                .on_drag(|_| Box::new(ModsDrag(Mods::default()))),
        );
    let mut router = routed(root, 400.0, 400.0);
    let shift = Mods {
        shift: true,
        ..Mods::default()
    };
    let alt = Mods {
        alt: true,
        ..Mods::default()
    };

    router.set_modifiers(shift);
    let clicked = router.pointer_down(10.0, 10.0, &mut None);
    let clicked = dump(&router, clicked);
    router.pointer_up();
    router.pointer_down(60.0, 10.0, &mut None);
    let moved = router.pointer_move(100.0, 10.0);
    let moved = dump(&router, moved);
    router.set_modifiers(alt);
    let released = router.pointer_up();
    let released = dump(&router, released);

    assert_eq!(
        [clicked, moved, released],
        [
            "button [Click(\"shift\")]",
            "handle [Click(\"shift\")]",
            "handle [Click(\"alt\")]"
        ]
    );
}

// Regression: trackpad pixel deltas were rounded to lines one event at
// a time, so slow two-finger scrolling never moved anything.
#[test]
fn slow_trackpad_deltas_accumulate_into_lines() {
    let list = div()
        .w(200.0)
        .h(200.0)
        .test_id("list")
        .on_scroll(ScrollActionBuilder::new(|lines| {
            Msg::Scroll("list", lines).into()
        }));
    let mut router = routed(list, 200.0, 200.0);

    // 25 events of 2px are 50px: two whole 20px lines, in order.
    let mut delivered = Vec::new();
    for _ in 0..25 {
        let delivery = router.wheel(50.0, 50.0, 2.0);
        if delivery.node.is_some() {
            delivered.push(dump(&router, delivery));
        }
    }

    assert_eq!(
        delivered,
        [r#"list [Scroll("list", 1)]"#, r#"list [Scroll("list", 1)]"#]
    );
}

// Regression: a drag interrupted by the window losing focus never got a
// release, so it kept the pointer captured forever.
#[test]
fn cancel_pointer_cancels_the_drag_and_ends_capture() {
    let root = div().w(400.0).h(400.0).child(
        div()
            .w(50.0)
            .h(50.0)
            .test_id("handle")
            .on_drag(|_| Box::new(RecordDrag)),
    );
    let mut router = routed(root, 400.0, 400.0);

    router.pointer_down(10.0, 10.0, &mut None);
    let cancelled = router.cancel_pointer();
    let cancelled = dump(&router, cancelled);
    let moved = router.pointer_move(300.0, 300.0);
    let moved = dump(&router, moved);

    assert_eq!([cancelled, moved], ["handle [Cancel]", "-"]);
}

/// [`RecordDrag`] that agrees to move into a session carrying "tab".
struct TearDrag;

impl DragHandler for TearDrag {
    fn on_move(&mut self, x: f32, y: f32) -> Vec<Action> {
        RecordDrag.on_move(x, y)
    }

    fn on_release(&mut self) -> DragReleaseResult {
        RecordDrag.on_release()
    }

    fn on_cancel(&mut self) -> Vec<Action> {
        RecordDrag.on_cancel()
    }

    fn on_handoff(&mut self) -> Option<DragHandoff> {
        Some(DragHandoff {
            payload: Box::new("tab"),
            actions: vec![Msg::Click("handed off").into()],
        })
    }
}

fn drag_frame(start: fn(ClickEvent) -> Box<dyn DragHandler>) -> InputRouter {
    let root = div()
        .w(400.0)
        .h(400.0)
        .child(div().w(50.0).h(50.0).test_id("handle").on_drag(start));
    routed(root, 400.0, 400.0)
}

// A press that barely moves shows no preview; past the threshold the
// preview shows for the rest of the drag, back over the press too.
#[test]
fn a_drag_previews_only_once_past_the_threshold() {
    struct PreviewDrag(DragPreview);

    impl DragHandler for PreviewDrag {
        fn on_move(&mut self, _x: f32, _y: f32) -> Vec<Action> {
            Vec::new()
        }

        fn on_release(&mut self) -> DragReleaseResult {
            DragReleaseResult {
                actions: Vec::new(),
            }
        }

        fn on_cancel(&mut self) -> Vec<Action> {
            Vec::new()
        }

        fn preview(&self) -> Option<&DragPreview> {
            Some(&self.0)
        }
    }

    let mut router = drag_frame(|_| {
        let preview = DragPreview::new(0, (20.0, 20.0), |_, _| div().into_any());
        Box::new(PreviewDrag(preview))
    });
    router.pointer_down(10.0, 10.0, &mut None);

    let shown = [(12.0, 10.0), (20.0, 10.0), (10.0, 10.0)].map(|(x, y)| {
        router.pointer_move(x, y);
        router.drag_preview(router.frame()).is_some()
    });

    assert_eq!(shown, [false, true, true]);
}

// Catches a handed-off drag still driven by its source router: a blur
// of the source window cancelling a drag now over another window, or
// a release there committing a local drop. The session ends it, once.
#[test]
fn a_handed_off_drag_hears_only_from_its_session() {
    let mut router = drag_frame(|_| Box::new(TearDrag));
    router.pointer_down(10.0, 10.0, &mut None);
    router.pointer_move(30.0, 30.0);

    let (session, handed) = router
        .hand_off_capture(DragWindowId(1))
        .expect("handed off");
    let handed = dump(&router, handed);
    let after = [
        router.pointer_move(300.0, 300.0),
        router.cancel_pointer(),
        router.pointer_up(),
    ]
    .map(|delivery| dump(&router, delivery));
    let ended = session.cancel();

    assert_eq!(handed, r#"handle [Click("handed off")]"#);
    assert_eq!(after, ["-", "-", "-"]);
    assert_eq!(format!("{:?}", ended.actions), "[Cancel]");
    assert_eq!(ended.payload.downcast_ref::<&str>(), Some(&"tab"));
}

// Catches a handoff taking drags that never opted in (a scrollbar
// thumb, a text selection) away from their window.
#[test]
fn a_drag_that_refuses_handoff_keeps_the_pointer() {
    let mut router = drag_frame(|_| Box::new(RecordDrag));
    router.pointer_down(10.0, 10.0, &mut None);

    let refused = router.hand_off_capture(DragWindowId(1)).err();
    let moved = router.pointer_move(300.0, 300.0);

    assert_eq!(refused, Some(HandoffError::Refused));
    assert_eq!(dump(&router, moved), "handle [Move(300, 300)]");
}

// Regression: capture kept the pressed node's index, so once a new frame
// inserted a node before it, drag deliveries named some other node.
#[test]
fn capture_follows_its_node_into_a_reordered_frame() {
    let frame = |banner: bool| {
        let mut root = div().w(400.0).h(400.0).flex_col();
        if banner {
            root = root.child(button("banner", 400.0, 20.0));
        }
        root.child(
            div()
                .w(50.0)
                .h(50.0)
                .test_id("handle")
                .on_drag(|_| Box::new(RecordDrag)),
        )
    };
    let mut router = routed(frame(false), 400.0, 400.0);
    router.pointer_down(10.0, 10.0, &mut None);

    let next = routed(frame(true), 400.0, 400.0);
    router.set_frame(next.frame);
    let moved = router.pointer_move(300.0, 300.0);

    assert_eq!(dump(&router, moved), "handle [Move(300, 300)]");
}

// A tab strip: tabs close on a middle click, the strip only takes
// primary clicks.
#[test]
fn a_middle_click_needs_press_and_release_on_one_handler() {
    let tab = |name: &'static str| {
        div()
            .w(100.0)
            .h(40.0)
            .test_id(name)
            .on_click(Msg::Click(name))
            .on_middle_click(Msg::Click("close"))
    };
    let strip = div()
        .w(300.0)
        .h(40.0)
        .flex_row()
        .test_id("strip")
        .on_click(Msg::Click("strip"))
        .child(tab("a"))
        .child(tab("b"));
    let mut router = routed(strip, 300.0, 40.0);
    // (press x, release x, delivery)
    let cases = [
        (50.0, 60.0, r#"a [Click("close")]"#),
        (50.0, 150.0, "-"),
        (250.0, 250.0, "-"),
    ];
    for (down, up, expected) in cases {
        router.middle_down(down, 20.0);
        let delivery = router.middle_up(up, 20.0);
        assert_eq!(dump(&router, delivery), expected, "{down} -> {up}");
    }
}

// Regression: `.on_key("mod+s")` was compared as text against the
// pressed "ctrl+s", and keys with nothing focused went nowhere.
#[test]
fn mod_binding_fires_for_cmd_or_ctrl_with_nothing_focused() {
    let root = div()
        .w(100.0)
        .h(100.0)
        .test_id("root")
        .on_key("mod+s", Msg::Click("save"));
    let router = routed(root, 100.0, 100.0);

    let cases = [
        ("ctrl+s", r#"root [Click("save")]"#),
        ("cmd+s", r#"root [Click("save")]"#),
        ("s", "-"),
        ("ctrl+shift+s", "-"),
    ];
    for (pressed, expected) in cases {
        let pressed: Binding = pressed.parse().unwrap();
        let delivery = router.key_down(&pressed, None);
        assert_eq!(dump(&router, delivery), expected, "{pressed}");
    }
}

#[test]
fn binding_strings_parse_to_canonical_form() {
    let cases = [
        ("mod+s", Some("mod+s")),
        ("Shift+Ctrl+P", Some("ctrl+shift+p")),
        ("cmd+alt+esc", Some("cmd+alt+escape")),
        ("ctrl++", Some("ctrl++")),
        ("option+up", Some("alt+arrowup")),
        ("hyper+s", None),
        ("ctrl+", None),
        ("g g", None),
    ];
    for (text, expected) in cases {
        let parsed = text.parse::<Binding>().ok().map(|b| b.to_string());
        assert_eq!(parsed.as_deref(), expected, "{text}");
    }
}

// Regression: any press outside a text field cleared focus, so clicking
// Send blurred the composer it was sending from.
#[test]
fn press_moves_focus_only_to_focusable_nodes() {
    let field = FocusId::from_key("field");
    let root = div()
        .w(400.0)
        .h(300.0)
        .flex_col()
        .child(
            text_input("Message", "")
                .focus_target(field)
                .w(200.0)
                .h(40.0),
        )
        .child(button("send", 80.0, 30.0))
        .child(focusable("toggle").on_click(Msg::Click("toggle")))
        .child(div().w(400.0).h(100.0));
    let mut router = routed(root, 400.0, 300.0);

    // (press, focus after): the field is focused before each press.
    let cases = [
        ("send button", (10.0, 50.0), Some(field)),
        ("focusable", (10.0, 80.0), Some(FocusId::from_key("toggle"))),
        ("background", (10.0, 200.0), None),
    ];
    for (name, (x, y), expected) in cases {
        let mut focus = Some(field);
        router.pointer_down(x, y, &mut focus);
        router.pointer_up();
        assert_eq!(focus, expected, "{name}");
    }
}

mod streaming {
    use std::collections::HashMap;

    use quark::selection::BlockKey;

    use super::*;
    use crate::document::{
        Block, Document, DocumentEvent, DocumentRow, DocumentStyle, RowChrome, TextMeasurer,
    };
    use crate::virtual_list::RowKey;

    #[derive(Debug, Clone, PartialEq)]
    struct Ev(DocumentEvent);

    impl From<Ev> for Action {
        fn from(ev: Ev) -> Self {
            Action::new(ev)
        }
    }

    type Messages = HashMap<RowKey, DocumentRow>;

    fn message(i: u64, text: &str) -> DocumentRow {
        DocumentRow {
            key: RowKey(i),
            chrome: RowChrome {
                header_height: 22.0,
                label: Some(format!("author {i}").into()),
                kind: 0,
                ..RowChrome::default()
            },
            blocks: vec![Block::plain(BlockKey(i), text)],
            adornments: Vec::new(),
        }
    }

    /// A 400x300 document stuck to the bottom, with shared text state
    /// so each frame is painted the way the adapter paints it.
    struct Chat {
        messages: Messages,
        document: Document,
        text: TextSystem,
        layouts: LayoutCache,
        router: InputRouter,
    }

    impl Chat {
        fn new(n: u64) -> Self {
            let messages: Messages = (0..n)
                .map(|i| (RowKey(i), message(i, &format!("Message {i} text."))))
                .collect();
            let mut document = Document::new(DocumentStyle::for_font_size(14.0));
            document
                .extend((0..n).map(|i| &messages[&RowKey(i)]))
                .unwrap();
            let mut chat = Self {
                messages,
                document,
                text: TextSystem::vendored_only(&Default::default()),
                layouts: LayoutCache::default(),
                router: InputRouter::default(),
            };
            chat.frame();
            chat.document.scroll_to_bottom();
            chat.frame();
            chat
        }

        /// Prepare and paint a frame; route input through it from now on.
        fn frame(&mut self) {
            let (w, h) = (400.0, 300.0);
            self.document.prepare(
                w,
                h,
                0,
                false,
                &self.messages,
                &mut TextMeasurer::new(&mut self.text, &mut self.layouts, 14.0, 1.0),
            );
            let theme = Theme::default_dark();
            let signals = SignalStore::new();
            let mut root = self
                .document
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
                    self.document.handle(*event);
                }
            }
        }

        fn block_rect(&self, key: u64) -> Rect {
            self.document
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
            self.document.update(&message).unwrap();
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
            chat.document.selected_text(&chat.messages),
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

// Keyboard parity: a focused button that binds no keys was unreachable
// by keyboard; Enter or Space now clicks it, and only it.
#[test]
fn enter_and_space_click_the_focused_node_only() {
    let root = div()
        .w(200.0)
        .h(100.0)
        .test_id("card")
        .on_click(Msg::Click("card"))
        .child(button("save", 50.0, 20.0))
        .child(
            button("off", 50.0, 20.0)
                .accessibility_role(accesskit::Role::Button)
                .accessibility_disabled(true),
        )
        .child(
            div()
                .w(50.0)
                .h(20.0)
                .test_id("label")
                .focus_ring(FocusId::from_key("label")),
        );
    let router = routed(root, 200.0, 100.0);

    let save = Some(FocusId::from_key("save"));
    let cases = [
        ("enter", save, r#"save [Click("save")]"#),
        ("space", save, r#"save [Click("save")]"#),
        ("shift+enter", save, "-"),
        ("a", save, "-"),
        ("enter", Some(FocusId::from_key("off")), "-"),
        // Focus inside the card on a node without a click handler.
        ("enter", Some(FocusId::from_key("label")), "-"),
        ("enter", None, "-"),
    ];
    for (pressed, focus, expected) in cases {
        let pressed: Binding = pressed.parse().unwrap();
        let delivery = router.activate(&pressed, focus);
        assert_eq!(dump(&router, delivery), expected, "{pressed} on {focus:?}");
    }
}

// A custom title bar: blank parts move the window; its controls, a
// text field, a drag handle, and an excluded strip keep their presses.
// A tab strip too narrow for its tabs clips the last one away, leaving
// the bar beneath it draggable there.
#[test]
fn title_bar_press_moves_the_window_only_on_blank_chrome() {
    let bar = div()
        .w(400.0)
        .h(40.0)
        .flex_row()
        .window_drag_region()
        .child(button("close", 40.0, 40.0))
        .child(
            text_input("", "")
                .bare()
                .focus_target(FocusId::new(7))
                .w(80.0)
                .h(40.0),
        )
        .child(div().w(40.0).h(40.0).on_drag(|_| Box::new(RecordDrag)))
        .child(div().w(40.0).h(40.0).window_drag_exclude())
        .child(
            div()
                .w(80.0)
                .h(40.0)
                .flex_row()
                .flex_shrink_0()
                .overflow_hidden()
                .child(button("tab", 40.0, 40.0).flex_shrink_0())
                .child(div().w(40.0).h(40.0).flex_shrink_0())
                .child(button("clipped tab", 40.0, 40.0).flex_shrink_0()),
        );
    let mut router = routed(div().w(400.0).h(100.0).child(bar), 400.0, 100.0);
    let cases = [
        ("blank chrome", 360.0, 20.0, true),
        ("close button", 20.0, 20.0, false),
        ("text field", 80.0, 20.0, false),
        ("drag handle", 140.0, 20.0, false),
        ("excluded strip", 180.0, 20.0, false),
        ("tab in the strip", 210.0, 20.0, false),
        ("tab clipped off the strip", 300.0, 20.0, true),
        ("below the bar", 360.0, 70.0, false),
    ];
    for (name, x, y, drags) in cases {
        let delivery = router.pointer_down(x, y, &mut None);
        router.pointer_up();
        assert_eq!(delivery.window_drag, drags, "{name}");
    }
}
