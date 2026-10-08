//! The shell (stream A): the top bar, the thread sidebar, and the width
//! policy that decides which side regions fit.
//!
//! Width policy (design section 3): at 1200 points and wider both side
//! regions show as the user left them; below 1200 the right dock collapses
//! behind its toggle; below 1040 the sidebar leaves the dock and opens as a
//! dismissible overlay. The user's wish for each region is kept apart from
//! what fits, so widening the window restores the earlier layout (and the
//! dock keeps its sizes).

pub mod sidebar;
pub mod titlebar;

use quark_app::InputEvent;
use quark_app::quark_ui::FocusId;
use quark_app::quark_ui::text_input::{TextEditCommand, TextEditOutcome};
use quark_app::winit::keyboard::NamedKey;
use quark_components::DockRegion;

use crate::contracts::{CommandId, EditCx, Effect, Effects, SurfaceCx, ThreadId, WidthPolicy};

pub use sidebar::SEARCH;
pub use titlebar::view as titlebar;

/// Narrower than this, the right dock collapses behind its toggle.
pub const DOCK_BREAKPOINT: f32 = 1200.0;
/// Narrower than this, the sidebar becomes an overlay.
pub const SIDEBAR_BREAKPOINT: f32 = 1040.0;

pub struct State {
    pub sidebar: sidebar::State,
    /// The user wants the sidebar (Mod+B toggles).
    pub sidebar_wanted: bool,
    /// The user wants the right dock (Mod+Alt+B toggles).
    pub dock_wanted: bool,
    /// Narrow layouts: the user opened the collapsed dock anyway. Cleared
    /// once the window is wide enough to show it by itself.
    pub dock_forced: bool,
    /// Narrow layouts: the sidebar overlay is open.
    pub sidebar_overlay: bool,
    /// The policy last applied to the dock.
    pub applied: Option<WidthPolicy>,
    /// The title bar's content as of the last frame.
    title: Option<std::rc::Rc<titlebar::TitleData>>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    SelectThread(ThreadId),
    FocusSearch,
    ClearSearch,
    /// Collapse or expand a sidebar section.
    ToggleSection(sidebar::Section),
    /// Wheel lines over the sidebar list, or a thumb drag to a pixel.
    ScrollLines(i32),
    ScrollTo(u32),
    /// Arrow keys on the focused list: move the selection this many
    /// threads (`i32::MIN`/`MAX` for the first and last).
    Step(i32),
    DismissOverlay,
}

pub fn new_state() -> State {
    State {
        sidebar: sidebar::State::default(),
        sidebar_wanted: true,
        dock_wanted: true,
        dock_forced: false,
        sidebar_overlay: false,
        applied: None,
        title: None,
    }
}

/// What fits at `width`, given what the user wants.
pub fn width_policy(state: &State, width: f32) -> WidthPolicy {
    WidthPolicy {
        sidebar_docked: state.sidebar_wanted && width >= SIDEBAR_BREAKPOINT,
        right_dock_shown: state.dock_wanted && (width >= DOCK_BREAKPOINT || state.dock_forced),
    }
}

/// Whether the sidebar overlay is drawn over the main window.
pub fn overlay_open(state: &State, policy: WidthPolicy) -> bool {
    state.sidebar_overlay && !policy.sidebar_docked
}

/// The policy for the main window at `width`, and the region changes the
/// dock needs to match it (none when nothing changed since last time).
pub fn sync_width(state: &mut State, width: f32, fx: &mut Effects) -> WidthPolicy {
    if width >= DOCK_BREAKPOINT {
        state.dock_forced = false;
    }
    let policy = width_policy(state, width);
    if state.applied != Some(policy) {
        fx.push(Effect::SetRegionVisible(
            DockRegion::Left,
            policy.sidebar_docked,
        ));
        fx.push(Effect::SetRegionVisible(
            DockRegion::Right,
            policy.right_dock_shown,
        ));
        if policy.sidebar_docked {
            state.sidebar_overlay = false;
        }
        state.applied = Some(policy);
    }
    policy
}

pub fn update(state: &mut State, action: Action, scx: &SurfaceCx, fx: &mut Effects) {
    match action {
        Action::SelectThread(id) => {
            if !scx.width_policy.sidebar_docked {
                state.sidebar_overlay = false;
            }
            fx.push(Effect::SelectThread(id));
        }
        Action::FocusSearch => fx.push(Effect::Focus(Some(SEARCH))),
        Action::ClearSearch => {
            state.sidebar.clear_search();
            fx.push(Effect::Focus(Some(SEARCH)));
        }
        Action::ToggleSection(section) => state.sidebar.toggle(section),
        Action::ScrollLines(lines) => state.sidebar.scroll_by_lines(lines),
        Action::ScrollTo(px) => state.sidebar.scroll_to(px as f32),
        Action::Step(step) => {
            if let Some(next) = state.sidebar.step(scx.model, scx.model.selected, step) {
                state.sidebar.reveal(next);
                fx.push(Effect::SelectThread(next));
            }
        }
        Action::DismissOverlay => state.sidebar_overlay = false,
    }
}

/// Escape closes the sidebar overlay.
pub fn event(state: &mut State, event: &InputEvent, scx: &SurfaceCx, _fx: &mut Effects) -> bool {
    let InputEvent::KeyPress(chord) = event else {
        return false;
    };
    if chord.named() == Some(NamedKey::Escape) && overlay_open(state, scx.width_policy) {
        state.sidebar_overlay = false;
        return true;
    }
    false
}

pub fn edit_text(
    state: &mut State,
    target: FocusId,
    command: TextEditCommand,
    ecx: &EditCx,
) -> Option<TextEditOutcome> {
    (target == SEARCH).then(|| state.sidebar.edit_search(command, ecx.now_ms))
}

/// Commands the shell owns: sidebar and dock toggles, thread stepping.
pub fn command(state: &mut State, id: CommandId, scx: &SurfaceCx, fx: &mut Effects) -> bool {
    match id {
        CommandId::ToggleSidebar => {
            if scx.size.0 < SIDEBAR_BREAKPOINT {
                state.sidebar_overlay = !state.sidebar_overlay;
            } else {
                state.sidebar_wanted = !state.sidebar_wanted;
            }
        }
        CommandId::ToggleRightDock => {
            if scx.size.0 >= DOCK_BREAKPOINT {
                state.dock_wanted = !state.dock_wanted;
            } else if scx.width_policy.right_dock_shown {
                state.dock_forced = false;
            } else {
                // Narrow: the toggle shows the collapsed dock anyway, and
                // the thread shrinks toward its minimum.
                state.dock_wanted = true;
                state.dock_forced = true;
            }
        }
        CommandId::NextThread | CommandId::PreviousThread => {
            let step = if id == CommandId::NextThread { 1 } else { -1 };
            if let Some(next) = state.sidebar.step(scx.model, scx.model.selected, step) {
                fx.push(Effect::SelectThread(next));
            }
        }
        _ => return false,
    }
    true
}
