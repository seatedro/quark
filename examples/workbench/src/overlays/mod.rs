//! Overlays (stream F): the command palette, menus, tooltips, and toasts,
//! with one overlay stack per host window. Escape closes the topmost
//! transient layer; a modal suspends background commands.

pub mod feedback;
pub mod menus;
pub mod palette;

use quark_app::quark_ui::FocusId;
use quark_app::quark_ui::element::AnyElement;
use quark_app::quark_ui::text_input::{TextEditCommand, TextEditOutcome};
use quark_app::{InputEvent, ViewContext};

use crate::contracts::{CommandId, EditCx, Effects, SurfaceCx, Toast};

#[derive(Default)]
pub struct State {}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Dismiss,
}

pub fn new_state() -> State {
    State::default()
}

/// The overlay layer of `scx.window`, drawn above everything; `None`
/// when nothing is open there.
pub fn view(_state: &mut State, _scx: &SurfaceCx, _vcx: &mut ViewContext) -> Option<AnyElement> {
    None
}

pub fn update(_state: &mut State, action: Action, _scx: &SurfaceCx, _fx: &mut Effects) {
    match action {
        Action::Dismiss => {}
    }
}

/// Sees key input before every other surface while an overlay is open.
pub fn event(_state: &mut State, _event: &InputEvent, _scx: &SurfaceCx, _fx: &mut Effects) -> bool {
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

/// Commands the overlays own (`OpenPalette`). True when handled.
pub fn command(_state: &mut State, _id: CommandId, _scx: &SurfaceCx, _fx: &mut Effects) -> bool {
    false
}

/// Whether a modal is open in `scx.window`: background commands wait.
pub fn blocks_background(_state: &State, _scx: &SurfaceCx) -> bool {
    false
}

/// Show `toast` in the host of `scx.window`.
pub fn toast(_state: &mut State, _toast: Toast, _scx: &SurfaceCx) {}
