//! The Diff panel's controls against the fixture patch: find, copy,
//! wrap, and expanding the context the patch leaves out.

mod common;

use accesskit::Role;
use common::*;
use quark_app::testing::{By, Node, UiTestHarness};
use quark_workbench::Workbench;
use quark_workbench::contracts::ScenarioKind;

fn button(name: &str) -> By {
    By::role_name(Role::Button, name)
}

/// The review thread with its Diff panel shown.
fn diff_panel() -> UiTestHarness<Workbench> {
    let mut ui = harness(ScenarioKind::Review);
    ui.click_node(By::role_name(Role::Tab, "Diff"));
    ui
}

/// The diff's line rows (left edges inside the panel's list).
fn lines(ui: &UiTestHarness<Workbench>) -> Vec<Node> {
    let list = ui.find(By::id("workbench.diff")).bounds;
    ui.find_all(By::role(Role::ListItem))
        .into_iter()
        .filter(|n| {
            let (x, y) = (n.bounds.x, n.bounds.y + 1.0);
            x >= list.x && x < list.right() && y >= list.y && y < list.y + list.height
        })
        .collect()
}

// Catches a find control that does not search the patch: typing counts
// the new side's matches and Next steps to the second.
#[test]
fn diff_find_counts_and_steps_through_matches() {
    let mut ui = diff_panel();
    ui.click_node(button("Find in diff"));
    ui.type_text("onKeyDown");
    assert!(ui.try_find(By::role_name(Role::Status, "1 of 3")).is_some());
    ui.click_node(button("Next match"));
    assert!(ui.try_find(By::role_name(Role::Status, "2 of 3")).is_some());
}

// Catches Copy patch copying nothing or only what is on screen: the
// clipboard gets both files' patches.
#[test]
fn diff_copy_puts_the_whole_patch_on_the_clipboard() {
    let mut ui = diff_panel();
    ui.click_node(button("Copy patch"));
    let patch = ui.clipboard_text().unwrap_or_default();
    assert!(patch.contains("+++ b/src/App.tsx\n"), "{patch}");
    assert!(patch.contains("+++ b/src/commands.ts\n"), "{patch}");
}

// Catches context the patch leaves out staying out of reach: with the
// fixture's sources behind the patch, Expand shows App.tsx's last lines.
#[test]
fn diff_expand_all_shows_lines_outside_the_hunks() {
    let mut ui = diff_panel();
    let has = |ui: &UiTestHarness<Workbench>, text: &str| {
        lines(ui).iter().any(|n| n.name.as_deref() == Some(text))
    };
    assert!(!has(&ui, "    </main>"));
    ui.click_node(button("Expand all unchanged lines"));
    assert!(has(&ui, "    </main>"));
}

// Catches Wrap lines leaving long lines clipped in the narrow panel: the
// second import wraps onto several rows.
#[test]
fn diff_wrap_wraps_long_lines() {
    let mut ui = diff_panel();
    let long = "import { CommandList, commandForKey } from \"./commands\";";
    let height = |ui: &UiTestHarness<Workbench>| {
        lines(ui)
            .into_iter()
            .find(|n| n.name.as_deref() == Some(long))
            .map(|n| n.bounds.height)
            .unwrap()
    };
    let before = height(&ui);
    ui.click_node(button("Wrap lines"));
    assert!(height(&ui) >= before * 2.0, "{before} -> {}", height(&ui));
}
