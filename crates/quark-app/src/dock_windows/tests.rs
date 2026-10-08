//! A dock across windows driven through the headless harness: desktop
//! pointer drags under the source window's grab, located by a scripted
//! window stack, and the window lifecycle around them.

use accesskit::Role;
use quark_components::{
    Dock, DockEvent, DockLayout, DockRegion, DockState, MovePayload, PanelId, TabPolicy,
};
use quark_ui::FocusId;
use quark_ui::element::{AnyElement, IntoAnyElement, div, text, text_input};
use quark_ui::style::Styled;
use quark_ui::text_input::{TextEditCommand, TextEditOutcome, TextField};

use super::*;
use crate::platform::dock_drag::ScriptedStack;
use crate::platform::placement::{MonitorInfo, PhysicalRect};
use crate::testing::{By, UiTestHarness};
use crate::{PlatformCapabilities, UiApp, ViewContext};

const ALPHA: PanelId = PanelId(1);
const BRAVO: PanelId = PanelId(2);
const CHARLIE: PanelId = PanelId(3);
const DELTA: PanelId = PanelId(4);
const NOTES: PanelId = PanelId(5);

const NOTES_FIELD: FocusId = FocusId::from_key("notes");

/// The main window, at the desktop's corner with no decorations, so its
/// points are desktop units.
const SIZE: (f32, f32) = (1200.0, 800.0);

/// Right of the main window, over nothing of the app's.
const DESKTOP: (f64, f64) = (1500.0, 300.0);

fn name(panel: PanelId) -> String {
    match panel {
        ALPHA => "Alpha",
        BRAVO => "Bravo",
        CHARLIE => "Charlie",
        DELTA => "Delta",
        _ => "Notes",
    }
    .to_owned()
}

/// Alpha on the left, Bravo in the center, Charlie and Delta on the right,
/// and a Notes panel with a text field along the bottom.
struct Desk {
    dock: DockState,
    windows: DockWindows,
    notes: TextField,
}

impl Desk {
    fn new(windows: DockWindows) -> Self {
        let mut dock = DockState::new(DockLayout::default());
        dock.open(DockRegion::Left, ALPHA);
        dock.open(DockRegion::Center, BRAVO);
        dock.open(DockRegion::Right, CHARLIE);
        dock.open(DockRegion::Right, DELTA);
        dock.open(DockRegion::Bottom, NOTES);
        Self {
            dock,
            windows,
            notes: TextField::new(""),
        }
    }
}

impl UiApp for Desk {
    type Action = DockEvent;
    /// Dock events from outside the dock, as a menu sends them.
    type Message = DockEvent;

    fn init(&mut self, cx: &mut UiContext) {
        self.windows.init(&mut self.dock, cx);
    }

    fn view(&mut self, cx: &mut ViewContext) -> AnyElement {
        let size = cx.frame.size();
        let Some(host) = self.windows.host(cx.window_handle()) else {
            return div().into_any();
        };
        let focused = cx.is_focused(NOTES_FIELD);
        let mut dock = Dock::new(&self.dock, size, quark_ui::Action::new)
            .group_grips(true)
            .host(host);
        for region in DockRegion::ALL {
            dock = dock.always_show_tabs(region);
        }
        dock.build(cx.theme, name, |panel, (w, h)| match panel {
            NOTES => text_input("Notes field", "")
                .field(&self.notes)
                .focus_target(NOTES_FIELD)
                .focused(focused)
                .w(w)
                .h(40.0)
                .into_any(),
            panel => div().w(w).h(h).child(text(name(panel))).into_any(),
        })
    }

    fn update(&mut self, event: DockEvent, cx: &mut UiContext) {
        self.windows.apply(&mut self.dock, event, cx);
    }

    fn message(&mut self, event: DockEvent, cx: &mut UiContext) {
        self.windows.apply(&mut self.dock, event, cx);
    }

    fn event(&mut self, event: &InputEvent, cx: &mut UiContext) -> bool {
        self.windows.input(&mut self.dock, event, cx)
    }

    fn app_event(&mut self, event: AppEvent, cx: &mut UiContext) {
        self.windows.app_event(&event, cx);
    }

    fn window_opened(&mut self, window: WindowHandle, cx: &mut UiContext) {
        self.windows.window_opened(&mut self.dock, window, cx);
    }

    fn window_closed(&mut self, window: WindowHandle, reason: CloseReason, cx: &mut UiContext) {
        self.windows
            .window_closed(&mut self.dock, window, reason, cx);
    }

    fn window_close_requested(&mut self, reason: CloseReason, cx: &mut UiContext) -> bool {
        self.windows.close_requested(&mut self.dock, reason, cx)
    }

    fn drag_session_ended(&mut self, end: DragEnd, cx: &mut UiContext) {
        self.windows.drag_session_ended(&mut self.dock, &end, cx);
    }

    fn edit_text_in(
        &mut self,
        _window: WindowHandle,
        target: FocusId,
        command: TextEditCommand,
    ) -> TextEditOutcome {
        assert_eq!(target, NOTES_FIELD);
        self.notes.apply(command)
    }
}

/// The desk in its main window, its drags located by `stack` with the main
/// window raised.
fn desk(stack: &ScriptedStack) -> UiTestHarness<Desk> {
    let script = stack.clone();
    let windows = DockWindows::new(name).window_stack(move || Box::new(script.clone()));
    let ui = UiTestHarness::new(Desk::new(windows), SIZE, 1.0);
    stack.raise(ui.main_window());
    ui
}

fn desktop((x, y): (f32, f32)) -> DesktopPoint {
    (f64::from(x), f64::from(y))
}

/// Press at desktop point `at` and drag past the drag threshold.
fn grab(ui: &mut UiTestHarness<Desk>, at: DesktopPoint) {
    ui.desktop_move(at);
    ui.desktop_press();
    ui.desktop_move((at.0 + 8.0, at.1));
}

/// The main window's tab named `name`, in desktop units.
fn tab(ui: &UiTestHarness<Desk>, name: &str) -> DesktopPoint {
    desktop(ui.find(By::role_name(Role::Tab, name)).center())
}

/// The one window that is not the main one.
fn floating(ui: &UiTestHarness<Desk>) -> WindowHandle {
    let others: Vec<WindowHandle> = ui
        .windows()
        .into_iter()
        .filter(|w| *w != ui.main_window())
        .collect();
    assert_eq!(others.len(), 1, "{others:?}");
    others[0]
}

/// The tabs `window` shows, in order.
fn tabs_in(ui: &mut UiTestHarness<Desk>, window: WindowHandle) -> Vec<String> {
    ui.window(window)
        .find_all(By::role(Role::Tab))
        .into_iter()
        .filter_map(|n| n.name)
        .collect()
}

/// The main window's tab list named `list`, in order.
fn strip(ui: &UiTestHarness<Desk>, list: &str) -> Vec<String> {
    let tree = ui.accessibility_tree();
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

/// What `window` last asked assistive tech to speak.
fn announcement(ui: &mut UiTestHarness<Desk>, window: WindowHandle) -> Option<String> {
    ui.window(window)
        .accessibility_update()
        .nodes
        .iter()
        .find(|(_, node)| node.role() == Role::Status)
        .and_then(|(_, node)| node.label().map(str::to_owned))
}

/// Alpha torn off to [`DESKTOP`] and released there.
fn tear_off_alpha(ui: &mut UiTestHarness<Desk>) -> WindowHandle {
    let alpha = tab(ui, "Alpha");
    grab(ui, alpha);
    ui.desktop_move(DESKTOP);
    ui.desktop_release();
    floating(ui)
}

// Catches a drag that stays in its window past the edge, a torn-off window
// that does not follow the pointer or loses the grab point, and a release
// on the desktop that drops or snaps back instead of leaving it there.
#[test]
fn a_tab_dragged_off_every_window_follows_the_pointer_and_stays_where_released() {
    let stack = ScriptedStack::new();
    let mut ui = desk(&stack);
    let alpha = tab(&ui, "Alpha");
    grab(&mut ui, alpha);
    ui.desktop_move(DESKTOP);
    let torn = floating(&ui);
    let grab_point = |ui: &mut UiTestHarness<Desk>| {
        let window = ui.window(torn);
        let (x, y) = window.find(By::role_name(Role::Tab, "Alpha")).center();
        let outer = window.placement().outer_position.unwrap();
        (outer.0 + f64::from(x), outer.1 + f64::from(y))
    };
    assert_eq!(grab_point(&mut ui), DESKTOP);
    let later = (DESKTOP.0 + 100.0, DESKTOP.1 + 50.0);
    ui.desktop_move(later);
    assert_eq!(grab_point(&mut ui), later);

    ui.desktop_release();
    assert_eq!(ui.windows(), [ui.main_window(), torn]);
    assert_eq!(grab_point(&mut ui), later);
    assert_eq!(tabs_in(&mut ui, torn), ["Alpha"]);
    assert!(ui.try_find(By::role_name(Role::Tab, "Alpha")).is_none());
    assert_eq!(
        announcement(&mut ui, torn).as_deref(),
        Some("Moved Alpha to a new window")
    );
    assert_eq!(ui.window(torn).title(), "Alpha");
}

// Catches a group grip that moves only the active tab, reorders the
// group, or loses its selection on the way out.
#[test]
fn a_group_dragged_by_its_grip_tears_off_whole() {
    let stack = ScriptedStack::new();
    let mut ui = desk(&stack);
    let right = ui.app().dock.location(CHARLIE).unwrap().0.pane;
    let grip = ui
        .geometry()
        .by_id(&format!("dock:grip:{}", right.0))
        .unwrap();
    let b = grip.bounds;
    grab(
        &mut ui,
        desktop((b.x + b.width / 2.0, b.y + b.height / 2.0)),
    );
    ui.desktop_move(DESKTOP);
    ui.desktop_release();

    let torn = floating(&ui);
    assert_eq!(tabs_in(&mut ui, torn), ["Charlie", "Delta"]);
    ui.window(torn).find(By::role_name(Role::TabPanel, "Delta"));
    assert!(ui.try_find(By::role_name(Role::Tab, "Charlie")).is_none());
}

// Catches a drop into another window that is ignored, lands in the source
// window, or leaves the emptied floating window open.
#[test]
fn a_floating_tab_dragged_onto_a_group_docks_there_and_its_window_closes() {
    let stack = ScriptedStack::new();
    let mut ui = desk(&stack);
    let torn = tear_off_alpha(&mut ui);
    stack.raise(torn);
    let alpha = {
        let window = ui.window(torn);
        let (x, y) = window.find(By::role_name(Role::Tab, "Alpha")).center();
        let outer = window.placement().outer_position.unwrap();
        (outer.0 + f64::from(x), outer.1 + f64::from(y))
    };
    let bravo = desktop(ui.find(By::role_name(Role::TabPanel, "Bravo")).center());

    grab(&mut ui, alpha);
    ui.desktop_move(bravo);
    ui.desktop_release();

    assert_eq!(ui.windows(), [ui.main_window()]);
    assert_eq!(strip(&ui, "Center"), ["Bravo", "Alpha"]);
    let main = ui.main_window();
    assert_eq!(ui.focused().and_then(|n| n.name).as_deref(), Some("Alpha"));
    assert_eq!(
        announcement(&mut ui, main).as_deref(),
        Some("Moved Alpha to Center")
    );
}

// Catches a drop resolved against the wrong part of another window's
// group: a strip slot read as the body, a body edge read as its center.
#[test]
fn a_drop_from_another_window_lands_where_the_target_shows() {
    type Target = fn(&UiTestHarness<Desk>) -> DesktopPoint;
    let cases: [(&str, Target, &[&str], usize); 3] = [
        (
            "before Bravo's tab",
            |ui| {
                let (x, y) = tab(ui, "Bravo");
                (x - 30.0, y)
            },
            &["Alpha", "Bravo"],
            1,
        ),
        (
            "after the strip's tabs",
            |ui| {
                let (x, y) = tab(ui, "Bravo");
                (x + 150.0, y)
            },
            &["Bravo", "Alpha"],
            1,
        ),
        (
            "on the body's right edge",
            |ui| {
                let b = ui.find(By::role_name(Role::TabPanel, "Bravo")).bounds;
                desktop((b.x + b.width - 10.0, b.y + b.height / 2.0))
            },
            &["Bravo"],
            2,
        ),
    ];
    for (name, target, center, groups) in cases {
        let stack = ScriptedStack::new();
        let mut ui = desk(&stack);
        let torn = tear_off_alpha(&mut ui);
        stack.raise(torn);
        let alpha = {
            let window = ui.window(torn);
            let (x, y) = window.find(By::role_name(Role::Tab, "Alpha")).center();
            let outer = window.placement().outer_position.unwrap();
            (outer.0 + f64::from(x), outer.1 + f64::from(y))
        };
        let at = target(&ui);
        grab(&mut ui, alpha);
        ui.desktop_move(at);
        ui.desktop_release();
        // A split region numbers its groups' names in reading order.
        let first = if groups == 1 { "Center" } else { "Center 1" };
        assert_eq!(strip(&ui, first), center, "{name}");
        let lists = (1..=groups)
            .filter(|n| {
                let list = if groups == 1 {
                    "Center".to_owned()
                } else {
                    format!("Center {n}")
                };
                !ui.find_all(By::role_name(Role::TabList, &list)).is_empty()
            })
            .count();
        assert_eq!(lists, groups, "{name}");
    }
}

// Catches Escape that ends the drag but leaves the torn-off window, or
// puts the tab back somewhere other than where it was.
#[test]
fn escape_puts_a_torn_off_tab_back_and_closes_its_window() {
    let stack = ScriptedStack::new();
    let mut ui = desk(&stack);
    let main = ui.main_window();
    let before = tabs_in(&mut ui, main);
    let alpha = tab(&ui, "Alpha");
    grab(&mut ui, alpha);
    ui.desktop_move(DESKTOP);
    assert_eq!(ui.windows().len(), 2);

    ui.key("escape");
    ui.desktop_release();
    assert_eq!(ui.windows(), [main]);
    assert_eq!(tabs_in(&mut ui, main), before);
    assert!(ui.app().dock.hosts().is_empty());
}

// Catches a platform that cannot move windows with the pointer losing
// tear-off altogether: there a release outside opens the window instead.
#[test]
fn without_window_positions_a_release_outside_opens_the_window_then() {
    let stack = ScriptedStack::new();
    let mut ui = desk(&stack);
    ui.set_capabilities(PlatformCapabilities {
        window_positions: false,
        desktop_pointer: true,
    });
    let alpha = tab(&ui, "Alpha");
    grab(&mut ui, alpha);
    ui.desktop_move(DESKTOP);
    assert_eq!(ui.windows().len(), 1, "nothing follows the pointer");

    ui.desktop_release();
    let torn = floating(&ui);
    assert_eq!(tabs_in(&mut ui, torn), ["Alpha"]);
    assert!(ui.try_find(By::role_name(Role::Tab, "Alpha")).is_none());
}

// Catches a closed floating window whose tabs are lost or land somewhere
// other than where they left from.
#[test]
fn closing_a_floating_window_docks_its_tabs_back() {
    let stack = ScriptedStack::new();
    let mut ui = desk(&stack);
    let main = ui.main_window();
    let before = tabs_in(&mut ui, main);
    ui.send_message(DockEvent::MoveToNewHost(MovePayload::Panel(ALPHA)));
    let torn = floating(&ui);
    assert_eq!(tabs_in(&mut ui, torn), ["Alpha"]);

    assert!(ui.window(torn).request_close());
    assert_eq!(ui.windows(), [main]);
    assert_eq!(tabs_in(&mut ui, main), before);
    assert_eq!(ui.focused().and_then(|n| n.name).as_deref(), Some("Alpha"));
}

// Catches a close that re-docks some tabs and not others, or closes the
// window anyway, when the main window takes none of them.
#[test]
fn a_floating_window_whose_tab_has_nowhere_to_go_stays_open_and_says_why() {
    let stack = ScriptedStack::new();
    let mut ui = desk(&stack);
    ui.send_message(DockEvent::MoveToNewHost(MovePayload::Panel(ALPHA)));
    let torn = floating(&ui);
    for region in DockRegion::ALL {
        let closed = TabPolicy {
            can_leave: true,
            accepts: false,
        };
        ui.app_mut().dock.set_policy(region, closed);
    }

    assert!(!ui.window(torn).request_close());
    assert_eq!(tabs_in(&mut ui, torn), ["Alpha"]);
    let why = "Cannot close this window: no group takes Alpha";
    assert_eq!(ui.app().windows.notice(torn), Some(why));
    assert_eq!(announcement(&mut ui, torn).as_deref(), Some(why));
}

// Catches a quit that re-docks floating windows before saving, and a
// restore that loses their tabs or where they were.
#[test]
fn a_quit_saves_floating_windows_and_a_restart_reopens_them_in_place() {
    let path = std::env::temp_dir().join(format!(
        "quark-dock-windows-test-{}.json",
        std::process::id()
    ));
    let monitor = MonitorInfo {
        name: Some("Desk".into()),
        bounds: PhysicalRect {
            x: 0,
            y: 0,
            width: 2560,
            height: 1440,
        },
        scale_factor: 1.0,
        work_area: None,
    };
    let windows = || DockWindows::new(name).save_to(&path);
    let mut ui = UiTestHarness::with_monitor(Desk::new(windows()), SIZE, 1.0, monitor.clone());
    ui.send_message(DockEvent::MoveToNewHost(MovePayload::Panel(ALPHA)));
    let torn = floating(&ui);
    ui.window(torn).move_to((1500.0, 200.0));
    ui.window(torn).resize(500.0, 400.0);
    assert!(ui.request_quit());

    let mut app = Desk::new(windows());
    assert!(app.windows.restore(&mut app.dock, |_| true));
    let mut ui = UiTestHarness::with_monitor(app, SIZE, 1.0, monitor);
    let _ = std::fs::remove_file(&path);
    let reopened = floating(&ui);
    assert_eq!(tabs_in(&mut ui, reopened), ["Alpha"]);
    let placement = ui.window(reopened).placement();
    assert_eq!(placement.outer_position, Some((1500.0, 200.0)));
    assert_eq!(placement.size, (500.0, 400.0));
}

// Catches a moved panel that loses its text, leaves focus behind in the
// window it left, or takes no typing in its new window.
#[test]
fn a_panel_moved_to_a_new_window_keeps_its_text_and_takes_input_there() {
    let stack = ScriptedStack::new();
    let mut ui = desk(&stack);
    ui.click_node(By::name("Notes field"));
    ui.type_text("a");
    ui.send_message(DockEvent::MoveToNewHost(MovePayload::Panel(NOTES)));
    let torn = floating(&ui);
    assert_eq!(
        ui.window(torn).focused().and_then(|n| n.name).as_deref(),
        Some("Notes")
    );
    assert!(!ui.ime().allowed, "the main window has no field focused");

    ui.window(torn).click_node(By::name("Notes field"));
    ui.window(torn).type_text("b");
    assert_eq!(ui.app().notes.text(), "ab");
    assert!(ui.window(torn).ime().allowed);
    assert!(!ui.ime().allowed);
}
