//! Dock behavior: the Diff, Terminal, and Files panels against the
//! fixture file store, and panels moving between windows with their state.

mod common;

use std::collections::HashMap;

use accesskit::{NodeId, Role, TreeUpdate};
use common::*;
use quark::scene::Primitive;
use quark_app::testing::{By, UiTestHarness};
use quark_app::winit::window::Theme as SystemTheme;
use quark_app::{AppEvent, WindowHandle};
use quark_workbench::Workbench;
use quark_workbench::contracts::{ScenarioKind, ThemeChoice};
use quark_workbench::design;
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
    // The panel's Undo, not the one on the "Applied" toast.
    let undo = surface_button(&ui, "Undo").center();
    ui.click(undo);
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

// Catches terminal sessions sharing one screen: "New terminal" opens a
// second session with only a prompt, and switching back to the first
// shows what ran there.
#[test]
fn dock_terminal_sessions_keep_their_own_screens() {
    let mut ui = harness(ScenarioKind::Review);
    let listing = "App.tsx  commands.ts  styles.css  trips.ts";
    let help = "Demo terminal: a scripted shell";
    ui.click_node(tab("Terminal"));
    ui.click_node(By::role(Role::Terminal));
    ui.type_text("ls src\n");

    ui.click_node(By::role_name(Role::Button, "New terminal"));
    ui.type_text("help\n");
    let second = ui.painted_text();
    ui.click_node(tab("Shell 1"));
    let first = ui.painted_text();

    assert!(
        second.contains(help) && !second.contains(listing),
        "{second}"
    );
    assert!(first.contains(listing) && !first.contains(help), "{first}");
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

/// The color painted in the terminal's padding in `window`, just inside
/// its top-left corner: the last filled rect drawn over that point.
fn terminal_background(
    ui: &mut UiTestHarness<Workbench>,
    window: WindowHandle,
) -> Option<quark::Color> {
    let w = ui.window(window);
    let term = w.find(By::role(Role::Terminal)).bounds;
    let (x, y) = (term.x - 3.0, term.y - 3.0);
    w.scene().primitives.iter().rev().find_map(|p| match p {
        Primitive::Rect(r) if r.rect.contains(x, y) => Some(r.color),
        Primitive::RoundedRect(r) if r.rect.contains(x, y) => Some(r.color),
        _ => None,
    })
}

// Catches a floating window keeping the theme it opened with: a system
// theme change repaints the detached terminal in the new theme's editor
// surface, both ways.
#[test]
fn dock_theme_changes_reach_floating_panels() {
    let mut ui = harness(ScenarioKind::Review);
    let window = move_to_new_window(&mut ui, "Terminal");
    let (light, dark) = design::themes_for(ThemeChoice::System);

    let mut seen = Vec::new();
    for system in [SystemTheme::Light, SystemTheme::Dark] {
        ui.app_event(AppEvent::ThemeChanged(system));
        seen.push(terminal_background(&mut ui, window));
    }

    assert_eq!(
        seen,
        [
            Some(light.colors.editor_surface),
            Some(dark.colors.editor_surface)
        ]
    );
}

// Catches a panel command that shows the dock behind the shell's back:
// after Mod+Alt+B hides the dock, Mod+J brings the terminal back, and a
// single Mod+Alt+B hides it again.
#[test]
fn dock_terminal_command_keeps_the_dock_toggle_in_step() {
    let mut ui = harness(ScenarioKind::Review);
    ui.key("mod+alt+b");
    let hidden = ui.try_find(tab("Terminal")).is_none();
    ui.key("mod+j");
    let shown = ui.try_find(tab("Terminal")).is_some();
    ui.key("mod+alt+b");
    let hidden_again = ui.try_find(tab("Terminal")).is_none();

    assert_eq!((hidden, shown, hidden_again), (true, true, true));
}

/// Whether `primitives` (cached chunks included) paint an image with any
/// visible pixel whose center lies in `area`.
fn paints_icon_in(primitives: &[Primitive], offset: [f32; 2], area: quark::Rect) -> bool {
    primitives.iter().any(|p| match p {
        Primitive::Image(image) => {
            let r = image.rect.offset(offset[0], offset[1]);
            area.contains(r.x + r.width / 2.0, r.y + r.height / 2.0)
                && image.rgba.as_chunks::<4>().0.iter().any(|px| px[3] > 0)
        }
        Primitive::Chunk(chunk) => {
            let at = [offset[0] + chunk.offset[0], offset[1] + chunk.offset[1]];
            paints_icon_in(chunk.chunk.primitives(), at, area)
        }
        _ => false,
    })
}

/// Whether the close button of the dock tab `name` shows its icon.
fn close_shown(ui: &UiTestHarness<Workbench>, name: &str) -> bool {
    let close = ui
        .find_all(By::role(Role::Button))
        .into_iter()
        .find(|n| {
            n.name
                .as_deref()
                .is_some_and(|label| label.starts_with("Close") && label.contains(name))
        })
        .expect("close button");
    paints_icon_in(&ui.scene().primitives, [0.0, 0.0], close.bounds)
}

// Catches the workbench's tab strip losing its hover-only close buttons:
// an inactive tab hides its close icon until the pointer is over the tab
// (the button stays in the accessibility tree), and the active tab always
// shows its own.
#[test]
fn dock_inactive_tab_shows_close_only_on_hover() {
    let mut ui = harness(ScenarioKind::Review);
    ui.pointer_move((10.0, 10.0));
    let resting = (close_shown(&ui, "Terminal"), close_shown(&ui, "Diff"));
    ui.pointer_move(ui.find(tab("Terminal")).center());
    let hovered = close_shown(&ui, "Terminal");

    assert_eq!((resting, hovered), ((false, true), true));
}
