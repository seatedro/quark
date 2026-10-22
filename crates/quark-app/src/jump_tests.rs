//! Jump mode: labels on the actionable elements on screen, typed to click
//! or focus one.

use quark_ui::FocusId;
use quark_ui::element::{AnyElement, IntoAnyElement, div, text_input};
use quark_ui::style::Styled;
use quark_ui::text_input::{TextEditCommand, TextEditOutcome, TextField};

use crate::testing::UiTestHarness;
use crate::ui::{UiApp, UiContext, ViewContext};

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Jump,
    Click(&'static str),
}

impl From<Msg> for quark_ui::Action {
    fn from(msg: Msg) -> Self {
        quark_ui::Action::new(msg)
    }
}

const FIELD: FocusId = FocusId::from_key("field");

#[derive(Default)]
enum Layout {
    #[default]
    Page,
    Dialog,
    OffScreen,
    /// Ten buttons, five to a row.
    Grid,
}

const GRID: [&str; 10] = ["b0", "b1", "b2", "b3", "b4", "b5", "b6", "b7", "b8", "b9"];

/// `f` starts jump mode, `x` logs. The page paints save before open, and
/// a disabled button and a plain box that get no label.
#[derive(Default)]
struct Page {
    layout: Layout,
    field: TextField,
    log: Vec<&'static str>,
    started: Option<(usize, bool)>,
}

fn button(name: &'static str, x: f32, y: f32) -> quark_ui::element::Div {
    div()
        .id(name)
        .test_id(name)
        .absolute()
        .left(x)
        .top(y)
        .w(80.0)
        .h(30.0)
        .on_click(Msg::Click(name))
}

impl UiApp for Page {
    type Action = Msg;
    type Message = ();

    fn view(&mut self, cx: &mut ViewContext) -> AnyElement {
        let root = div()
            .size_full()
            .on_key("f", Msg::Jump)
            .on_key("x", Msg::Click("x"));
        let root = match self.layout {
            Layout::Page => root
                .child(button("save", 200.0, 0.0))
                .child(button("open", 0.0, 0.0))
                .child(button("close", 0.0, 50.0))
                .child(button("off", 100.0, 50.0).accessibility_disabled(true))
                .child(div().absolute().left(200.0).top(50.0).w(80.0).h(30.0))
                .child(
                    div().absolute().left(0.0).top(100.0).child(
                        text_input("Name", "")
                            .field(&self.field)
                            .focus_target(FIELD)
                            .focused(cx.is_focused(FIELD))
                            .w(200.0)
                            .h(40.0),
                    ),
                ),
            Layout::Dialog => root.child(button("save", 0.0, 0.0)).child(
                div()
                    .absolute()
                    .left(0.0)
                    .top(0.0)
                    .size_full()
                    .focus_scope("dialog")
                    .trap_focus(true)
                    .child(button("ok", 100.0, 200.0)),
            ),
            Layout::OffScreen => root.child(button("below", 0.0, 400.0)),
            Layout::Grid => root.children(GRID.iter().enumerate().map(|(i, name)| {
                button(name, (i % 5) as f32 * 80.0, (i / 5) as f32 * 40.0).into_any()
            })),
        };
        root.into_any()
    }

    fn update(&mut self, msg: Msg, cx: &mut UiContext) {
        match msg {
            Msg::Jump => {
                let count = cx.start_jump_hints();
                self.started = Some((count, cx.jump_hints_active()));
            }
            Msg::Click(name) => self.log.push(name),
        }
    }

    fn edit_text(&mut self, _target: FocusId, command: TextEditCommand) -> TextEditOutcome {
        self.field.apply(command)
    }
}

fn harness(layout: Layout) -> UiTestHarness<Page> {
    let page = Page {
        layout,
        ..Page::default()
    };
    let mut ui = UiTestHarness::new(page, (400.0, 300.0), 1.0);
    ui.frame();
    ui
}

/// Each painted label with the element under its chip's top left corner,
/// named by test id or accessible name.
fn labels(ui: &UiTestHarness<Page>) -> Vec<(String, String)> {
    ui.painted_texts()
        .into_iter()
        .filter(|t| {
            (1..=2).contains(&t.text.len()) && t.text.chars().all(|c| "asdfghjkl".contains(c))
        })
        .filter_map(|t| {
            let node = ui.hit_test((t.bounds.x, t.bounds.y + 1.0))?;
            Some((t.text, node.test_id.or(node.name)?))
        })
        .collect()
}

fn pairs(expected: &[(&str, &str)]) -> Vec<(String, String)> {
    expected
        .iter()
        .map(|(label, node)| (label.to_string(), node.to_string()))
        .collect()
}

#[test]
fn every_visible_enabled_actionable_element_gets_a_label_in_reading_order() {
    let mut ui = harness(Layout::Page);
    ui.key("f");
    assert_eq!(ui.app().started, Some((4, true)));
    assert_eq!(
        labels(&ui),
        pairs(&[("a", "open"), ("s", "save"), ("d", "close"), ("f", "Name")])
    );
}

#[test]
fn typing_a_label_focuses_and_clicks_its_element_and_ends_the_mode() {
    let mut ui = harness(Layout::Page);
    ui.key("f");
    ui.key("s");
    assert_eq!(ui.app().log, ["save"]);
    assert_eq!(ui.focus(), Some(FocusId::from_key("save")));
    assert!(labels(&ui).is_empty());
    ui.key("x");
    assert_eq!(ui.app().log, ["save", "x"]);
}

#[test]
fn shift_with_a_label_focuses_without_clicking() {
    let mut ui = harness(Layout::Page);
    ui.key("f");
    ui.key("shift+d");
    assert!(ui.app().log.is_empty());
    assert_eq!(ui.focus(), Some(FocusId::from_key("close")));
}

#[test]
fn a_text_fields_label_focuses_it_without_typing_the_key() {
    let mut ui = harness(Layout::Page);
    ui.key("f");
    ui.key("f");
    assert_eq!(ui.focus(), Some(FIELD));
    assert_eq!(ui.app().field.text(), "");
    ui.type_text("ok");
    assert_eq!(ui.app().field.text(), "ok");
}

#[test]
fn escape_and_other_keys_end_the_mode_without_acting() {
    for key in ["escape", "x", "mod+s"] {
        let mut ui = harness(Layout::Page);
        ui.key("f");
        ui.key(key);
        assert!(ui.app().log.is_empty(), "{key}");
        assert_eq!(ui.focus(), None, "{key}");
        assert!(labels(&ui).is_empty(), "{key}");
        ui.key("s");
        assert!(ui.app().log.is_empty(), "{key}");
    }
}

#[test]
fn an_open_modal_scope_limits_the_labels_to_its_elements() {
    let mut ui = harness(Layout::Dialog);
    ui.key("f");
    assert_eq!(labels(&ui), pairs(&[("a", "ok")]));
    ui.key("a");
    assert_eq!(ui.app().log, ["ok"]);
}

#[test]
fn with_nothing_on_screen_to_label_the_mode_never_starts() {
    let mut ui = harness(Layout::OffScreen);
    ui.key("f");
    assert_eq!(ui.app().started, Some((0, false)));
    ui.key("x");
    assert_eq!(ui.app().log, ["x"]);
}

#[test]
fn past_nine_elements_labels_take_two_keys_that_narrow_as_typed() {
    let mut ui = harness(Layout::Grid);
    ui.key("f");
    assert_eq!(ui.app().started, Some((10, true)));
    for key in ["a", "backspace", "s"] {
        ui.key(key);
    }
    assert_eq!(labels(&ui), pairs(&[("s", "b9"), ("a", "b9")]));
    assert!(ui.app().log.is_empty());
    ui.key("a");
    assert_eq!(ui.app().log, ["b9"]);
}
