//! Composer behavior: sending, IME, mentions, growth, attachments, and the
//! Send/Stop button.

mod common;

use accesskit::Role;
use common::*;
use quark_app::testing::{By, UiTestHarness};
use quark_workbench::Workbench;
use quark_workbench::composer::{self, completions};
use quark_workbench::contracts::{ATTACHMENT_LIMIT_BYTES, ScenarioKind, ThreadId};

fn review() -> UiTestHarness<Workbench> {
    harness(ScenarioKind::Review)
}

fn draft(ui: &UiTestHarness<Workbench>) -> String {
    ui.find(By::name("Message")).value.unwrap_or_default()
}

/// The user turns in thread 1's transcript, oldest first.
fn sent(ui: &UiTestHarness<Workbench>) -> Vec<String> {
    let wb: &Workbench = ui.app();
    let thread = wb.model.thread(ThreadId(1)).expect("thread");
    let count = |r: &&quark_workbench::model::Row| r.role == quark_workbench::model::Role::User;
    thread
        .transcript
        .rows()
        .iter()
        .filter(count)
        .map(|r| r.markdown.clone())
        .collect()
}

// Catches Enter inserting a newline instead of sending, or Shift+Enter
// sending instead of breaking the line.
#[test]
fn composer_enter_sends_and_shift_enter_breaks_the_line() {
    let cases = [("enter", "", true), ("shift+enter", "hi\n", false)];
    for (key, left, sends) in cases {
        let mut ui = review();
        let before = sent(&ui).len();
        type_in_composer(&mut ui, "hi");
        ui.key(key);

        let after = sent(&ui);
        assert_eq!(draft(&ui), left, "{key}");
        assert_eq!(after.len() - before, usize::from(sends), "{key}");
        if sends {
            assert_eq!(after.last().map(String::as_str), Some("hi"));
        }
    }
}

// Catches Enter confirming an IME candidate also sending the half-composed
// prompt.
#[test]
fn composer_enter_during_ime_composition_does_not_send() {
    let mut ui = review();
    let before = sent(&ui).len();
    type_in_composer(&mut ui, "ni ");
    ui.ime_preedit("hao", Some((3, 3)));
    ui.key("enter");
    ui.ime_commit("好");

    assert_eq!(sent(&ui).len(), before);
    assert_eq!(draft(&ui), "ni 好");
}

// Catches a picked mention leaving its query text behind, losing its chip,
// or Undo not bringing the typed query back.
#[test]
fn composer_mention_replaces_its_query_with_a_chip_and_undo_restores_it() {
    let mut ui = review();
    type_in_composer(&mut ui, "see @comm");
    let option = ui.find(By::role(Role::ListBoxOption));
    let name = option.name.clone().expect("option name");
    ui.key("enter");

    let editor = ui.app().composer.editor();
    let chip = editor.atoms().first().expect("chip").clone();
    assert_eq!(editor.text(), format!("see @{name} "));
    assert_eq!(chip.range, 4..5 + name.len());
    assert_eq!(chip.id.kind, completions::FILE_CHIP);

    ui.key("mod+z");
    assert_eq!(draft(&ui), "see @comm");
    assert!(ui.app().composer.editor().atoms().is_empty());
}

// Catches a lookup answer arriving after the user moved on (typed further,
// or switched threads and typed the same query again) replacing the
// current suggestions with stale ones.
#[test]
fn composer_stale_lookup_answers_cannot_reopen_suggestions() {
    let cases = ["typed further", "switched threads"];
    for case in cases {
        let mut ui = review();
        type_in_composer(&mut ui, "@co");
        let composer = &ui.app().composer;
        let asked = composer.completion().query().expect("query").seq;
        let thread_gen = composer.thread_generation();
        match case {
            "typed further" => ui.type_text("m"),
            _ => {
                ui.key("backspace");
                ui.key("backspace");
                ui.key("backspace");
                ui.click_node(thread_row("Share trips as read-only links, unread"));
                ui.click_node(thread_row("Add keyboard shortcuts"));
                ui.click_node(By::name("Message"));
                ui.type_text("@co");
            }
        }
        let stale = completions::answer(
            &completions::Lookup {
                thread_gen,
                seq: asked,
                query: "co".into(),
            },
            ["stale/costume.ts"].into_iter(),
        );
        let taken = ui.app_mut().composer.deliver(stale);
        ui.frame();

        assert!(!taken, "{case}");
        let options: Vec<_> = ui
            .find_all(By::role(Role::ListBoxOption))
            .into_iter()
            .filter_map(|n| n.name)
            .collect();
        assert_eq!(options, ["commands.ts"], "{case}");
    }
}

// Catches the field growing without bound, or not growing: one line keeps
// the 48pt area, each line adds 22pt up to six, and past six the field
// stops at 144pt and scrolls.
#[test]
fn composer_growth_stops_at_six_lines() {
    let mut ui = review();
    type_in_composer(&mut ui, "1");
    let mut heights = vec![ui.find(By::name("Message")).bounds.height];
    for line in 2..=8 {
        ui.key("shift+enter");
        ui.type_text(&line.to_string());
        heights.push(ui.find(By::name("Message")).bounds.height);
    }
    // The field is the text area less 6pt of padding above and below.
    let areas: Vec<f32> = heights.iter().map(|h| h + 12.0).collect();
    assert_eq!(areas, [48.0, 56.0, 78.0, 100.0, 122.0, 144.0, 144.0, 144.0]);
    assert!(
        ui.app().composer.editor().scroll_y > 0.0,
        "the last line is scrolled in"
    );
}

// Catches an oversized attachment wiping or altering the draft, or failing
// without saying why.
#[test]
fn composer_attachment_rejection_preserves_draft() {
    let mut ui = review();
    type_in_composer(&mut ui, "keep me");
    ui.set_clipboard_text("x".repeat(ATTACHMENT_LIMIT_BYTES + 1));
    ui.key("mod+v");

    assert_eq!(draft(&ui), "keep me");
    assert!(ui.app().composer.attachments().items.is_empty());
    let alert = ui.find(By::role(Role::Alert));
    assert!(
        alert.name.as_deref().is_some_and(|n| n.contains("10.0 MB")),
        "{alert:?}"
    );
}

// Catches keyboard focus dropping out of the composer when Send turns into
// Stop and back, because the button's identity changed with its label.
#[test]
fn composer_send_stop_preserves_focus() {
    let mut ui = review();
    type_in_composer(&mut ui, "go");
    ui.key("tab");
    ui.key("tab");
    ui.key("tab");
    ui.key("tab");
    assert_eq!(ui.focus(), Some(composer::SEND), "tabbed to Send");
    ui.key("enter");
    let running = ui.focused().and_then(|n| n.name);
    ui.key("enter");
    let stopped = ui.focused().and_then(|n| n.name);

    assert_eq!(
        (running.as_deref(), stopped.as_deref()),
        (Some("Stop"), Some("Send"))
    );
}

// Catches suggestions opening inside the composer's layout (pushing the
// draft) or below a composer at the bottom of the window, off screen.
#[test]
fn composer_suggestions_open_above_the_caret_without_moving_the_draft() {
    let mut ui = review();
    type_in_composer(&mut ui, "see ");
    let before = ui.find(By::name("Message")).bounds;
    ui.type_text("@");
    let list = ui.find(By::role(Role::ListBox)).bounds;
    let after = ui.find(By::name("Message")).bounds;

    assert_eq!(before, after);
    assert!(
        list.bottom() <= after.y + 22.0,
        "list {list:?} field {after:?}"
    );
    assert!(list.y >= 0.0);
}
