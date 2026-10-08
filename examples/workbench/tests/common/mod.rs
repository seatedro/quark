//! Shared harness setup for the workbench's integration tests. Each test
//! file includes it with `mod common;`.
#![allow(dead_code)]

use accesskit::Role;
use quark_app::testing::{By, Node, UiTestHarness};
use quark_workbench::contracts::{Options, ScenarioKind};
use quark_workbench::{Workbench, adapter};

/// The design's initial window size.
pub const WIDE: (f32, f32) = (1440.0, 900.0);

/// The workbench for `options` in a headless window of `size` at scale 1,
/// after its first frame. State is session-only (no state dir).
pub fn harness_with(options: Options, size: (f32, f32)) -> UiTestHarness<Workbench> {
    let mut ui = UiTestHarness::with_adapter(adapter(Workbench::new(options)), size, 1.0);
    ui.frame();
    ui
}

/// `scenario` at the initial size, live clock (the harness's fake clock).
pub fn harness(scenario: ScenarioKind) -> UiTestHarness<Workbench> {
    harness_with(
        Options {
            scenario,
            seed: 7,
            ..Options::default()
        },
        WIDE,
    )
}

/// A sidebar thread row by its accessible name (title plus ", running",
/// ", failed", ", unread" as they apply).
pub fn thread_row(name: &str) -> By {
    By::role_name(Role::ListItem, name)
}

/// Names of the threads the sidebar lists (its list items, not the
/// transcript's).
pub fn listed_threads(ui: &UiTestHarness<Workbench>) -> Vec<String> {
    let Some(list) = ui.try_find(By::role_name(Role::List, "Threads")) else {
        return Vec::new();
    };
    let inside = |b: quark::Rect| {
        b.x >= list.bounds.x && b.right() <= list.bounds.right() && b.y >= list.bounds.y - 1.0
    };
    ui.find_all(By::role(Role::ListItem))
        .into_iter()
        .filter(|n| inside(n.bounds))
        .filter_map(|n| n.name)
        .collect()
}

/// Names of the sidebar threads the accessibility tree reports as
/// selected.
pub fn selected_rows(ui: &UiTestHarness<Workbench>) -> Vec<String> {
    let listed = listed_threads(ui);
    ui.accessibility_update()
        .nodes
        .iter()
        .filter(|(_, n)| n.role() == Role::ListItem && n.is_selected() == Some(true))
        .filter_map(|(_, n)| n.label().map(str::to_owned))
        .filter(|name| listed.contains(name))
        .collect()
}

/// Click the composer and type `text` into it.
pub fn type_in_composer(ui: &mut UiTestHarness<Workbench>, text: &str) {
    ui.click_node(By::name("Message"));
    ui.type_text(text);
}

/// Whether `node` is a button on a toast (`quark_components::Toast` gives
/// its action buttons `toast-action:` ids).
fn on_toast(node: &Node) -> bool {
    node.id
        .as_deref()
        .is_some_and(|id| id.starts_with("toast-action:"))
}

/// The one button named `name` on a toast. Apply and the like offer an
/// Undo toast while their panel shows an Undo of its own.
#[track_caller]
pub fn toast_button(ui: &UiTestHarness<Workbench>, name: &str) -> Node {
    one(ui, name, true)
}

/// The one button named `name` that is not on a toast.
#[track_caller]
pub fn surface_button(ui: &UiTestHarness<Workbench>, name: &str) -> Node {
    one(ui, name, false)
}

#[track_caller]
fn one(ui: &UiTestHarness<Workbench>, name: &str, toast: bool) -> Node {
    let found: Vec<Node> = ui
        .find_all(By::role_name(Role::Button, name))
        .into_iter()
        .filter(|n| on_toast(n) == toast)
        .collect();
    assert_eq!(found.len(), 1, "buttons named {name:?} (toast {toast}): {found:?}");
    found.into_iter().next().expect("one")
}
