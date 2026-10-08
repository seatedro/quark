//! A chat composer assembled from quark-ui's text input primitives, as a
//! reference for apps (Portal's composer follows it):
//!
//! - `@` files, `/` commands (line start only), `#` items, and `$` skills
//!   open a completion list ([`Completion`]). Files, items, and skills
//!   insert atomic chips ([`RichText::atom`]); commands insert text. Items
//!   answer asynchronously through the app's [`UiSender`], as a lookup on a
//!   worker would.
//! - Enter sends, Shift+Enter inserts a newline, and Mod+Enter sends as the
//!   alternate action (queue or steer, say). The button sends, or stops a
//!   running turn.
//! - Pastes over [`PASTE_ATTACHMENT_BYTES`] and dropped files become
//!   attachment chips above the input ([`InputHooks`]); so do clipboard
//!   images with the `clipboard-image` feature.
//! - Arrow Up at the start of the text recalls earlier prompts
//!   ([`PromptHistory`]); Arrow Down walks forward to the draft.
//! - While an approval is pending its panel renders above the input and
//!   the composer takes no input.

use std::path::Path;

use accesskit::Role;
use quark::view;
use quark_app::quark_ui::element::{
    AnyElement, Binding, IntoAnyElement, ScrollActionBuilder, div, text,
};
use quark_app::quark_ui::style::Styled;
use quark_app::quark_ui::text_input::{
    Answer, AtomId, Completion, CompletionItem, CompletionKey, CompletionProvider, CompletionQuery,
    Editor, InputHooks, Insertion, PromptHistory, RichText, TextEditCommand, TextEditOutcome,
    TriggerRule, completion_list, text_editor_element,
};
use quark_app::quark_ui::theme::Theme;
use quark_app::quark_ui::{Action, FocusId};
use quark_app::{InputEvent, UiApp, UiContext, UiSender, ViewContext, WindowOptions};

const INPUT: FocusId = FocusId::from_key("composer.input");
/// Pastes longer than this become a text attachment instead of text.
const PASTE_ATTACHMENT_BYTES: usize = 32 * 1024;

const RULES: [TriggerRule; 4] = [
    TriggerRule::word('@'),
    TriggerRule::line_start('/'),
    TriggerRule::word('#'),
    TriggerRule::word('$'),
];

/// [`AtomId::kind`] of each chip.
const FILE: u32 = 1;
const ITEM: u32 = 2;
const SKILL: u32 = 3;

const FILES: &[&str] = &["src/main.rs", "src/map.rs", "Cargo.toml", "README.md"];
const COMMANDS: &[(&str, &str)] = &[
    ("plan", "Plan before editing"),
    ("default", "Back to the default mode"),
    ("clear", "Clear the conversation"),
];
const ITEMS: &[(u64, &str)] = &[(12, "Fix scroll jump"), (14, "Mentions in the composer")];
const SKILLS: &[&str] = &["review", "release"];

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Send,
    Stop,
    Pick(usize),
    RemoveAttachment(u64),
    Approve(bool),
    RequestApproval,
    Scroll(i32),
}

impl From<Msg> for Action {
    fn from(msg: Msg) -> Self {
        Action::new(msg)
    }
}

/// Messages from completion lookups.
enum Reply {
    Items(u64, Vec<CompletionItem>),
}

#[derive(Debug, Clone, PartialEq)]
enum AttachmentKind {
    Image,
    File,
    Text(String),
}

#[derive(Debug, Clone, PartialEq)]
struct Attachment {
    id: u64,
    name: String,
    kind: AttachmentKind,
}

/// The attachments above the input, and the paste and drop policy that
/// fills them.
#[derive(Default)]
struct Attachments {
    items: Vec<Attachment>,
    next_id: u64,
}

impl Attachments {
    fn add(&mut self, name: String, kind: AttachmentKind) {
        self.next_id += 1;
        self.items.push(Attachment {
            id: self.next_id,
            name,
            kind,
        });
    }
}

impl InputHooks for Attachments {
    fn paste(&mut self, pasted: Insertion) -> Insertion {
        match pasted {
            Insertion::Text(text) if text.len() > PASTE_ATTACHMENT_BYTES => {
                let name = format!("Pasted text ({} KB)", text.len().div_ceil(1024));
                self.add(name, AttachmentKind::Text(text));
                Insertion::Nothing
            }
            other => other,
        }
    }

    fn drop_path(&mut self, path: &Path) -> Insertion {
        let image = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| ["png", "jpg", "jpeg", "gif", "webp"].contains(&e));
        let name = path.file_name().map_or_else(
            || path.display().to_string(),
            |n| n.to_string_lossy().into(),
        );
        let kind = if image {
            AttachmentKind::Image
        } else {
            AttachmentKind::File
        };
        self.add(name, kind);
        Insertion::Nothing
    }
}

/// Completion items for each trigger. `#` items go through the sender, as
/// a lookup on a worker thread would.
struct Suggestions {
    sender: Option<UiSender<Reply>>,
}

fn chip(label: &str, kind: u32, key: u64, export: String) -> Insertion {
    Insertion::Rich(RichText::atom(label, AtomId::new(kind, key), export, " "))
}

fn item(label: String, detail: &str, insertion: Insertion) -> CompletionItem {
    CompletionItem {
        label,
        detail: detail.to_owned(),
        insertion,
    }
}

fn item_suggestions(query: &str) -> Vec<CompletionItem> {
    ITEMS
        .iter()
        .filter(|(n, _)| n.to_string().starts_with(query))
        .map(|&(n, title)| {
            let insertion = chip(&format!("#{n}"), ITEM, n, format!("[#{n}](issue/{n})"));
            item(format!("#{n}"), title, insertion)
        })
        .collect()
}

impl CompletionProvider for Suggestions {
    fn complete(&mut self, query: &CompletionQuery, items: &mut Vec<CompletionItem>) -> Answer {
        let q = query.query.as_str();
        match query.trigger.ch {
            '@' => items.extend(FILES.iter().enumerate().filter(|(_, f)| f.contains(q)).map(
                |(i, f)| {
                    item(
                        f.to_string(),
                        "",
                        chip(&format!("@{f}"), FILE, i as u64, format!("[{f}]({f})")),
                    )
                },
            )),
            '/' => items.extend(COMMANDS.iter().filter(|(name, _)| name.starts_with(q)).map(
                |(name, detail)| {
                    item(
                        format!("/{name}"),
                        detail,
                        Insertion::Text(format!("/{name} ")),
                    )
                },
            )),
            '$' => items.extend(
                SKILLS
                    .iter()
                    .enumerate()
                    .filter(|(_, s)| s.starts_with(q))
                    .map(|(i, s)| {
                        item(
                            format!("${s}"),
                            "skill",
                            chip(&format!("${s}"), SKILL, i as u64, format!("${s}")),
                        )
                    }),
            ),
            '#' => {
                if let Some(sender) = &self.sender {
                    sender.send(Reply::Items(query.seq, item_suggestions(q)));
                    return Answer::Pending;
                }
            }
            _ => {}
        }
        Answer::Ready
    }
}

/// What a send produced.
#[derive(Debug, Clone, PartialEq)]
struct Sent {
    /// The prompt as exported text, chips as their Markdown.
    text: String,
    attachments: usize,
    alternate: bool,
}

struct ComposerDemo {
    editor: Editor,
    completion: Completion,
    suggestions: Suggestions,
    history: PromptHistory,
    /// This conversation's sent prompts, oldest first.
    prompts: Vec<RichText>,
    attachments: Attachments,
    running: bool,
    approval_pending: bool,
    sent: Vec<Sent>,
}

impl ComposerDemo {
    fn new() -> Self {
        Self {
            editor: Editor::default(),
            completion: Completion::new(RULES),
            suggestions: Suggestions { sender: None },
            history: PromptHistory::default(),
            prompts: Vec::new(),
            attachments: Attachments::default(),
            running: false,
            approval_pending: false,
            sent: Vec::new(),
        }
    }

    fn refresh_completion(&mut self) {
        self.completion.refresh(&self.editor, &mut self.suggestions);
    }

    fn send(&mut self, alternate: bool) {
        let prompt = self.editor.rich_text();
        let empty = prompt.text.trim().is_empty() && self.attachments.items.is_empty();
        if self.approval_pending || empty {
            return;
        }
        self.sent.push(Sent {
            text: prompt.export(),
            attachments: self.attachments.items.len(),
            alternate,
        });
        if !prompt.text.trim().is_empty() && self.prompts.last() != Some(&prompt) {
            self.prompts.push(prompt);
        }
        self.editor.set_text("");
        self.attachments.items.clear();
        self.history = PromptHistory::default();
        self.running = true;
        self.refresh_completion();
    }

    /// Keys the composer takes before the editor: completion navigation,
    /// send and newline, and history recall.
    fn key(&mut self, binding: &Binding) -> bool {
        match self.completion.handle_key(binding, &mut self.editor) {
            CompletionKey::Ignored => {}
            CompletionKey::Handled | CompletionKey::Accepted(_) => {
                self.refresh_completion();
                return true;
            }
        }
        let m = binding.mods;
        let primary = m.cmd || m.ctrl;
        match (binding.key.as_str(), primary, m.shift, m.alt) {
            ("enter", false, false, false) => self.send(false),
            ("enter", true, false, false) => self.send(true),
            ("enter", false, true, false) => {
                self.editor.insert(&Insertion::Text("\n".into()));
            }
            ("arrowup", false, false, false) => {
                return self.history.back(&mut self.editor, &self.prompts);
            }
            ("arrowdown", false, false, false) => {
                return self.history.forward(&mut self.editor, &self.prompts);
            }
            _ => return false,
        }
        true
    }

    fn button(label: &str, msg: Msg, enabled: bool, theme: &Theme) -> AnyElement {
        let colors = &theme.colors;
        view! {
            <div accessibility_role={Role::Button} aria-label={label} aria-disabled={!enabled}
                 class="px-3 h-7 items-center justify-center rounded-[6]"
                 @when {enabled} {
                     on:click={msg} bg={colors.accent} hover_bg={colors.accent_strong}
                 }
                 @when {!enabled} { bg={colors.border_soft} }>
                <text class="text-sm" color={if enabled { colors.on_accent } else { colors.text_strong }}>
                    {label}
                </text>
            </div>
        }
    }

    fn approval_panel(theme: &Theme) -> AnyElement {
        let colors = &theme.colors;
        view! {
            <div accessibility_role={Role::Group} aria-label="Approval"
                 class="flex-row items-center gap-2 p-2 rounded-[8] bg-[colors.background]">
                <div class="flex-1">
                    <text class="text-sm" color={colors.text}>"Run `cargo test`?"</text>
                </div>
                {Self::button("Approve", Msg::Approve(true), true, theme)}
                {Self::button("Deny", Msg::Approve(false), true, theme)}
            </div>
        }
    }

    fn attachment_row(&self, theme: &Theme) -> AnyElement {
        let colors = &theme.colors;
        view! {
            <div class="flex-row gap-[6]">
                for a in &self.attachments.items {
                    <div accessibility_role={Role::Group} aria-label={a.name.clone()}
                         class="flex-row items-center gap-[6] px-2 h-6 rounded-[6] bg-[colors.background]">
                        <text class="text-xs" color={colors.text_muted}>
                            {match a.kind {
                                AttachmentKind::Image => "image",
                                AttachmentKind::File => "file",
                                AttachmentKind::Text(_) => "text",
                            }}
                        </text>
                        <text class="text-xs" color={colors.text}>{a.name.clone()}</text>
                        <div accessibility_role={Role::Button} aria-label={format!("Remove {}", a.name)}
                             on:click={Msg::RemoveAttachment(a.id)}>
                            <text class="text-xs" color={colors.text_muted}>"x"</text>
                        </div>
                    </div>
                }
            </div>
        }
    }
}

impl UiApp for ComposerDemo {
    type Action = Msg;
    type Message = Reply;

    fn init(&mut self, cx: &mut UiContext) {
        self.suggestions.sender = Some(cx.sender());
        cx.set_focus(Some(INPUT));
    }

    fn view(&mut self, cx: &mut ViewContext) -> AnyElement {
        let (width, height) = cx.frame.size();
        let theme = cx.theme;
        let colors = &theme.colors;
        let input_w = (width - 48.0).max(100.0);
        self.editor.set_clock(cx.frame.elapsed().as_millis() as u64);
        self.editor.sync_size(input_w, 72.0);
        self.editor.flush(&mut cx.frame.text().system);

        let (label, msg) = if self.running {
            ("Stop", Msg::Stop)
        } else {
            ("Send", Msg::Send)
        };
        let can_act = !self.approval_pending
            && (self.running || !self.editor.is_empty() || !self.attachments.items.is_empty());

        view! {
            <div w={width} h={height} class="flex-col p-3 gap-3 bg-[colors.background]">
                <div class="flex-1 flex-col gap-[6]">
                    for sent in &self.sent {
                        <text class="text-sm" color={colors.text}>{sent.text.clone()}</text>
                    }
                </div>
                <div class="flex-col gap-2 p-3 rounded-[12] bg-[colors.surface]">
                    if self.approval_pending {
                        {Self::approval_panel(theme)}
                    }
                    if let Some(list) =
                        completion_list(&self.completion, theme, |i| Msg::Pick(i).into())
                    {
                        {list}
                    }
                    if !self.attachments.items.is_empty() {
                        {self.attachment_row(theme)}
                    }
                    <text_editor_element(
                        INPUT,
                        ScrollActionBuilder::new(|lines| Msg::Scroll(lines).into()),
                    )
                        editor_snapshot={&self.editor}
                        placeholder="Ask anything, @ to mention a file"
                        focused={cx.is_focused(INPUT)}
                        text_color={colors.text}
                        w={input_w}
                        h={72.0} />
                    <div class="flex-row items-center">
                        <div class="flex-1">
                            <text class="text-xs" color={colors.text_muted}>
                                "Enter to send, Shift+Enter for a newline"
                            </text>
                        </div>
                        {Self::button(label, msg, can_act, theme)}
                        {Self::button("Simulate approval", Msg::RequestApproval, true, theme)}
                    </div>
                </div>
            </div>
        }
    }

    fn update(&mut self, msg: Msg, cx: &mut UiContext) {
        match msg {
            Msg::Send => self.send(false),
            Msg::Stop => self.running = false,
            Msg::Pick(i) => {
                self.completion.accept(i, &mut self.editor);
                self.refresh_completion();
                cx.set_focus(Some(INPUT));
            }
            Msg::RemoveAttachment(id) => self.attachments.items.retain(|a| a.id != id),
            Msg::Approve(_) => self.approval_pending = false,
            Msg::RequestApproval => self.approval_pending = true,
            Msg::Scroll(lines) => self
                .editor
                .scroll(lines as f32 * self.editor.scroll_line_height_px()),
        }
    }

    fn message(&mut self, reply: Reply, cx: &mut UiContext) {
        let Reply::Items(seq, items) = reply;
        if self.completion.resolve(seq, items) {
            cx.window.request_redraw();
        }
    }

    fn edit_text(&mut self, target: FocusId, command: TextEditCommand) -> TextEditOutcome {
        if target != INPUT || self.approval_pending {
            return TextEditOutcome::default();
        }
        let outcome = self.editor.apply_with(command, &mut self.attachments);
        if outcome.text_changed || outcome.selection_changed {
            self.refresh_completion();
        }
        outcome
    }

    fn set_preedit(&mut self, target: FocusId, text: String, cursor: Option<(usize, usize)>) {
        if target == INPUT && !self.approval_pending {
            self.editor.set_preedit(text, cursor);
        }
    }

    fn set_text_value(&mut self, target: FocusId, value: String, _cx: &mut UiContext) {
        if target == INPUT && !self.approval_pending {
            self.editor.set_text(&value);
        }
    }

    fn event(&mut self, event: &InputEvent, cx: &mut UiContext) -> bool {
        if !cx.focus().is_some_and(|f| f == INPUT) {
            return false;
        }
        let taken = match event {
            InputEvent::FileDropped(path) if !self.approval_pending => {
                self.editor.drop_path(path, &mut self.attachments);
                true
            }
            // A pending approval holds typing and Enter; editing keys reach
            // `edit_text`, which ignores them, and Tab still moves focus to
            // the approval buttons.
            InputEvent::TextInput(_) if self.approval_pending => return true,
            InputEvent::KeyPress(chord) if self.approval_pending => {
                return chord.binding().is_some_and(|b| b.key == "enter");
            }
            #[cfg(feature = "clipboard-image")]
            InputEvent::KeyPress(chord)
                if chord
                    .binding()
                    .is_some_and(|b| b.key == "v" && (b.mods.cmd || b.mods.ctrl)) =>
            {
                match cx.window.clipboard_image() {
                    Some(image) => {
                        let name = format!("Pasted image {}x{}", image.width, image.height);
                        self.attachments.add(name, AttachmentKind::Image);
                        true
                    }
                    None => false,
                }
            }
            InputEvent::KeyPress(chord) => chord.binding().is_some_and(|b| self.key(&b)),
            _ => false,
        };
        if taken {
            cx.window.request_redraw();
        }
        taken
    }
}

fn main() -> Result<(), quark_app::RunError> {
    quark_app::run_ui(
        ComposerDemo::new(),
        WindowOptions {
            title: "Composer".into(),
            size: (720.0, 520.0),
            ..WindowOptions::default()
        },
    )
}

/// The assembly driven headlessly: the parts the primitives' own tests do
/// not cover, which are this example's key routing and policies.
#[cfg(test)]
mod tests {
    use quark_app::testing::{By, UiTestHarness};

    use super::*;

    fn harness() -> UiTestHarness<ComposerDemo> {
        UiTestHarness::new(ComposerDemo::new(), (720.0, 520.0), 1.0)
    }

    #[test]
    fn enter_sends_shift_enter_breaks_the_line_and_mod_enter_sends_as_the_alternate() {
        let cases = [
            ("enter", "", Some(false)),
            ("shift+enter", "hi\n", None),
            ("mod+enter", "", Some(true)),
        ];
        for (key, left, alternate) in cases {
            let mut ui = harness();
            ui.type_text("hi");
            ui.key(key);
            let app = ui.app();
            assert_eq!(app.editor.text(), left, "{key}");
            let sent = app.sent.first().map(|s| (s.text.as_str(), s.alternate));
            assert_eq!(sent, alternate.map(|a| ("hi", a)), "{key}");
        }
    }

    #[test]
    fn a_paste_over_32_kib_becomes_a_text_attachment() {
        let mut ui = harness();
        ui.set_clipboard_text("x".repeat(PASTE_ATTACHMENT_BYTES + 1));
        ui.key("mod+v");
        ui.set_clipboard_text("short");
        ui.key("mod+v");

        assert_eq!(ui.app().editor.text(), "short");
        ui.find(By::role_name(Role::Group, "Pasted text (33 KB)"));
    }

    #[test]
    fn an_asynchronous_item_list_takes_arrow_and_enter_and_inserts_a_chip() {
        let mut ui = harness();
        ui.type_text("see #1");
        // The items arrived as a message after the query went out.
        ui.find(By::role_name(Role::ListBoxOption, "#14"));
        ui.key("arrowdown");
        ui.key("enter");

        let editor = &ui.app().editor;
        assert_eq!(editor.text(), "see #14 ");
        assert_eq!(editor.atoms()[0].id, AtomId::new(ITEM, 14));
        assert!(ui.app().sent.is_empty(), "enter went to the list");
    }

    #[test]
    fn a_pending_approval_holds_typing_and_enter() {
        let mut ui = harness();
        ui.type_text("hi");
        ui.click_node(By::role_name(Role::Button, "Simulate approval"));
        ui.click_node(By::role_name(
            Role::MultilineTextInput,
            "Ask anything, @ to mention a file",
        ));
        ui.type_text("!");
        ui.key("enter");
        assert_eq!((ui.app().editor.text(), ui.app().sent.len()), ("hi", 0));

        ui.click_node(By::role_name(Role::Button, "Approve"));
        ui.click_node(By::role_name(
            Role::MultilineTextInput,
            "Ask anything, @ to mention a file",
        ));
        ui.key("enter");
        assert_eq!(ui.app().sent.len(), 1);
    }
}
