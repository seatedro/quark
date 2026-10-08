//! Overlay policy: the per-window layer stack, the palette and menus going
//! through the command registry, the settings modal, toasts, tooltips, and
//! completion announcements.

mod common;

use std::collections::BTreeSet;

use accesskit::Role;
use common::*;
use quark_app::InputEvent;
use quark_app::testing::{By, UiTestHarness};
use quark_app::winit::event::{ElementState, MouseButton};
use quark_components::PALETTE_INPUT;
use quark_workbench::Workbench;
use quark_workbench::composer;
use quark_workbench::contracts::{Options, ScenarioKind};
use quark_workbench::settings::forms::LIMIT;

type Ui = UiTestHarness<Workbench>;

fn palette() -> By {
    By::role_name(Role::Dialog, "Command palette")
}

fn settings() -> By {
    By::role_name(Role::Dialog, "Settings")
}

fn shown(ui: &Ui, by: By) -> bool {
    ui.try_find(by).is_some()
}

fn right_click(ui: &mut Ui, at: (f32, f32)) {
    ui.pointer_move(at);
    for state in [ElementState::Pressed, ElementState::Released] {
        ui.send_event(InputEvent::PointerButton {
            button: MouseButton::Right,
            state,
        });
    }
    ui.frame();
}

/// Right-click thread `name`'s row and choose `item` from its menu.
fn thread_menu(ui: &mut Ui, name: &str, item: &str) {
    let row = ui.find(thread_row(name)).center();
    right_click(ui, row);
    ui.click_node(By::role_name(Role::MenuItem, item));
}

/// The accessible description of the node with `role` and `name`.
fn description(ui: &Ui, role: Role, name: &str) -> Option<String> {
    ui.accessibility_update()
        .nodes
        .iter()
        .find(|(_, n)| n.role() == role && n.label() == Some(name))
        .and_then(|(_, n)| n.description().map(str::to_owned))
}

/// Labels of the toasts on screen.
fn toasts(ui: &Ui) -> Vec<String> {
    ui.accessibility_update()
        .nodes
        .iter()
        .filter(|(_, n)| {
            n.role() == Role::Status
                && n.live().is_some()
                && n.bounds().is_some_and(|b| b.width() > 0.0)
        })
        .filter_map(|(_, n)| n.label().map(str::to_owned))
        .collect()
}

// Catches Escape closing every layer at once, or the dialog under a
// picker: with the theme picker open over settings, the first Escape
// closes only the picker, the second only the dialog.
#[test]
fn overlays_escape_dismisses_only_the_top_layer() {
    let mut ui = harness(ScenarioKind::Review);
    ui.key("mod+,");
    ui.key("enter");
    assert!(
        shown(&ui, By::role(Role::ListBox)),
        "the theme picker opened"
    );

    ui.key("escape");
    assert_eq!(
        (shown(&ui, By::role(Role::ListBox)), shown(&ui, settings())),
        (false, true)
    );
    ui.key("escape");
    assert!(!shown(&ui, settings()));
}

// Catches an overlay forgetting who opened it: the palette opened from the
// composer gives focus back to the composer when it closes.
#[test]
fn overlays_focus_returns_to_invoker() {
    let mut ui = harness(ScenarioKind::Review);
    ui.click_node(By::name("Message"));
    ui.key("mod+k");
    assert_eq!(ui.focus(), Some(PALETTE_INPUT));

    ui.key("escape");
    assert_eq!(
        (shown(&ui, palette()), ui.focus()),
        (false, Some(composer::INPUT))
    );
}

// Catches a palette with its own command handling: choosing "Settings"
// in it opens the same dialog Mod+, does.
#[test]
fn overlays_palette_runs_registry_commands() {
    let mut ui = harness(ScenarioKind::Review);
    ui.key("mod+k");
    ui.type_text("Settings");
    ui.key("enter");
    assert_eq!(
        (shown(&ui, palette()), shown(&ui, settings())),
        (false, true)
    );
}

// Catches a modal that lets shortcuts act behind it: with settings open,
// Mod+B leaves the sidebar alone and Mod+K opens no palette.
#[test]
fn overlays_modal_blocks_background_shortcut() {
    let mut ui = harness(ScenarioKind::Review);
    ui.key("mod+,");
    ui.key("mod+b");
    ui.key("mod+k");
    assert_eq!(
        (
            shown(&ui, thread_row("Add keyboard shortcuts")),
            shown(&ui, palette())
        ),
        (true, false)
    );
}

// Catches Cancel keeping a previewed theme: picking Light repaints the
// app light at once, and Cancel brings back the theme it had (dark: the
// harness reports no desktop preference).
#[test]
fn overlays_settings_cancel_reverts_preview() {
    let mut ui = harness(ScenarioKind::Review);
    let before = ui.theme().colors.background;
    ui.key("mod+,");
    // Theme: Match system, Light, Dark.
    ui.key("enter");
    ui.key("arrowdown");
    ui.key("enter");
    let light = quark_workbench::design::light().colors.background;
    assert_ne!(light, before);
    assert_eq!(ui.theme().colors.background, light, "previewed");

    ui.click_node(By::role_name(Role::Button, "Cancel"));
    assert_eq!(
        (shown(&ui, settings()), ui.theme().colors.background),
        (false, before)
    );
}

// Catches Save accepting an invalid value or hiding why: an out-of-range
// limit keeps the dialog open with the error under the field and focus in
// it.
#[test]
fn overlays_settings_validation_blocks_save() {
    let mut ui = harness(ScenarioKind::Review);
    ui.key("mod+,");
    ui.click_node(By::role_name(Role::TextInput, "Tool output limit"));
    ui.key("mod+a");
    ui.type_text("5");
    ui.click_node(By::role_name(Role::Button, "Save"));

    let error = "Enter a whole number from 100 to 10,000.";
    let field = description(&ui, Role::Group, "Tool output limit");
    assert_eq!(
        (shown(&ui, settings()), field.as_deref(), ui.focus()),
        (true, Some(error), Some(LIMIT))
    );
    assert_eq!(ui.app().settings.saved().output_limit, 400);
}

// Catches the menu path skipping the shared routing: a thread row's
// right-click menu opens at the pointer, and "Open thread" selects it.
#[test]
fn overlays_context_menu_opens_thread() {
    let mut ui = harness(ScenarioKind::Review);
    let row = ui.find(thread_row("Offline tile cache")).center();
    right_click(&mut ui, row);
    let menu = ui.find(By::role(Role::Menu)).bounds;
    assert_eq!((menu.x, menu.y), row);

    ui.click_node(By::role_name(Role::MenuItem, "Open thread"));
    assert_eq!(
        (shown(&ui, By::role(Role::Menu)), selected_rows(&ui)),
        (false, vec!["Offline tile cache".to_owned()])
    );
}

// Catches toasts piling up behind the three shown: of four in a row the
// oldest is gone, so dismissing the newest leaves two, not the oldest
// sliding back in.
#[test]
fn overlays_toast_stack_keeps_three() {
    let mut ui = harness(ScenarioKind::Review);
    for name in [
        "Add keyboard shortcuts",
        "Offline tile cache",
        "Share trips as read-only links, unread",
        "Trip export to GPX",
    ] {
        thread_menu(&mut ui, name, "Copy title");
    }
    ui.advance(500);
    let three = toasts(&ui).len();
    let newest = ui
        .find_all(By::role_name(Role::Button, "Dismiss"))
        .into_iter()
        .max_by_key(|b| {
            b.id.as_deref()
                .and_then(|id| id.rsplit(':').next()?.parse::<u64>().ok())
        })
        .expect("dismiss buttons");
    ui.click(newest.center());
    ui.advance(500);
    let left = toasts(&ui);
    assert_eq!((three, left.len()), (3, 2), "{left:?}");
    assert!(
        !left.iter().any(|t| t.contains("Add keyboard shortcuts")),
        "{left:?}"
    );
}

// Catches an Undo button that does not reach the command registry: Undo
// on the "Applied" toast puts the fixture files back.
#[test]
fn overlays_toast_undo_runs_command() {
    let mut ui = harness(ScenarioKind::Review);
    ui.key("mod+k");
    ui.type_text("Apply proposed");
    ui.key("enter");
    assert!(!ui.app().model.files.changed().is_empty(), "applied");

    ui.click_node(By::role_name(Role::Button, "Undo"));
    assert!(ui.app().model.files.changed().is_empty());
}

// Catches a tooltip that shows at once, never, or takes focus: resting on
// the palette button shows its tooltip after 500 ms, focus untouched.
#[test]
fn overlays_tooltip_waits_and_keeps_focus() {
    let mut ui = harness(ScenarioKind::Review);
    ui.click_node(By::name("Message"));
    let button = ui
        .find(By::role_name(Role::Button, "Command palette"))
        .center();
    ui.pointer_move(button);
    ui.advance(400);
    let early = shown(&ui, By::role(Role::Tooltip));
    ui.advance(100);
    assert_eq!(
        (
            early,
            shown(&ui, By::role_name(Role::Tooltip, "Command palette")),
            ui.focus()
        ),
        (false, true, Some(composer::INPUT))
    );
}

// Catches Escape cancelling an IME composition and closing the palette
// with it: mid-composition, Escape belongs to the IME.
#[test]
fn overlays_composition_keeps_escape() {
    let mut ui = harness(ScenarioKind::Review);
    ui.key("mod+k");
    ui.ime_preedit("に", Some((3, 3)));
    ui.key("escape");
    assert!(shown(&ui, palette()));
}

// Catches Escape closing the palette while a drag is held in it: the
// first Escape only ends the drag.
#[test]
fn overlays_drag_cancel_keeps_escape() {
    let mut ui = harness(ScenarioKind::Review);
    ui.key("mod+k");
    ui.type_text("thread");
    let field = ui
        .find(By::role_name(Role::SearchInput, "Command palette"))
        .bounds;
    let y = field.y + field.height / 2.0;
    ui.pointer_down((field.x + field.width - 4.0, y));
    ui.pointer_move((field.x + 4.0, y));
    ui.key("escape");
    assert!(shown(&ui, palette()));
}

/// Announcements spoken while `ui` runs for `ms`, in order: each new
/// announcement node once.
fn announcements_over(ui: &mut Ui, ms: u64) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut spoken = Vec::new();
    for _ in 0..ms / 50 {
        ui.advance(50);
        for (id, node) in &ui.accessibility_update().nodes {
            if node.role() == Role::Status
                && node.live().is_some()
                && node.bounds().is_some_and(|b| b.width() == 0.0)
                && seen.insert(*id)
            {
                spoken.push(node.label().unwrap_or_default().to_owned());
            }
        }
    }
    spoken
}

// Catches completions announced per streamed chunk, or lost: one run
// speaks each of its three tool completions and the run's completion
// exactly once.
#[test]
fn overlays_completion_announced_once() {
    let mut ui = harness(ScenarioKind::Review);
    type_in_composer(&mut ui, "Make it layout independent");
    ui.key("enter");
    let spoken = announcements_over(&mut ui, 7_000);
    assert_eq!(
        (
            mentions(&spoken, "Tool finished"),
            mentions(&spoken, "Run complete")
        ),
        (3, 1),
        "{spoken:?}"
    );
}

/// How often `phrase` occurs across `spoken`.
fn mentions(spoken: &[String], phrase: &str) -> usize {
    spoken.iter().map(|s| s.matches(phrase).count()).sum()
}

// Catches completions that arrive together overwriting each other: when a
// frame finally plays the whole run at once (the window was hidden, or the
// manual clock jumped), every completion is still spoken.
#[test]
fn overlays_batched_completions_all_announced() {
    let mut ui = harness_with(
        Options {
            manual_clock: true,
            ..Options::default()
        },
        WIDE,
    );
    type_in_composer(&mut ui, "Make it layout independent");
    ui.key("enter");
    let spoken_before = announcements_over(&mut ui, 100);
    ui.app_mut().clock.set(10_000);
    ui.frame();
    let spoken = announcements_over(&mut ui, 100);
    assert_eq!(
        (
            mentions(&spoken, "Tool finished"),
            mentions(&spoken, "Run complete")
        ),
        (3, 1),
        "{spoken_before:?} then {spoken:?}"
    );
}
