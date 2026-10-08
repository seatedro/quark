//! A combobox: a text field that filters a list of options as you type,
//! with fuzzy matching, and optionally loads its options asynchronously.
//!
//! The app owns a [`ComboboxState`] and wires it in three places:
//!
//! - `UiApp::edit_text` for [`ComboboxState::focus_id`] goes to
//!   [`ComboboxState::edit`], which edits the field, refilters, and turns
//!   the Up and Down arrows into list movement.
//! - The [`ComboboxMsg`]s the rendered [`combobox`] emits go to
//!   [`ComboboxState::update`].
//! - For async options, after each edit the app takes the
//!   [`ComboboxState::take_request`], loads options for its query however it
//!   likes, and hands them back with [`ComboboxState::resolve_options`].
//!   Results for a query the user has since typed past are dropped.
//!
//! Keyboard: typing filters and opens the list; Down and Up move the
//! highlight (opening the list when closed); Enter chooses the highlighted
//! option and puts its label in the field; Escape closes the list. Focus
//! stays in the field.

use std::ops::Range;
use std::rc::Rc;

use quark::view;

use quark_ui::element::{
    AnyElement, ElementContext, IntoAnyElement, RenderOnce, TextInput, div, text, text_input,
};
use quark_ui::style::Styled;
use quark_ui::text_input::{TextEditCommand, TextEditOutcome, TextField};
use quark_ui::theme::{Color, Theme, scaled_or};
use quark_ui::{Action, FocusId};

use crate::list_nav;
use crate::popover::{PopoverSide, anchored, list_padding, popover_panel};
use crate::select::RowSizes;

/// Score of `text` against the fuzzy `query`, appending the matched byte
/// ranges of `text` to `ranges`. `None` (with `ranges` untouched) when the
/// query's characters do not all appear in order. Matching ignores case;
/// matches at word starts and runs of consecutive matches score higher,
/// and a later first match scores lower.
pub fn fuzzy_match(query: &str, text: &str, ranges: &mut Vec<(usize, usize)>) -> Option<i32> {
    let first = query.chars().flat_map(char::to_lowercase).next()?;
    // Greedy matching from each occurrence of the query's first character,
    // keeping the best: "grf" in "Big Grapefruit" should start at "G".
    let best = text
        .char_indices()
        .filter(|&(_, ch)| ch.to_lowercase().next() == Some(first))
        .filter_map(|(at, _)| Some((greedy_match(query, text, at, None)?, at)))
        .max_by_key(|&(score, at)| (score, std::cmp::Reverse(at)))?;
    greedy_match(query, text, best.1, Some(ranges))
}

/// Match `query` in `text` from byte `from` on, taking each query
/// character at its first occurrence. Pushes ranges when given a vector.
fn greedy_match(
    query: &str,
    text: &str,
    from: usize,
    mut ranges: Option<&mut Vec<(usize, usize)>>,
) -> Option<i32> {
    let mut score = -(text[..from].chars().count().min(10) as i32);
    let mut wanted = query.chars().flat_map(char::to_lowercase).peekable();
    let mut previous = text[..from].chars().next_back();
    let mut last_end: Option<usize> = None;
    for (offset, ch) in text[from..].char_indices() {
        let Some(&want) = wanted.peek() else {
            break;
        };
        let at = from + offset;
        if ch.to_lowercase().next() == Some(want) {
            wanted.next();
            let end = at + ch.len_utf8();
            let word_start = previous.is_none_or(|p| !p.is_alphanumeric())
                || previous.is_some_and(|p| p.is_lowercase() && ch.is_uppercase());
            score += if word_start { 9 } else { 1 };
            let consecutive = last_end == Some(at);
            if consecutive {
                score += 5;
            }
            if let Some(ranges) = ranges.as_deref_mut() {
                match ranges.last_mut() {
                    Some(last) if consecutive => last.1 = end,
                    _ => ranges.push((at, end)),
                }
            }
            last_end = Some(end);
        }
        previous = Some(ch);
    }
    wanted.peek().is_none().then_some(score)
}

/// What a combobox asks its [`ComboboxState`] to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComboboxMsg {
    Open,
    Close,
    /// Move the highlight by this many matches.
    Move(i32),
    /// Choose the highlighted match.
    CommitHighlighted,
    /// Choose match `n` of the list (a click on it).
    Commit(usize),
}

/// What [`ComboboxState::update`] changed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ComboboxOutcome {
    /// The chosen option (an index into the option set), when it changed.
    pub changed: Option<usize>,
    /// Where keyboard focus should go (`UiContext::set_focus`).
    pub focus: Option<FocusId>,
}

/// A query to load options for: see [`ComboboxState::take_request`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptionsRequest {
    pub generation: u64,
    pub query: String,
}

/// The filtered list as the combobox shows it, rebuilt when the query or
/// the options change rather than every frame.
#[derive(Debug, Default)]
struct Matches {
    /// Option index of each match, best first.
    options: Vec<usize>,
    /// Each match's slice of `ranges`.
    spans: Vec<Range<usize>>,
    /// Matched byte ranges of the labels, flat.
    ranges: Vec<(usize, usize)>,
}

#[derive(Debug)]
pub struct ComboboxState {
    id: Rc<str>,
    focus: FocusId,
    field: TextField,
    options: Rc<[String]>,
    matches: Rc<Matches>,
    highlighted: Option<usize>,
    open: bool,
    selected: Option<usize>,
    asynchronous: bool,
    loading: bool,
    generation: u64,
    request: Option<OptionsRequest>,
}

impl ComboboxState {
    /// A combobox filtering `options`. `id` must be unique in the window.
    pub fn new(id: &str, options: Vec<String>) -> Self {
        let id: Rc<str> = Rc::from(id);
        let mut state = Self {
            focus: FocusId::from_key(&id),
            id,
            field: TextField::new(""),
            options: options.into(),
            matches: Rc::default(),
            highlighted: None,
            open: false,
            selected: None,
            asynchronous: false,
            loading: false,
            generation: 0,
            request: None,
        };
        state.refilter();
        state
    }

    /// A combobox whose options the app loads per query: see
    /// [`Self::take_request`].
    pub fn new_async(id: &str) -> Self {
        Self {
            asynchronous: true,
            ..Self::new(id, Vec::new())
        }
    }

    /// Focus target of the text field.
    pub fn focus_id(&self) -> FocusId {
        self.focus
    }

    pub fn field(&self) -> &TextField {
        &self.field
    }

    pub fn query(&self) -> &str {
        self.field.text()
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Whether an async load for the current query is outstanding.
    pub fn is_loading(&self) -> bool {
        self.loading
    }

    /// The option last chosen, while the field still shows it.
    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    pub fn options(&self) -> &[String] {
        &self.options
    }

    /// Labels of the current matches, best first.
    pub fn match_labels(&self) -> impl Iterator<Item = &str> {
        self.matches
            .options
            .iter()
            .map(|&i| self.options[i].as_str())
    }

    /// The highlighted match's label while the list is open.
    pub fn highlighted_label(&self) -> Option<&str> {
        let m = self.highlighted.filter(|_| self.open)?;
        Some(self.options[self.matches.options[m]].as_str())
    }

    /// Apply a text edit from `UiApp::edit_text`. Up and Down move the
    /// list's highlight instead of the caret; an edit that changes the text
    /// refilters and opens the list.
    pub fn edit(&mut self, command: TextEditCommand, now_ms: u64) -> TextEditOutcome {
        let delta = match command {
            TextEditCommand::CursorDown => Some(1),
            TextEditCommand::CursorUp => Some(-1),
            _ => None,
        };
        if let Some(delta) = delta {
            self.update(ComboboxMsg::Move(delta));
            // Not a text change, but the list changed: ask for a redraw.
            return TextEditOutcome {
                selection_changed: true,
                ..TextEditOutcome::default()
            };
        }
        let outcome = self.field.apply_at(command, now_ms);
        if outcome.text_changed {
            self.selected = None;
            self.open = true;
            self.query_changed();
        }
        outcome
    }

    /// Forward of `UiApp::set_preedit` for the field.
    pub fn set_preedit(&mut self, text: String, cursor: Option<(usize, usize)>) {
        self.field.set_preedit(text, cursor);
    }

    /// Replace the field's text, as assistive tech does
    /// (`UiApp::set_text_value`).
    pub fn set_text(&mut self, text: String) {
        self.field.set_text(text);
        self.selected = None;
        self.query_changed();
    }

    pub fn update(&mut self, msg: ComboboxMsg) -> ComboboxOutcome {
        let mut out = ComboboxOutcome::default();
        let count = self.matches.options.len();
        match msg {
            ComboboxMsg::Open => {
                self.open = true;
                self.highlighted = self.highlighted.filter(|&m| m < count).or(
                    // Reopening on a chosen option highlights it.
                    self.selected
                        .and_then(|s| self.matches.options.iter().position(|&o| o == s))
                        .or((count > 0).then_some(0)),
                );
            }
            ComboboxMsg::Close => self.open = false,
            ComboboxMsg::Move(_) if !self.open => return self.update(ComboboxMsg::Open),
            ComboboxMsg::Move(delta) => {
                self.highlighted = list_nav::step(count, self.highlighted, delta, false, |_| false);
            }
            ComboboxMsg::CommitHighlighted => {
                if let Some(m) = self.highlighted.filter(|_| self.open) {
                    return self.update(ComboboxMsg::Commit(m));
                }
            }
            ComboboxMsg::Commit(m) => {
                if let Some(&option) = self.matches.options.get(m) {
                    self.field.set_text(self.options[option].clone());
                    out.changed = (self.selected != Some(option)).then_some(option);
                    self.selected = Some(option);
                    self.open = false;
                    out.focus = Some(self.focus);
                }
            }
        }
        out
    }

    /// The query to load options for, once per change of the query, for a
    /// combobox made with [`Self::new_async`].
    pub fn take_request(&mut self) -> Option<OptionsRequest> {
        self.request.take()
    }

    /// Options loaded for request `generation`. Returns false, changing
    /// nothing, when the query has changed since that request.
    pub fn resolve_options(&mut self, generation: u64, options: Vec<String>) -> bool {
        if generation != self.generation {
            return false;
        }
        self.options = options.into();
        self.loading = false;
        self.selected = None;
        self.refilter();
        true
    }

    fn query_changed(&mut self) {
        if self.asynchronous {
            self.generation += 1;
            self.loading = true;
            self.request = Some(OptionsRequest {
                generation: self.generation,
                query: self.field.text().to_owned(),
            });
        }
        self.refilter();
    }

    /// Rebuild the matches for the current query and highlight the best.
    fn refilter(&mut self) {
        let query = self.field.text();
        // Reuse the buffers unless a frame still holds the last list.
        let matches = unique(&mut self.matches);
        matches.options.clear();
        matches.spans.clear();
        matches.ranges.clear();
        let mut scored: Vec<(i32, usize, Range<usize>)> = Vec::new();
        for (i, label) in self.options.iter().enumerate() {
            let start = matches.ranges.len();
            if query.is_empty() {
                scored.push((0, i, start..start));
            } else if let Some(score) = fuzzy_match(query, label, &mut matches.ranges) {
                scored.push((score, i, start..matches.ranges.len()));
            }
        }
        // Best score first; ties keep the options' order.
        scored.sort_by_key(|&(score, i, _)| (std::cmp::Reverse(score), i));
        for (_, i, span) in scored {
            matches.options.push(i);
            matches.spans.push(span);
        }
        self.highlighted = (!matches.options.is_empty()).then_some(0);
    }
}

/// Unique access to an `Rc`'s value, replacing it with a default one when a
/// frame still shares it.
fn unique<T: Default>(rc: &mut Rc<T>) -> &mut T {
    if Rc::get_mut(rc).is_none() {
        *rc = Rc::default();
    }
    Rc::get_mut(rc).expect("unique after replacing")
}

/// A combobox rendered from `state`; `on_msg` wraps what it emits in the
/// app's action.
pub fn combobox(
    state: &ComboboxState,
    label: impl Into<String>,
    on_msg: impl Fn(ComboboxMsg) -> Action + 'static,
) -> Combobox {
    let label = label.into();
    Combobox {
        input: text_input(label.clone(), "")
            .field(&state.field)
            .focus_target(state.focus),
        label,
        id: state.id.clone(),
        focus: state.focus,
        options: state.options.clone(),
        matches: state.matches.clone(),
        highlighted: state.highlighted,
        selected: state.selected,
        open: state.open,
        loading: state.loading,
        on_msg: Box::new(on_msg),
        width: None,
        viewport: (f32::INFINITY, f32::INFINITY),
        placeholder: String::new(),
    }
}

pub struct Combobox {
    input: TextInput,
    label: String,
    id: Rc<str>,
    focus: FocusId,
    options: Rc<[String]>,
    matches: Rc<Matches>,
    highlighted: Option<usize>,
    selected: Option<usize>,
    open: bool,
    loading: bool,
    on_msg: Box<dyn Fn(ComboboxMsg) -> Action>,
    width: Option<f32>,
    viewport: (f32, f32),
    placeholder: String,
}

impl Combobox {
    pub fn width(mut self, width: f32) -> Self {
        self.width = Some(width);
        self
    }

    /// The window size, so the list flips above the field and stays on
    /// screen near the window's edges.
    pub fn viewport(mut self, size: (f32, f32)) -> Self {
        self.viewport = size;
        self
    }

    pub fn placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = placeholder.into();
        self
    }
}

impl RenderOnce for Combobox {
    fn render(self, cx: &ElementContext) -> AnyElement {
        let theme = cx.theme;
        let m = &theme.metrics;
        let scale = m.ui_scale();
        let width = self.width.unwrap_or((260.0 * scale).round());
        let msg = &self.on_msg;
        let focused = cx.focus == Some(self.focus);
        let open = self.open && focused;
        let field_h = scaled_or(
            theme.components.select.height,
            scale,
            (m.ui_row_height * 1.6).round(),
        );

        view! {
            <div class="relative flex-col" w={width} accessibility_id={&*self.id}
                 test_id="combobox" accessibility_role={accesskit::Role::ComboBox}
                 aria-label={self.label.clone()} aria-expanded={open}
                 on_key={("alt+arrowdown", msg(ComboboxMsg::Open))}
                 @when {open} {
                     on_key={("enter", msg(ComboboxMsg::CommitHighlighted))}
                     on_key={("escape", msg(ComboboxMsg::Close))}
                 }>
                <{self.input} placeholder={self.placeholder} focused={focused} class="w-full"
                              h={field_h} />
                if open {
                    <anchored(
                        view! {
                            <popover_panel(theme) w={width} py={list_padding(theme)}
                                accessibility_id={format!("{}-listbox", self.id)}
                                test_id="combobox-listbox"
                                accessibility_role={accesskit::Role::ListBox}
                                aria-label={self.label.clone()}>
                                for (i, &option) in self.matches.options.iter().enumerate() {
                                    {match_row(
                                        &self.options[option],
                                        &self.matches.ranges[self.matches.spans[i].clone()],
                                        self.highlighted == Some(i),
                                        self.selected == Some(option),
                                        msg(ComboboxMsg::Commit(i)),
                                        theme,
                                    )}
                                }
                                if self.loading || self.matches.options.is_empty() {
                                    {status_row(self.loading, theme)}
                                }
                            </popover_panel>
                        },
                        PopoverSide::Bottom,
                        self.viewport,
                    ) />
                }
            </div>
        }
    }
}

/// The list's only row while options load or when none match.
fn status_row(loading: bool, theme: &Theme) -> AnyElement {
    let sz = RowSizes::resolve(theme.components.option, theme);
    let note = quark_ui::i18n::tr(if loading {
        "quark-loading"
    } else {
        "quark-find-no-matches"
    });
    view! {
        <div px={sz.px} py={sz.py}
             accessibility_role={accesskit::Role::Label} aria-label={note.clone()}>
            <text size={sz.font} color={theme.colors.text_muted}>{note}</text>
        </div>
    }
}

/// A row of the list: the label with its fuzzy-matched characters in the
/// accent color. Rows have no stable id, so they take no focus: a click
/// leaves focus in the field, and the arrows move the highlight.
fn match_row(
    label: &str,
    ranges: &[(usize, usize)],
    highlighted: bool,
    selected: bool,
    commit: Action,
    theme: &Theme,
) -> AnyElement {
    let tc = &theme.colors;
    let sz = RowSizes::resolve(theme.components.option, theme);
    // Each range with where the plain text before it starts.
    let runs = ranges.iter().scan(0, |at, &(start, end)| {
        Some((std::mem::replace(at, end), start, end))
    });
    let tail = ranges.last().map_or(0, |&(_, end)| end);
    view! {
        <div class="flex-row items-center w-full" px={sz.px} py={sz.py}
             @when {sz.height.is_some()} { h={sz.height.unwrap()} py={0.0} }
             accessibility_role={accesskit::Role::ListBoxOption}
             aria-label={label.to_owned()} aria-selected={highlighted || selected}
             bg={if highlighted { tc.ghost_element_selected } else { Color::TRANSPARENT }}
             hover_bg={tc.ghost_element_hover} class="cursor-pointer" on:click={commit}>
            <div class="flex-row flex-1 overflow-hidden">
                for (at, start, end) in runs {
                    if start > at {
                        <text size={sz.font} color={tc.text}>{label[at..start].to_owned()}</text>
                    }
                    <text class="font-semibold" size={sz.font} color={tc.text_accent}>
                        {label[start..end].to_owned()}
                    </text>
                }
                if tail < label.len() {
                    <text size={sz.font} color={tc.text}>{label[tail..].to_owned()}</text>
                }
            </div>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ranked(query: &str, labels: &[&str]) -> Vec<String> {
        let mut state = ComboboxState::new("c", labels.iter().map(|s| s.to_string()).collect());
        state.set_text(query.to_owned());
        state.match_labels().map(str::to_owned).collect()
    }

    #[test]
    fn fuzzy_match_ranks_word_starts_and_runs_above_scattered_hits() {
        let labels = ["Gooseberry", "Grape", "Grapefruit", "Pomegranate", "Guava"];
        assert_eq!(
            ranked("gra", &labels),
            ["Grape", "Grapefruit", "Pomegranate"]
        );
        assert_eq!(ranked("gf", &labels), ["Grapefruit"]);
        assert_eq!(ranked("xyz", &labels), Vec::<String>::new());
    }

    #[test]
    fn fuzzy_match_reports_merged_byte_ranges() {
        let mut ranges = Vec::new();
        assert!(fuzzy_match("ngf", "Big Grapefruit", &mut ranges).is_none());
        assert!(ranges.is_empty());
        fuzzy_match("grf", "Big Grapefruit", &mut ranges).expect("matches");
        // "G" at 4, "r" at 5 merge; "f" at 9.
        assert_eq!(ranges, [(4, 6), (9, 10)]);
    }

    #[test]
    fn async_results_for_a_stale_query_are_dropped() {
        let mut state = ComboboxState::new_async("c");
        state.set_text("ap".to_owned());
        let first = state.take_request().expect("request for ap");
        state.set_text("apr".to_owned());
        let second = state.take_request().expect("request for apr");
        assert_eq!(second.query, "apr");

        assert!(!state.resolve_options(first.generation, vec!["Apple".into()]));
        assert!(state.is_loading());
        assert!(state.resolve_options(second.generation, vec!["Apricot".into(), "Pear".into()]));
        assert!(!state.is_loading());
        assert_eq!(state.match_labels().collect::<Vec<_>>(), ["Apricot"]);
    }
}
