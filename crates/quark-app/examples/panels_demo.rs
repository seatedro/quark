//! A T3 Code style workspace built from `Dock` and `Split`: a thread
//! sidebar on the left, the chat in the center, a right panel with Preview,
//! Terminal, Diff, Files, and Pull requests tabs, and a terminal drawer
//! along the bottom. Drag the dividers (double click one to reset it), or
//! focus one and use the arrow keys and Enter. Drag a tab along its strip to
//! reorder it, onto another strip or panel to move it there, or onto a
//! panel's edge to split it. The right panel is sealed: its tabs split and
//! reorder inside it but never leave, and other tabs cannot enter. Middle
//! click a tab to close it.
//! Mod+B toggles the sidebar, Mod+Alt+B the right panel, Mod+J the drawer.
//! The layout is saved to the system temp directory and restored on the
//! next launch. Escape quits.

use std::path::PathBuf;

use quark_app::quark_ui::Action;
use quark_app::quark_ui::element::{AnyElement, IntoAnyElement, div, text};
use quark_app::quark_ui::style::Styled;
use quark_app::quark_ui::theme::Theme;
use quark_app::winit::keyboard::NamedKey;
use quark_app::{InputEvent, UiApp, UiContext, ViewContext, WindowOptions};
use quark_components::{
    Dock, DockEvent, DockLayout, DockRegion, DockSnapshot, DockState, Pane, PanelId, TabPolicy,
};

const THREADS: PanelId = PanelId(1);
const CHAT: PanelId = PanelId(2);
const PREVIEW: PanelId = PanelId(10);
const TERMINAL: PanelId = PanelId(11);
const DIFF: PanelId = PanelId(12);
const FILES: PanelId = PanelId(13);
const PULL_REQUESTS: PanelId = PanelId(14);
const DRAWER: PanelId = PanelId(20);

const PANELS: [(PanelId, &str); 8] = [
    (THREADS, "Threads"),
    (CHAT, "Chat"),
    (PREVIEW, "Preview"),
    (TERMINAL, "Terminal"),
    (DIFF, "Diff"),
    (FILES, "Files"),
    (PULL_REQUESTS, "Pull requests"),
    (DRAWER, "Terminal drawer"),
];

fn title(id: PanelId) -> &'static str {
    PANELS
        .iter()
        .find(|(p, _)| *p == id)
        .map_or("Panel", |(_, t)| t)
}

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Dock(DockEvent),
}

impl From<Msg> for Action {
    fn from(msg: Msg) -> Self {
        Action::new(msg)
    }
}

struct PanelsDemo {
    dock: DockState,
    /// Where settled layout changes are saved; `None` in tests.
    save_to: Option<PathBuf>,
}

impl PanelsDemo {
    fn new() -> Self {
        let mut dock = DockState::new(DockLayout {
            left: Pane::fixed("Sidebar", 260.0).min(180.0).max(480.0),
            right: Pane::fixed("Right panel", 420.0).min(280.0).max(900.0),
            bottom: Pane::fixed("Drawer", 220.0).min(100.0).max(600.0),
            center_label: "Chat",
            center_min_width: 320.0,
            center_min_height: 160.0,
        });
        dock.open(DockRegion::Left, THREADS);
        dock.open(DockRegion::Center, CHAT);
        for panel in [PREVIEW, TERMINAL, DIFF, FILES, PULL_REQUESTS] {
            dock.open(DockRegion::Right, panel);
        }
        dock.open(DockRegion::Right, PREVIEW);
        dock.set_policy(DockRegion::Right, TabPolicy::SEALED);
        dock.open(DockRegion::Bottom, DRAWER);
        dock.set_visible(DockRegion::Bottom, false);
        Self {
            dock,
            save_to: None,
        }
    }

    fn restore(&mut self, snapshot: &DockSnapshot) {
        self.dock
            .restore(snapshot, |id| PANELS.iter().any(|(p, _)| *p == id));
    }

    fn content(id: PanelId, (width, height): (f32, f32), theme: &Theme) -> AnyElement {
        let colors = &theme.colors;
        let body = match id {
            THREADS => "Fix flaky login test\nAdd dark mode\nUpgrade dependencies",
            CHAT => "Ask anything about this repository.",
            PREVIEW => "localhost:5173",
            TERMINAL => "$ cargo test",
            DIFF => "3 files changed, 42 insertions(+), 7 deletions(-)",
            FILES => "src/\nCargo.toml\nREADME.md",
            PULL_REQUESTS => "#42 Add panels demo",
            _ => "$ ",
        };
        div()
            .w(width)
            .h(height)
            .p(12.0)
            .gap(6.0)
            .flex_col()
            .child(text(title(id)).semibold().color(colors.text_strong))
            .children_from(
                body.lines()
                    .map(|line| text(line).text_sm().color(colors.text_muted)),
            )
            .into_any()
    }
}

impl UiApp for PanelsDemo {
    type Action = Msg;
    type Message = ();

    fn view(&mut self, cx: &mut ViewContext) -> AnyElement {
        let size = cx.frame.size();
        let theme = cx.theme;
        Dock::new(&self.dock, size, |e| Msg::Dock(e).into())
            .toggle_key(DockRegion::Left, "mod+b")
            .toggle_key(DockRegion::Right, "mod+alt+b")
            .toggle_key(DockRegion::Bottom, "mod+j")
            .always_show_tabs(DockRegion::Left)
            .always_show_tabs(DockRegion::Center)
            .always_show_tabs(DockRegion::Right)
            .always_show_tabs(DockRegion::Bottom)
            .build(
                theme,
                |id| title(id).to_owned(),
                |id, size| Self::content(id, size, theme),
            )
    }

    fn update(&mut self, msg: Msg, cx: &mut UiContext) {
        let Msg::Dock(event) = msg;
        let now_ms = cx.window.elapsed().as_millis() as u64;
        if self.dock.apply(event, now_ms)
            && let Some(path) = &self.save_to
            && let Ok(json) = serde_json::to_string(&self.dock.snapshot())
        {
            let _ = std::fs::write(path, json);
        }
    }

    fn event(&mut self, event: &InputEvent, cx: &mut UiContext) -> bool {
        match event {
            InputEvent::KeyPress(chord) if chord.named() == Some(NamedKey::Escape) => {
                cx.window.exit();
                true
            }
            _ => false,
        }
    }
}

fn main() -> Result<(), quark_app::RunError> {
    let path = std::env::temp_dir().join("quark-panels-demo.json");
    let mut app = PanelsDemo::new();
    if let Some(snapshot) = std::fs::read_to_string(&path)
        .ok()
        .and_then(|json| serde_json::from_str(&json).ok())
    {
        app.restore(&snapshot);
    }
    app.save_to = Some(path);
    quark_app::run_ui(
        app,
        WindowOptions {
            title: "Quark Panels".into(),
            size: (1200.0, 760.0),
            ..WindowOptions::default()
        },
    )
}

/// The dock driven as a user would: dragging dividers and tabs, keys on
/// focused dividers and tabs, and the published accessibility tree.
#[cfg(test)]
mod tests {
    use accesskit::Role;
    use quark_app::testing::{By, UiTestHarness};

    use super::*;

    const SIZE: (f32, f32) = (1200.0, 760.0);

    fn harness() -> UiTestHarness<PanelsDemo> {
        UiTestHarness::new(PanelsDemo::new(), SIZE, 1.0)
    }

    fn divider(label: &str) -> By {
        By::role_name(Role::Splitter, format!("Resize {label}"))
    }

    /// The size a divider announces, or `None` when it is not shown.
    fn announced(ui: &UiTestHarness<PanelsDemo>, label: &str) -> Option<String> {
        ui.try_find(divider(label)).and_then(|n| n.value)
    }

    /// Where a divider's line is along x: the right edge of the sidebar or
    /// the left edge of the right panel.
    fn divider_x(ui: &UiTestHarness<PanelsDemo>, label: &str) -> f32 {
        ui.find(divider(label)).center().0.floor()
    }

    fn drag_by(ui: &mut UiTestHarness<PanelsDemo>, label: &str, dx: f32) {
        let (x, y) = ui.find(divider(label)).center();
        ui.drag((x, y), (x + dx, y));
    }

    /// The right panel's tabs, in order.
    fn tab_names(ui: &UiTestHarness<PanelsDemo>) -> Vec<String> {
        ui.find_all(By::role(Role::Tab))
            .into_iter()
            .filter_map(|n| n.name)
            .filter(|name| !matches!(name.as_str(), "Threads" | "Chat"))
            .collect()
    }

    /// Lines of the accessibility tree with dock roles, to compare layouts.
    fn dock_tree(ui: &UiTestHarness<PanelsDemo>) -> String {
        ui.accessibility_tree()
            .lines()
            .filter(|l| {
                let role = l.trim_start().split(' ').next().unwrap_or_default();
                matches!(role, "Splitter" | "TabList" | "Tab" | "TabPanel")
            })
            .map(|l| format!("{}\n", l.trim_end_matches(" [focused]")))
            .collect()
    }

    #[test]
    fn dragging_a_divider_resizes_within_its_limits() {
        // (divider, drag dx, expected announced size). The sidebar is
        // 180..=480 and collapses below 90; the right panel is 280..=900.
        // With the center at its 320, either pushes the other side down to
        // its minimum: the sidebar reaches its 480 maximum, and the right
        // panel stops at 1200 - 2 dividers - 320 - 180 = 698.
        let cases: &[(&str, f32, Option<&str>)] = &[
            ("Sidebar", 100.0, Some("360")),
            ("Sidebar", 1000.0, Some("480")),
            ("Sidebar", -60.0, Some("200")),
            ("Sidebar", -150.0, Some("180")),
            ("Sidebar", -200.0, None),
            ("Right panel", -100.0, Some("520")),
            ("Right panel", -1000.0, Some("698")),
            ("Right panel", 200.0, Some("280")),
        ];
        for &(label, dx, expected) in cases {
            let mut ui = harness();
            drag_by(&mut ui, label, dx);
            assert_eq!(
                announced(&ui, label).as_deref(),
                expected,
                "{label} dragged by {dx}"
            );
        }
    }

    #[test]
    fn the_divider_line_follows_the_drag() {
        let mut ui = harness();
        assert_eq!(divider_x(&ui, "Sidebar"), 260.0);
        drag_by(&mut ui, "Sidebar", 75.0);
        assert_eq!(divider_x(&ui, "Sidebar"), 335.0);
        assert_eq!(
            ui.find(By::role_name(Role::TabPanel, "Chat")).bounds.x,
            336.0
        );
    }

    #[test]
    fn a_collapsed_region_restores_to_its_last_size() {
        let mut ui = harness();
        drag_by(&mut ui, "Sidebar", 140.0);
        // Past the double click window, so the click does not reset it.
        ui.advance(1_000);
        ui.click_node(divider("Sidebar"));
        ui.key("enter");
        assert_eq!(announced(&ui, "Sidebar"), None);
        assert!(
            ui.try_find(By::role_name(Role::TabPanel, "Threads"))
                .is_none()
        );

        ui.key("mod+b");
        assert_eq!(announced(&ui, "Sidebar").as_deref(), Some("400"));
        assert_eq!(divider_x(&ui, "Sidebar"), 400.0);
    }

    #[test]
    fn toggle_keys_hide_and_show_regions() {
        let mut ui = harness();
        assert!(
            ui.try_find(By::role_name(Role::TabPanel, "Terminal drawer"))
                .is_none()
        );
        ui.key("mod+j");
        assert_eq!(announced(&ui, "Drawer").as_deref(), Some("220"));
        ui.key("mod+alt+b");
        assert!(ui.try_find(By::role(Role::TabList)).is_none());
        assert_eq!(
            ui.find(By::role_name(Role::TabPanel, "Chat")).bounds.width,
            SIZE.0 - 260.0 - 1.0
        );
    }

    #[test]
    fn arrow_keys_resize_the_focused_divider() {
        let mut ui = harness();
        ui.click_node(divider("Sidebar"));
        ui.key("right");
        assert_eq!(announced(&ui, "Sidebar").as_deref(), Some("270"));
        ui.key("shift+right");
        assert_eq!(announced(&ui, "Sidebar").as_deref(), Some("320"));
        ui.key("left");
        assert_eq!(announced(&ui, "Sidebar").as_deref(), Some("310"));

        // The right panel grows leftward.
        ui.click_node(divider("Right panel"));
        ui.key("left");
        assert_eq!(announced(&ui, "Right panel").as_deref(), Some("430"));
    }

    #[test]
    fn double_clicking_a_divider_resets_it() {
        let mut ui = harness();
        drag_by(&mut ui, "Sidebar", 120.0);
        ui.advance(1_000);
        ui.click_node(divider("Sidebar"));
        assert_eq!(announced(&ui, "Sidebar").as_deref(), Some("380"));
        ui.click_node(divider("Sidebar"));
        assert_eq!(announced(&ui, "Sidebar").as_deref(), Some("260"));
    }

    #[test]
    fn a_saved_layout_restores_the_same_dock() {
        let mut ui = harness();
        drag_by(&mut ui, "Sidebar", 90.0);
        drag_by(&mut ui, "Right panel", -60.0);
        ui.key("mod+j");
        ui.click_node(By::role_name(Role::Tab, "Diff"));
        ui.click_node(By::role_name(Role::Button, "Close Files"));
        let json = serde_json::to_string(&ui.app().dock.snapshot()).unwrap();

        let snapshot: DockSnapshot = serde_json::from_str(&json).unwrap();
        let mut app = PanelsDemo::new();
        app.restore(&snapshot);
        let restored = UiTestHarness::new(app, SIZE, 1.0);
        assert_eq!(dock_tree(&restored), dock_tree(&ui));
    }

    #[test]
    fn clicking_a_tab_shows_its_panel() {
        let mut ui = harness();
        ui.click_node(By::role_name(Role::Tab, "Diff"));
        ui.find(By::role_name(Role::TabPanel, "Diff"));
        assert!(
            ui.try_find(By::role_name(Role::TabPanel, "Preview"))
                .is_none()
        );
    }

    #[test]
    fn dragging_a_tab_reorders_the_strip() {
        let mut ui = harness();
        let (x, y) = ui.find(By::role_name(Role::Tab, "Preview")).center();
        // Five tabs share the 420 wide strip: 84 each, so two slots.
        ui.drag((x, y), (x + 170.0, y));
        assert_eq!(
            tab_names(&ui),
            ["Terminal", "Diff", "Preview", "Files", "Pull requests"]
        );
        // The dragged tab is the selected one.
        ui.find(By::role_name(Role::TabPanel, "Preview"));
    }

    #[test]
    fn arrow_keys_move_selection_and_focus_along_the_tabs() {
        let mut ui = harness();
        ui.click_node(By::role_name(Role::Tab, "Preview"));
        ui.key("left");
        ui.find(By::role_name(Role::TabPanel, "Pull requests"));
        assert_eq!(
            ui.focused().and_then(|n| n.name).as_deref(),
            Some("Pull requests")
        );
        ui.key("home");
        ui.find(By::role_name(Role::TabPanel, "Preview"));
    }

    #[test]
    fn a_middle_click_closes_a_tab() {
        let mut ui = harness();
        ui.middle_click_node(By::role_name(Role::Tab, "Diff"));
        assert_eq!(
            tab_names(&ui),
            ["Preview", "Terminal", "Files", "Pull requests"]
        );
    }

    fn tab_center(ui: &UiTestHarness<PanelsDemo>, name: &str) -> (f32, f32) {
        ui.find(By::role_name(Role::Tab, name)).center()
    }

    #[test]
    fn a_tab_dragged_onto_an_edge_previews_then_splits_the_panel() {
        let mut ui = harness();
        let chat = ui.find(By::role_name(Role::TabPanel, "Chat")).bounds;
        let edge = (chat.x + chat.width - 20.0, chat.y + chat.height / 2.0);
        ui.pointer_down(tab_center(&ui, "Threads"));
        ui.pointer_move(edge);
        // The preview covers the half of Chat the new group would take.
        let preview = ui.find(By::test_id("dock-drop-preview")).bounds;
        assert_eq!(
            (preview.x, preview.y, preview.width, preview.height),
            (
                chat.x + (chat.width / 2.0).ceil(),
                chat.y,
                (chat.width / 2.0).floor(),
                chat.height
            )
        );
        ui.pointer_up(edge);

        assert!(ui.try_find(By::test_id("dock-drop-preview")).is_none());
        let chat_after = ui.find(By::role_name(Role::TabPanel, "Chat")).bounds;
        let threads = ui.find(By::role_name(Role::TabPanel, "Threads")).bounds;
        assert_eq!(chat_after.y, threads.y);
        assert!(threads.x > chat_after.x + chat_after.width, "{threads:?}");
        // The emptied sidebar hides.
        assert_eq!(announced(&ui, "Sidebar"), None);
    }

    #[test]
    fn right_panel_tabs_split_inside_it_but_never_leave() {
        let mut ui = harness();
        let chat = ui.find(By::role_name(Role::TabPanel, "Chat")).center();
        ui.pointer_down(tab_center(&ui, "Diff"));
        ui.pointer_move(chat);
        assert!(ui.try_find(By::test_id("dock-drop-preview")).is_none());
        ui.pointer_up(chat);
        assert!(ui.try_find(By::role_name(Role::TabPanel, "Diff")).is_some());
        assert_eq!(
            tab_names(&ui),
            ["Preview", "Terminal", "Diff", "Files", "Pull requests"]
        );

        // Nor does it take tabs from elsewhere.
        let strip = tab_center(&ui, "Files");
        ui.drag(tab_center(&ui, "Threads"), strip);
        assert_eq!(announced(&ui, "Sidebar").as_deref(), Some("260"));

        // Its own bottom edge splits it.
        let panel = ui.find(By::role_name(Role::TabPanel, "Diff")).bounds;
        ui.drag(
            tab_center(&ui, "Diff"),
            (panel.x + panel.width / 2.0, panel.y + panel.height - 20.0),
        );
        let top = ui.find(By::role_name(Role::TabPanel, "Files")).bounds;
        let bottom = ui.find(By::role_name(Role::TabPanel, "Diff")).bounds;
        assert_eq!((top.x, top.width), (bottom.x, bottom.width));
        assert!(bottom.y > top.y + top.height, "{top:?} over {bottom:?}");
    }

    #[test]
    fn closing_the_last_tab_hides_the_region() {
        let mut ui = harness();
        for name in ["Preview", "Terminal", "Diff", "Files", "Pull requests"] {
            ui.click_node(By::role_name(Role::Button, format!("Close {name}")));
        }
        assert!(ui.try_find(By::role(Role::TabList)).is_none());
        assert_eq!(announced(&ui, "Right panel"), None);
    }

    #[test]
    fn the_dock_publishes_tab_and_separator_roles() {
        let ui = harness();
        assert_eq!(
            dock_tree(&ui),
            r#"
  TabList "Sidebar" @dock-tabs
    Tab "Threads" @dock-tab
  TabPanel "Threads"
  Splitter "Resize Sidebar" = "260" @split-divider
  TabList "Chat" @dock-tabs
    Tab "Chat" @dock-tab
  TabPanel "Chat"
  Splitter "Resize Right panel" = "420" @split-divider
  TabList "Right panel" @dock-tabs
    Tab "Preview" @dock-tab
    Tab "Terminal" @dock-tab
    Tab "Diff" @dock-tab
    Tab "Files" @dock-tab
    Tab "Pull requests" @dock-tab
  TabPanel "Preview"
"#
            .trim_start_matches('\n')
        );
    }
}
