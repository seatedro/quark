//! `@` file mentions and `/` commands anchored to the caret (stream D).
//!
//! Commands answer at once. File lookups answer like a worker would: the
//! provider queues them and the composer answers them on its next frame,
//! so every answer carries the query's sequence number and the thread
//! generation it was asked under. An answer to a query the user has typed
//! past, or to a thread they have switched away from, is dropped.

use quark_app::quark_ui::element::{AnyElement, IntoAnyElement, div, svg_icon, text};
use quark_app::quark_ui::icons::lucide;
use quark_app::quark_ui::style::Styled;
use quark_app::quark_ui::text_input::{
    self, AtomId, Completion, CompletionItem, CompletionProvider, CompletionQuery, Insertion,
    RichText, TriggerRule, caret_popup,
};
use quark_app::quark_ui::theme::Theme;

use super::Action;

/// `@` anywhere a word starts; `/` only at the start of a line.
pub const RULES: [TriggerRule; 2] = [TriggerRule::word('@'), TriggerRule::line_start('/')];
/// The most suggestions a popup lists.
pub const MAX_RESULTS: usize = 6;
/// [`AtomId::kind`] of a file mention chip.
pub const FILE_CHIP: u32 = 1;
/// Width of the suggestion popup (the design's floor is 280).
const POPUP_WIDTH: f32 = 320.0;
/// Height of one suggestion row.
const ROW_HEIGHT: f32 = 32.0;
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
/// while closed. `inset` is how far the composer's card reaches above
/// its text field: the popup opens above the card, so it never covers
/// the draft or the card's edge.
pub fn popup(
    completion: &Completion,
    anchor: &text_input::CaretAnchor,
    inset: f32,
    theme: &Theme,
    window: (f32, f32),
) -> Option<AnyElement> {
    let list = list(completion, theme)?;
    let scale = theme.metrics.ui_scale();
    let recipe = theme.components.popover;
    let mut panel = div()
        .w((POPUP_WIDTH * scale).round())
        .z_index(POPUP_Z)
        .test_id("composer.suggestions")
        .child(list);
    if let Some(shadow) = recipe.shadow {
        panel = panel.shadow(
            shadow.blur * scale,
            shadow.offset_y * scale,
            quark_app::quark_ui::theme::Color::rgba(0, 0, 0, shadow.alpha),
        );
    }
    // From the caret line up to the card's top edge, plus a gap.
    let gap = anchor
        .get()
        .map_or(4.0, |g| g.caret.y - g.field.y + inset + 6.0 * scale);
    Some(
        caret_popup(anchor, panel, window)
            .gap(gap)
            .prefer_above(true)
            .into_any(),
    )
}

/// The rows: an icon by kind (file or command), the label in strong
/// text, and the path or description muted after it, at most
/// [`MAX_RESULTS`]. Same roles and names as quark's `completion_list`.
fn list(completion: &Completion, theme: &Theme) -> Option<AnyElement> {
    if !completion.is_open() {
        return None;
    }
    let colors = &theme.colors;
    let scale = theme.metrics.ui_scale();
    let px = |points: f32| (points * scale).round();
    let rows = completion
        .items()
        .iter()
        .take(MAX_RESULTS)
        .enumerate()
        .map(|(i, item)| {
            let selected = i == completion.selected();
            let icon = if item.label.starts_with('/') {
                lucide::COMMAND
            } else {
                lucide::FILE_CODE
            };
            let mut row = div()
                .flex_row()
                .items_center()
                .flex_shrink_0()
                .w_full()
                .h(px(ROW_HEIGHT))
                .px(px(8.0))
                .gap(px(8.0))
                .rounded(px(6.0))
                .on_click(Action::Pick(i))
                .accessibility_role(accesskit::Role::ListBoxOption)
                .accessibility_label(item.label.clone())
                .accessibility_selected(selected)
                .child(svg_icon(icon, px(14.0)).color(if selected {
                    colors.accent
                } else {
                    colors.icon
                }))
                .child(
                    text(item.label.clone())
                        .size(px(13.0))
                        .color(colors.text_strong),
                );
            if !item.detail.is_empty() {
                row = row.child(
                    div().flex_1().min_w(0.0).overflow_hidden().child(
                        text(item.detail.clone())
                            .size(px(12.0))
                            .color(colors.text_muted)
                            .truncate(),
                    ),
                );
            }
            if selected {
                row = row.bg(colors.sidebar_row_selected);
            } else {
                row = row.hover_bg(colors.sidebar_row_hover);
            }
            row.into_any()
        });
    Some(
        div()
            .flex_col()
            .w_full()
            .p(px(4.0))
            .gap(px(2.0))
            .rounded(px(8.0))
            .bg(colors.elevated_surface)
            .border(colors.border)
            .accessibility_role(accesskit::Role::ListBox)
            .accessibility_label(quark_app::quark_ui::i18n::tr("quark-suggestions"))
            .children(rows)
            .into_any(),
    )
}
