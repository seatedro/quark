//! The composer (stream D): an editor that grows from 48 to 144 points and
//! then scrolls, `@` mention and `/` command suggestions at the caret,
//! attachment chips, the model and reasoning pickers, and Send/Stop.
//!
//! One [`Editor`] edits the selected thread's draft. Switching threads
//! parks the draft (text, chips, attachments) under its thread and loads
//! the next one, and bumps the completion generation so suggestions asked
//! for the old thread never open in the new one.
//!
//! Keys: Enter sends, Shift+Enter breaks the line, and Enter while an IME
//! composition is open goes to the IME. With suggestions open, the arrows,
//! Enter, Tab, and Escape drive the list instead. The Send button keeps
//! one focus target while it turns into Stop, so keyboard focus survives a
//! run starting and ending.

pub mod attachments;
pub mod completions;

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use accesskit::Role;
use quark::view;
use quark_app::quark_ui::FocusId;
use quark_app::quark_ui::element::{
    AnyElement, Binding, IntoAnyElement, ScrollActionBuilder, cached, div, inputs_hash, svg_icon,
    text,
};
use quark_app::quark_ui::icons::lucide;
use quark_app::quark_ui::style::Styled;
use quark_app::quark_ui::text_input::{
    Completion, CompletionKey, Editor, Insertion, RichText, TextEditCommand, TextEditOutcome,
    text_editor_element,
};
use quark_app::quark_ui::theme::ThemeColors;
use quark_app::{InputEvent, ViewContext};
use quark_components::{SelectMsg, SelectOption, SelectState, select};

use crate::assets;
use crate::contracts::{AttachmentId, CommandId, EditCx, Effect, Effects, SurfaceCx, ThreadId};
use attachments::{Attachment, Attachments, Kind};
use completions::Provider;

/// The composer's text field.
pub const INPUT: FocusId = FocusId::from_key("composer.input");
/// Stable id of the Send/Stop button, whatever it shows.
pub const SEND_ID: &str = "composer.send";
/// The Send/Stop button's focus target.
pub const SEND: FocusId = FocusId::from_key(SEND_ID);
const ATTACH_ID: &str = "composer.attach";

/// The text area's height: one line plus padding, up to six lines plus
/// padding; past that the text scrolls inside.
pub const MIN_TEXT_HEIGHT: f32 = 48.0;
pub const MAX_TEXT_HEIGHT: f32 = 144.0;
/// Space above and below the text inside the text area.
const TEXT_PAD: f32 = 6.0;
const FONT_SIZE: f32 = 14.0;
const LINE_HEIGHT: f32 = 22.0;
const TOOLBAR_HEIGHT: f32 = 32.0;
/// Padding inside the composer's card, and around it.
const CARD_PAD: f32 = 8.0;
const OUTER_TOP: f32 = 8.0;
const OUTER_BOTTOM: f32 = 16.0;
const CHIPS_HEIGHT: f32 = 28.0;
const ERROR_HEIGHT: f32 = 24.0;
const GAP: f32 = 8.0;
/// The widest the composer gets, matching the timeline's column.
const MAX_WIDTH: f32 = 760.0;

pub const MODELS: &[&str] = &["Atlas Pro", "Atlas Fast", "Atlas Mini"];
pub const REASONING: &[&str] = &["Low reasoning", "Medium reasoning", "High reasoning"];

/// A thread's parked draft.
#[derive(Debug, Default)]
struct Draft {
    text: RichText,
    attachments: Vec<Attachment>,
}

pub struct State {
    editor: Editor,
    completion: Completion,
    provider: Provider,
    attachments: Attachments,
    /// Drafts of the threads not shown.
    drafts: HashMap<ThreadId, Draft>,
    /// The thread whose draft the editor holds.
    thread: Option<ThreadId>,
    model: SelectState,
    reasoning: SelectState,
    model_options: Rc<[SelectOption]>,
    reasoning_options: Rc<[SelectOption]>,
    /// The editor's viewport height from the last frame (without padding).
    text_height: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    FocusInput,
    Send,
    Stop,
    /// A click on suggestion `n`.
    Pick(usize),
    RemoveAttachment(AttachmentId),
    /// The attach button: adds the fixture image.
    AttachFixture,
    /// Wheel over the editor, in lines.
    Scroll(i32),
    Model(SelectMsg),
    Reasoning(SelectMsg),
}

pub fn new_state() -> State {
    let options = |labels: &[&str]| -> Rc<[SelectOption]> {
        labels.iter().map(|l| SelectOption::new(*l)).collect()
    };
    let mut editor = Editor::default();
    editor.set_font_size(FONT_SIZE);
    editor.set_line_height(Some(LINE_HEIGHT));
    let mut model = SelectState::new("composer.model");
    model.set_selected(Some(0));
    let mut reasoning = SelectState::new("composer.reasoning");
    reasoning.set_selected(Some(1));
    State {
        editor,
        completion: Completion::new(completions::RULES),
        provider: Provider::default(),
        attachments: Attachments::default(),
        drafts: HashMap::new(),
        thread: None,
        model,
        reasoning,
        model_options: options(MODELS),
        reasoning_options: options(REASONING),
        text_height: MIN_TEXT_HEIGHT - 2.0 * TEXT_PAD,
    }
}

impl State {
    /// `thread`'s draft text ("" when it has none).
    pub fn draft(&self, thread: ThreadId) -> &str {
        if self.thread == Some(thread) {
            self.editor.text()
        } else {
            self.drafts
                .get(&thread)
                .map_or("", |d| d.text.text.as_str())
        }
    }

    pub fn editor(&self) -> &Editor {
        &self.editor
    }

    pub fn completion(&self) -> &Completion {
        &self.completion
    }

    pub fn attachments(&self) -> &Attachments {
        &self.attachments
    }

    /// The chosen model's name.
    pub fn model(&self) -> &str {
        MODELS[self.model.selected().unwrap_or(0)]
    }

    /// The completion generation: bumped each time the editor shows
    /// another thread's draft.
    pub fn thread_generation(&self) -> u64 {
        self.provider.thread_gen
    }

    /// Whether a transient surface of the composer (suggestions, a picker
    /// list) is open, so Escape belongs to it.
    pub fn has_transient(&self) -> bool {
        self.completion.is_open() || self.model.is_open() || self.reasoning.is_open()
    }

    /// Show `thread`'s draft, parking the current one.
    fn show_thread(&mut self, thread: ThreadId) {
        if self.thread == Some(thread) {
            return;
        }
        if let Some(old) = self.thread {
            let draft = Draft {
                text: self.editor.rich_text(),
                attachments: self.attachments.take(),
            };
            if draft.text.text.is_empty() && draft.attachments.is_empty() {
                self.drafts.remove(&old);
            } else {
                self.drafts.insert(old, draft);
            }
        }
        let draft = self.drafts.remove(&thread).unwrap_or_default();
        self.editor.set_rich_text(&draft.text);
        self.attachments.take();
        self.attachments.items = draft.attachments;
        self.thread = Some(thread);
        // Answers to the old thread's lookups now miss.
        self.provider.thread_gen += 1;
        self.completion = Completion::new(completions::RULES);
    }

    fn refresh_completion(&mut self) {
        self.completion.refresh(&self.editor, &mut self.provider);
    }

    /// Answer the file lookups asked since the last frame, as a worker
    /// would reply.
    fn answer_lookups(&mut self, scx: &SurfaceCx) {
        for lookup in self.provider.take_pending() {
            let answer = completions::answer(&lookup, scx.model.files.paths());
            completions::deliver(&mut self.completion, &self.provider, answer);
        }
    }

    /// Hand in a lookup answer that arrived from elsewhere; dropped when it
    /// is for another thread or an older query.
    pub fn deliver(&mut self, answer: completions::Answer) -> bool {
        completions::deliver(&mut self.completion, &self.provider, answer)
    }

    fn running(&self, scx: &SurfaceCx) -> bool {
        scx.model.run(scx.model.selected).is_some()
    }
}

/// The height the composer needs, as of its last frame; the timeline gets
/// the rest of the thread panel. A change shows on the next frame, which
/// the view asks for.
pub fn height(state: &State, scx: &SurfaceCx) -> f32 {
    total_height(state, state.text_height, zoom(scx.theme))
}

/// The theme's zoom: the design's points are at 100%.
fn zoom(theme: &quark_app::quark_ui::theme::Theme) -> f32 {
    theme.metrics.ui_scale()
}

/// Everything but the text area is fixed: the padding around the card and
/// inside it, the optional error and chip rows, and the toolbar.
fn total_height(state: &State, text_height: f32, z: f32) -> f32 {
    let mut h = (OUTER_TOP + OUTER_BOTTOM + 2.0 * CARD_PAD) * z;
    h += text_height + (2.0 * TEXT_PAD + GAP + TOOLBAR_HEIGHT) * z;
    if !state.attachments.items.is_empty() {
        h += (CHIPS_HEIGHT + GAP) * z;
    }
    if state.attachments.error.is_some() {
        h += (ERROR_HEIGHT + GAP) * z;
    }
    h
}

pub fn view(state: &mut State, scx: &SurfaceCx, vcx: &mut ViewContext) -> AnyElement {
    state.show_thread(scx.model.selected);
    state.answer_lookups(scx);
    let theme = scx.theme;
    let colors = &theme.colors;
    let z = zoom(theme);
    let (width, _) = scx.size;
    let column = (width - 32.0 * z).clamp(0.0, MAX_WIDTH * z);
    let text_w = (column - (2.0 * CARD_PAD + 4.0) * z).max(40.0);

    let before = height(state, scx);
    state
        .editor
        .set_clock(vcx.frame.elapsed().as_millis() as u64);
    state.editor.set_font_size(FONT_SIZE * z);
    state.editor.set_line_height(Some(LINE_HEIGHT * z));
    state.text_height = state.editor.flush_fit(
        &mut vcx.frame.text().system,
        text_w,
        (MIN_TEXT_HEIGHT - 2.0 * TEXT_PAD) * z,
        (MAX_TEXT_HEIGHT - 2.0 * TEXT_PAD) * z,
    );
    let height = total_height(state, state.text_height, z);
    if (height - before).abs() > 0.5 || (height - scx.size.1).abs() > 0.5 {
        vcx.frame.request_frame();
    }

    let running = state.running(scx);
    let empty = state.editor.is_empty() && state.attachments.items.is_empty();
    let (send_label, send_action) = if running {
        ("Stop", Action::Stop)
    } else {
        ("Send", Action::Send)
    };
    let send_enabled = running || !empty;
    let window = vcx.frame.size();

    let editor = text_editor_element(
        INPUT,
        ScrollActionBuilder::new(|lines| Action::Scroll(lines).into()),
    )
    .editor_snapshot(&state.editor)
    .label("Message")
    .placeholder("Ask for a change, or type @ to mention a file")
    .focused(scx.is_focused(INPUT))
    .font_size(FONT_SIZE * z)
    .text_color(colors.text)
    .w(text_w)
    .h(state.text_height);

    let toolbar = Toolbar {
        model: state.model.clone(),
        model_options: state.model_options.clone(),
        reasoning: state.reasoning.clone(),
        reasoning_options: state.reasoning_options.clone(),
        window,
        z,
        send_label,
        send_action,
        send_enabled,
    }
    .view(theme);
    let popup = completions::popup(
        &state.completion,
        state.editor.caret_anchor(),
        (TEXT_PAD + CARD_PAD) * z,
        theme,
        window,
    );
    let error = state.attachments.error.clone();

    view! {
        <div
            w={width}
            h={height}
            class="flex-col items-center"
            pt={OUTER_TOP * z}
            pb={OUTER_BOTTOM * z}
            bg={colors.background}
        >
            <div w={column} class="flex-col" gap={GAP * z}>
                if let Some(error) = error {
                    <div
                        accessibility_role={Role::Alert}
                        aria-label={error.clone()}
                        test_id="composer.error"
                        class="flex-row items-center gap-[6] px-2 rounded-[6]"
                        h={ERROR_HEIGHT * z}
                    >
                        {svg_icon(lucide::ALERT_CIRCLE, 14.0).color(colors.status_error)}
                        <text class="text-xs" color={colors.status_error}>{error}</text>
                    </div>
                }
                <div
                    class="flex-col"
                    rounded={12.0 * z}
                    gap={GAP * z}
                    p={CARD_PAD * z}
                    border={colors.border_variant}
                    bg={colors.surface}
                >
                    if !state.attachments.items.is_empty() {
                        <div h={CHIPS_HEIGHT * z} class="flex-row overflow-hidden">
                            {attachments::chips(&state.attachments, theme)}
                        </div>
                    }
                    <div px={2.0 * z} py={TEXT_PAD * z}>{editor}</div>
                    {toolbar}
                </div>
            </div>
            if let Some(popup) = popup {
                {popup}
            }
        </div>
    }
    .into_any()
}

/// The row under the text area: model and reasoning pickers, attach, and
/// Send/Stop. It holds no text input, so it replays from the element cache
/// until a picker, the run state, or the draft's emptiness changes.
struct Toolbar {
    model: SelectState,
    model_options: Rc<[SelectOption]>,
    reasoning: SelectState,
    reasoning_options: Rc<[SelectOption]>,
    window: (f32, f32),
    z: f32,
    send_label: &'static str,
    send_action: Action,
    send_enabled: bool,
}

impl Toolbar {
    fn view(self, theme: &quark_app::quark_ui::theme::Theme) -> AnyElement {
        let picker = |s: &SelectState, options: &Rc<[SelectOption]>| {
            (
                s.selected(),
                s.is_open(),
                s.highlighted(),
                Rc::as_ptr(options) as *const u8,
            )
        };
        let hash = inputs_hash(&(
            picker(&self.model, &self.model_options),
            picker(&self.reasoning, &self.reasoning_options),
            (
                self.window.0.to_bits(),
                self.window.1.to_bits(),
                self.z.to_bits(),
            ),
            (self.send_label, self.send_enabled),
        ));
        let height = TOOLBAR_HEIGHT * self.z;
        let colors = theme.colors;
        cached("composer.toolbar", hash, move || self.build(colors))
            .w_full()
            .h(height)
            .into_any()
    }

    fn build(self, colors: ThemeColors) -> AnyElement {
        let z = self.z;
        let model = select(&self.model, self.model_options, |m| Action::Model(m).into())
            .label("Model")
            .width(132.0 * z)
            .viewport(self.window);
        let reasoning = select(&self.reasoning, self.reasoning_options, |m| {
            Action::Reasoning(m).into()
        })
        .label("Reasoning")
        .width(156.0 * z)
        .viewport(self.window);
        view! {
            <div class="flex-row items-center" gap={GAP * z} h={TOOLBAR_HEIGHT * z}>
                {model}
                {reasoning}
                <div class="flex-1" />
                <div
                    id={ATTACH_ID}
                    accessibility_role={Role::Button}
                    aria-label="Attach fixture image"
                    tooltip="Attach fixture image"
                    class="w-[28] h-[28] items-center justify-center rounded-[6]"
                    hover_bg={colors.element_hover}
                    on:click={Action::AttachFixture}
                >
                    {svg_icon(lucide::PLUS, 16.0).color(colors.text_muted)}
                </div>
                {send_button(
                    self.send_label,
                    self.send_action,
                    self.send_enabled,
                    &colors
                )}
            </div>
        }
        .into_any()
    }
}

/// Send or Stop under one id, so focus stays on it when it changes. It
/// stays clickable while disabled (a click does nothing) so it never
/// drops out of the focus order under the keyboard.
fn send_button(label: &str, action: Action, enabled: bool, colors: &ThemeColors) -> AnyElement {
    let (bg, fg) = if enabled {
        (colors.accent, colors.on_accent)
    } else {
        (colors.element_background, colors.text_muted)
    };
    let stop = label == "Stop";
    view! {
        <div
            id={SEND_ID}
            accessibility_role={Role::Button}
            aria-label={label}
            aria-disabled={!enabled}
            class="w-[32] h-[32] items-center justify-center rounded-[8]"
            bg={bg}
            on:click={action}
        >
            if stop {
                <div class="w-[10] h-[10] rounded-[2]" bg={fg} />
            } else {
                {svg_icon(lucide::ARROW_UP, 16.0).color(fg)}
            }
        </div>
    }
    .into_any()
}

pub fn update(state: &mut State, action: Action, scx: &SurfaceCx, fx: &mut Effects) {
    match action {
        Action::FocusInput => fx.push(Effect::Focus(Some(INPUT))),
        Action::Send => send(state, scx, fx),
        Action::Stop => stop(scx, fx),
        Action::Pick(i) => {
            state.completion.accept(i, &mut state.editor);
            state.refresh_completion();
            fx.push(Effect::Focus(Some(INPUT)));
        }
        Action::RemoveAttachment(id) => {
            let name = state
                .attachments
                .items
                .iter()
                .find(|a| a.id == id)
                .map(|a| a.name.clone());
            state.attachments.remove(id);
            if let Some(name) = name {
                fx.push(Effect::Announce(format!("Removed {name}")));
            }
            fx.push(Effect::Focus(Some(INPUT)));
        }
        Action::AttachFixture => {
            let bytes: Arc<[u8]> = Arc::from(assets::ATTACHMENT_PNG);
            if state
                .attachments
                .add("attachment.png".into(), Kind::Image, bytes)
            {
                fx.push(Effect::Announce("Attached attachment.png".into()));
            }
        }
        Action::Scroll(lines) => {
            let step = state.editor.scroll_line_height_px();
            state.editor.scroll(lines as f32 * step);
        }
        Action::Model(msg) => {
            let out = state.model.update(msg, &state.model_options, scx.now_ms);
            if let Some(focus) = out.focus {
                fx.push(Effect::Focus(Some(focus)));
            }
        }
        Action::Reasoning(msg) => {
            let out = state
                .reasoning
                .update(msg, &state.reasoning_options, scx.now_ms);
            if let Some(focus) = out.focus {
                fx.push(Effect::Focus(Some(focus)));
            }
        }
    }
}

fn stop(scx: &SurfaceCx, fx: &mut Effects) {
    if scx.model.run(scx.model.selected).is_some() {
        fx.push(Effect::StopRun(scx.model.selected));
    }
}

fn send(state: &mut State, scx: &SurfaceCx, fx: &mut Effects) {
    let thread = scx.model.selected;
    state.show_thread(thread);
    let prompt = state.editor.rich_text();
    let empty = prompt.text.trim().is_empty() && state.attachments.items.is_empty();
    if empty || state.running(scx) || state.editor.preedit().is_some() {
        return;
    }
    fx.push(Effect::SendPrompt {
        thread,
        text: prompt.export().trim().to_owned(),
        attachments: state.attachments.ids(),
    });
    state.editor.set_text("");
    state.attachments.take();
    state.refresh_completion();
}

/// Keys the composer takes before its editor: the suggestion list, then
/// Enter to send and Shift+Enter for a newline. Enter during an IME
/// composition is the IME's.
pub fn event(state: &mut State, event: &InputEvent, scx: &SurfaceCx, fx: &mut Effects) -> bool {
    let InputEvent::KeyPress(chord) = event else {
        if let InputEvent::FileDropped(path) = event
            && scx.is_focused(INPUT)
        {
            state.show_thread(scx.model.selected);
            state.editor.drop_path(path, &mut state.attachments);
            return true;
        }
        return false;
    };
    if !scx.is_focused(INPUT) {
        return false;
    }
    let Some(binding) = chord.binding() else {
        return false;
    };
    state.show_thread(scx.model.selected);
    key(state, &binding, scx, fx)
}

fn key(state: &mut State, binding: &Binding, scx: &SurfaceCx, fx: &mut Effects) -> bool {
    if state.editor.preedit().is_some() {
        return false;
    }
    match state.completion.handle_key(binding, &mut state.editor) {
        CompletionKey::Ignored => {}
        CompletionKey::Handled | CompletionKey::Accepted(_) => {
            state.refresh_completion();
            return true;
        }
    }
    let m = binding.mods;
    if binding.key != "enter" || m.cmd || m.ctrl || m.alt {
        return false;
    }
    if m.shift {
        state.editor.insert(&Insertion::Text("\n".into()));
        state.attachments.edits += 1;
        state.refresh_completion();
    } else {
        send(state, scx, fx);
    }
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
    state.show_thread(ecx.model.selected);
    // The adapter repaints only for text or caret changes, so a change to
    // the chips alone reports itself as a caret change to be shown.
    let shown = TextEditOutcome {
        selection_changed: true,
        ..TextEditOutcome::default()
    };
    // Undo after removing a chip, before typing again, puts the chip back.
    if command == TextEditCommand::Undo && state.attachments.undo_remove() {
        return Some(shown);
    }
    state.editor.set_clock(ecx.now_ms);
    let chips = (
        state.attachments.items.len(),
        state.attachments.error.clone(),
    );
    let mut outcome = state.editor.apply_with(command, &mut state.attachments);
    if outcome.text_changed {
        state.attachments.edits += 1;
    }
    if chips
        != (
            state.attachments.items.len(),
            state.attachments.error.clone(),
        )
    {
        outcome.selection_changed = true;
    }
    if outcome.text_changed || outcome.selection_changed {
        state.refresh_completion();
    }
    Some(outcome)
}

/// An IME composition in the editor changed.
pub fn set_preedit(
    state: &mut State,
    target: FocusId,
    text: String,
    cursor: Option<(usize, usize)>,
) -> bool {
    if target != INPUT {
        return false;
    }
    state.editor.set_preedit(text, cursor);
    state.refresh_completion();
    true
}

/// Commands the composer owns (`FocusComposer`, `SendPrompt`, `StopRun`).
pub fn command(state: &mut State, id: CommandId, scx: &SurfaceCx, fx: &mut Effects) -> bool {
    match id {
        CommandId::FocusComposer => fx.push(Effect::Focus(Some(INPUT))),
        CommandId::SendPrompt => send(state, scx, fx),
        CommandId::StopRun => stop(scx, fx),
        _ => return false,
    }
    true
}
