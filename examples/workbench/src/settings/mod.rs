//! Settings (stream F): a modal with staged edits, Save/Cancel, and
//! previewed appearance that Cancel reverts.

pub mod forms;

use quark_app::quark_ui::FocusId;
use quark_app::quark_ui::element::AnyElement;
use quark_app::quark_ui::text_input::{TextEditCommand, TextEditOutcome};
use quark_app::{InputEvent, ViewContext};

use crate::contracts::{CommandId, EditCx, Effects, SurfaceCx};

#[derive(Default)]
pub struct State {}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Save,
    Cancel,
}

pub fn new_state() -> State {
    State::default()
}

/// The settings modal, when open in `scx.window`.
pub fn view(_state: &mut State, _scx: &SurfaceCx, _vcx: &mut ViewContext) -> Option<AnyElement> {
    None
}

pub fn update(_state: &mut State, action: Action, _scx: &SurfaceCx, _fx: &mut Effects) {
    match action {
        Action::Save | Action::Cancel => {}
    }
}

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

/// Commands settings owns (`OpenSettings`). True when handled.
pub fn command(_state: &mut State, _id: CommandId, _scx: &SurfaceCx, _fx: &mut Effects) -> bool {
    false
}

/// Whether the settings modal is open (it blocks background commands).
pub fn is_open(_state: &State) -> bool {
    false
}
