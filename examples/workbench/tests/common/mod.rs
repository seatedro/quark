//! Shared harness setup for the workbench's integration tests. Each test
//! file includes it with `mod common;`.
#![allow(dead_code)]

use accesskit::Role;
use quark_app::testing::{By, UiTestHarness};
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

/// Names of the rows the accessibility tree reports as selected.
pub fn selected_rows(ui: &UiTestHarness<Workbench>) -> Vec<String> {
    ui.accessibility_update()
        .nodes
        .iter()
        .filter(|(_, n)| n.role() == Role::ListItem && n.is_selected() == Some(true))
        .filter_map(|(_, n)| n.label().map(str::to_owned))
        .collect()
}

/// Click the composer and type `text` into it.
pub fn type_in_composer(ui: &mut UiTestHarness<Workbench>, text: &str) {
    ui.click_node(By::name("Message"));
    ui.type_text(text);
}
