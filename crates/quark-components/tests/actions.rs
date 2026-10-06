//! Components emit the app actions their caller supplies.

use quark::reactive::SignalStore;
use quark_components::{Modal, PickerItem, Toast, ToastKind, ToastStack, picker_list};
use quark_render::Scene;
use quark_ui::Action;
use quark_ui::animation::AnimationState;
use quark_ui::element::{
    AnyElement, ClickHandlerActionExt, ElementContext, IntoAnyElement, ScrollActionBuilder,
    render_element,
};
use quark_ui::icons::lucide;
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

fn hit_actions(mut root: AnyElement) -> (Vec<Action>, Vec<Action>) {
    let mut font_system = glyphon::FontSystem::new();
    quark_render::fonts::configure_font_system(&mut font_system);
    let mut store = SignalStore::new();
    let theme = Box::leak(Box::new(Theme::default_dark()));
    let mut cx = ElementContext::new(theme, 1.0, &mut font_system, None, &mut store);
    let mut scene = Scene::default();
    render_element(&mut root, &mut scene, &mut cx, 800.0, 600.0);
    let clicks = cx
        .hits
        .iter()
        .flat_map(|hit| hit.on_click.peek_actions())
        .collect();
    let scrolls = cx
        .scroll_regions
        .iter()
        .map(|region| region.action_builder.build(2))
        .collect();
    (clicks, scrolls)
}

#[test]
fn modal_backdrop_emits_on_dismiss() {
    let modal = Modal::new(
        "Title",
        "",
        lucide::SETTINGS,
        360.0,
        800.0,
        600.0,
        Demo::Dismiss,
    );
    let (clicks, _) = hit_actions(modal.into_any());
    assert!(clicks.contains(&Demo::Dismiss.into()));
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
    }];
    let animation = AnimationState::default();
    let stack = ToastStack::new(&toasts, &animation, 800.0, 600.0, 1.0, 0.0, 0, &[], |i| {
        Demo::DismissToast(i).into()
    });
    let (clicks, _) = hit_actions(stack.build().into_any());
    assert!(clicks.contains(&Demo::DismissToast(0).into()));
}
