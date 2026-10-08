//! `@` file mentions and `/` commands anchored to the caret (stream D).
//!
//! Commands answer at once. File lookups answer like a worker would: the
//! provider queues them and the composer answers them on its next frame,
//! so every answer carries the query's sequence number and the thread
//! generation it was asked under. An answer to a query the user has typed
//! past, or to a thread they have switched away from, is dropped.

use quark_app::quark_ui::element::{AnyElement, IntoAnyElement, div};
use quark_app::quark_ui::style::Styled;
use quark_app::quark_ui::text_input::{
    self, AtomId, Completion, CompletionItem, CompletionProvider, CompletionQuery, Insertion,
    RichText, TriggerRule, caret_popup, completion_list,
};
use quark_app::quark_ui::theme::Theme;

use super::Action;

/// `@` anywhere a word starts; `/` only at the start of a line.
pub const RULES: [TriggerRule; 2] = [TriggerRule::word('@'), TriggerRule::line_start('/')];
/// The most suggestions a popup lists.
pub const MAX_RESULTS: usize = 6;
/// [`AtomId::kind`] of a file mention chip.
pub const FILE_CHIP: u32 = 1;
/// Width of the suggestion popup.
const POPUP_WIDTH: f32 = 320.0;
/// Above the timeline and the dock; below modal overlays.
const POPUP_Z: i32 = 300;

/// The slash commands, with what each does.
pub const COMMANDS: &[(&str, &str)] = &[
    ("review", "Review the proposed changes"),
    ("test", "Run the fixture tests"),
    ("explain", "Explain the selected code"),
    ("plan", "Plan before editing"),
    ("undo", "Undo the last applied change"),
];

/// A file lookup waiting for its answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lookup {
    pub thread_gen: u64,
    pub seq: u64,
    pub query: String,
}

/// Items for one lookup, tagged with what it was asked under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    pub thread_gen: u64,
    pub seq: u64,
    pub items: Vec<CompletionItem>,
}

/// The completion source: commands at once, files later.
#[derive(Debug, Default)]
pub struct Provider {
    /// Bumped whenever the composer shows another thread's draft.
    pub thread_gen: u64,
    pending: Vec<Lookup>,
}

impl Provider {
    /// The lookups asked since the last call, oldest first.
    pub fn take_pending(&mut self) -> Vec<Lookup> {
        std::mem::take(&mut self.pending)
    }
}

impl CompletionProvider for Provider {
    fn complete(
        &mut self,
        query: &CompletionQuery,
        items: &mut Vec<CompletionItem>,
    ) -> text_input::Answer {
        match query.trigger.ch {
            '/' => {
                items.extend(command_items(&query.query));
                text_input::Answer::Ready
            }
            _ => {
                self.pending.push(Lookup {
                    thread_gen: self.thread_gen,
                    seq: query.seq,
                    query: query.query.clone(),
                });
                text_input::Answer::Pending
            }
        }
    }
}

fn command_items(query: &str) -> impl Iterator<Item = CompletionItem> + '_ {
    COMMANDS
        .iter()
        .filter(move |(name, _)| name.starts_with(query))
        .take(MAX_RESULTS)
        .map(|(name, detail)| CompletionItem {
            label: format!("/{name}"),
            detail: (*detail).to_owned(),
            insertion: Insertion::Text(format!("/{name} ")),
        })
}

/// The answer to `lookup` from the fixture file `paths`: files whose path
/// contains the query (ignoring case), those whose name starts with it
/// first, at most [`MAX_RESULTS`]. Each inserts a chip exporting a
/// Markdown link to the file.
pub fn answer<'a>(lookup: &Lookup, paths: impl Iterator<Item = &'a str>) -> Answer {
    let query = lookup.query.to_lowercase();
    let mut found: Vec<(bool, usize, &str)> = paths
        .enumerate()
        .filter(|(_, path)| path.to_lowercase().contains(&query))
        .map(|(i, path)| (!file_name(path).to_lowercase().starts_with(&query), i, path))
        .collect();
    found.sort();
    let items = found
        .into_iter()
        .take(MAX_RESULTS)
        .map(|(_, i, path)| {
            let name = file_name(path);
            CompletionItem {
                label: name.to_owned(),
                detail: path.to_owned(),
                insertion: Insertion::Rich(RichText::atom(
                    &format!("@{name}"),
                    AtomId::new(FILE_CHIP, i as u64),
                    format!("[{name}]({path})"),
                    " ",
                )),
            }
        })
        .collect();
    Answer {
        thread_gen: lookup.thread_gen,
        seq: lookup.seq,
        items,
    }
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Hand `answer` to `completion` if it is still for the current thread
/// and query; returns whether it was taken.
pub fn deliver(completion: &mut Completion, provider: &Provider, answer: Answer) -> bool {
    answer.thread_gen == provider.thread_gen && completion.resolve(answer.seq, answer.items)
}

/// The open suggestion list at the editor's caret, out of flow, or `None`
/// while closed.
pub fn popup(
    completion: &Completion,
    anchor: &text_input::CaretAnchor,
    theme: &Theme,
    window: (f32, f32),
) -> Option<AnyElement> {
    let list = completion_list(completion, theme, |i| Action::Pick(i).into())?;
    let panel = div()
        .w(POPUP_WIDTH)
        .z_index(POPUP_Z)
        .test_id("composer.suggestions")
        .shadow_preset(quark_app::quark_ui::design::Shadow::POPOVER)
        .rounded(8.0)
        .child(list);
    Some(caret_popup(anchor, panel, window).into_any())
}
