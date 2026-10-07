//! Components emit the app actions their caller supplies.

use quark::StyleState;
use quark::reactive::SignalStore;
use quark_components::{Modal, PickerItem, Toast, ToastKind, ToastStack, picker_list};
use quark_render::Scene;
use quark_ui::Action;
use quark_ui::animation::AnimationTable;
use quark_ui::element::{
    AnyElement, ElementContext, InputRouter, IntoAnyElement, ScrollActionBuilder, render_element,
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
