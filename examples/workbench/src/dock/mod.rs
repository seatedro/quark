//! The dock (stream E): panel placement across windows through
//! `DockState` and `DockWindows`, and the Diff, Terminal, Files, and
//! Snapshot preview panels. Panel content state is keyed by `PanelId`, so
//! moving a panel to another window keeps it.
//!
//! `app.rs` renders the dock for every host and forwards the window hooks
//! below. The shell fills `SIDEBAR` and the thread fills `THREAD`; this
//! module builds every other panel's content through [`panel_view`].
//!
//! Tabs move between groups and windows by dragging (DockWindows tears a
//! tab off into a window that follows the pointer), and from the keyboard:
//! Shift+F10 (or the menu key) on a focused tab opens a menu of the groups
//! it can join and "Move to new window". Closing a floating window docks
//! its panels back where they came from.

pub mod diff;
pub mod files;
pub mod preview;
pub mod terminal;

use quark::view;
use quark_app::dock_windows::DockWindows;
use quark_app::quark_ui::FocusId;
use quark_app::quark_ui::element::*;
use quark_app::quark_ui::style::Styled;
use quark_app::quark_ui::text_input::{TextEditCommand, TextEditOutcome};
use quark_app::winit::event::ElementState;
use quark_app::{AppEvent, CloseReason, InputEvent, UiContext, ViewContext, WindowHandle};
use quark_components::{
    ContextMenuEntry, ContextMenuOutcome, ContextMenuState, DockDestination, DockEvent, DockLayout,
    DockRegion, DockState, DropZone, MovePayload, MoveTarget, Pane, PanelId, TabPolicy,
};

use crate::contracts::{
    CommandId, EditCx, Effect, Effects, Msg, Options, PANELS, PanelSpec, SurfaceCx, panel_spec,
    panel_title, panels,
};
use crate::design::tokens;
use crate::model::Model;

/// The most a dock tab takes; shorter titles get narrower tabs.
pub const TAB_MAX_WIDTH: f32 = 160.0;

/// Where the tab move menu opens inside its panel, below the tab strip.
const MENU_INSET: (f32, f32) = (8.0, 4.0);

/// Panel placement plus each panel's content state. The fields are
/// separate so the app can borrow `layout` for the dock element while
/// panel views borrow `panels` mutably.
pub struct State {
    pub layout: DockState,
    pub windows: DockWindows,
    pub panels: Panels,
    /// Panel titles' widths, for tabs sized to their titles.
    pub tab_labels: TabLabels,
}

/// Every panel title's width at the tab strip's font, measured again only
/// when the font size or scale changes, so a frame allocates nothing for
/// it.
#[derive(Default)]
pub struct TabLabels {
    key: Option<(u32, u32)>,
    widths: Vec<(PanelId, f32)>,
}

impl TabLabels {
    /// Measure the titles for `theme` on `vcx`'s window if its tab font or
    /// scale changed.
    pub fn update(&mut self, theme: &quark_app::quark_ui::theme::Theme, vcx: &mut ViewContext) {
        use quark_text::{TextQuery, TextStyle};
        let size = theme.metrics.ui_small_font_size;
        let scale = vcx.frame.scale_factor();
        let key = (size.to_bits(), scale.to_bits());
        if self.key == Some(key) {
            return;
        }
        self.key = Some(key);
        let text = vcx.frame.text();
        self.widths.clear();
        for spec in PANELS {
            let query = TextQuery::new(spec.title, TextStyle::new(size)).scale_factor(scale);
            let width = text
                .layouts
                .layout_query(&mut text.system, &query)
                .map_or(0.0, |layout| layout.size().0.ceil());
            self.widths.push((spec.id, width));
        }
    }

    /// The width of `panel`'s title; a long guess for an unknown panel, so
    /// its tab takes the most room a tab may.
    pub fn width(&self, panel: PanelId) -> f32 {
        self.widths
            .iter()
            .find(|(id, _)| *id == panel)
            .map_or(f32::MAX, |(_, width)| *width)
    }
}

/// Content state of the dock panels, keyed by panel, not by host window:
/// one state per [`PanelId`], wherever the dock shows it.
#[derive(Default)]
pub struct Panels {
    pub diff: diff::State,
    pub terminal: terminal::State,
    pub files: files::State,
    pub preview: preview::State,
    /// The tab move menu, drawn in its panel's body.
    menu: ContextMenuState,
    menu_panel: Option<PanelId>,
    /// The pointer's last position in the window it is over.
    pointer: Option<(WindowHandle, f32, f32)>,
}

impl Panels {
    fn close_menu(&mut self) {
        self.menu.close();
        self.menu_panel = None;
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Dock(DockEvent),
    Diff(diff::Action),
    Terminal(terminal::Action),
    Files(files::Action),
    Preview(preview::Action),
}

/// Dock layout per the design: sidebar 232 left, right dock 400.
pub fn new_state(_options: &Options) -> State {
    let mut layout = DockState::new(DockLayout {
        left: Pane::fixed("Sidebar", tokens::SIDEBAR_WIDTH)
            .min(180.0)
            .max(420.0),
        right: Pane::fixed("Right dock", tokens::RIGHT_DOCK_WIDTH)
            .min(280.0)
            .max(900.0),
        bottom: Pane::fixed("Bottom", 220.0).min(100.0).max(600.0),
        center_label: "Thread",
        center_min_width: 360.0,
        center_min_height: 240.0,
    });
    for PanelSpec { id, region, .. } in PANELS {
        layout.open(region, id);
    }
    // The first right panel is the active tab.
    layout.open(DockRegion::Right, panels::DIFF);
    layout.set_policy(
        DockRegion::Left,
        TabPolicy {
            can_leave: false,
            accepts: false,
        },
    );
    layout.set_policy(
        DockRegion::Center,
        TabPolicy {
            can_leave: false,
            accepts: false,
        },
    );
    layout.set_visible(DockRegion::Bottom, false);
    let windows = DockWindows::new(|id| panel_title(id).to_owned()).app_title("Quark Workbench");
    State {
        layout,
        windows,
        panels: Panels::default(),
        tab_labels: TabLabels::default(),
    }
}

/// Whether a restored snapshot may keep `id`.
pub fn known_panel(id: PanelId) -> bool {
    panel_spec(id).is_some()
}

/// Content of a dock-owned panel (`DIFF`, `TERMINAL`, `FILES`, `PREVIEW`).
pub fn panel_view(
    panels: &mut Panels,
    id: PanelId,
    scx: &SurfaceCx,
    vcx: &mut ViewContext,
) -> AnyElement {
    let content = match id {
        panels::DIFF => diff::view(&mut panels.diff, scx, vcx),
        panels::TERMINAL => terminal::view(&mut panels.terminal, scx, vcx),
        panels::FILES => files::view(&mut panels.files, scx, vcx),
        panels::PREVIEW => preview::view(&mut panels.preview, scx, vcx),
        _ => div().into_any(),
    };
    // The move menu sits in its tab's panel: panels clip their content,
    // and this is the part of the window under the tab the dock lets a
    // panel draw in.
    let menu = (panels.menu_panel == Some(id))
        .then(|| panels.menu.render(scx.size, scx.theme))
        .flatten();
    let (width, height) = scx.size;
    view! {
        <div class="relative" w={width} h={height}>
            {content}
            {?menu}
        </div>
    }
    .into_any()
}

/// Dock events need the window context (they open and close windows), so
/// this update takes `cx` and the model instead of a `SurfaceCx`.
pub fn update(
    state: &mut State,
    action: Action,
    model: &Model,
    fx: &mut Effects,
    cx: &mut UiContext,
) {
    let panels = &mut state.panels;
    match action {
        Action::Dock(event) => {
            // A choice from the move menu, or anything else done in the
            // dock, closes it; pointer motion over a drag target does not.
            if !matches!(event, DockEvent::TabHover { .. } | DockEvent::Hover { .. }) {
                panels.close_menu();
            }
            state.windows.apply(&mut state.layout, event, cx);
        }
        Action::Diff(a) => diff::update(&mut panels.diff, a, model, fx, cx),
        Action::Terminal(a) => terminal::update(&mut panels.terminal, a, cx),
        Action::Files(a) => files::update(&mut panels.files, a, cx),
        Action::Preview(a) => preview::update(&mut panels.preview, a),
    }
    cx.window.request_redraw_all();
}

pub fn set_region_visible(state: &mut State, region: DockRegion, visible: bool) {
    state.layout.set_visible(region, visible);
}

/// Make `panel` active wherever it is and focus its content. In the main
/// window the app reveals it, through the shell's width policy, so the
/// shell's idea of whether the dock is wanted stays true.
fn show(state: &mut State, panel: PanelId, focus: FocusId, fx: &mut Effects) {
    let floating = state
        .layout
        .location(panel)
        .is_some_and(|(at, _)| at.host != quark_components::HostId::MAIN);
    if floating {
        reveal_panel(state, panel);
    } else {
        fx.push(Effect::RevealPanel(panel));
    }
    fx.push(Effect::Focus(Some(focus)));
}

/// Commands the dock owns (`ShowDiff`, `ShowFiles`, `ShowPreview`,
/// `ToggleTerminal`). True when handled. Apply and Undo stay with the app,
/// which owns the file store.
pub fn command(state: &mut State, id: CommandId, scx: &SurfaceCx, fx: &mut Effects) -> bool {
    match id {
        CommandId::ShowDiff => fx.push(Effect::RevealDiff(None)),
        CommandId::ShowFiles => {
            let path = state.panels.files.open_path().to_owned();
            fx.push(Effect::OpenFile(path));
        }
        CommandId::ShowPreview => show(
            state,
            panels::PREVIEW,
            FocusId::from_key("workbench.preview.zoom"),
            fx,
        ),
        CommandId::ToggleTerminal => {
            let shown = state
                .layout
                .location(panels::TERMINAL)
                .is_some_and(|(at, _)| {
                    state
                        .layout
                        .group(at.pane)
                        .and_then(|g| g.panels.get(g.active))
                        == Some(&panels::TERMINAL)
                        && (at.host != quark_components::HostId::MAIN
                            || state.layout.is_visible(at.region))
                });
            if shown && scx.is_focused(terminal::FOCUS) {
                // Second press: back to the composer, as editors do with
                // their terminal toggle.
                fx.push(Effect::Command(CommandId::FocusComposer));
            } else {
                show(state, panels::TERMINAL, terminal::FOCUS, fx);
            }
        }
        _ => return false,
    }
    true
}

pub fn edit_text(
    state: &mut State,
    target: FocusId,
    command: TextEditCommand,
    ecx: &EditCx,
) -> Option<TextEditOutcome> {
    files::edit_text(&mut state.panels.files, target, command, ecx.now_ms)
}

/// Show `path` in the Files panel.
pub fn open_file(state: &mut State, path: &str) {
    state.panels.files.open(path);
    state.layout.open(DockRegion::Right, panels::FILES);
}

/// Show the Diff panel, at `path` when given.
pub fn reveal_diff(state: &mut State, path: Option<&str>) {
    state.layout.open(DockRegion::Right, panels::DIFF);
    if let Some(path) = path {
        state.panels.diff.reveal(path);
    }
}

/// Open the move menu for `panel`'s tab: every group that takes it, and a
/// new window. The panel becomes its group's active tab so the menu shows
/// over it.
fn open_move_menu(state: &mut State, panel: PanelId, cx: &mut UiContext) {
    let Some((at, index)) = state.layout.location(panel) else {
        return;
    };
    let payload = MovePayload::Panel(panel);
    let mut entries: Vec<ContextMenuEntry> = state
        .layout
        .move_options(payload)
        .into_iter()
        .map(|target| match target {
            MoveTarget::Group { host, pane } => {
                let event = DockEvent::Transfer {
                    payload,
                    destination: DockDestination {
                        host,
                        pane,
                        zone: DropZone::Center,
                    },
                };
                let name = state.windows.group_name(&state.layout, pane);
                ContextMenuEntry::item(format!("Move to {name}"), Msg::Dock(Action::Dock(event)))
            }
            MoveTarget::NewHost => ContextMenuEntry::item(
                "Move to new window",
                Msg::Dock(Action::Dock(DockEvent::MoveToNewHost(payload))),
            ),
        })
        .collect();
    if entries.is_empty() {
        entries
            .push(ContextMenuEntry::item("No other place takes this tab", NoopAction).disabled());
    }
    state.layout.select(at.pane, index);
    state.panels.menu.open(entries, MENU_INSET.0, MENU_INSET.1);
    state.panels.menu_panel = Some(panel);
    cx.window.request_redraw_all();
}

/// The move menu's share of raw input: Shift+F10 or the menu key on a
/// focused dock tab opens it, keys drive it while open, and a press
/// outside it closes it.
fn menu_input(state: &mut State, event: &InputEvent, cx: &mut UiContext) -> bool {
    let panels = &mut state.panels;
    match event {
        InputEvent::PointerMoved { x, y } => {
            panels.pointer = cx.window_handle().map(|w| (w, *x, *y));
            false
        }
        InputEvent::PointerButton {
            state: ElementState::Pressed,
            ..
        } if panels.menu.visible => {
            let inside = panels
                .pointer
                .zip(cx.window_handle())
                .is_some_and(|((w, x, y), here)| {
                    w == here
                        && cx
                            .geometry()
                            .by_id("context-menu")
                            .is_ok_and(|m| m.bounds.contains(x, y))
                });
            if !inside {
                panels.close_menu();
                cx.window.request_redraw_all();
            }
            false
        }
        InputEvent::KeyPress(chord) => {
            let Some(pressed) = chord.binding() else {
                return false;
            };
            if panels.menu.visible {
                let Some(outcome) = panels.menu.handle_key(&pressed) else {
                    return false;
                };
                if let ContextMenuOutcome::Activate(action) = outcome {
                    panels.close_menu();
                    if let Some(Msg::Dock(Action::Dock(event))) = action.downcast_ref::<Msg>() {
                        state.windows.apply(&mut state.layout, *event, cx);
                    }
                } else if !panels.menu.visible {
                    panels.close_menu();
                }
                cx.window.request_redraw_all();
                return true;
            }
            let opens = ["shift+f10", "contextmenu"]
                .iter()
                .any(|k| k.parse::<Binding>().is_ok_and(|b| b.matches(&pressed)));
            let panel = state.layout.focused_panel(cx.focus());
            match panel {
                Some(panel) if opens && panel_spec(panel).is_some_and(|p| p.detachable) => {
                    open_move_menu(state, panel, cx);
                    true
                }
                _ => false,
            }
        }
        _ => false,
    }
}

/// Make `panel` the active tab of its group wherever it lives, adding it
/// to the right dock when no group has it (for the main window, the app
/// has already asked the shell to show the dock).
pub fn reveal_panel(state: &mut State, panel: PanelId) {
    state.layout.open(DockRegion::Right, panel);
}

// Window hooks, forwarded from `UiApp`.

pub fn init(state: &mut State, cx: &mut UiContext) {
    state.windows.init(&mut state.layout, cx);
}

/// Raw input for every window, before the surfaces see it: a drag between
/// windows, the move menu, then the terminal when it has focus.
pub fn input(state: &mut State, event: &InputEvent, cx: &mut UiContext) -> bool {
    if state.windows.input(&mut state.layout, event, cx) {
        return true;
    }
    if menu_input(state, event, cx) {
        return true;
    }
    terminal::input(&mut state.panels.terminal, event, cx)
}

pub fn wake(state: &mut State, cx: &mut UiContext) {
    state.windows.wake(&mut state.layout, cx);
}

pub fn app_event(state: &mut State, event: &AppEvent, cx: &mut UiContext) {
    state.windows.app_event(event, cx);
}

pub fn window_opened(state: &mut State, window: WindowHandle, cx: &mut UiContext) {
    state.windows.window_opened(&mut state.layout, window, cx);
}

pub fn window_closed(
    state: &mut State,
    window: WindowHandle,
    reason: CloseReason,
    cx: &mut UiContext,
) {
    state.panels.close_menu();
    state
        .windows
        .window_closed(&mut state.layout, window, reason, cx);
}

pub fn close_requested(state: &mut State, reason: CloseReason, cx: &mut UiContext) -> bool {
    state.windows.close_requested(&mut state.layout, reason, cx)
}

pub fn drag_session_ended(state: &mut State, end: &DragEnd, cx: &mut UiContext) {
    state.windows.drag_session_ended(&mut state.layout, end, cx);
}
