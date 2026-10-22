//! Key bindings with context predicates, resolved along the focus path by
//! the adapter, against elements' own `on_key` handlers.

use quark_ui::FocusId;
use quark_ui::element::{AnyElement, IntoAnyElement, div, text_input};
use quark_ui::key_context::KeyBindings;
use quark_ui::style::Styled;
use quark_ui::text_input::{TextEditCommand, TextEditOutcome, TextField};

use crate::testing::UiTestHarness;
use crate::ui::{UiAdapter, UiApp, UiContext, ViewContext};

#[derive(Debug, Clone, PartialEq)]
struct Cmd(&'static str);

impl From<Cmd> for quark_ui::Action {
    fn from(cmd: Cmd) -> Self {
        quark_ui::Action::new(cmd)
    }
}

/// workspace > pane > editor, beside a sidebar; the workspace element
/// handles `mod+p` itself.
struct Workspace {
    mode: &'static str,
    log: Vec<&'static str>,
}

impl UiApp for Workspace {
    type Action = Cmd;
    type Message = ();

    fn view(&mut self, _cx: &mut ViewContext) -> AnyElement {
        div()
            .size_full()
            .flex_row()
            .key_context("workspace")
            .on_key("mod+p", Cmd("element palette"))
            .child(
                div().key_context("pane").child(
                    div()
                        .w(100.0)
                        .h(100.0)
                        .key_context(format!("editor mode={}", self.mode))
                        .focus_ring(FocusId::from_key("editor")),
                ),
            )
            .child(
                div()
                    .w(100.0)
                    .h(100.0)
                    .key_context("sidebar")
                    .focus_ring(FocusId::from_key("sidebar")),
            )
            .into_any()
    }

    fn update(&mut self, Cmd(name): Cmd, _cx: &mut UiContext) {
        self.log.push(name);
    }
}

fn bindings() -> KeyBindings {
    let mut keys = KeyBindings::new();
    for (key, predicate, name) in [
        ("mod+p", None, "global palette"),
        ("mod+p", Some("editor"), "editor palette"),
        ("escape", Some("editor && mode == insert"), "normal mode"),
        ("i", Some("editor && mode != insert"), "insert mode"),
        ("mod+w", Some("pane > editor"), "close editor"),
        ("mod+s", None, "save"),
    ] {
        keys.bind(key, predicate, Cmd(name)).unwrap();
    }
    keys
}

#[test]
fn bindings_resolve_by_context_predicate_and_depth() {
    let cases = [
        ("editor", "insert", "escape", "normal mode"),
        ("editor", "normal", "escape", "-"),
        ("editor", "normal", "i", "insert mode"),
        // The editor's context is inside the workspace's own handler.
        ("editor", "insert", "ctrl+p", "editor palette"),
        ("sidebar", "insert", "ctrl+p", "element palette"),
        ("editor", "normal", "ctrl+w", "close editor"),
        ("sidebar", "normal", "ctrl+w", "-"),
        // No element handles it: the binding without a predicate does.
        ("sidebar", "normal", "ctrl+s", "save"),
    ];
    for (focus, mode, key, expected) in cases {
        let app = Workspace {
            mode,
            log: Vec::new(),
        };
        let adapter = UiAdapter::new(app, "keys").with_key_bindings(bindings());
        let mut ui = UiTestHarness::with_adapter(adapter, (300.0, 200.0), 1.0);
        ui.frame();
        // Tab order: editor, then sidebar.
        let tabs = if focus == "editor" { 1 } else { 2 };
        for _ in 0..tabs {
            ui.key("tab");
        }
        assert_eq!(ui.focus(), Some(FocusId::from_key(focus)));
        ui.key(key);
        let got = ui.app().log.last().copied().unwrap_or("-");
        assert_eq!(got, expected, "{key} on {focus} in {mode} mode");
    }
}

const FIELD: FocusId = FocusId::from_key("field");

struct Composer {
    field: TextField,
    log: Vec<&'static str>,
}

impl UiApp for Composer {
    type Action = Cmd;
    type Message = ();

    fn view(&mut self, cx: &mut ViewContext) -> AnyElement {
        div()
            .size_full()
            .key_context("workspace")
            .on_key("k", Cmd("element up"))
            .child(
                text_input("Comment", "")
                    .field(&self.field)
                    .focus_target(FIELD)
                    .focused(cx.is_focused(FIELD))
                    .w(200.0)
                    .h(40.0),
            )
            .into_any()
    }

    fn update(&mut self, Cmd(name): Cmd, _cx: &mut UiContext) {
        self.log.push(name);
    }

    fn edit_text(&mut self, _target: FocusId, command: TextEditCommand) -> TextEditOutcome {
        self.field.apply(command)
    }
}

#[test]
fn printable_keys_type_into_a_focused_field_instead_of_firing_bindings() {
    let mut keys = KeyBindings::new();
    for (key, name) in [("space", "page down"), ("j", "down"), ("mod+s", "save")] {
        keys.bind(key, Some("workspace"), Cmd(name)).unwrap();
    }
    let app = Composer {
        field: TextField::new(""),
        log: Vec::new(),
    };
    let adapter = UiAdapter::new(app, "keys").with_key_bindings(keys);
    let mut ui = UiTestHarness::with_adapter(adapter, (300.0, 200.0), 1.0);
    ui.frame();
    ui.click((10.0, 10.0));
    assert_eq!(ui.focus(), Some(FIELD));

    ui.type_text("j k");
    ui.key("mod+s");

    assert_eq!(ui.app().field.text(), "j k");
    assert_eq!(ui.app().log, ["save"]);
}
