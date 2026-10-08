//! Shell behavior: the sidebar, thread switching, the width policy, and the
//! run lifecycle the shell surfaces.

mod common;

use accesskit::Role;
use common::*;
use quark_app::testing::By;
use quark_workbench::contracts::{Options, ScenarioKind, ThreadId};

// Catches drafts living in one shared field: text typed in a thread is
// still there after visiting another thread, and the other thread starts
// empty.
#[test]
fn shell_thread_switch_retains_draft() {
    let mut ui = harness(ScenarioKind::Review);
    type_in_composer(&mut ui, "keep this draft");
    ui.click_node(thread_row("Share trips as read-only links, unread"));
    let elsewhere = ui.find(By::name("Message")).value;
    ui.click_node(thread_row("Add keyboard shortcuts"));

    let back = ui.find(By::name("Message")).value;
    assert_eq!(
        (elsewhere.as_deref().unwrap_or(""), back.as_deref()),
        ("", Some("keep this draft"))
    );
}

// Catches selection following a row index: filtering moves the selected
// thread's row, and clearing the filter brings every row back, but the
// same thread stays selected throughout.
#[test]
fn shell_filtering_retains_stable_selection() {
    let mut ui = harness(ScenarioKind::Review);
    ui.click_node(thread_row("Offline tile cache"));
    let before = selected_rows(&ui);
    ui.click_node(By::name("Search threads"));
    ui.type_text("tile");
    let filtered = selected_rows(&ui);
    let shown = ui.find_all(By::role(Role::ListItem)).len();
    ui.click_node(By::role_name(Role::Button, "Clear"));
    let cleared = selected_rows(&ui);

    let want = vec!["Offline tile cache".to_owned()];
    assert_eq!((&before, &filtered, &cleared), (&want, &want, &want));
    assert_eq!(shown, 1, "only the matching thread is listed");
}

// Catches a stopped run still writing: after Stop, the scenario's later
// events (more text, tool output, completion) change nothing.
#[test]
fn shell_stopped_runs_reject_late_events() {
    let mut ui = harness(ScenarioKind::Review);
    type_in_composer(&mut ui, "Make it layout independent");
    ui.key("enter");
    ui.advance(1_000);
    ui.click_node(By::role_name(Role::Button, "Stop"));
    let rows = |ui: &quark_app::testing::UiTestHarness<_>| {
        let wb: &quark_workbench::Workbench = ui.app();
        let t = wb.model.thread(ThreadId(1)).expect("thread");
        t.transcript
            .rows()
            .iter()
            .map(|r| {
                format!(
                    "{:?} {} {:?}",
                    r.role,
                    r.markdown,
                    r.tool.as_ref().map(|t| t.status)
                )
            })
            .collect::<Vec<_>>()
    };
    let stopped = rows(&ui);

    ui.advance(10_000);

    assert_eq!(rows(&ui), stopped);
    assert!(ui.try_find(By::role_name(Role::Button, "Send")).is_some());
}

// Catches the minimum window losing the composer: at 960x640 the sidebar
// leaves the dock, the right dock collapses, and the composer field is on
// screen with room to type.
#[test]
fn shell_960_layout_leaves_composer_reachable() {
    let mut ui = harness_with(
        Options {
            scenario: ScenarioKind::Review,
            ..Options::default()
        },
        (960.0, 640.0),
    );
    let field = ui.find(By::name("Message")).bounds;
    let sidebar = ui.try_find(By::role_name(Role::List, "Threads"));
    type_in_composer(&mut ui, "fits");

    assert!(sidebar.is_none(), "the sidebar is an overlay at 960");
    assert!(
        field.x >= 0.0 && field.right() <= 960.0 && field.bottom() <= 640.0 && field.width >= 400.0,
        "composer field at {field:?}"
    );
    assert_eq!(ui.find(By::name("Message")).value.as_deref(), Some("fits"));
}

// Catches the sidebar costing a Tab stop per thread or ignoring arrow
// keys: the list is one stop, Down and End move the selection through the
// listed threads, and the title bar follows.
#[test]
fn shell_arrow_keys_move_the_selection_from_the_focused_list() {
    let mut ui = harness(ScenarioKind::Review);
    ui.click_node(By::role_name(Role::List, "Threads"));
    ui.key("down");
    let after_down = selected_rows(&ui);
    ui.key("end");

    assert_eq!(after_down, ["Share trips as read-only links"]);
    assert_eq!(selected_rows(&ui), ["Empty library onboarding"]);
    assert!(ui.find(By::role(Role::List)).focused);
}
