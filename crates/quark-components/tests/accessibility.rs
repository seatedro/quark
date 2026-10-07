//! Components publish the roles and states screen readers expect, and
//! everything a pointer can press is reachable from the keyboard.

use quark::StyleState;
use quark::reactive::SignalStore;
use quark_components::{
    DropdownItem, SegmentedControl, SegmentedItem, TabItem, Toast, ToastKind, ToastStack, checkbox,
    dropdown, progress_bar, tab_bar, toggle,
};
use quark_render::Scene;
use quark_ui::accessibility::dump_accessibility_states;
use quark_ui::animation::AnimationTable;
use quark_ui::element::{AnyElement, ElementContext, IntoAnyElement, div, render_element};
use quark_ui::style::Styled;
use quark_ui::theme::Theme;
use quark_ui::{Action, FocusId};

#[derive(Debug, Clone, PartialEq)]
struct Pick(&'static str);

impl From<Pick> for Action {
    fn from(value: Pick) -> Self {
        Action::new(value)
    }
}

/// Paint `root` at 800x600 and return the frame's semantic and
/// accessibility output.
fn paint(root: AnyElement) -> ElementOutput {
    paint_focused(root, None)
}

fn paint_focused(root: AnyElement, focus: Option<FocusId>) -> ElementOutput {
    let mut text = quark_text::TextSystem::vendored_only(&Default::default());
    let mut layouts = quark_text::LayoutCache::default();
    let store = SignalStore::new();
    let theme = Theme::default_dark();
    let mut cx =
        ElementContext::new(&theme, 1.0, &mut text, &mut layouts, None, &store).with_focus(focus);
    cx.accessibility = quark_ui::accessibility::AccessibilityFrame::new(800.0, 600.0);
    let mut root = root;
    render_element(&mut root, &mut Scene::default(), &mut cx, 800.0, 600.0);
    ElementOutput {
        states: dump_accessibility_states(&cx.accessibility.tree_update("Test", None)),
        semantic: std::mem::take(&mut cx.semantic),
    }
}

struct ElementOutput {
    states: String,
    semantic: quark::SemanticFrame,
}

/// `role | name | states` per node, without author ids (they embed action
/// debug output).
fn states(output: &ElementOutput) -> String {
    output
        .states
        .lines()
        .filter_map(|line| line.split_once(" | ").map(|(_, rest)| format!("{rest}\n")))
        .collect()
}

fn gallery() -> AnyElement {
    let toasts = [Toast {
        id: 7,
        kind: ToastKind::Error,
        message: "Upload failed".into(),
        description: Some("The server is offline".into()),
        created_at_ms: 0,
        hovered: false,
        progress: None,
    }];
    let animation = AnimationTable::new();
    let toast_stack = ToastStack::new(&toasts, &animation, 800.0, 600.0, 1.0, 0.0, 0, &[], |_| {
        Pick("dismiss").into()
    })
    .build();
    div()
        .w(800.0)
        .h(600.0)
        .flex_col()
        .child(
            checkbox(true)
                .label("Remember me")
                .on_toggle(Pick("remember")),
        )
        .child(
            toggle(false)
                .label("Wi-Fi")
                .on_toggle(Pick("wifi"))
                .disabled(true),
        )
        .child(SegmentedControl::new(vec![
            SegmentedItem::new("Day", Pick("day"), true),
            SegmentedItem::new("Week", Pick("week"), false),
        ]))
        .child(tab_bar(vec![
            TabItem::new("Files", Pick("files")).active(true),
            TabItem::new("Logs", Pick("logs")),
        ]))
        .child(
            dropdown(
                "Sort",
                vec![DropdownItem::new("Newest", Pick("newest")).selected(true)],
            )
            .open(true)
            .on_toggle(Pick("sort")),
        )
        .child(progress_bar(0.42))
        .child(toast_stack)
        .into_any()
}

#[test]
fn components_publish_their_roles_and_states() {
    assert_eq!(
        states(&paint(gallery())),
        "CheckBox | Remember me | checked\n\
         Switch | Wi-Fi | disabled | unchecked\n\
         RadioGroup | -\n\
         RadioButton | Day | selected | checked\n\
         RadioButton | Week | unselected | unchecked\n\
         TabList | -\n\
         Tab | Files | selected\n\
         Tab | Logs | unselected\n\
         ComboBox | Sort | expanded\n\
         Menu | -\n\
         MenuItem | Newest | selected\n\
         ProgressIndicator | - | range=42/0..100\n\
         Alert | Upload failed | desc=\"The server is offline\" | live=assertive\n\
         Label | Upload failed\n\
         Label | The server is offline\n\
         Button | Dismiss\n"
    );
}

// Keyboard parity: every node a pointer can click has a focus target, so
// Tab reaches it and Enter or Space presses it.
#[test]
fn every_clickable_component_is_in_the_tab_order() {
    let output = paint(gallery());
    let frame = &output.semantic;
    let unreachable: Vec<String> = (0..frame.nodes().len())
        .filter(|&i| frame.nodes()[i].actions.click && !frame.nodes()[i].state.disabled)
        .filter(|&i| frame.focus_id(i).is_none())
        .map(|i| {
            let node = &frame.nodes()[i];
            format!("{:?} {:?}", node.role, node.label)
        })
        .collect();
    assert_eq!(unreachable, Vec::<String>::new());
}

// Keyboard focus is visible: the focused checkbox paints its focus ring
// (FOCUS_VISIBLE), and nothing else does.
#[test]
fn keyboard_focus_rings_the_focused_component() {
    let frame = paint(gallery()).semantic;
    let checkbox = (0..frame.nodes().len())
        .find(|&i| frame.nodes()[i].label.as_deref() == Some("Remember me"))
        .and_then(|i| frame.focus_id(i))
        .expect("checkbox focus target");

    let focused = paint_focused(gallery(), Some(checkbox)).semantic;
    let ringed: Vec<_> = focused
        .nodes()
        .iter()
        .filter(|node| node.state.style_state.contains(StyleState::FOCUS_VISIBLE))
        .map(|node| node.label.clone())
        .collect();
    assert_eq!(ringed, [Some("Remember me".to_owned())]);
}
