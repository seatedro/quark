//! Context menu requests: a secondary click at the pointer, and Shift+F10
//! or the Menu key on the focused element, routed to `on_context_menu`.

use quark_ui::element::{AnyElement, ContextMenuEvent, IntoAnyElement, div};
use quark_ui::style::Styled;

use crate::testing::UiTestHarness;
use crate::ui::{UiApp, UiContext, ViewContext};

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Select(&'static str),
    Menu(&'static str, ContextMenuEvent),
}

impl From<Msg> for quark_ui::Action {
    fn from(msg: Msg) -> Self {
        quark_ui::Action::new(msg)
    }
}

/// Two 200x30 rows stacked from the top left; row b holds an icon with no
/// handlers of its own.
struct Sidebar {
    log: Vec<Msg>,
}

impl UiApp for Sidebar {
    type Action = Msg;
    type Message = ();

    fn view(&mut self, _cx: &mut ViewContext) -> AnyElement {
        let row = |name: &'static str| {
            div()
                .id(name)
                .w(200.0)
                .h(30.0)
                .on_click(Msg::Select(name))
                .on_context_menu(move |at| Msg::Menu(name, at).into())
        };
        div()
            .size_full()
            .flex_col()
            .child(row("a"))
            .child(row("b").child(div().w(20.0).h(20.0)))
            .into_any()
    }

    fn update(&mut self, msg: Msg, _cx: &mut UiContext) {
        self.log.push(msg);
    }
}

fn harness() -> UiTestHarness<Sidebar> {
    let mut ui = UiTestHarness::new(Sidebar { log: Vec::new() }, (400.0, 300.0), 1.0);
    ui.frame();
    ui
}

#[test]
fn a_secondary_click_bubbles_to_the_row_menu_at_the_pointer_without_clicking() {
    let mut ui = harness();
    ui.right_click((10.0, 40.0));
    let at = ContextMenuEvent {
        x: 10.0,
        y: 40.0,
        keyboard: false,
    };
    assert_eq!(ui.app().log, [Msg::Menu("b", at)]);
}

#[test]
fn shift_f10_and_the_menu_key_open_the_focused_rows_menu_at_its_corner() {
    let mut ui = harness();
    for key in [
        "tab",
        "tab",
        "shift+f10",
        "contextmenu",
        "f10",
        "ctrl+contextmenu",
    ] {
        ui.key(key);
    }
    let at = ContextMenuEvent {
        x: 0.0,
        y: 60.0,
        keyboard: true,
    };
    assert_eq!(ui.app().log, [Msg::Menu("b", at), Msg::Menu("b", at)]);
}
