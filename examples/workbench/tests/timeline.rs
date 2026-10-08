//! The transcript surface (stream C): tool cards, copy across rows,
//! streaming, tables, and Jump to latest, driven through the whole app.

mod common;

use accesskit::Role;
use common::*;
use quark_app::testing::{By, UiTestHarness};
use quark_workbench::Workbench;
use quark_workbench::contracts::{Options, ScenarioKind, ThreadId};

/// The failed `npm test` card of "Add keyboard shortcuts" (row 9).
const FAILED_CARD: &str = "Ran npm test, failed";
/// A line of that card's output.
const FAILED_OUTPUT: &str = "Tests  1 failed | 12 passed (13)";
/// A line of the finished search card's output (row 3), collapsed by
/// default.
const SEARCH_OUTPUT: &str = "0 results in 4 files";

fn transcript(ui: &UiTestHarness<Workbench>) -> quark_app::testing::Node {
    ui.find(By::role_name(Role::List, "Transcript"))
}

/// Whether the node named `name` reports itself expanded.
fn expanded(ui: &UiTestHarness<Workbench>, name: &str) -> Option<bool> {
    ui.accessibility_update()
        .nodes
        .iter()
        .find(|(_, n)| n.label() == Some(name))
        .and_then(|(_, n)| n.is_expanded())
}

/// Wheels the transcript up until `by` is published inside it.
fn scroll_up_to(ui: &mut UiTestHarness<Workbench>, by: By) -> quark_app::testing::Node {
    let list = transcript(ui).bounds;
    ui.pointer_move((list.x + list.width * 0.5, list.y + list.height * 0.5));
    for _ in 0..60 {
        // The first match: a role can name several nodes.
        if let Some(node) = ui.find_all(by.clone()).into_iter().next() {
            let b = node.bounds;
            if b.y >= list.y && b.y + b.height <= list.y + list.height {
                return node;
            }
        }
        ui.wheel(0.0, -120.0);
    }
    panic!("{by:?} never scrolled into the transcript");
}

/// Everything the transcript publishes as text, in tree order.
fn transcript_text(ui: &UiTestHarness<Workbench>) -> String {
    ui.accessibility_tree()
}

// Catches a collapsing card moving under the pointer, keeping its output in
// the document, or dropping focus from its disclosure.
#[test]
fn timeline_collapsing_a_tool_card_keeps_it_in_place_and_hides_its_output() {
    let mut ui = harness(ScenarioKind::Review);
    let card = scroll_up_to(&mut ui, By::role_name(Role::Button, FAILED_CARD));
    let shown = transcript_text(&ui).contains(FAILED_OUTPUT);

    ui.click(card.center());

    let after = ui.find(By::role_name(Role::Button, FAILED_CARD));
    let focused = ui.focused().and_then(|n| n.name);
    assert_eq!(
        (
            shown,
            transcript_text(&ui).contains(FAILED_OUTPUT),
            expanded(&ui, FAILED_CARD),
            after.bounds.y,
            focused.as_deref(),
        ),
        (true, false, Some(false), card.bounds.y, Some(FAILED_CARD))
    );
}

// Catches a card's controls lost under its row label: the disclosure and
// Retry must stay reachable by name, and Retry must start a run.
#[test]
fn timeline_tool_card_controls_stay_accessible_and_retry_runs() {
    let mut ui = harness(ScenarioKind::Review);
    let card = scroll_up_to(&mut ui, By::role_name(Role::Button, FAILED_CARD));
    let row = ui.find(By::role_name(
        Role::ListItem,
        format!("Tool: {FAILED_CARD}"),
    ));
    // The card's Retry sits on its header, right of the disclosure.
    let retry = ui
        .find_all(By::role_name(Role::Button, "Retry"))
        .into_iter()
        .find(|b| (b.bounds.y - card.bounds.y).abs() < card.bounds.height)
        .expect("the failed card offers Retry");

    ui.click(retry.center());

    let wb: &Workbench = ui.app();
    assert_eq!(
        (
            expanded(&ui, FAILED_CARD),
            row.bounds.y <= card.bounds.y,
            wb.model.run(ThreadId(1)).is_some()
        ),
        (Some(true), true, true)
    );
}

// Catches copy reading only materialized rows, or copying the output of a
// collapsed card: Select all then Copy takes the whole thread as shown.
#[test]
fn timeline_copy_spans_every_row_after_virtualization() {
    let mut ui = harness_with(
        Options {
            scenario: ScenarioKind::Review,
            seed: 7,
            ..Options::default()
        },
        (1000.0, 640.0),
    );
    let list = transcript(&ui).bounds;
    // A press in the transcript takes focus off the composer.
    type_in_composer(&mut ui, "draft");
    ui.click((list.x + 40.0, list.y + 40.0));

    ui.key("mod+a");
    ui.key("mod+c");

    let copied = ui.clipboard_text().unwrap_or_default();
    let wb: &Workbench = ui.app();
    let doc = wb
        .timeline
        .thread_view(ThreadId(1))
        .expect("the thread is shown")
        .document()
        .document();
    let checks = [
        "Add keyboard shortcuts for the common actions",
        "retrying continues from the last tool result.",
        FAILED_OUTPUT,
    ]
    .map(|text| copied.contains(text));
    assert_eq!(
        (
            checks,
            copied.contains(SEARCH_OUTPUT),
            doc.visible_rows().len() < doc.len()
        ),
        ([true; 3], false, true),
        "{copied}"
    );
}

// Catches a streamed answer showing an open fence as prose until the
// closing fence arrives.
#[test]
fn timeline_streamed_unfinished_fence_becomes_code() {
    let mut ui = harness(ScenarioKind::Review);
    let before = ui.find_all(By::role_name(Role::Button, "Copy code")).len();
    ui.click_node(By::role_name(Role::Button, "Retry"));
    // Play the run until its answer holds an open `ts` fence.
    let mut open = false;
    for _ in 0..200 {
        ui.advance(40);
        let wb: &Workbench = ui.app();
        let thread = wb.model.thread(ThreadId(1)).expect("thread");
        let answer = wb
            .model
            .run(ThreadId(1))
            .and_then(|run| run.answer)
            .and_then(|id| thread.transcript.get(id));
        if let Some(row) = answer
            && let Some((_, code)) = row.markdown.split_once("```ts\n")
            && !code.is_empty()
            && !code.contains("```")
        {
            open = true;
            break;
        }
    }
    ui.frame();

    let after = ui.find_all(By::role_name(Role::Button, "Copy code")).len();
    assert_eq!((open, after), (true, before + 1));
}

/// Cell texts of the first markdown table in `text`, row by row.
fn table_cells(text: &str) -> Vec<Vec<String>> {
    let doc = quark_app::quark_ui::markdown::MarkdownDoc::parse(text);
    let Some(block) =
        (0..doc.len()).find(|&b| doc.kind(b) == quark_app::quark_ui::markdown::BlockKind::Table)
    else {
        return Vec::new();
    };
    let mut rows: Vec<Vec<String>> = Vec::new();
    for cell in doc.cells(block) {
        let (row, _) = doc.cell_position(cell);
        rows.resize_with(rows.len().max(row + 1), Vec::new);
        rows[row].push(doc.cell_text(cell).to_owned());
    }
    rows
}

// Catches a copied table that is not a markdown table any more: dragging
// from its first header cell to its last cell must paste as the same table.
#[test]
fn timeline_table_copy_is_valid_markdown() {
    let mut ui = harness(ScenarioKind::Review);
    let first = scroll_up_to(&mut ui, By::role(Role::ColumnHeader));
    let last = ui
        .find_all(By::role(Role::Cell))
        .into_iter()
        .last()
        .expect("the table has cells");
    let from = (first.bounds.x + 2.0, first.center().1);
    let to = (last.bounds.x + last.bounds.width - 2.0, last.center().1);

    ui.drag(from, to);
    ui.key("mod+c");

    let copied = ui.clipboard_text().unwrap_or_default();
    let wb: &Workbench = ui.app();
    let thread = wb.model.thread(ThreadId(1)).expect("thread");
    let source = thread
        .transcript
        .rows()
        .iter()
        .find(|r| r.markdown.contains("| Command |"))
        .expect("the fixture table row");
    let expected = table_cells(&source.markdown);
    assert_eq!(
        (expected.len(), table_cells(&copied)),
        (4, expected.clone()),
        "{copied}"
    );
}

// Catches content arriving while scrolled up moving the view, or Jump to
// latest not appearing or not taking the view back to the end.
#[test]
fn timeline_jump_to_latest_appears_without_moving_the_view() {
    let mut ui = harness(ScenarioKind::Review);
    ui.click_node(By::role_name(Role::Button, "Retry"));
    let anchor = scroll_up_to(&mut ui, By::role_name(Role::Button, FAILED_CARD));

    ui.advance(2_000);
    ui.frame();
    let held = ui.find(By::role_name(Role::Button, FAILED_CARD)).bounds.y;
    let jump = ui.find(By::role_name(Role::Button, "Jump to latest"));
    ui.click(jump.center());

    let wb: &Workbench = ui.app();
    let doc = wb
        .timeline
        .thread_view(ThreadId(1))
        .expect("shown")
        .document()
        .document();
    assert_eq!(
        (
            held,
            doc.is_stuck_to_bottom(),
            ui.try_find(By::role_name(Role::Button, "Jump to latest"))
                .is_some()
        ),
        (anchor.bounds.y, true, false)
    );
}
