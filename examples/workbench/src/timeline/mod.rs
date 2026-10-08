//! The transcript (stream C): virtualized markdown rows, tool cards,
//! Jump to latest, and find. Placeholder: the selected thread's newest rows
//! as plain text.

pub mod rows;
pub mod tool_card;

use quark::view;
use quark_app::quark_ui::FocusId;
use quark_app::quark_ui::element::*;
use quark_app::quark_ui::style::Styled;
use quark_app::quark_ui::text_input::{TextEditCommand, TextEditOutcome};
use quark_app::{InputEvent, ViewContext};

use crate::contracts::{CommandId, EditCx, Effects, SurfaceCx, ToolId};

/// Per-thread document state lives here, keyed by `ThreadId`.
#[derive(Default)]
pub struct State {}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Retry a failed tool or error row.
    Retry(ToolId),
}

pub fn new_state() -> State {
    State::default()
}

/// The transcript of `scx.model.selected`, filling `scx.size`.
pub fn view(_state: &mut State, scx: &SurfaceCx, _vcx: &mut ViewContext) -> AnyElement {
    let colors = &scx.theme.colors;
    let (width, height) = scx.size;
    let rows = scx
        .model
        .selected_thread()
        .map(|t| t.transcript.rows())
        .unwrap_or_default();
    let tail = &rows[rows.len().saturating_sub(12)..];
    view! {
        <div w={width} h={height} class="flex-col p-4 gap-[8] overflow-hidden"
             bg={colors.background} aria-label="Transcript">
            for row in tail {
                <text size={13.0} color={colors.text}>
                    {format!("{}: {}", row.author(), row.tool.as_ref().map_or(row.markdown.as_str(), |t| t.target.as_str()))}
                </text>
            }
        </div>
    }
    .into_any()
}

pub fn update(_state: &mut State, action: Action, scx: &SurfaceCx, fx: &mut Effects) {
    match action {
        Action::Retry(tool) => fx.push(crate::contracts::Effect::RetryTool {
            thread: scx.model.selected,
            tool,
        }),
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

/// Commands the timeline owns (`FindInDocument`). True when handled.
pub fn command(_state: &mut State, _id: CommandId, _scx: &SurfaceCx, _fx: &mut Effects) -> bool {
    false
}
