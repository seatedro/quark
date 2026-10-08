//! A T3 Code style workspace built from `Dock` and `Split`: a thread
//! sidebar on the left, the chat in the center, a right panel with Preview,
//! Terminal, Diff, Files, and Pull requests tabs, and a terminal drawer
//! along the bottom. Drag the dividers (double click one to reset it), or
//! focus one and use the arrow keys and Enter. Drag a tab along its strip to
//! reorder it, onto another strip or panel to move it there, or onto a
//! panel's edge to split it. The right panel is sealed: its tabs split and
//! reorder inside it but never leave, and other tabs cannot enter. Middle
//! click a tab to close it.
//! On a focused tab, Mod+Shift+Page Up and Page Down move it into the
//! previous or next group that takes it, and Shift+F10 opens a menu under
//! it to move it to any such group or split its group with it. The
//! dividers between split groups move with the arrow keys too. The
//! terminal drawer's sessions are a `TabBar` whose tabs close by their
//! close button, a middle click, or Delete.
//! Mod+B toggles the sidebar, Mod+Alt+B the right panel, Mod+J the drawer.
//! The layout is saved to the system temp directory and restored on the
//! next launch. Escape cancels a tab drag, else closes the menu, else
//! quits.

use std::path::PathBuf;

use quark::view;
use quark_app::quark_ui::Action;
use quark_app::quark_ui::accessibility::Politeness;
use quark_app::quark_ui::element::{AnyElement, Binding, IntoAnyElement, NoopAction, div, text};
use quark_app::quark_ui::style::Styled;
use quark_app::quark_ui::theme::Theme;
use quark_app::winit::keyboard::NamedKey;
use quark_app::{InputEvent, UiApp, UiContext, ViewContext, WindowOptions};
use quark_components::{
    ContextMenuEntry, ContextMenuOutcome, ContextMenuState, Dock, DockEvent, DockLayout,
    DockRegion, DockSnapshot, DockState, DropZone, Pane, PaneDrop, PaneId, PanelId, TabItem,
    TabPolicy, tab_bar,
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

/// The terminal drawer's sessions, as `(id, name)`.
const SESSIONS: [(u32, &str); 3] = [(1, "zsh"), (2, "cargo watch"), (3, "dev server")];

fn session_key(id: u32) -> String {
    format!("session:{id}")
}

/// Space between a tab and the menu opened under it.
const MENU_GAP: f32 = 4.0;

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Dock(DockEvent),
    /// Split the panel's group, putting the panel in a new group on
    /// this side: a drop on the group's edge, from the keyboard.
    SplitTab(PanelId, DropZone),
    SelectSession(u32),
    CloseSession(u32),
}

impl From<Msg> for Action {
    fn from(msg: Msg) -> Self {
        Action::new(msg)
    }
}

struct PanelsDemo {
    dock: DockState,
    /// The "Move to group" menu.
    menu: ContextMenuState,
    sessions: Vec<(u32, &'static str)>,
    session: u32,
    /// The window's size as of the last frame, to place the menu.
    size: (f32, f32),
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
            menu: ContextMenuState::default(),
            sessions: SESSIONS.to_vec(),
            session: SESSIONS[0].0,
            size: (0.0, 0.0),
            save_to: None,
        }
    }

    /// A group's name for the menu and announcements: its region's label,
    /// numbered when the region is split.
    fn group_name(&self, pane: PaneId) -> String {
        DockRegion::ALL
            .into_iter()
            .find_map(|region| {
                let groups = self.dock.root(region).groups();
                let at = groups.iter().position(|g| g.id == pane)?;
                let label = self.dock.label(region);
                Some(if groups.len() == 1 {
                    label.to_owned()
                } else {
                    format!("{label} {}", at + 1)
                })
            })
            .unwrap_or_default()
    }

    /// Open the "Move to group" menu for `panel` under its tab, listing
    /// every group the dock lets it move into and the splits of its own
    /// group it can make.
    fn open_move_menu(&mut self, panel: PanelId, cx: &UiContext) {
        let mut entries: Vec<ContextMenuEntry> = self
            .dock
            .move_targets(panel)
            .into_iter()
            .map(|destination| {
                let event = DockEvent::MoveTab {
                    panel,
                    destination,
                    index: None,
                };
                ContextMenuEntry::item(
                    format!("Move to {}", self.group_name(destination)),
                    Msg::Dock(event),
                )
            })
            .collect();
        if let Some((_, pane, _)) = self.dock.locate(panel) {
            for (zone, label) in [
                (DropZone::Right, "Split right"),
                (DropZone::Bottom, "Split down"),
            ] {
                if self.dock.can_drop(panel, PaneDrop { pane, zone }) {
                    entries.push(ContextMenuEntry::item(label, Msg::SplitTab(panel, zone)));
                }
            }
        }
        if entries.is_empty() {
            entries.push(ContextMenuEntry::item("No group takes this tab", NoopAction).disabled());
        }
        // Under the tab, wherever the last frame put it; before any frame,
        // near the top of the window.
        let (x, y) = cx.geometry().by_id(&Dock::tab_id(panel)).map_or(
            ((self.size.0 / 2.0 - 110.0).max(0.0), self.size.1 / 4.0),
            |tab| (tab.bounds.x, tab.bounds.bottom() + MENU_GAP),
        );
        self.menu.open(entries, x, y);
    }

    fn split_tab(&mut self, panel: PanelId, zone: DropZone, cx: &mut UiContext) {
        self.menu.close();
        let Some((_, pane, _)) = self.dock.locate(panel) else {
            return;
        };
        if self.dock.drop_panel(panel, PaneDrop { pane, zone }) {
            // The tab's new group, the region's most recent.
            let pane = self.dock.locate(panel).map(|(_, pane, _)| pane);
            cx.set_focus(pane.map(Dock::tab_focus));
            self.save();
        }
    }

    fn save(&self) {
        if let Some(path) = &self.save_to
            && let Ok(json) = serde_json::to_string(&self.dock.snapshot())
        {
            let _ = std::fs::write(path, json);
        }
    }

    fn close_session(&mut self, id: u32, cx: &mut UiContext) {
        let Some(at) = self.sessions.iter().position(|(s, _)| *s == id) else {
            return;
        };
        let key = session_key(id);
        let had_focus = [TabItem::focus_id(&key), TabItem::close_focus_id(&key)]
            .into_iter()
            .any(|f| cx.focus() == Some(f));
        self.sessions.remove(at);
        // The session after it takes over, or the one before at the end.
        if self.session == id
            && let Some((next, _)) = self.sessions.get(at).or(self.sessions.last())
        {
            self.session = *next;
        }
        if had_focus {
            let next = self.sessions.iter().find(|(s, _)| *s == self.session);
            cx.set_focus(next.map(|(s, _)| TabItem::focus_id(&session_key(*s))));
        }
    }

    fn drawer(&self, (width, height): (f32, f32), theme: &Theme) -> AnyElement {
        let colors = &theme.colors;
        let tabs = self
            .sessions
            .iter()
            .map(|&(id, name)| {
                TabItem::new(name, Msg::SelectSession(id))
                    .id(session_key(id))
                    .active(id == self.session)
                    .on_close(Msg::CloseSession(id))
            })
            .collect();
        let current = self
            .sessions
            .iter()
            .find(|(s, _)| *s == self.session)
            .map_or("No session", |(_, name)| name);
        view! {
            <div w={width} h={height} class="flex-col">
                {tab_bar(tabs)}
                <div class="p-3 gap-[6] flex-col">
                    <text class="font-semibold" color={colors.text_strong}>{current}</text>
                    <text class="text-sm" color={colors.text_muted}>"$ cargo test"</text>
                </div>
            </div>
        }
    }

    fn run(&mut self, action: Action, cx: &mut UiContext) {
        if let Some(msg) = action.downcast_ref::<Msg>().cloned() {
            self.update(msg, cx);
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
        view! {
            <div w={width} h={height} class="p-3 gap-[6] flex-col">
                <text class="font-semibold" color={colors.text_strong}>{title(id)}</text>
                for line in body.lines() {
                    <text class="text-sm" color={colors.text_muted}>{line}</text>
                }
            </div>
        }
    }
}

impl UiApp for PanelsDemo {
    type Action = Msg;
    type Message = ();

    fn view(&mut self, cx: &mut ViewContext) -> AnyElement {
        let size = cx.frame.size();
        self.size = size;
        let theme = cx.theme;
        let dock = Dock::new(&self.dock, size, |e| Msg::Dock(e).into())
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
                |id, size| match id {
                    DRAWER => self.drawer(size, theme),
                    _ => Self::content(id, size, theme),
                },
            );
        let menu = self.menu.render(size, theme);
        view! {
            <div class="relative" w={size.0} h={size.1}>
                {dock}
                if let Some(menu) = menu {
                    {menu}
                }
            </div>
        }
    }

    fn update(&mut self, msg: Msg, cx: &mut UiContext) {
        let event = match msg {
            Msg::Dock(event) => event,
            Msg::SplitTab(panel, zone) => {
                self.split_tab(panel, zone, cx);
                return;
            }
            Msg::SelectSession(id) => {
                self.session = id;
                return;
            }
            Msg::CloseSession(id) => {
                self.close_session(id, cx);
                return;
            }
        };
        // A choice from the menu, or anything else done in the dock,
        // closes it.
        self.menu.close();
        let now_ms = cx.window.elapsed().as_millis() as u64;
        let outcome = self.dock.apply_event(event, now_ms);
        if let Some(focus) = outcome.focus() {
            cx.set_focus(Some(focus));
        }
        if let Some(moved) = outcome.moved {
            let to = self.group_name(moved.pane);
            cx.announce(
                format!("Moved {} to {to}", title(moved.panel)),
                Politeness::Polite,
            );
        }
        if outcome.settled {
            self.save();
        }
    }

    fn event(&mut self, event: &InputEvent, cx: &mut UiContext) -> bool {
        let InputEvent::KeyPress(chord) = event else {
            return false;
        };
        // A tab dragged over the dock goes back where it was.
        if chord.named() == Some(NamedKey::Escape) && cx.is_dragging() {
            cx.cancel_drag();
            return true;
        }
        if let Some(pressed) = chord.binding() {
            if let Some(outcome) = self.menu.handle_key(&pressed) {
                if let ContextMenuOutcome::Activate(action) = outcome {
                    self.run(action, cx);
                }
                cx.window.request_redraw();
                return true;
            }
            let menu_key: Binding = "shift+f10".parse().expect("valid binding");
            if menu_key.matches(&pressed)
                && let Some(panel) = self.dock.focused_panel(cx.focus())
            {
                self.open_move_menu(panel, cx);
                cx.window.request_redraw();
                return true;
            }
        }
        if chord.named() == Some(NamedKey::Escape) {
            cx.window.exit();
            return true;
        }
        false
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
    use quark::Rect;
    use quark_app::testing::{By, Node, UiTestHarness};

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

    /// What the app last asked assistive tech to speak.
    fn announcement(ui: &UiTestHarness<PanelsDemo>) -> Option<String> {
        ui.accessibility_update()
            .nodes
            .iter()
            .find(|(_, node)| node.role() == Role::Status)
            .and_then(|(_, node)| node.label().map(str::to_owned))
    }

    fn focused_name(ui: &UiTestHarness<PanelsDemo>) -> Option<String> {
        ui.focused().and_then(|n| n.name)
    }

    /// The tabs of the strip named `list`, in order.
    fn strip(ui: &UiTestHarness<PanelsDemo>, list: &str) -> Vec<String> {
        let tree = dock_tree(ui);
        let header = format!("TabList \"{list}\"");
        let mut lines = tree
            .lines()
            .skip_while(|l| !l.trim_start().starts_with(&header));
        let depth = |l: &str| l.len() - l.trim_start().len();
        let Some(first) = lines.next() else {
            return Vec::new();
        };
        let below = depth(first);
        lines
            .take_while(|l| depth(l) > below)
            .filter_map(|l| l.trim_start().strip_prefix("Tab \""))
            .filter_map(|l| l.split('"').next().map(str::to_owned))
            .collect()
    }

    #[test]
    fn move_keys_move_the_focused_tab_to_the_next_group() {
        let mut ui = harness();
        // The first Tab stop is the sidebar's tab.
        ui.key("tab");
        ui.key("mod+shift+pagedown");
        assert_eq!(strip(&ui, "Chat"), ["Chat", "Threads"]);
        ui.find(By::role_name(Role::TabPanel, "Threads"));
        assert_eq!(focused_name(&ui).as_deref(), Some("Threads"));
        assert_eq!(announcement(&ui).as_deref(), Some("Moved Threads to Chat"));
    }

    #[test]
    fn a_refused_move_leaves_the_dock_and_focus() {
        // (active tab, key): the sidebar is first in tree order, and the
        // sealed right panel takes nothing and gives nothing.
        let cases = [
            ("Threads", "mod+shift+pageup"),
            ("Preview", "mod+shift+pagedown"),
            ("Preview", "mod+shift+pageup"),
        ];
        for (tab, key) in cases {
            let mut ui = harness();
            ui.click_node(By::role_name(Role::Tab, tab));
            let before = dock_tree(&ui);
            ui.key(key);
            assert_eq!(dock_tree(&ui), before, "{tab} {key}");
            assert_eq!(focused_name(&ui).as_deref(), Some(tab), "{tab} {key}");
            assert_eq!(announcement(&ui), None, "{tab} {key}");
        }
    }

    #[test]
    fn the_move_to_group_menu_moves_the_focused_tab() {
        let mut ui = harness();
        ui.key("mod+j");
        ui.click_node(By::role_name(Role::Tab, "Threads"));
        ui.key("shift+f10");
        let items: Vec<String> = ui
            .find_all(By::role(Role::MenuItem))
            .into_iter()
            .filter_map(|n| n.name)
            .collect();
        assert_eq!(items, ["Move to Chat", "Move to Drawer"]);

        ui.click_node(By::role_name(Role::MenuItem, "Move to Drawer"));
        assert!(ui.try_find(By::role(Role::Menu)).is_none());
        assert_eq!(strip(&ui, "Drawer"), ["Terminal drawer", "Threads"]);
        assert_eq!(focused_name(&ui).as_deref(), Some("Threads"));
        assert_eq!(
            announcement(&ui).as_deref(),
            Some("Moved Threads to Drawer")
        );
    }

    #[test]
    fn closing_the_focused_session_focuses_the_next_one() {
        let mut ui = harness();
        ui.key("mod+j");
        ui.click_node(By::role_name(Role::Tab, "zsh"));
        ui.key("delete");
        assert!(ui.try_find(By::role_name(Role::Tab, "zsh")).is_none());
        let states = quark_app::quark_ui::accessibility::dump_accessibility_states(
            &ui.accessibility_update(),
        );
        assert!(
            states.contains("| Tab | cargo watch | selected"),
            "{states}"
        );
        assert_eq!(focused_name(&ui).as_deref(), Some("cargo watch"));
    }

    #[test]
    fn clicking_an_inactive_tab_focuses_it() {
        let mut ui = harness();
        ui.click_node(By::role_name(Role::Tab, "Diff"));
        assert_eq!(focused_name(&ui).as_deref(), Some("Diff"));
        // Focus is the group's, so the arrow keys move on from it.
        ui.key("right");
        assert_eq!(focused_name(&ui).as_deref(), Some("Files"));
    }

    /// The published node with accessibility id `id`.
    fn ax_node(ui: &UiTestHarness<PanelsDemo>, id: &str) -> (accesskit::NodeId, accesskit::Node) {
        ui.accessibility_update()
            .nodes
            .into_iter()
            .find(|(_, node)| node.author_id() == Some(id))
            .unwrap_or_else(|| panic!("no node {id:?}:\n{}", ui.accessibility_tree()))
    }

    /// Deliver `action` from assistive tech to the node `id`.
    fn ax_action(
        ui: &mut UiTestHarness<PanelsDemo>,
        id: &str,
        action: accesskit::Action,
        data: Option<accesskit::ActionData>,
    ) {
        let (target_node, _) = ax_node(ui, id);
        ui.accessibility_action(accesskit::ActionRequest {
            action,
            target_tree: accesskit::TreeId::ROOT,
            target_node,
            data,
        });
    }

    #[test]
    fn assistive_tech_clicks_a_tab_to_select_and_focus_it() {
        let mut ui = harness();
        ax_action(
            &mut ui,
            &Dock::tab_id(FILES),
            accesskit::Action::Click,
            None,
        );
        ui.find(By::role_name(Role::TabPanel, "Files"));
        assert_eq!(focused_name(&ui).as_deref(), Some("Files"));
    }

    #[test]
    fn the_move_menu_opens_under_the_focused_tab() {
        let mut ui = harness();
        ui.click_node(By::role_name(Role::Tab, "Diff"));
        ui.key("shift+f10");
        let tab = ui.find(By::role_name(Role::Tab, "Diff")).bounds;
        let menu = ui.find(By::role(Role::Menu)).bounds;
        assert_eq!((menu.x, menu.y), (tab.x, tab.y + tab.height + MENU_GAP));
    }

    /// Split the right panel from the keyboard: Preview into a new group
    /// right of the other four tabs.
    fn split_right_panel(ui: &mut UiTestHarness<PanelsDemo>) {
        ui.click_node(By::role_name(Role::Tab, "Preview"));
        ui.key("shift+f10");
        ui.click_node(By::role_name(Role::MenuItem, "Split right"));
    }

    fn pane_divider(ui: &UiTestHarness<PanelsDemo>) -> Node {
        ui.find(By::test_id("dock-pane-divider"))
    }

    /// The pane divider's (value, min, max) and value text.
    fn divider_value(ui: &UiTestHarness<PanelsDemo>) -> ((f64, f64, f64), Option<String>) {
        let id = pane_divider(ui)
            .id
            .expect("divider has an accessibility id");
        let (_, node) = ax_node(ui, &id);
        let numeric = (
            node.numeric_value().unwrap_or(f64::NAN),
            node.min_numeric_value().unwrap_or(f64::NAN),
            node.max_numeric_value().unwrap_or(f64::NAN),
        );
        (numeric, node.value().map(str::to_owned))
    }

    fn panel_width(ui: &UiTestHarness<PanelsDemo>, name: &str) -> f32 {
        ui.find(By::role_name(Role::TabPanel, name)).bounds.width
    }

    #[test]
    fn arrow_keys_move_a_divider_between_groups() {
        let mut ui = harness();
        split_right_panel(&mut ui);
        // The 420 point panel: 210 and 209 points either side of a 1 point
        // line, each group at least 100.
        assert_eq!(
            divider_value(&ui),
            ((210.0, 100.0, 319.0), Some("50%".into()))
        );
        ui.click_node(By::test_id("dock-pane-divider"));
        // (key, divider position, Terminal's group width)
        let steps = [
            ("right", 220.0, 220.0),
            ("shift+right", 270.0, 270.0),
            ("left", 260.0, 260.0),
        ];
        for (key, at, width) in steps {
            ui.key(key);
            assert_eq!(divider_value(&ui).0.0, at, "{key}");
            assert_eq!(panel_width(&ui, "Terminal"), width, "{key}");
            assert_eq!(panel_width(&ui, "Preview"), 419.0 - width, "{key}");
        }
        // The line stands upright though it moves sideways.
        let id = pane_divider(&ui).id.unwrap();
        assert_eq!(
            ax_node(&ui, &id).1.orientation(),
            Some(accesskit::Orientation::Vertical)
        );
    }

    #[test]
    fn assistive_tech_sets_and_steps_a_divider_within_its_range() {
        use accesskit::{Action, ActionData};
        // (action, value, divider position after), each from the split.
        let cases = [
            (
                Action::SetValue,
                Some(ActionData::NumericValue(150.0)),
                150.0,
            ),
            // Past either end, the groups stop at their minimums.
            (
                Action::SetValue,
                Some(ActionData::NumericValue(1000.0)),
                319.0,
            ),
            (Action::SetValue, Some(ActionData::NumericValue(0.0)), 100.0),
            (Action::Increment, None, 220.0),
            (Action::Decrement, None, 200.0),
        ];
        for (action, data, at) in cases {
            let mut ui = harness();
            split_right_panel(&mut ui);
            let id = pane_divider(&ui).id.unwrap();
            ax_action(&mut ui, &id, action, data.clone());
            assert_eq!(divider_value(&ui).0.0, at, "{action:?} {data:?}");
            assert_eq!(
                panel_width(&ui, "Terminal"),
                at as f32,
                "{action:?} {data:?}"
            );
        }
    }

    #[test]
    fn assistive_tech_sets_a_split_pane_size() {
        let mut ui = harness();
        ax_action(
            &mut ui,
            "dock:columns:divider:0",
            accesskit::Action::SetValue,
            Some(accesskit::ActionData::NumericValue(300.0)),
        );
        assert_eq!(announced(&ui, "Sidebar").as_deref(), Some("300"));
        // The right panel's divider grows its pane leftward all the same.
        ax_action(
            &mut ui,
            "dock:columns:divider:1",
            accesskit::Action::Increment,
            None,
        );
        assert_eq!(announced(&ui, "Right panel").as_deref(), Some("430"));
    }

    /// The painted runs of `text`, in paint order: a drag's preview last.
    fn runs_of(ui: &UiTestHarness<PanelsDemo>, text: &str) -> Vec<Rect> {
        ui.painted_texts()
            .into_iter()
            .filter(|run| run.text == text)
            .map(|run| run.bounds)
            .collect()
    }

    #[test]
    fn a_dragged_tab_shows_only_past_the_drag_threshold() {
        let mut ui = harness();
        let (x, y) = tab_center(&ui, "Threads");
        // The tab's label and its panel's heading.
        let resting = runs_of(&ui, "Threads").len();
        ui.pointer_down((x, y));
        ui.pointer_move((x + 2.0, y));
        assert_eq!(runs_of(&ui, "Threads").len(), resting);
        ui.pointer_move((x + 40.0, y));
        assert_eq!(runs_of(&ui, "Threads").len(), resting + 1);
    }

    #[test]
    fn a_dragged_tab_follows_the_pointer_while_its_target_stays() {
        let mut ui = harness();
        let (x, y) = tab_center(&ui, "Threads");
        let label = runs_of(&ui, "Threads")[0];
        let chat = ui.find(By::role_name(Role::TabPanel, "Chat")).center();
        ui.pointer_down((x, y));
        ui.pointer_move(chat);
        let target = ui.find(By::test_id("dock-drop-preview")).bounds;
        let first = *runs_of(&ui, "Threads").last().unwrap();
        // Held where it was pressed: the label sits as far from the
        // pointer as it did from the press.
        assert_eq!(
            (first.x - chat.0, first.y - chat.1),
            (label.x - x, label.y - y)
        );

        ui.pointer_move((chat.0 + 30.0, chat.1 + 20.0));
        let second = *runs_of(&ui, "Threads").last().unwrap();
        assert_eq!((second.x - first.x, second.y - first.y), (30.0, 20.0));
        assert_eq!(ui.find(By::test_id("dock-drop-preview")).bounds, target);
    }

    #[test]
    fn a_dragged_tab_is_paint_only() {
        let mut ui = harness();
        let (x, y) = tab_center(&ui, "Threads");
        // Over the Diff tab, which the sealed right panel keeps from being
        // a target.
        let diff = tab_center(&ui, "Diff");
        let under = ui.hit_test(diff).and_then(|n| n.name);
        assert_eq!(under.as_deref(), Some("Diff"));
        ui.pointer_down((x, y));
        ui.pointer_move(diff);
        assert_eq!(runs_of(&ui, "Threads").len(), 3, "the picture shows");
        // No second tab for assistive tech, and the pointer still reaches
        // what is under the picture.
        assert_eq!(ui.find_all(By::role_name(Role::Tab, "Threads")).len(), 1);
        assert_eq!(ui.hit_test(diff).and_then(|n| n.name), under);
    }

    #[test]
    fn a_dragged_tab_closed_mid_drag_stops_showing() {
        let mut ui = harness();
        let chat = ui.find(By::role_name(Role::TabPanel, "Chat")).center();
        ui.pointer_down(tab_center(&ui, "Diff"));
        ui.pointer_move(chat);
        assert_eq!(
            runs_of(&ui, "Diff").len(),
            3,
            "the tab, its heading, its picture"
        );
        // The press focused it; Delete closes it under the pointer.
        ui.key("delete");
        assert_eq!(runs_of(&ui, "Diff"), []);
    }

    #[test]
    fn a_cancelled_tab_drag_leaves_the_tab_where_it_was() {
        type Cancel = fn(&mut UiTestHarness<PanelsDemo>);
        let cancels: [(&str, Cancel); 2] = [
            ("escape", |ui| ui.key("escape")),
            ("focus loss", |ui| ui.focus_loss()),
        ];
        for (name, cancel) in cancels {
            let mut ui = harness();
            let resting = runs_of(&ui, "Threads").len();
            let chat = ui.find(By::role_name(Role::TabPanel, "Chat")).center();
            ui.pointer_down(tab_center(&ui, "Threads"));
            ui.pointer_move(chat);
            cancel(&mut ui);
            assert!(!ui.exit_requested(), "{name}");
            assert!(
                ui.try_find(By::test_id("dock-drop-preview")).is_none(),
                "{name}"
            );
            assert_eq!(runs_of(&ui, "Threads").len(), resting, "{name}");
            ui.pointer_up(chat);
            assert_eq!(strip(&ui, "Sidebar"), ["Threads"], "{name}");
            assert_eq!(strip(&ui, "Chat"), ["Chat"], "{name}");
        }
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
