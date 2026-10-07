//! Completion popups driven by trigger characters: the state behind one
//! (query, items, selection), the keys that drive it, and its list element.
//!
//! The app owns a [`Completion`] beside its [`Editor`]. After each edit it
//! calls [`Completion::refresh`], which finds the trigger at the caret
//! ([`Editor::active_trigger`]) and asks the app's [`CompletionProvider`]
//! for items whenever the query changes. A provider can answer at once, or
//! return [`Answer::Pending`], look items up on a worker, and send them back
//! to the UI thread (through quark-app's `UiSender`) for
//! [`Completion::resolve`]; answers to queries that have since changed are
//! dropped by sequence number. Keys reach [`Completion::handle_key`] from
//! the app's key handler before the editor sees them.

use crate::Action;
use crate::element::{AnyElement, Binding, IntoAnyElement, div, text};
use crate::style::Styled;
use crate::theme::Theme;

use super::hooks::Insertion;
use super::trigger::{TriggerMatch, TriggerRule};
use super::{Editor, TextEditOutcome, TextOffset};

/// One row of a completion list and what accepting it inserts in place of
/// the trigger and query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionItem {
    pub label: String,
    pub detail: String,
    pub insertion: Insertion,
}

/// What the provider is asked to complete. `seq` identifies it for
/// [`Completion::resolve`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionQuery {
    pub seq: u64,
    pub trigger: TriggerMatch,
    /// The text typed after the trigger character.
    pub query: String,
}

/// Whether a provider filled in its items.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    Ready,
    /// The items come later through [`Completion::resolve`].
    Pending,
}

/// The app's source of completion items.
pub trait CompletionProvider {
    /// Push the items for `query` onto `items` (empty on entry) and return
    /// [`Answer::Ready`], or start a lookup and return [`Answer::Pending`].
    fn complete(&mut self, query: &CompletionQuery, items: &mut Vec<CompletionItem>) -> Answer;
}

/// What [`Completion::handle_key`] did with a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionKey {
    /// Not a completion key, or no popup is open: let the editor have it.
    Ignored,
    /// Moved the selection or dismissed the popup.
    Handled,
    /// Inserted the selected item.
    Accepted(TextEditOutcome),
}

/// Completion popup state for one editor.
#[derive(Debug, Clone, Default)]
pub struct Completion {
    rules: Vec<TriggerRule>,
    active: Option<CompletionQuery>,
    items: Vec<CompletionItem>,
    selected: usize,
    pending: bool,
    /// Start of a trigger that Escape or an accept closed; it stays closed
    /// until the caret leaves it.
    closed_at: Option<TextOffset>,
    next_seq: u64,
}

impl Completion {
    pub fn new(rules: impl Into<Vec<TriggerRule>>) -> Self {
        Self {
            rules: rules.into(),
            ..Self::default()
        }
    }

    /// The query being completed, while the popup is open or waiting.
    pub fn query(&self) -> Option<&CompletionQuery> {
        self.active.as_ref()
    }

    pub fn items(&self) -> &[CompletionItem] {
        &self.items
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    /// Whether a provider answer is still outstanding.
    pub fn is_pending(&self) -> bool {
        self.active.is_some() && self.pending
    }

    /// Whether the popup shows: a query is active and has items.
    pub fn is_open(&self) -> bool {
        self.active.is_some() && !self.items.is_empty()
    }

    /// Follow `editor` after an edit or caret move. Asks `provider` when
    /// the query changed; returns whether anything the popup shows did.
    pub fn refresh(&mut self, editor: &Editor, provider: &mut dyn CompletionProvider) -> bool {
        let Some(trigger) = editor.active_trigger(&self.rules) else {
            self.closed_at = None;
            return self.close();
        };
        if self.closed_at == Some(trigger.range.start) {
            return self.close();
        }
        self.closed_at = None;
        let query = trigger.query(editor.text());
        if let Some(active) = &self.active
            && active.trigger == trigger
            && active.query == query
        {
            return false;
        }
        self.next_seq += 1;
        let active = self.active.insert(CompletionQuery {
            seq: self.next_seq,
            query: query.to_owned(),
            trigger,
        });
        self.items.clear();
        self.selected = 0;
        self.pending = provider.complete(active, &mut self.items) == Answer::Pending;
        true
    }

    /// Items for the query numbered `seq`, from a provider that answered
    /// [`Answer::Pending`]. Ignored, returning false, once the query moved
    /// on.
    pub fn resolve(&mut self, seq: u64, items: Vec<CompletionItem>) -> bool {
        if self.active.as_ref().is_none_or(|q| q.seq != seq) {
            return false;
        }
        self.items = items;
        self.selected = 0;
        self.pending = false;
        true
    }

    /// Arrow Up and Down move the selection (wrapping), Enter and Tab
    /// accept it into `editor`, and Escape dismisses the popup until the
    /// caret leaves the trigger. Other keys, and every key while the popup
    /// is closed, are [`CompletionKey::Ignored`].
    pub fn handle_key(&mut self, binding: &Binding, editor: &mut Editor) -> CompletionKey {
        let m = binding.mods;
        if !self.is_open() || m.cmd || m.ctrl || m.alt || m.primary {
            return CompletionKey::Ignored;
        }
        let len = self.items.len();
        match (binding.key.as_str(), m.shift) {
            ("arrowdown", false) => self.selected = (self.selected + 1) % len,
            ("arrowup", false) => self.selected = (self.selected + len - 1) % len,
            ("enter" | "tab", false) => {
                return CompletionKey::Accepted(self.accept(self.selected, editor));
            }
            ("escape", false) => {
                self.closed_at = self.active.as_ref().map(|q| q.trigger.range.start);
                self.close();
            }
            _ => return CompletionKey::Ignored,
        }
        CompletionKey::Handled
    }

    /// Replace the trigger and query with item `index` and close.
    pub fn accept(&mut self, index: usize, editor: &mut Editor) -> TextEditOutcome {
        let (Some(active), Some(item)) = (&self.active, self.items.get(index)) else {
            return TextEditOutcome::default();
        };
        let range = active.trigger.range.start.get()..active.trigger.range.end.get();
        let outcome = editor.replace_range(range, &item.insertion);
        // Inserted text can end in a trigger of its own (`/clear`); do not
        // reopen on it.
        self.closed_at = editor
            .active_trigger(&self.rules)
            .map(|trigger| trigger.range.start);
        self.close();
        outcome
    }

    fn close(&mut self) -> bool {
        let was_shown = self.active.is_some();
        self.active = None;
        self.items.clear();
        self.selected = 0;
        self.pending = false;
        was_shown
    }
}

/// The open popup's rows, or `None` while it is closed. `on_pick` makes
/// the action a click on row `index` sends; handle it with
/// [`Completion::accept`]. Place it where the app wants it (above the
/// editor, say).
pub fn completion_list(
    completion: &Completion,
    theme: &Theme,
    on_pick: impl Fn(usize) -> Action,
) -> Option<AnyElement> {
    if !completion.is_open() {
        return None;
    }
    let colors = &theme.colors;
    let rows = completion.items().iter().enumerate().map(|(i, item)| {
        let selected = i == completion.selected();
        let mut row = div()
            .flex_row()
            .items_center()
            .gap(8.0)
            .px(10.0)
            .h(28.0)
            .rounded(6.0)
            .on_click(on_pick(i))
            .accessibility_role(accesskit::Role::ListBoxOption)
            .accessibility_label(item.label.clone())
            .accessibility_selected(selected)
            .child(
                text(item.label.clone())
                    .text_sm()
                    .color(colors.text)
                    .truncate(),
            );
        if selected {
            row = row.bg(colors.sidebar_row_selected);
        } else {
            row = row.hover_bg(colors.sidebar_row_hover);
        }
        if !item.detail.is_empty() {
            row = row.child(
                text(item.detail.clone())
                    .text_xs()
                    .color(colors.text_muted)
                    .truncate(),
            );
        }
        row.into_any()
    });
    Some(
        div()
            .flex_col()
            .w_full()
            .p(4.0)
            .gap(2.0)
            .rounded(8.0)
            .bg(colors.surface)
            .border(colors.border_soft)
            .accessibility_role(accesskit::Role::ListBox)
            .accessibility_label(quark_i18n::tr("quark-suggestions"))
            .children(rows)
            .into_any(),
    )
}

#[cfg(test)]
mod tests {
    use super::super::{AtomId, RichText, TextEditCommand};
    use super::*;

    const RULES: [TriggerRule; 2] = [TriggerRule::word('@'), TriggerRule::line_start('/')];

    /// Files whose names start with the query, as atoms; or, when
    /// `pending`, nothing until resolved.
    struct Files {
        pending: bool,
    }

    impl CompletionProvider for Files {
        fn complete(&mut self, query: &CompletionQuery, items: &mut Vec<CompletionItem>) -> Answer {
            if self.pending {
                return Answer::Pending;
            }
            items.extend(files(&query.query));
            Answer::Ready
        }
    }

    fn files(query: &str) -> Vec<CompletionItem> {
        ["main.rs", "map.rs", "lib.rs"]
            .iter()
            .enumerate()
            .filter(|(_, name)| name.starts_with(query))
            .map(|(i, name)| CompletionItem {
                label: (*name).to_owned(),
                detail: String::new(),
                insertion: Insertion::Rich(RichText::atom(
                    &format!("@{name}"),
                    AtomId::new(0, i as u64),
                    format!("[{name}]({name})"),
                    " ",
                )),
            })
            .collect()
    }

    fn typed(editor: &mut Editor, completion: &mut Completion, provider: &mut Files, s: &str) {
        for ch in s.chars() {
            editor.apply(TextEditCommand::InsertText(ch.to_string()));
            completion.refresh(editor, provider);
        }
    }

    fn key(completion: &mut Completion, editor: &mut Editor, binding: &str) -> CompletionKey {
        completion.handle_key(&binding.parse().expect("binding"), editor)
    }

    #[test]
    fn arrow_then_enter_replaces_the_trigger_with_the_selected_atom() {
        let (mut editor, mut completion) = (Editor::default(), Completion::new(RULES));
        let mut provider = Files { pending: false };
        typed(&mut editor, &mut completion, &mut provider, "see @ma");
        assert_eq!(completion.items().len(), 2);

        assert_eq!(
            key(&mut completion, &mut editor, "arrowdown"),
            CompletionKey::Handled
        );
        let accepted = key(&mut completion, &mut editor, "enter");

        assert!(matches!(accepted, CompletionKey::Accepted(o) if o.text_changed));
        assert_eq!(editor.text(), "see @map.rs ");
        assert_eq!(editor.atoms()[0].range, 4..11);
        assert!(!completion.is_open());
    }

    #[test]
    fn a_late_answer_to_an_earlier_query_is_dropped() {
        let (mut editor, mut completion) = (Editor::default(), Completion::new(RULES));
        let mut provider = Files { pending: true };
        typed(&mut editor, &mut completion, &mut provider, "@m");
        let first = completion.query().expect("query").seq;
        typed(&mut editor, &mut completion, &mut provider, "a");
        let second = completion.query().expect("query").seq;

        assert!(!completion.resolve(first, files("m")));
        assert!(completion.resolve(second, files("ma")));
        assert_eq!(completion.items().len(), 2);
    }

    #[test]
    fn escape_keeps_the_popup_closed_until_the_caret_leaves_the_trigger() {
        let (mut editor, mut completion) = (Editor::default(), Completion::new(RULES));
        let mut provider = Files { pending: false };
        typed(&mut editor, &mut completion, &mut provider, "@m");
        assert_eq!(
            key(&mut completion, &mut editor, "escape"),
            CompletionKey::Handled
        );
        typed(&mut editor, &mut completion, &mut provider, "a");
        assert!(!completion.is_open(), "still dismissed");
        typed(&mut editor, &mut completion, &mut provider, " @");
        assert!(completion.is_open(), "a new trigger opens");
    }
}
