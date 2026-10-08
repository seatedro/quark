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
    ui.frame();
    ui.frame();
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
    assert!(ui.try_find(By::name("1.3.0")).is_some());
}

// Catches Escape denying nothing: it must answer the prompt as Deny.
#[test]
fn escape_denies_the_pending_command() {
    let mut ui = harness("approval");
    ui.key("escape");
    ui.frame();
    assert!(ui.try_find(By::role(Role::AlertDialog)).is_none());
    assert!(ui.try_find(By::name("declined.")).is_some());
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
