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

use quark_components::{Button, ButtonStyle};

use crate::contracts::{CommandId, EditCx, Effect, Effects, SurfaceCx, ToolId};
use crate::model::ToolStatus;

/// Per-thread document state lives here, keyed by `ThreadId`.
#[derive(Default)]
pub struct State {}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Retry a failed tool or error row.
    Retry(ToolId),
    /// The empty thread's "Review changes" starter prompt.
    ReviewChanges,
}

/// The empty state's starter prompt.
pub const REVIEW_PROMPT: &str = "Review the uncommitted changes and summarize the risks.";

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
    if rows.is_empty() {
        return view! {
            <div w={width} h={height} class="flex-col items-center justify-center gap-[12]"
                 bg={colors.background}>
                <text size={22.0} class="font-semibold" color={colors.text_strong}>"Start a thread"</text>
                <Button on:click={Action::ReviewChanges} variant={ButtonStyle::Subtle}
                        label="Review changes" />
            </div>
        }
        .into_any();
    }
    let retry = tail
        .iter()
        .rev()
        .find(|r| {
            r.retry
                || r.tool
                    .as_ref()
                    .is_some_and(|t| t.status == ToolStatus::Failed)
        })
        .map(|r| ToolId(r.id.0));
    view! {
        <div w={width} h={height} class="flex-col p-4 gap-[8] overflow-hidden"
             bg={colors.background} aria-label="Transcript">
            for row in tail {
                <text size={13.0} color={colors.text}>
                    {format!("{}: {}", row.author(), row.tool.as_ref().map_or(row.markdown.as_str(), |t| t.target.as_str()))}
                </text>
            }
            if let Some(tool) = retry {
                <Button on:click={Action::Retry(tool)} label="Retry" />
            }
        </div>
    }
    .into_any()
}

pub fn update(_state: &mut State, action: Action, scx: &SurfaceCx, fx: &mut Effects) {
    match action {
        Action::Retry(tool) => fx.push(Effect::RetryTool {
            thread: scx.model.selected,
            tool,
        }),
        Action::ReviewChanges => fx.push(Effect::SendPrompt {
            thread: scx.model.selected,
            text: REVIEW_PROMPT.to_owned(),
            attachments: Vec::new(),
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
