//! Flows driven through the real app headlessly: the approval card, the
//! slash list, and View changes opening the Changes tab.

use accesskit::Role;
use quark_app::testing::{By, UiTestHarness};
use quark_codex::{Codex, Options, TerminalMode, adapter};

fn harness(scene: &str) -> UiTestHarness<Codex> {
    let options = Options {
        scene: Some(scene.to_owned()),
        terminal: TerminalMode::Scripted,
        ..Options::default()
    };
    let mut ui = UiTestHarness::with_adapter(adapter(Codex::new(options)), (984.0, 738.0), 1.0);
    // Scenes may nudge the transcript's scroll a few frames in.
    for _ in 0..4 {
        ui.frame();
    }
    ui
}

// Catches Allow once not reaching the turn: the card must give way to the
// composer and the answer must land in the transcript.
#[test]
fn allowing_the_command_replaces_the_card_with_the_answer() {
    let mut ui = harness("approval");
    assert!(ui.try_find(By::role(Role::AlertDialog)).is_some());
    ui.click_node(By::role_name(Role::Button, "Allow once"));
    ui.frame();
    assert!(ui.try_find(By::role(Role::AlertDialog)).is_none());
    assert!(ui.painted_text().contains("1.3.0"), "{}", ui.painted_text());
}

// Catches Escape denying nothing: it must answer the prompt as Deny.
#[test]
fn escape_denies_the_pending_command() {
    let mut ui = harness("approval");
    ui.key("escape");
    ui.frame();
    assert!(ui.try_find(By::role(Role::AlertDialog)).is_none());
    assert!(ui.painted_text().contains("declined."), "{}", ui.painted_text());
}

// Catches the slash list not following the draft: typing "/" lists the
// commands, and narrowing it keeps only the matches.
#[test]
fn typing_a_slash_lists_and_filters_commands() {
    let mut ui = harness("home");
    ui.type_text("/");
    ui.frame();
    ui.frame();
    assert!(
        ui.try_find(By::role_name(Role::MenuItem, "Code review"))
            .is_some()
    );
    ui.type_text("pl");
    ui.frame();
    ui.frame();
    assert!(
        ui.try_find(By::role_name(Role::MenuItem, "Code review"))
            .is_none()
    );
    assert!(
        ui.try_find(By::role_name(Role::MenuItem, "Plan mode"))
            .is_some()
    );
}

// Catches View changes on the file change card not opening the panel.
#[test]
fn view_changes_opens_the_changes_tab() {
    let mut ui = harness("file-change");
    assert!(ui.try_find(By::role_name(Role::Tab, "Changes")).is_none());
    ui.click_node(By::role_name(Role::Button, "View changes"));
    ui.frame();
    assert!(ui.try_find(By::role_name(Role::Tab, "Changes")).is_some());
    assert!(
        ui.try_find(By::role_name(Role::Heading, "cart.js"))
            .is_some()
    );
}

// Catches row actions that never appear, or appear on every row: hovering
// one chat row reveals its Pin and Archive buttons, inside the row, and no
// other row's.
#[test]
fn hovering_a_chat_row_reveals_only_its_actions() {
    let mut ui = harness("home");
    assert!(ui.find_all(By::role_name(Role::Button, "Archive chat")).is_empty());
    let row = ui.find(By::role_name(Role::ListItem, "Design self-improving intent layer")).bounds;
    ui.pointer_move((row.x + 40.0, row.y + row.height / 2.0));
    ui.frame();
    let archive = ui.find_all(By::role_name(Role::Button, "Archive chat"));
    assert_eq!(archive.len(), 1);
    let b = archive[0].bounds;
    assert!(b.x > row.x + row.width / 2.0 && b.x + b.width <= row.x + row.width, "{b:?} in {row:?}");
    assert!(b.y >= row.y && b.y + b.height <= row.y + row.height, "{b:?} in {row:?}");
}
