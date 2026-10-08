//! Dock behavior: the Diff, Terminal, and Files panels against the
//! fixture file store, and panels moving between windows with their state.

mod common;

use std::collections::HashMap;

use accesskit::{NodeId, Role, TreeUpdate};
use common::*;
use quark_app::WindowHandle;
use quark_app::testing::{By, UiTestHarness};
use quark_workbench::Workbench;
use quark_workbench::contracts::ScenarioKind;
use quark_workbench::dock::files::SOURCE_LABEL;

fn tab(name: &str) -> By {
    By::role_name(Role::Tab, name)
}

/// The text of the Files panel's source view: its text runs, joined.
fn source_in(tree: &TreeUpdate) -> String {
    let nodes: HashMap<NodeId, &accesskit::Node> =
        tree.nodes.iter().map(|(id, n)| (*id, n)).collect();
    let Some(view) = nodes.values().find(|n| n.label() == Some(SOURCE_LABEL)) else {
        return String::new();
    };
    view.children()
        .iter()
        .filter_map(|id| nodes.get(id)?.value())
        .collect()
}

fn source(ui: &UiTestHarness<Workbench>) -> String {
    source_in(&ui.accessibility_update())
}

/// Every tab named `name` across the main window's tab strips.
fn tab_count(ui: &UiTestHarness<Workbench>, name: &str) -> usize {
    ui.find_all(tab(name)).len()
}

/// Move the dock tab `name` to a new window from the keyboard, as the
/// e2e spec does: focus the tab, Shift+F10, "Move to new window". Returns
/// the new window.
fn move_to_new_window(ui: &mut UiTestHarness<Workbench>, name: &str) -> WindowHandle {
    ui.click_node(tab(name));
    ui.key("shift+f10");
    ui.click_node(By::role_name(Role::MenuItem, "Move to new window"));
    let main = ui.main_window();
    let floating: Vec<_> = ui.windows().into_iter().filter(|w| *w != main).collect();
    assert_eq!(floating.len(), 1, "{floating:?}");
    floating[0]
}

// Catches Apply and Undo that do not reach the file store, or a source
// view that does not follow it: the Files panel shows the patched
// App.tsx after Apply and the original after Undo.
#[test]
fn dock_apply_and_undo_change_the_fixture_source() {
    let mut ui = harness(ScenarioKind::Review);
    let added = "window.addEventListener(\"keydown\", onKeyDown);";
    ui.click_node(tab("Files"));
    let before = source(&ui);

    ui.click_node(tab("Diff"));
    ui.click_node(By::role_name(Role::Button, "Apply"));
    ui.click_node(tab("Files"));
    let applied = source(&ui);
    ui.click_node(tab("Diff"));
    ui.click_node(By::role_name(Role::Button, "Undo"));
    ui.click_node(tab("Files"));
    let undone = source(&ui);

    assert!(!before.contains(added), "{before}");
    assert!(applied.contains(added), "{applied}");
    assert_eq!(undone, before);
}

// Catches typed bytes that never reach the scripted shell, or output fed
// somewhere the VT view does not paint: `ls src` typed into the focused
// terminal echoes and paints the fixture directory listing.
#[test]
fn dock_terminal_bytes_produce_visible_output() {
    let mut ui = harness(ScenarioKind::Review);
    ui.click_node(tab("Terminal"));
    ui.click_node(By::role(Role::Terminal));
    ui.type_text("ls src\n");

    let painted = ui.painted_text();
    assert!(painted.contains("$ ls src"), "{painted}");
    assert!(
        painted.contains("App.tsx  commands.ts  styles.css  trips.ts"),
        "{painted}"
    );
}

// Catches panel content kept per window: the file picked in the Files
// panel is still open after the panel moves to a window of its own.
#[test]
fn dock_transfer_preserves_panel_state() {
    let mut ui = harness(ScenarioKind::Review);
    ui.click_node(tab("Files"));
    ui.click_node(By::role_name(Role::TreeItem, "README.md"));
    let picked = source(&ui);

    let window = move_to_new_window(&mut ui, "Files");
    let floating = source_in(&ui.window(window).accessibility_update());

    assert!(picked.starts_with("# Atlas"), "{picked}");
    assert_eq!(floating, picked);
}

// Catches a floating window whose close loses or duplicates its panel:
// closing the Diff's window puts exactly one Diff tab back in the main
// window and leaves no other window.
#[test]
fn dock_closing_floating_host_redocks_once() {
    let mut ui = harness(ScenarioKind::Review);
    let window = move_to_new_window(&mut ui, "Diff");
    let while_floating = tab_count(&ui, "Diff");

    assert!(ui.window(window).request_close());

    assert_eq!(ui.windows(), [ui.main_window()]);
    assert_eq!((while_floating, tab_count(&ui, "Diff")), (0, 1));
}
