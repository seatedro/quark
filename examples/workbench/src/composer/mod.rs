//! The composer (stream D): growing editor, completions, attachments,
//! model picker, and Send/Stop. Placeholder: one single-line field per
//! thread and a Send/Stop button. Drafts are keyed by `ThreadId`, so a
//! thread keeps its draft while another is selected.

pub mod attachments;
pub mod completions;

use std::collections::HashMap;

use quark::view;
use quark_app::quark_ui::FocusId;
use quark_app::quark_ui::element::*;
use quark_app::quark_ui::style::Styled;
use quark_app::quark_ui::text_input::{TextEditCommand, TextEditOutcome, TextField};
use quark_app::winit::keyboard::NamedKey;
use quark_app::{InputEvent, ViewContext};
use quark_components::{Button, ButtonStyle};

use crate::contracts::{CommandId, EditCx, Effect, Effects, SurfaceCx, ThreadId};

/// The composer's text field.
pub const INPUT: FocusId = FocusId::from_key("composer.input");
/// Height the placeholder composer takes below the timeline.
const HEIGHT: f32 = 112.0;

#[derive(Default)]
pub struct State {
    drafts: HashMap<ThreadId, TextField>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    FocusInput,
    Send,
    Stop,
}

pub fn new_state() -> State {
    State::default()
}

impl State {
    /// `thread`'s draft text ("" when it has none).
    pub fn draft(&self, thread: ThreadId) -> &str {
        self.drafts.get(&thread).map_or("", TextField::text)
    }
}

/// The height the composer needs at `scx.size.0` wide; the timeline gets
/// the rest of the thread panel.
pub fn height(_state: &State, _scx: &SurfaceCx) -> f32 {
    HEIGHT
}

pub fn view(state: &mut State, scx: &SurfaceCx, _vcx: &mut ViewContext) -> AnyElement {
    let colors = &scx.theme.colors;
    let (width, height) = scx.size;
    let thread = scx.model.selected;
    let field = state
        .drafts
        .entry(thread)
        .or_insert_with(|| TextField::new(""));
    let running = scx.model.run(thread).is_some();
    let input = text_input("Message", "")
        .w_full()
        .placeholder("Ask for a change, or type @ to mention a file")
        .focus_target(INPUT)
        .focused(scx.is_focused(INPUT))
        .field(field)
        .on_click(Action::FocusInput)
        .bare();
    let (label, action) = if running {
        ("Stop", Action::Stop)
    } else {
        ("Send", Action::Send)
    };
    view! {
        <div w={width} h={height} class="flex-col px-4 pb-4 pt-2 gap-[8]" bg={colors.background}>
            <div class="flex-col p-3 gap-[8] rounded-[12]" border={colors.border} bg={colors.surface}>
                <div class="w-full h-[40]">{input}</div>
                <div class="flex-row justify-end">
                    <Button on:click={action} variant={ButtonStyle::Filled} label={label} />
                </div>
            </div>
        </div>
    }
    .into_any()
}

pub fn update(state: &mut State, action: Action, scx: &SurfaceCx, fx: &mut Effects) {
    match action {
        Action::FocusInput => fx.push(Effect::Focus(Some(INPUT))),
        Action::Send => send(state, scx, fx),
        Action::Stop => fx.push(Effect::StopRun(scx.model.selected)),
    }
}

fn send(state: &mut State, scx: &SurfaceCx, fx: &mut Effects) {
    let thread = scx.model.selected;
    let Some(field) = state.drafts.get_mut(&thread) else {
        return;
    };
    let text = field.text().trim().to_owned();
    if text.is_empty() || scx.model.run(thread).is_some() {
        return;
    }
    field.set_text("");
    fx.push(Effect::SendPrompt {
        thread,
        text,
        attachments: Vec::new(),
    });
}

/// Enter in the focused field sends; Shift+Enter is left to the editor.
pub fn event(state: &mut State, event: &InputEvent, scx: &SurfaceCx, fx: &mut Effects) -> bool {
    let InputEvent::KeyPress(chord) = event else {
        return false;
    };
    if !scx.is_focused(INPUT) || chord.named() != Some(NamedKey::Enter) || chord.shift() {
        return false;
    }
    send(state, scx, fx);
    true
}

pub fn edit_text(
    state: &mut State,
    target: FocusId,
    command: TextEditCommand,
    ecx: &EditCx,
) -> Option<TextEditOutcome> {
    if target != INPUT {
        return None;
    }
    let field = state
        .drafts
        .entry(ecx.model.selected)
        .or_insert_with(|| TextField::new(""));
    Some(field.apply_at(command, ecx.now_ms))
}

/// The IME composition in `target` changed. Placeholder: stream D shows
/// the preedit and keeps Enter from sending while it is active.
pub fn set_preedit(
    _state: &mut State,
    _target: FocusId,
    _text: String,
    _cursor: Option<(usize, usize)>,
) {
}

/// Commands the composer owns (`FocusComposer`, `SendPrompt`, `StopRun`).
pub fn command(state: &mut State, id: CommandId, scx: &SurfaceCx, fx: &mut Effects) -> bool {
    match id {
        CommandId::FocusComposer => fx.push(Effect::Focus(Some(INPUT))),
        CommandId::SendPrompt => send(state, scx, fx),
        CommandId::StopRun => fx.push(Effect::StopRun(scx.model.selected)),
        _ => return false,
    }
    true
}
