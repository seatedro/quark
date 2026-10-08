//! Components emit the app actions their caller supplies.

use quark::StyleState;
use quark::reactive::SignalStore;
use quark_components::{
    Modal, PickerItem, TabItem, Toast, ToastKind, ToastStack, picker_list, tab_bar,
};
use quark_render::Scene;
use quark_ui::Action;
use quark_ui::animation::AnimationTable;
use quark_ui::element::{
    AnyElement, Binding, ElementContext, InputRouter, IntoAnyElement, ScrollActionBuilder,
    render_element,
};
use quark_ui::icons::lucide;
use quark_ui::style::Styled;
use quark_ui::theme::Theme;

#[derive(Debug, Clone, PartialEq)]
enum Demo {
    Dismiss,
    Select(usize),
    Scroll(i32),
    DismissToast(usize),
    Close(&'static str),
}

impl From<Demo> for Action {
    fn from(value: Demo) -> Self {
        Action::new(value)
    }
}

struct Item(&'static str);

impl PickerItem for Item {
    fn label(&self) -> &str {
        self.0
    }
    fn detail(&self) -> Option<&str> {
        None
    }
}

/// Paint `root` at 800x600 with the pointer at `pointer`; return the router
/// for the frame and the test ids of nodes painted hovered.
fn paint(mut root: AnyElement, pointer: Option<(f32, f32)>) -> (InputRouter, Vec<String>) {
    let mut text = quark_text::TextSystem::vendored_only(&Default::default());
    let mut layouts = quark_text::LayoutCache::default();
    let store = SignalStore::new();
    let theme = Theme::default_dark();
    let mut cx = ElementContext::new(&theme, 1.0, &mut text, &mut layouts, pointer, &store);
    render_element(&mut root, &mut Scene::default(), &mut cx, 800.0, 600.0);
    let hovered = cx
        .semantic
        .nodes()
        .iter()
        .filter(|node| node.state.style_state.contains(StyleState::HOVER))
        .filter_map(|node| node.test_id.as_ref().map(|id| id.as_str().to_owned()))
        .collect();
    let mut router = InputRouter::default();
    router.set_frame(cx.take_input_frame());
    (router, hovered)
}

/// Actions from clicking the center of every clickable node, and from a
/// two-line wheel over every scrollable node.
fn hit_actions(root: AnyElement) -> (Vec<Action>, Vec<Action>) {
    let (mut router, _) = paint(root, None);
    let centers = |want: fn(&quark::SemanticActions) -> bool| -> Vec<(f32, f32)> {
        router
            .frame()
            .semantic
            .nodes()
            .iter()
            .filter(|node| want(&node.actions))
            .map(|node| {
                let b = node.bounds;
                (b.x + b.width / 2.0, b.y + b.height / 2.0)
            })
            .collect()
    };
    let click_points = centers(|actions| actions.click);
    let scroll_points = centers(|actions| actions.scroll);
    let clicks = click_points
        .into_iter()
        .flat_map(|(x, y)| router.pointer_down(x, y, &mut None).actions)
        .collect();
    let scrolls = scroll_points
        .into_iter()
        .flat_map(|(x, y)| {
            router
                .wheel(x, y, 2.0 * quark_ui::element::WHEEL_LINE_PX)
                .actions
        })
        .collect();
    (clicks, scrolls)
}

#[test]
fn modal_scrim_blocks_hover_and_click_beneath() {
    let under = quark_ui::element::div()
        .w(100.0)
        .h(100.0)
        .test_id("under")
        .hover_bg(quark_ui::theme::Color::rgba(255, 0, 0, 255))
        .on_click(Demo::Select(0));
    let modal = Modal::new(
        "Title",
        "",
        lucide::SETTINGS,
        360.0,
        800.0,
        600.0,
        Demo::Dismiss,
    );
    let root = quark_ui::element::div()
        .w(800.0)
        .h(600.0)
        .child(under)
        .child(modal.into_any());

    let (mut router, hovered) = paint(root.into_any(), Some((50.0, 50.0)));
    let clicked = router.pointer_down(50.0, 50.0, &mut None).actions;

    assert_eq!(hovered, ["modal-backdrop"]);
    assert_eq!(clicked, [Action::from(Demo::Dismiss)]);
}

#[test]
fn picker_rows_emit_select_and_scroll() {
    let theme = Theme::default_dark();
    let items = [Item("one"), Item("two")];
    let list = picker_list(
        &items,
        0,
        0.0,
        8,
        &theme,
        |i| Demo::Select(i).into(),
        ScrollActionBuilder::new(|d| Demo::Scroll(d).into()),
    );
    let (clicks, scrolls) = hit_actions(list);
    assert!(clicks.contains(&Demo::Select(0).into()));
    assert!(clicks.contains(&Demo::Select(1).into()));
    assert_eq!(scrolls, vec![Action::from(Demo::Scroll(2))]);
}

#[test]
fn toast_emits_on_dismiss_with_index() {
    let toasts = [Toast {
        id: 1,
        kind: ToastKind::Info,
        message: "Saved".into(),
        description: None,
        created_at_ms: 0,
        hovered: false,
        progress: None,
        ..Default::default()
    }];
    let animation = AnimationTable::new();
    let stack = ToastStack::new(&toasts, &animation, 800.0, 600.0, 1.0, 0.0, 0, &[], |i| {
        Demo::DismissToast(i).into()
    });
    let (clicks, _) = hit_actions(stack.build().into_any());
    assert!(clicks.contains(&Demo::DismissToast(0).into()));
}

/// Files (active) and Logs, both closable.
fn closable_tabs() -> InputRouter {
    let tab = |id: &'static str, label: &'static str| {
        TabItem::new(label, Demo::Select(0))
            .id(id)
            .on_close(Demo::Close(id))
    };
    let bar = tab_bar(vec![
        tab("files", "Files").active(true),
        tab("logs", "Logs"),
    ]);
    paint(bar.into_any(), None).0
}

/// The center of the node with this test id and label.
fn center_of(router: &InputRouter, test_id: &str, label: &str) -> (f32, f32) {
    let node = router
        .frame()
        .semantic
        .nodes()
        .iter()
        .find(|n| {
            n.test_id.as_ref().map(|t| t.as_str()) == Some(test_id)
                && n.label.as_deref() == Some(label)
        })
        .expect("node is painted");
    let b = node.bounds;
    (b.x + b.width / 2.0, b.y + b.height / 2.0)
}

#[test]
fn a_tab_close_button_closes_without_selecting() {
    let mut router = closable_tabs();
    let (x, y) = center_of(&router, "tab-close", "Close Logs");
    let actions = router.pointer_down(x, y, &mut None).actions;
    assert_eq!(actions, [Action::from(Demo::Close("logs"))]);
}

#[test]
fn a_middle_click_closes_an_inactive_tab_without_selecting_it() {
    let mut router = closable_tabs();
    let tab = router
        .frame()
        .semantic
        .nodes()
        .iter()
        .find(|n| n.label.as_deref() == Some("Logs"))
        .expect("Logs tab")
        .bounds;
    // Over the label, away from the close button.
    let (x, y) = (tab.x + 12.0, tab.y + tab.height / 2.0);
    router.middle_down(x, y);
    assert_eq!(
        router.middle_up(x, y).actions,
        [Action::from(Demo::Close("logs"))]
    );
}

#[test]
fn delete_closes_the_focused_tab() {
    let router = closable_tabs();
    let delete: Binding = "delete".parse().unwrap();
    let focus = Some(TabItem::focus_id("files"));
    assert_eq!(
        router.key_down(&delete, focus).actions,
        [Action::from(Demo::Close("files"))]
    );
}
