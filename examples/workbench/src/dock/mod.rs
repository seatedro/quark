//! The dock (stream E): panel placement across windows through
//! `DockState` and `DockWindows`, and the Diff, Terminal, Files, and
//! Snapshot preview panels. Panel content state is keyed by `PanelId`, so
//! moving a panel to another window keeps it.
//!
//! `app.rs` renders the dock for every host and forwards the window hooks
//! below. The shell fills `SIDEBAR` and the thread fills `THREAD`; this
//! module builds every other panel's content through [`panel_view`].

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
use quark_app::{AppEvent, CloseReason, InputEvent, UiContext, ViewContext, WindowHandle};
use quark_components::{DockEvent, DockLayout, DockRegion, DockState, Pane, PanelId, TabPolicy};

use crate::contracts::{
    CommandId, EditCx, Effects, Options, PANELS, PanelSpec, SurfaceCx, panel_spec, panel_title,
    panels,
};
use crate::design::tokens;
use crate::model::Model;

/// Panel placement plus each panel's content state. The fields are
/// separate so the app can borrow `layout` for the dock element while
/// panel views borrow `panels` mutably.
pub struct State {
    pub layout: DockState,
    pub windows: DockWindows,
    pub panels: Panels,
}

/// Content state of the dock panels, keyed by panel, not by host window.
#[derive(Default)]
pub struct Panels {}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Dock(DockEvent),
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
    }
}

/// Whether a restored snapshot may keep `id`.
pub fn known_panel(id: PanelId) -> bool {
    panel_spec(id).is_some()
}

/// Content of a dock-owned panel (`DIFF`, `TERMINAL`, `FILES`, `PREVIEW`).
pub fn panel_view(
    _panels: &mut Panels,
    id: PanelId,
    scx: &SurfaceCx,
    _vcx: &mut ViewContext,
) -> AnyElement {
    let colors = &scx.theme.colors;
    let (width, height) = scx.size;
    let note = match id {
        panels::DIFF => "2 files changed",
        panels::TERMINAL => "$ npm test",
        panels::FILES => "src/App.tsx",
        panels::PREVIEW => "Snapshot preview",
        _ => "",
    };
    view! {
        <div w={width} h={height} class="flex-col p-4 gap-[6]" bg={colors.panel}>
            <text class="font-semibold" color={colors.text_strong}>{panel_title(id)}</text>
            <text size={12.0} color={colors.text_muted}>{note}</text>
        </div>
    }
    .into_any()
}

/// Dock events need the window context (they open and close windows), so
/// this update takes `cx` and the model instead of a `SurfaceCx`.
pub fn update(
    state: &mut State,
    action: Action,
    _model: &Model,
    _fx: &mut Effects,
    cx: &mut UiContext,
) {
    match action {
        Action::Dock(event) => {
            state.windows.apply(&mut state.layout, event, cx);
        }
    }
}

pub fn set_region_visible(state: &mut State, region: DockRegion, visible: bool) {
    state.layout.set_visible(region, visible);
}

/// Commands the dock owns (`ShowDiff`, `ShowFiles`, `ShowPreview`,
/// `ToggleTerminal`, `ApplyDiff`, `UndoDiff`). True when handled.
pub fn command(_state: &mut State, _id: CommandId, _scx: &SurfaceCx, _fx: &mut Effects) -> bool {
    false
}

pub fn edit_text(
    _state: &mut State,
    _target: FocusId,
    _command: TextEditCommand,
    _ecx: &EditCx,
) -> Option<TextEditOutcome> {
    None
}

/// Show `path` in the Files panel.
pub fn open_file(_state: &mut State, _path: &str) {}

/// Show the Diff panel, at `path` when given.
pub fn reveal_diff(state: &mut State, _path: Option<&str>) {
    state.layout.open(DockRegion::Right, panels::DIFF);
}

// Window hooks, forwarded from `UiApp`.

pub fn init(state: &mut State, cx: &mut UiContext) {
    state.windows.init(&mut state.layout, cx);
}

pub fn input(state: &mut State, event: &InputEvent, cx: &mut UiContext) -> bool {
    state.windows.input(&mut state.layout, event, cx)
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
