//! Settings (stream F): a modal with staged edits, Save/Cancel, and
//! previewed appearance that Cancel reverts.
//!
//! Opening copies the saved values into a staged copy; the form edits only
//! that. A theme choice previews at once (the whole app repaints in it);
//! Cancel, Escape, or a click outside puts the saved theme back. Save
//! validates first: an invalid field keeps the dialog open, shows its
//! error under it, and takes focus. While open the dialog is modal in its
//! window: Tab stays inside, and every command waits (see [`command`]).

pub mod forms;

use quark::view;
use quark_app::quark_ui::FocusId;
use quark_app::quark_ui::element::*;
use quark_app::quark_ui::icons::lucide;
use quark_app::quark_ui::style::Styled;
use quark_app::quark_ui::text_input::{TextEditCommand, TextEditOutcome, TextField};
use quark_app::winit::event::{ElementState, MouseButton};
use quark_app::{InputEvent, ViewContext};
use quark_components::{Button, ButtonStyle, HostId, Modal, SelectMsg, SelectState};

use crate::contracts::{
    CommandId, EditCx, Effect, Effects, SurfaceCx, ThemeChoice, Toast, ToastKind,
};
use crate::design::tokens;
use forms::{LIMIT, MODELS, THEMES, Values};

/// The dialog's accessibility id (the Modal names it after its title).
pub const DIALOG: &str = "modal:Settings";
const MAX_SIZE: (f32, f32) = (680.0, 560.0);

#[derive(Default)]
pub struct State {
    saved: Values,
    open: Option<Open>,
}

/// The dialog while it is open.
struct Open {
    host: HostId,
    return_focus: Option<FocusId>,
    staged: Values,
    theme: SelectState,
    model: SelectState,
    limit: TextField,
    limit_error: Option<String>,
    scroll: ScrollHandle,
    /// An IME composition in a field owns Escape.
    composing: bool,
}

impl Open {
    fn picker_open(&self) -> bool {
        self.theme.is_open() || self.model.is_open()
    }
}

impl State {
    /// The theme the app launched with, so Cancel reverts to it.
    pub fn with_theme(mut self, theme: ThemeChoice) -> Self {
        self.saved.theme = theme;
        self
    }

    /// The values Save last kept.
    pub fn saved(&self) -> Values {
        self.saved
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Theme(SelectMsg),
    Model(SelectMsg),
    FocusLimit,
    Save,
    Cancel,
}

pub fn new_state() -> State {
    State::default()
}

/// The settings modal, when open in `scx.window`.
pub fn view(state: &mut State, scx: &SurfaceCx, vcx: &mut ViewContext) -> Option<AnyElement> {
    let open = state.open.as_ref().filter(|o| o.host == scx.host)?;
    let (width, height) = scx.size;
    let theme = scx.theme;
    let theme_field = forms::select_field(
        "settings.theme",
        "Theme",
        &open.theme,
        forms::options(THEMES.iter().map(|(_, label)| *label)),
        scx.size,
        |msg| Action::Theme(msg).into(),
    );
    let model_field = forms::select_field(
        "settings.model",
        "Default model",
        &open.model,
        forms::options(MODELS),
        scx.size,
        |msg| Action::Model(msg).into(),
    );
    let limit_field = forms::limit_field(
        &open.limit,
        vcx.is_focused(LIMIT),
        open.limit_error.as_deref(),
        Action::FocusLimit.into(),
        theme,
    );
    let (meta, _) = tokens::TYPE_META;
    let about = view! {
        <text size={meta} color={theme.colors.text_muted}>
            "Quark Workbench runs a simulated agent against fixture files. Nothing \
             here reaches the network or your projects."
        </text>
    }
    .into_any();
    let scroll = div()
        .flex_col()
        .w_full()
        .flex_1()
        .min_h(0.0)
        .gap(tokens::SPACE_24)
        .track_scroll(&open.scroll)
        .overflow_y_scroll()
        .test_id("settings-body")
        .child(forms::section("Appearance", vec![theme_field], theme))
        .child(forms::section(
            "Agent",
            vec![model_field, limit_field],
            theme,
        ))
        .child(forms::section("About", vec![about], theme));
    let footer = div()
        .flex_row()
        .w_full()
        .flex_shrink_0()
        .items_center()
        .gap(tokens::SPACE_8)
        .child(div().flex_1())
        .child(
            Button::new(Action::Cancel)
                .label("Cancel")
                .style(ButtonStyle::Subtle),
        )
        .child(
            Button::new(Action::Save)
                .label("Save")
                .style(ButtonStyle::Filled),
        );
    // The footer lives in the body, not the Modal's footer slot, whose
    // spacer would split the free height with the scrolling area: the
    // area takes all of it and the footer stays pinned under it.
    let body = div()
        .flex_col()
        .w_full()
        .flex_1()
        .min_h(0.0)
        .gap(tokens::SPACE_16)
        .child(scroll)
        .child(footer);
    let modal = Modal::new(
        "Settings",
        "Changes apply when you save.",
        lucide::SETTINGS,
        MAX_SIZE.0,
        width,
        height,
        Action::Cancel,
    )
    .height(MAX_SIZE.1)
    .body_child(body);
    Some(modal.into_any())
}

pub fn update(state: &mut State, action: Action, scx: &SurfaceCx, fx: &mut Effects) {
    let Some(open) = state.open.as_mut() else {
        return;
    };
    match action {
        Action::Theme(msg) => {
            let options = forms::options(THEMES.iter().map(|(_, label)| *label));
            let outcome = open.theme.update(msg, &options, scx.now_ms);
            if let Some(i) = outcome.changed {
                open.staged.theme = THEMES[i].0;
                // Preview: the whole app repaints in the staged theme.
                fx.push(Effect::SetTheme(open.staged.theme));
            }
            if outcome.focus.is_some() {
                fx.push(Effect::Focus(outcome.focus));
            }
        }
        Action::Model(msg) => {
            let outcome = open.model.update(msg, &forms::options(MODELS), scx.now_ms);
            if let Some(i) = outcome.changed {
                open.staged.model = i;
            }
            if outcome.focus.is_some() {
                fx.push(Effect::Focus(outcome.focus));
            }
        }
        Action::FocusLimit => fx.push(Effect::Focus(Some(LIMIT))),
        Action::Save => save(state, fx),
        Action::Cancel => cancel(state, fx),
    }
}

fn save(state: &mut State, fx: &mut Effects) {
    let Some(open) = state.open.as_mut() else {
        return;
    };
    match forms::parse_limit(open.limit.text()) {
        Ok(limit) => open.staged.output_limit = limit,
        Err(message) => {
            open.limit_error = Some(message);
            fx.push(Effect::Focus(Some(LIMIT)));
            return;
        }
    }
    let Some(open) = state.open.take() else {
        return;
    };
    state.saved = open.staged;
    fx.push(Effect::Focus(open.return_focus));
    fx.push(Effect::Toast(Toast {
        kind: ToastKind::Success,
        text: "Settings saved".to_owned(),
        undo: None,
    }));
}

fn cancel(state: &mut State, fx: &mut Effects) {
    let Some(open) = state.open.take() else {
        return;
    };
    if open.staged.theme != state.saved.theme {
        fx.push(Effect::SetTheme(state.saved.theme));
    }
    fx.push(Effect::Focus(open.return_focus));
}

/// While open: Escape cancels, unless a picker is open (it closes itself)
/// or a composition owns it; right-clicks open nothing under the dialog.
pub fn event(state: &mut State, event: &InputEvent, scx: &SurfaceCx, fx: &mut Effects) -> bool {
    let Some(open) = state.open.as_mut().filter(|o| o.host == scx.host) else {
        return false;
    };
    match event {
        InputEvent::KeyPress(chord) => {
            let escape = chord
                .binding()
                .is_some_and(|b| b.key == "escape" && b.mods == Default::default());
            if !escape || open.composing || open.picker_open() {
                return false;
            }
            cancel(state, fx);
            true
        }
        InputEvent::PointerButton {
            button: MouseButton::Right,
            state: ElementState::Pressed,
        } => true,
        InputEvent::ImePreedit(text, _) => {
            open.composing = !text.is_empty();
            false
        }
        InputEvent::ImeCommit(_) | InputEvent::Focused(false) => {
            open.composing = false;
            false
        }
        _ => false,
    }
}

pub fn edit_text(
    state: &mut State,
    target: FocusId,
    command: TextEditCommand,
    ecx: &EditCx,
) -> Option<TextEditOutcome> {
    let open = state.open.as_mut().filter(|_| target == LIMIT)?;
    let outcome = open.limit.apply_at(command, ecx.now_ms);
    if outcome.text_changed {
        // The error described the old text.
        open.limit_error = None;
    }
    Some(outcome)
}

/// An IME composition in the limit field.
pub fn set_preedit(
    state: &mut State,
    target: FocusId,
    text: String,
    cursor: Option<(usize, usize)>,
) {
    if let Some(open) = state.open.as_mut().filter(|_| target == LIMIT) {
        open.limit.set_preedit(text, cursor);
    }
}

/// Commands settings owns (`OpenSettings`), and its modality: while the
/// dialog is open, every other command waits. Theme commands run while it
/// is closed become the saved theme. True when handled.
pub fn command(state: &mut State, id: CommandId, scx: &SurfaceCx, fx: &mut Effects) -> bool {
    if state.open.is_some() {
        return true;
    }
    match id {
        CommandId::OpenSettings => {
            let saved = state.saved;
            let mut theme = SelectState::new("settings.theme");
            theme.set_selected(Some(forms::theme_index(saved.theme)));
            let mut model = SelectState::new("settings.model");
            model.set_selected(Some(saved.model));
            let first = theme.focus_id();
            state.open = Some(Open {
                host: scx.host,
                return_focus: scx.focus,
                staged: saved,
                theme,
                model,
                limit: TextField::new(saved.output_limit.to_string()),
                limit_error: None,
                scroll: ScrollHandle::new(),
                composing: false,
            });
            fx.push(Effect::Focus(Some(first)));
            true
        }
        CommandId::ThemeSystem | CommandId::ThemeLight | CommandId::ThemeDark => {
            state.saved.theme = match id {
                CommandId::ThemeLight => ThemeChoice::Light,
                CommandId::ThemeDark => ThemeChoice::Dark,
                _ => ThemeChoice::System,
            };
            false
        }
        _ => false,
    }
}

/// Whether the settings modal is open (it blocks background commands).
pub fn is_open(state: &State) -> bool {
    state.open.is_some()
}
