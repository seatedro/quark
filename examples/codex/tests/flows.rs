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
    assert!(
        ui.painted_text().contains("declined."),
        "{}",
        ui.painted_text()
    );
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
    assert!(
        ui.find_all(By::role_name(Role::Button, "Archive chat"))
            .is_empty()
    );
    let row = ui
        .find(By::role_name(
            Role::ListItem,
            "Design self-improving intent layer",
        ))
        .bounds;
    ui.pointer_move((row.x + 40.0, row.y + row.height / 2.0));
    ui.frame();
    let archive = ui.find_all(By::role_name(Role::Button, "Archive chat"));
    assert_eq!(archive.len(), 1);
    let b = archive[0].bounds;
    assert!(
        b.x > row.x + row.width / 2.0 && b.x + b.width <= row.x + row.width,
        "{b:?} in {row:?}"
    );
    assert!(
        b.y >= row.y && b.y + b.height <= row.y + row.height,
        "{b:?} in {row:?}"
    );
}

// Catches a popover that lets the pointer through: with the profile menu
// open over the chat list, the row beneath the pointer must not hover and
// reveal its Pin and Archive buttons through the menu.
#[test]
fn an_open_menu_keeps_the_rows_beneath_it_from_hovering() {
    let mut ui = harness("file-change");
    ui.click_node(By::role_name(Role::Button, "Open profile menu"));
    ui.frame();
    let menu = ui.find(By::role(Role::Menu)).bounds;
    let inside = |x: f32, y: f32| {
        x > menu.x && x < menu.x + menu.width && y > menu.y && y < menu.y + menu.height
    };
    let under = ui
        .find_all(By::role(Role::ListItem))
        .into_iter()
        .map(|n| (n.bounds.x + 40.0, n.bounds.y + n.bounds.height / 2.0))
        .find(|&(x, y)| inside(x, y))
        .expect("a chat row under the menu");
    ui.pointer_move(under);
    ui.frame();
    assert!(
        ui.find_all(By::role_name(Role::Button, "Archive chat"))
            .is_empty()
    );
}

/// The diff lines a diff view shows (its list items), top to bottom.
fn diff_lines(ui: &UiTestHarness<Codex>, view: &str) -> Vec<(String, f32)> {
    let Some(list) = ui.try_find(By::id(view)) else {
        return Vec::new();
    };
    let b = list.bounds;
    ui.find_all(By::role(Role::ListItem))
        .into_iter()
        // Unwrapped lines are as wide as the widest line: their left edge
        // says which view holds them.
        .filter(|n| {
            let (x, y) = (n.bounds.x, n.bounds.y + 1.0);
            x >= b.x && x < b.x + b.width && y >= b.y && y < b.y + b.height
        })
        .map(|n| (n.name.unwrap_or_default(), n.bounds.y))
        .collect()
}

fn names(lines: &[(String, f32)]) -> Vec<&str> {
    lines.iter().map(|(name, _)| name.as_str()).collect()
}

const INLINE: &str = "codex.diff.inline";
const REVIEW: &str = "codex.diff.review";

// Catches the inline card and the Changes panel drawing different
// sources: both are the shared viewer over one snapshot, so the narrow
// (unified) panel shows exactly the card's lines and counts.
#[test]
fn the_inline_card_and_the_panel_show_the_same_change() {
    let ui = harness("changes");
    let inline = diff_lines(&ui, INLINE);
    let review = diff_lines(&ui, REVIEW);
    assert_eq!(
        names(&inline),
        [
            "export function subtotal(items) {",
            "  return items.reduce((sum, item) => sum + item.price, 0);",
            "  return items.reduce((sum, item) => sum + item.price * item.qty, 0);",
            "}",
            "export function applyDiscount(total, percent) {",
            "  return total - percent;",
            "  return total - total * percent / 100;",
            "}",
        ]
    );
    assert_eq!(names(&review), names(&inline));
    // The card header, the scope pill, and the file header count alike.
    assert_eq!(ui.find_all(By::name("+2")).len(), 3);
    assert_eq!(ui.find_all(By::name("-2")).len(), 3);
}

// Catches the card's copy button doing nothing: it puts the change on
// the clipboard as a patch.
#[test]
fn the_inline_card_copies_its_patch() {
    let mut ui = harness("diff-open");
    ui.click_node(By::role_name(Role::Button, "Copy diff"));
    let patch = ui.clipboard_text().unwrap_or_default();
    assert!(patch.contains("-  return total - percent;\n"), "{patch}");
    assert!(
        patch.contains("+  return total - total * percent / 100;\n"),
        "{patch}"
    );
}

// Catches a preview that hides rows without a way to the rest: Open full
// diff opens the Changes tab, which shows the lines the card left out.
#[test]
fn open_full_diff_opens_the_changes_tab_at_the_hidden_rows() {
    let mut ui = harness("diff-open");
    let old: String = (1..=30).map(|i| format!("const v{i} = {i};\n")).collect();
    let new = ["3", "9", "15", "21", "27"]
        .iter()
        .fold(old.clone(), |s, n| {
            s.replace(&format!(" = {n};"), &format!(" = {n}0;"))
        });
    ui.app_mut().changes = quark_codex::diff::Changes::new("big.js", &old, &new);
    // Tall enough that the whole card clears the composer.
    ui.resize(984.0, 1400.0);
    for _ in 0..3 {
        ui.frame();
    }
    let shown = diff_lines(&ui, INLINE);
    assert!(!shown.iter().any(|(n, _)| n == "const v27 = 270;"));
    assert!(ui.try_find(By::role_name(Role::Tab, "Changes")).is_none());

    ui.click_node(By::role_name(Role::Button, "Open full diff"));
    ui.frame();

    assert!(ui.try_find(By::role_name(Role::Tab, "Changes")).is_some());
    let review = diff_lines(&ui, REVIEW);
    assert!(
        review.iter().any(|(n, _)| n == "const v27 = 270;"),
        "{review:?}"
    );
}

// Catches the find control being decorative: typing searches the new
// side of the file ("subtotal" counts too), and Enter steps to the next
// match.
#[test]
fn find_counts_matches_and_steps_with_enter() {
    let mut ui = harness("changes");
    ui.click_node(By::role_name(Role::Button, "Find in changes"));
    ui.frame();
    ui.type_text("total");
    ui.frame();
    assert!(ui.try_find(By::role_name(Role::Status, "1 of 4")).is_some());
    ui.key("enter");
    ui.frame();
    assert!(ui.try_find(By::role_name(Role::Status, "2 of 4")).is_some());
}

// Catches a fold bar that cannot be opened: clicking it shows the line
// it hid.
#[test]
fn a_fold_bar_reveals_its_hidden_line() {
    let mut ui = harness("changes");
    let hidden = "// Shopping cart helpers.";
    assert!(!names(&diff_lines(&ui, REVIEW)).contains(&hidden));
    let first = ui.find_all(By::role_name(Role::Button, "Show 1 unmodified line"))[0].center();
    ui.click(first);
    ui.frame();
    assert_eq!(names(&diff_lines(&ui, REVIEW))[0], hidden);
}

// Catches Word wrap leaving long lines clipped: the panel's longest line
// wraps onto more rows.
#[test]
fn word_wrap_wraps_the_panels_long_lines() {
    let mut ui = harness("changes");
    let long = "  return items.reduce((sum, item) => sum + item.price * item.qty, 0);";
    let height = |ui: &UiTestHarness<Codex>| {
        let b = ui.find(By::id(REVIEW)).bounds;
        ui.find_all(By::role_name(Role::ListItem, long))
            .into_iter()
            .find(|n| n.bounds.x >= b.x)
            .unwrap()
            .bounds
            .height
    };
    let before = height(&ui);
    ui.click_node(By::role_name(Role::Button, "Options"));
    ui.frame();
    ui.click_node(By::role_name(Role::MenuItem, "Word wrap"));
    ui.frame();
    ui.frame();
    assert!(height(&ui) >= before * 2.0, "{before} -> {}", height(&ui));
}

// Catches a review thread that does not answer its controls or that
// moves the code around it: Reply adds a message and pushes only the
// lines below down; Resolve folds the thread to one line.
#[test]
fn a_comment_thread_replies_and_resolves_in_place() {
    let mut ui = harness("changes-comment");
    let y = |ui: &UiTestHarness<Codex>, name: &str| {
        diff_lines(ui, REVIEW)
            .into_iter()
            .find(|(n, _)| n == name)
            .unwrap()
            .1
    };
    let edited = "  return total - total * percent / 100;";
    let (above, below) = (y(&ui, edited), y(&ui, "}"));
    let below = diff_lines(&ui, REVIEW)
        .into_iter()
        .filter(|(n, _)| n == "}")
        .map(|(_, y)| y)
        .fold(below, f32::max);

    ui.click_node(By::role_name(Role::Button, "Reply"));
    ui.frame();
    assert!(
        ui.try_find(By::name(
            "Good catch: I'll clamp it and add a test for 0 and 100."
        ))
        .is_some()
    );
    let last_brace = |ui: &UiTestHarness<Codex>| {
        diff_lines(ui, REVIEW)
            .into_iter()
            .filter(|(n, _)| n == "}")
            .map(|(_, y)| y)
            .fold(f32::MIN, f32::max)
    };
    assert_eq!(y(&ui, edited), above);
    assert_eq!(last_brace(&ui), below + 42.0);

    ui.click_node(By::role_name(Role::Button, "Resolve"));
    ui.frame();
    assert!(
        ui.try_find(By::role_name(Role::Group, "Resolved comment on line 7"))
            .is_some()
    );
    assert!(last_brace(&ui) < below);
}

// Catches session diff views staying plain: with grammar packs, the
// Changes view colors JavaScript keywords. Skips without packs unless
// QUARK_REQUIRE_SYNTAX_PACKS=1.
#[test]
fn the_changes_view_highlights_javascript() {
    if quark_codex::grammar_store().is_none() {
        assert!(
            std::env::var_os("QUARK_REQUIRE_SYNTAX_PACKS").is_none(),
            "no grammar packs"
        );
        return;
    }
    use quark_app::quark_ui::quark_syntax::HighlightKind;
    let mut ui = harness("changes");
    ui.app_mut().changes.review.finish_syntax();
    ui.frame();
    let frame = ui.app().changes.review.frame().unwrap().clone();
    let export = frame
        .rows
        .iter()
        .filter_map(|r| r.paint.sides[1].as_ref())
        .find(|l| l.layout.text().starts_with("export function subtotal"))
        .unwrap();
    assert_eq!(export.tones.first(), Some(&HighlightKind::Keyword));
}

// Catches a material window that still paints the captured sidebar color
// over the material, or never asks for it: once the window has its
// material, the sidebar requests the Sidebar material over its own bounds.
#[test]
fn a_material_window_shows_the_sidebar_material() {
    let mut ui = harness("home");
    let main = ui.main_window();
    assert!(ui.window(main).material_regions().is_empty());
    ui.app_mut().vibrant = true;
    ui.frame();
    let sidebar = ui.find(By::role_name(Role::Navigation, "Sidebar")).bounds;
    let regions = ui.window(main).material_regions().to_vec();
    assert_eq!(regions.len(), 1);
    let (r, kind) = (regions[0].rect, regions[0].kind);
    assert_eq!(kind, quark::MaterialKind::Sidebar);
    // The card's clip may take the last point of the sidebar's height.
    assert!(
        (r.x, r.y, r.width) == (sidebar.x, sidebar.y, sidebar.width)
            && (r.height - sidebar.height).abs() <= 1.0,
        "{r:?} for {sidebar:?}"
    );
}
