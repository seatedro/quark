//! The transcript (stream C): one virtualized [`MarkdownDocument`] per
//! thread, mirrored from the model's change log, with tool cards, the
//! streaming cursor, Jump to latest, copy, and find.
//!
//! Each thread keeps its document while another is shown, so its scroll
//! anchor, selection, and expanded cards survive a thread switch. The
//! document is fed incrementally: the view applies the transcript changes
//! since the last frame (appends, history batches landing above, updates
//! from streaming), never diffing whole threads.

pub mod rows;
pub mod tool_card;

use std::collections::HashMap;

use quark::view;
use quark_app::quark_ui::FocusId;
use quark_app::quark_ui::document::{
    DocumentCommand, DocumentEvent, FindBarActions, MarkdownDocument, MarkdownEntry, RowAdornment,
    TextMeasurer, find_bar, key_command,
};
use quark_app::quark_ui::element::*;
use quark_app::quark_ui::icons::lucide;
use quark_app::quark_ui::style::Styled;
use quark_app::quark_ui::text_input::{TextEditCommand, TextEditOutcome, TextField};
use quark_app::quark_ui::virtual_list::{RowKey, ScrollAlign};
use quark_app::winit::keyboard::NamedKey;
use quark_app::{InputEvent, ViewContext};

use quark_components::{Button, ButtonStyle};

use crate::assets;
use crate::contracts::{
    CommandId, EditCx, Effect, Effects, MessageId, SurfaceCx, ThreadId, Toast, ToastKind, ToolId,
};
use crate::design::tokens;
use crate::model::{ChangeKind, Row, Thread};

/// The find field above the transcript.
pub const FIND_FIELD: FocusId = FocusId::from_key("timeline.find");

/// Per-thread transcript documents, keyed by `ThreadId`.
#[derive(Default)]
pub struct State {
    threads: HashMap<ThreadId, ThreadView>,
    /// The find field, while find is open.
    find: Option<TextField>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Retry a failed tool or error row.
    Retry(ToolId),
    /// The empty thread's "Review changes" starter prompt.
    ReviewChanges,
    /// Pointer, wheel, and scrollbar input from the document, and a code
    /// block's wrap toggle.
    Document(DocumentEvent),
    /// A tool card's disclosure.
    ToggleTool(ToolId),
    /// A code block's Copy: the block's whole source.
    CopyCode(String),
    JumpToLatest,
    FindNext,
    FindPrev,
    CloseFind,
}

/// The empty state's starter prompt.
pub const REVIEW_PROMPT: &str = "Review the uncommitted changes and summarize the risks.";

pub fn new_state() -> State {
    State::default()
}

/// One thread's document and what it mirrors.
pub struct ThreadView {
    doc: MarkdownDocument,
    /// The transcript change the document is in step with.
    seq: u64,
    /// Cards the user opened or closed; the rest follow their status.
    expanded: HashMap<ToolId, bool>,
    /// Disclosure focus targets of the thread's tool cards, so the
    /// document's keys work while one of them has focus.
    disclosures: HashMap<FocusId, MessageId>,
    /// What each row's chrome and adornments were last built from, so a
    /// streamed chunk only replaces the row's markdown.
    shapes: HashMap<MessageId, u64>,
}

impl ThreadView {
    fn new(thread: &Thread) -> Self {
        let mut doc = MarkdownDocument::new(rows::document_style());
        doc.set_decorator(rows::TimelineChrome);
        doc.document_mut().set_code_toolbar(true);
        doc.set_image_loader(assets::image_loader());
        let (w, h) = assets::PREVIEW_SIZE;
        doc.hint_image_size("preview.png", w, h);
        if let Some(store) = assets::grammar_store() {
            doc.set_grammar_store(store);
        }
        let mut view = Self {
            doc,
            seq: thread.transcript.last_seq(),
            expanded: HashMap::new(),
            disclosures: HashMap::new(),
            shapes: HashMap::new(),
        };
        let entries: Vec<(MarkdownEntry, _)> = thread
            .transcript
            .rows()
            .iter()
            .map(|row| view.entry(row))
            .collect();
        view.add(entries, false);
        view.doc.document_mut().scroll_to_bottom();
        view
    }

    pub fn document(&self) -> &MarkdownDocument {
        &self.doc
    }

    /// Whether `focus` is on one of the transcript's own controls (a card's
    /// disclosure, a wide block's scroll), where the document's keys apply.
    fn owns_focus(&self, focus: FocusId) -> bool {
        self.disclosures.contains_key(&focus) || self.doc.document().owns_focus(focus)
    }

    fn is_expanded(&self, row: &Row) -> bool {
        let Some(call) = &row.tool else {
            return false;
        };
        self.expanded
            .get(&ToolId(row.id.0))
            .copied()
            .unwrap_or_else(|| tool_card::expanded_by_default(call.status))
    }

    /// `row` as a new document entry, with its adornments.
    fn entry(&mut self, row: &Row) -> (MarkdownEntry, Vec<RowAdornment>) {
        if row.tool.is_some() {
            self.disclosures
                .insert(tool_card::disclosure_focus(ToolId(row.id.0)), row.id);
        }
        let expanded = self.is_expanded(row);
        self.shapes.insert(row.id, rows::shape(row, expanded));
        let content = rows::content(row, expanded);
        (
            MarkdownEntry {
                row: RowKey(row.id.0),
                chrome: content.chrome,
                markdown: content.markdown,
            },
            content.adornments,
        )
    }

    fn add(&mut self, entries: Vec<(MarkdownEntry, Vec<RowAdornment>)>, prepend: bool) {
        let keys: Vec<RowKey> = entries.iter().map(|(e, _)| e.row).collect();
        let (entries, adornments): (Vec<_>, Vec<_>) = entries.into_iter().unzip();
        let added = if prepend {
            self.doc.prepend(entries)
        } else {
            self.doc.extend(entries)
        };
        // A batch the model logged twice is a model bug; keep what landed.
        if added.is_err() {
            return;
        }
        for (key, adornments) in keys.into_iter().zip(adornments) {
            if !adornments.is_empty() {
                let _ = self.doc.set_adornments(key, adornments);
            }
        }
    }

    /// Brings `row`'s document row up to date.
    fn refresh(&mut self, row: &Row) {
        let key = RowKey(row.id.0);
        let expanded = self.is_expanded(row);
        let shape = rows::shape(row, expanded);
        if row.tool.is_none() && self.shapes.get(&row.id) == Some(&shape) {
            let _ = self.doc.set_markdown(key, &row.markdown);
            return;
        }
        self.shapes.insert(row.id, shape);
        let content = rows::content(row, expanded);
        let _ = self.doc.set_markdown(key, &content.markdown);
        let _ = self.doc.set_chrome(key, content.chrome);
        let _ = self.doc.set_adornments(key, content.adornments);
    }

    /// Applies the transcript changes since the last sync.
    fn sync(&mut self, thread: &Thread) {
        let transcript = &thread.transcript;
        let changes = transcript.changes_since(self.seq);
        if changes.is_empty() {
            return;
        }
        // History lands above everything: the rows prepended since the last
        // sync are the transcript's first ones, oldest first.
        let prepended = changes
            .iter()
            .filter(|c| c.kind == ChangeKind::Prepended)
            .count();
        if prepended > 0 {
            let entries = transcript.rows()[..prepended]
                .iter()
                .map(|row| self.entry(row))
                .collect();
            self.add(entries, true);
        }
        let mut appended = Vec::new();
        for change in changes {
            let Some(row) = transcript.get(change.row) else {
                continue;
            };
            match change.kind {
                ChangeKind::Prepended => {}
                ChangeKind::Appended => appended.push(self.entry(row)),
                ChangeKind::Updated => {
                    if !appended.is_empty() {
                        self.add(std::mem::take(&mut appended), false);
                    }
                    self.refresh(row);
                }
            }
        }
        if !appended.is_empty() {
            self.add(appended, false);
        }
        self.seq = transcript.last_seq();
    }
}

impl State {
    /// The document of `thread`, once the timeline has shown it.
    pub fn thread_view(&self, thread: ThreadId) -> Option<&ThreadView> {
        self.threads.get(&thread)
    }

    /// Measures every transcript row still holding an estimate, blocking
    /// until done, so tests and perf runs compare settled frames rather
    /// than frames racing the background measurer.
    pub fn finish_measures(&mut self) {
        for view in self.threads.values_mut() {
            view.doc.finish_measures();
            view.doc.finish_highlights();
            view.doc.finish_images();
        }
    }

    /// The selected thread's view, once shown, with the thread.
    fn selected<'s, 'm>(
        &'s mut self,
        scx: &SurfaceCx<'m>,
    ) -> Option<(&'s mut ThreadView, &'m Thread)> {
        let model: &'m crate::model::Model = scx.model;
        let thread = model.selected_thread()?;
        let view = self.threads.get_mut(&thread.id)?;
        Some((view, thread))
    }
}

/// The empty thread's starter.
fn empty_state(scx: &SurfaceCx) -> AnyElement {
    let colors = &scx.theme.colors;
    let (width, height) = scx.size;
    view! {
        <div w={width} h={height} class="flex-col items-center justify-center gap-[12]"
             bg={colors.background}>
            <text size={tokens::TYPE_EMPTY_HEADING.0} class="font-semibold" color={colors.text_strong}>
                "Start a thread"
            </text>
            <Button on:click={Action::ReviewChanges} variant={ButtonStyle::Subtle}
                    label="Review changes" />
        </div>
    }
    .into_any()
}

/// The transcript of `scx.model.selected`, filling `scx.size`.
pub fn view(state: &mut State, scx: &SurfaceCx, vcx: &mut ViewContext) -> AnyElement {
    let Some(thread) = scx.model.selected_thread() else {
        return empty_state(scx);
    };
    if thread.transcript.is_empty() {
        return empty_state(scx);
    }
    let tv = state
        .threads
        .entry(thread.id)
        .or_insert_with(|| ThreadView::new(thread));
    tv.sync(thread);
    // A focused card stays in the tree while it scrolls away.
    let kept = scx
        .focus
        .and_then(|focus| tv.disclosures.get(&focus))
        .map(|id| RowKey(id.0));
    tv.doc.document_mut().keep_materialized(kept);
    tv.doc.poll_highlights();
    tv.doc.poll_images();

    let colors = &scx.theme.colors;
    let (width, height) = scx.size;
    let style = rows::document_style();
    let doc_width = width.min(tokens::TIMELINE_MAX_WIDTH + style.pad_x * 2.0);
    let text = vcx.frame.text();
    let mut measurer = TextMeasurer::new(
        &mut text.system,
        &mut text.layouts,
        style.font_size,
        scx.scale,
    );
    tv.doc.prepare(doc_width, height, scx.now_ms, &mut measurer);
    if tv.doc.is_measuring() || tv.doc.is_loading_images() {
        vcx.frame.request_frame();
    }
    let element = tv
        .doc
        .element(scx.theme, |event| Action::Document(event).into())
        .on_copy_code(|copy| Action::CopyCode(copy.text.to_string()).into())
        .label("Transcript")
        .scrollbar_auto_hide();

    // Jump to latest appears without moving the view.
    let jump = tv.doc.document().has_content_below().then(|| {
        view! {
            <div class="absolute" left={(doc_width - 150.0) * 0.5} top={height - 52.0}
                 w={150.0} h={36.0} z_index={5}>
                <Button on:click={Action::JumpToLatest} icon={lucide::ARROW_DOWN}
                        variant={ButtonStyle::Filled} label="Jump to latest" />
            </div>
        }
        .into_any()
    });
    let find = state.find.as_ref().map(|field| {
        let bar = find_bar(
            tv.doc.document().find(),
            field,
            FIND_FIELD,
            scx.is_focused(FIND_FIELD),
            FindBarActions {
                next: Action::FindNext.into(),
                prev: Action::FindPrev.into(),
                close: Action::CloseFind.into(),
            },
            scx.theme,
        );
        view! {
            <div class="absolute" left={(doc_width - 320.0).max(0.0)} top={8.0} w={320.0}
                 z_index={6}>
                {bar}
            </div>
        }
        .into_any()
    });
    view! {
        <div w={width} h={height} class="flex-row justify-center" bg={colors.background}>
            <div w={doc_width} h={height} class="relative shrink-0">
                {element}
                {?jump}
                {?find}
            </div>
        </div>
    }
    .into_any()
}

pub fn update(state: &mut State, action: Action, scx: &SurfaceCx, fx: &mut Effects) {
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
        Action::Document(event) => {
            let Some((tv, _)) = state.selected(scx) else {
                return;
            };
            // A press in the transcript leaves a text field, as a press on
            // a page's body does, so Copy reaches the selection it starts.
            if matches!(event, DocumentEvent::PointerDown { .. })
                && scx.focus.is_some_and(|focus| !tv.owns_focus(focus))
            {
                fx.push(Effect::Focus(None));
            }
            tv.doc.handle(event);
        }
        Action::ToggleTool(tool) => {
            let Some((tv, thread)) = state.selected(scx) else {
                return;
            };
            let Some(row) = thread.transcript.get(MessageId(tool.0)) else {
                return;
            };
            let open = !tv.is_expanded(row);
            tv.expanded.insert(tool, open);
            tv.refresh(row);
            // The disclosure stays under the pointer, also while the view
            // follows the bottom.
            tv.doc.document_mut().hold_in_place(RowKey(tool.0));
        }
        Action::CopyCode(text) => {
            fx.push(Effect::CopyText(text));
            fx.push(Effect::Toast(Toast {
                kind: ToastKind::Success,
                text: "Copied code".to_owned(),
                undo: None,
            }));
        }
        Action::JumpToLatest => {
            if let Some((tv, _)) = state.selected(scx) {
                tv.doc.document_mut().scroll_to_bottom();
            }
        }
        Action::FindNext | Action::FindPrev => {
            if let Some((tv, _)) = state.selected(scx) {
                if action == Action::FindNext {
                    tv.doc.find_next(ScrollAlign::Center);
                } else {
                    tv.doc.find_prev(ScrollAlign::Center);
                }
            }
        }
        Action::CloseFind => close_find(state, scx, fx),
    }
}

fn close_find(state: &mut State, scx: &SurfaceCx, fx: &mut Effects) {
    state.find = None;
    if let Some((tv, _)) = state.selected(scx) {
        tv.doc.close_find();
    }
    if scx.is_focused(FIND_FIELD) {
        fx.push(Effect::Focus(None));
    }
}

/// Copy and Select all for the transcript, when focus is on nothing or on
/// one of its own controls (a text field keeps its own shortcuts), and
/// Enter/Escape in the find field.
pub fn event(state: &mut State, event: &InputEvent, scx: &SurfaceCx, fx: &mut Effects) -> bool {
    let InputEvent::KeyPress(chord) = event else {
        return false;
    };
    if state.find.is_some() && scx.is_focused(FIND_FIELD) {
        match chord.named() {
            Some(NamedKey::Escape) => {
                close_find(state, scx, fx);
                return true;
            }
            Some(NamedKey::Enter) => {
                let action = if chord.shift() {
                    Action::FindPrev
                } else {
                    Action::FindNext
                };
                update(state, action, scx, fx);
                return true;
            }
            _ => return false,
        }
    }
    let Some(command) = chord.binding().as_ref().and_then(key_command) else {
        return false;
    };
    let Some((tv, _)) = state.selected(scx) else {
        return false;
    };
    let ours = scx.focus.is_none_or(|focus| tv.owns_focus(focus));
    if !ours {
        return false;
    }
    match command {
        DocumentCommand::Copy => {
            let text = tv.doc.selected_text();
            if text.is_empty() {
                return false;
            }
            fx.push(Effect::CopyText(text));
            true
        }
        DocumentCommand::SelectAll => {
            tv.doc.document_mut().select_all();
            true
        }
        // Find is the `FindInDocument` command.
        DocumentCommand::Find => false,
    }
}

pub fn edit_text(
    state: &mut State,
    target: FocusId,
    command: TextEditCommand,
    ecx: &EditCx,
) -> Option<TextEditOutcome> {
    if target != FIND_FIELD {
        return None;
    }
    let field = state.find.as_mut()?;
    let outcome = field.apply_at(command, ecx.now_ms);
    let query = field.text().to_owned();
    if let Some(tv) = state.threads.get_mut(&ecx.model.selected) {
        tv.doc.set_find_query(&query);
        // Typing jumps to the first match, as browsers do.
        tv.doc.reveal_current_match(ScrollAlign::Center);
    }
    Some(outcome)
}

/// Commands the timeline owns (`FindInDocument`). True when handled.
pub fn command(state: &mut State, id: CommandId, scx: &SurfaceCx, fx: &mut Effects) -> bool {
    if id != CommandId::FindInDocument || !state.threads.contains_key(&scx.model.selected) {
        return false;
    }
    state.find.get_or_insert_with(|| TextField::new(""));
    fx.push(Effect::Focus(Some(FIND_FIELD)));
    true
}
