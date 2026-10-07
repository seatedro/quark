//! A command palette: a modal list of app-provided items with fuzzy search.
//!
//! The app registers [`PaletteProvider`]s; each one defines a section and
//! fills it with [`PaletteItem`]s (title, icon, keybinding hint, action)
//! when the palette opens. Quark supplies the modal, focus handoff, the
//! [`FuzzyMatcher`] with match highlighting, recents, `mod+1`..`mod+9`
//! quick select, and a virtualized result list.
//!
//! The palette never sees the app's context. The app wires it up in four
//! places:
//!
//! - open: `cx.set_focus(Some(palette.open(&state, cx.focus())))`;
//! - [`UiApp::event`]: pass key presses to [`CommandPalette::handle_key`]
//!   and return `true` when it returns `Some`;
//! - `edit_text` / `set_preedit` for [`PALETTE_INPUT`]: forward to
//!   [`CommandPalette::edit`] and [`CommandPalette::set_preedit`];
//! - `update`: forward its [`PaletteEvent`] action to
//!   [`CommandPalette::apply`].
//!
//! Every [`PaletteOutcome`] that closes the palette carries the focus to
//! restore, which is whatever was focused when it opened.
//!
//! [`UiApp::event`]: https://docs.rs/quark-app

use quark::SemanticRole;
use quark_ui::design::{Ico, Rad, Shadow, Sp, Sz};
use quark_ui::element::*;
use quark_ui::style::Styled;
use quark_ui::text_input::{TextEditCommand, TextEditOutcome, TextField};
use quark_ui::theme::{Color, Theme};
use quark_ui::{Action, FocusId};

// ---------------------------------------------------------------------------
// Fuzzy matching
// ---------------------------------------------------------------------------

/// Score of each matched character.
const SCORE_MATCH: i32 = 16;
/// Penalty for opening a gap between matched characters, and for each
/// further skipped character.
const GAP_START: i32 = -3;
const GAP_EXTENSION: i32 = -1;
/// Bonus for a match right after whitespace or at the start of the text.
const BONUS_BOUNDARY_WHITE: i32 = 10;
/// Bonus for a match right after a path or word delimiter (`/`, `-`, ...).
const BONUS_BOUNDARY_DELIMITER: i32 = 9;
/// Bonus for a match after any other non-alphanumeric character.
const BONUS_BOUNDARY: i32 = 8;
/// Bonus for an uppercase letter after a lowercase one, or a digit after a
/// non-digit.
const BONUS_CAMEL: i32 = 7;
/// Floor of the bonus a match adjacent to the previous match gets. It equals
/// the cost of a one-character gap, so a run beats a split match.
const BONUS_CONSECUTIVE: i32 = -(GAP_START + GAP_EXTENSION);
/// The first query character's bonus counts this many times: where a match
/// starts says more than where it continues.
const BONUS_FIRST_CHAR_MULTIPLIER: i32 = 2;
const UNMATCHED: i32 = i32::MIN / 2;
const NO_COLUMN: u32 = u32::MAX;

#[derive(Clone, Copy, PartialEq, Eq)]
enum CharClass {
    White,
    Delimiter,
    Lower,
    Upper,
    Number,
    Other,
}

fn char_class(c: char) -> CharClass {
    match c {
        c if c.is_whitespace() => CharClass::White,
        '/' | '\\' | '-' | '_' | '.' | ':' | ',' | ';' | '|' => CharClass::Delimiter,
        c if c.is_lowercase() => CharClass::Lower,
        c if c.is_uppercase() => CharClass::Upper,
        c if c.is_numeric() => CharClass::Number,
        c if c.is_alphabetic() => CharClass::Lower,
        _ => CharClass::Other,
    }
}

fn boundary_bonus(prev: CharClass, cur: CharClass) -> i32 {
    use CharClass::*;
    match (prev, cur) {
        (_, White | Delimiter | Other) => 0,
        (White, _) => BONUS_BOUNDARY_WHITE,
        (Delimiter, _) => BONUS_BOUNDARY_DELIMITER,
        (Other, _) => BONUS_BOUNDARY,
        (Lower, Upper) | (Lower | Upper, Number) => BONUS_CAMEL,
        _ => 0,
    }
}

fn fold(c: char, case_sensitive: bool) -> char {
    if case_sensitive {
        c
    } else {
        c.to_lowercase().next().unwrap_or(c)
    }
}

/// An fzf-style fuzzy scorer. The query's characters must appear in order
/// in the candidate; matches at word starts, camelCase humps, and in runs
/// score higher, and gaps cost. Matching is case-insensitive unless the
/// query has an uppercase letter.
///
/// It keeps its scratch buffers between calls, so scoring a list allocates
/// nothing once the buffers have grown to the longest candidate.
#[derive(Debug, Default)]
pub struct FuzzyMatcher {
    query: Vec<char>,
    case_sensitive: bool,
    chars: Vec<char>,
    offsets: Vec<u32>,
    bonus: Vec<i32>,
    /// Row-major `query.len() x chars.len()` tables: the best score with
    /// query char `i` matched at candidate char `j`, the bonus of the run
    /// that match continues, and the column query char `i - 1` matched at.
    score: Vec<i32>,
    run_bonus: Vec<i32>,
    from: Vec<u32>,
}

impl FuzzyMatcher {
    pub fn new(query: &str) -> Self {
        let mut matcher = Self::default();
        matcher.set_query(query);
        matcher
    }

    pub fn set_query(&mut self, query: &str) {
        self.case_sensitive = query.chars().any(char::is_uppercase);
        self.query.clear();
        let case_sensitive = self.case_sensitive;
        self.query.extend(
            query
                .chars()
                .filter(|c| !c.is_whitespace())
                .map(|c| fold(c, case_sensitive)),
        );
    }

    pub fn is_empty(&self) -> bool {
        self.query.is_empty()
    }

    /// Score `candidate`, or `None` when the query is not a subsequence of
    /// it. On a match, appends the byte offsets of the matched characters
    /// to `positions`, ascending. An empty query matches with score 0.
    pub fn score(&mut self, candidate: &str, positions: &mut Vec<u32>) -> Option<i32> {
        let n = self.query.len();
        if n == 0 {
            return Some(0);
        }
        self.chars.clear();
        self.offsets.clear();
        self.bonus.clear();
        let mut prev = CharClass::White;
        let mut next_query = 0;
        for (offset, c) in candidate.char_indices() {
            let class = char_class(c);
            let folded = fold(c, self.case_sensitive);
            if next_query < n && folded == self.query[next_query] {
                next_query += 1;
            }
            self.chars.push(folded);
            self.offsets.push(offset as u32);
            self.bonus.push(boundary_bonus(prev, class));
            prev = class;
        }
        if next_query < n {
            return None;
        }

        let m = self.chars.len();
        self.score.clear();
        self.score.resize(n * m, UNMATCHED);
        self.run_bonus.clear();
        self.run_bonus.resize(n * m, 0);
        self.from.clear();
        self.from.resize(n * m, NO_COLUMN);

        for j in 0..m {
            if self.chars[j] == self.query[0] {
                self.score[j] = SCORE_MATCH + self.bonus[j] * BONUS_FIRST_CHAR_MULTIPLIER;
                self.run_bonus[j] = self.bonus[j];
            }
        }
        for i in 1..n {
            let (above, row) = ((i - 1) * m..i * m, i * m..(i + 1) * m);
            // Best way to reach column j through a gap: the previous query
            // character matched at `carry_from`, at least two columns back.
            let mut carry = UNMATCHED;
            let mut carry_from = NO_COLUMN;
            for j in 0..m {
                if j >= 2 {
                    carry += GAP_EXTENSION;
                    let opened = self.score[above.start + j - 2] + GAP_START;
                    if opened > carry {
                        carry = opened;
                        carry_from = (j - 2) as u32;
                    }
                }
                if self.chars[j] != self.query[i] {
                    continue;
                }
                let mut best = UNMATCHED;
                if carry > UNMATCHED / 2 {
                    best = carry + SCORE_MATCH + self.bonus[j];
                    self.from[row.start + j] = carry_from;
                    self.run_bonus[row.start + j] = self.bonus[j];
                }
                if j >= 1 {
                    let diagonal = self.score[above.start + j - 1];
                    if diagonal > UNMATCHED / 2 {
                        let run = self.run_bonus[above.start + j - 1];
                        let bonus = self.bonus[j].max(run).max(BONUS_CONSECUTIVE);
                        let consecutive = diagonal + SCORE_MATCH + bonus;
                        // Ties go to the run: it highlights as one piece.
                        if consecutive >= best {
                            best = consecutive;
                            self.from[row.start + j] = (j - 1) as u32;
                            self.run_bonus[row.start + j] = bonus;
                        }
                    }
                }
                self.score[row.start + j] = best;
            }
        }

        let last = (n - 1) * m;
        let (end, best) = (0..m)
            .map(|j| (j, self.score[last + j]))
            .filter(|&(_, score)| score > UNMATCHED / 2)
            // The first best column: the earliest, tightest match.
            .fold((NO_COLUMN as usize, UNMATCHED), |acc, cur| {
                if cur.1 > acc.1 { cur } else { acc }
            });
        if end == NO_COLUMN as usize {
            return None;
        }
        let start = positions.len();
        let mut column = end;
        for i in (0..n).rev() {
            positions.push(self.offsets[column]);
            let from = self.from[i * m + column];
            if from == NO_COLUMN {
                break;
            }
            column = from as usize;
        }
        positions[start..].reverse();
        Some(best)
    }
}

// ---------------------------------------------------------------------------
// Keybinding hints
// ---------------------------------------------------------------------------

/// How a binding reads in a hint: `Ctrl+Shift+P`, or `Cmd+Shift+P` on
/// macOS for `mod+shift+p`.
pub fn binding_label(binding: &Binding) -> String {
    let m = binding.mods;
    let mac = cfg!(target_os = "macos");
    let mut out = String::new();
    let mut push = |part: &str| {
        if !out.is_empty() {
            out.push('+');
        }
        out.push_str(part);
    };
    if m.ctrl || (m.primary && !mac) {
        push("Ctrl");
    }
    if m.alt {
        push(if mac { "Option" } else { "Alt" });
    }
    if m.shift {
        push("Shift");
    }
    if m.cmd || (m.primary && mac) {
        push("Cmd");
    }
    let key = match binding.key.as_str() {
        "escape" => "Esc".to_owned(),
        "arrowup" => "Up".to_owned(),
        "arrowdown" => "Down".to_owned(),
        "arrowleft" => "Left".to_owned(),
        "arrowright" => "Right".to_owned(),
        "pageup" => "Page Up".to_owned(),
        "pagedown" => "Page Down".to_owned(),
        key => {
            let mut chars = key.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect(),
                None => String::new(),
            }
        }
    };
    push(&key);
    out
}

// ---------------------------------------------------------------------------
// Palette model
// ---------------------------------------------------------------------------

/// The palette's search field. Route this target's `edit_text` and
/// `set_preedit` to the palette.
pub const PALETTE_INPUT: FocusId = FocusId::from_key("quark.palette.input");

/// Recent items kept, most recent first.
pub const PALETTE_RECENT_LIMIT: usize = 8;
/// Result rows shown before the list scrolls.
pub const PALETTE_VISIBLE_ROWS: usize = 9;
/// Score added to a recent item when a query is typed, per place from the
/// end of the recents list, so recent items rise among equal matches.
const RECENT_BOOST: i32 = 2;
/// One row a provider offers.
#[derive(Debug, Clone, PartialEq)]
pub struct PaletteItem {
    /// Stable identity across openings, for recents.
    pub key: u64,
    pub title: String,
    pub subtitle: Option<String>,
    pub icon: Option<&'static str>,
    /// The item's own shortcut, shown as a hint.
    pub binding: Option<Binding>,
    /// Emitted through [`PaletteOutcome::Run`] when the item is chosen.
    pub action: Action,
}

impl PaletteItem {
    pub fn new(key: u64, title: impl Into<String>, action: impl Into<Action>) -> Self {
        Self {
            key,
            title: title.into(),
            subtitle: None,
            icon: None,
            binding: None,
            action: action.into(),
        }
    }

    pub fn subtitle(mut self, subtitle: impl Into<String>) -> Self {
        self.subtitle = Some(subtitle.into());
        self
    }

    pub fn icon(mut self, svg: &'static str) -> Self {
        self.icon = Some(svg);
        self
    }

    pub fn binding(mut self, binding: Binding) -> Self {
        self.binding = Some(binding);
        self
    }
}

/// A source of palette items: one section of results. `S` is the app
/// state the provider reads when the palette opens.
pub trait PaletteProvider<S: ?Sized> {
    /// The section's heading.
    fn title(&self) -> &str;
    /// Append this section's items to `out`.
    fn collect(&self, state: &S, out: &mut Vec<PaletteItem>);
}

/// What an element of the palette asks for. The app wraps it in its own
/// action and hands it back to [`CommandPalette::apply`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaletteEvent {
    /// A result row was clicked.
    Activate(usize),
    /// The list scrolled by lines.
    Scroll(i32),
    /// The backdrop was clicked.
    Dismiss,
}

/// What the app should do after the palette handled input.
#[derive(Debug, Clone, PartialEq)]
pub enum PaletteOutcome {
    /// Nothing beyond a redraw.
    Handled,
    /// An item was chosen: the palette closed. Restore focus, then run the
    /// action.
    Run {
        action: Action,
        restore_focus: Option<FocusId>,
    },
    /// The palette closed without choosing.
    Closed { restore_focus: Option<FocusId> },
}

#[derive(Debug)]
struct Section {
    title: String,
    items: std::ops::Range<usize>,
}

#[derive(Debug, Clone, Copy)]
struct Match {
    item: u32,
    score: i32,
    /// Range into `CommandPalette::positions`.
    highlights: (u32, u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Row {
    /// A section heading; `None` is the recents heading.
    Header(Option<u32>),
    /// An index into `matches`.
    Item(u32),
}

/// The palette's state. Results are recomputed only when the query or the
/// items change, into buffers it keeps, so idle frames do no matching.
pub struct CommandPalette<S: ?Sized + 'static> {
    providers: Vec<Box<dyn PaletteProvider<S>>>,
    open: bool,
    restore_focus: Option<FocusId>,
    query: TextField,
    placeholder: String,
    matcher: FuzzyMatcher,
    items: Vec<PaletteItem>,
    /// Parallel to `items`: their bindings as hint text.
    hints: Vec<Option<String>>,
    sections: Vec<Section>,
    matches: Vec<Match>,
    positions: Vec<u32>,
    rows: Vec<Row>,
    /// Selected row; always an item row while there are results.
    selected: usize,
    /// First row in view.
    scroll_top: usize,
    recent: Vec<u64>,
}

impl<S: ?Sized + 'static> Default for CommandPalette<S> {
    fn default() -> Self {
        Self::new()
    }
}

impl<S: ?Sized + 'static> CommandPalette<S> {
    pub fn new() -> Self {
        Self {
            providers: Vec::new(),
            open: false,
            restore_focus: None,
            query: TextField::new(""),
            placeholder: quark_ui::i18n::tr("quark-search-commands"),
            matcher: FuzzyMatcher::default(),
            items: Vec::new(),
            hints: Vec::new(),
            sections: Vec::new(),
            matches: Vec::new(),
            positions: Vec::new(),
            rows: Vec::new(),
            selected: 0,
            scroll_top: 0,
            recent: Vec::new(),
        }
    }

    /// Add a section. Sections show in registration order.
    pub fn register(&mut self, provider: impl PaletteProvider<S> + 'static) {
        self.providers.push(Box::new(provider));
    }

    pub fn placeholder(mut self, text: impl Into<String>) -> Self {
        self.placeholder = text.into();
        self
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn query(&self) -> &str {
        self.query.text()
    }

    /// Item keys chosen recently, most recent first.
    pub fn recent(&self) -> &[u64] {
        &self.recent
    }

    /// Restore recents saved from an earlier session.
    pub fn set_recent(&mut self, keys: impl IntoIterator<Item = u64>) {
        self.recent.clear();
        self.recent
            .extend(keys.into_iter().take(PALETTE_RECENT_LIMIT));
    }

    /// Open with an empty query, collecting every provider's items from
    /// `state`. Remembers `focus` to restore on close. Returns the focus
    /// to set: the palette's search field.
    pub fn open(&mut self, state: &S, focus: Option<FocusId>) -> FocusId {
        if !self.open {
            self.open = true;
            self.restore_focus = focus.filter(|f| *f != PALETTE_INPUT);
            self.query = TextField::new("");
        }
        self.reload(state);
        PALETTE_INPUT
    }

    /// Collect the providers' items again, as when the state they read
    /// changed while the palette is open. Keeps the query.
    pub fn reload(&mut self, state: &S) {
        self.items.clear();
        self.sections.clear();
        for provider in &self.providers {
            let start = self.items.len();
            provider.collect(state, &mut self.items);
            self.sections.push(Section {
                title: provider.title().to_owned(),
                items: start..self.items.len(),
            });
        }
        self.hints.clear();
        self.hints.extend(
            self.items
                .iter()
                .map(|item| item.binding.as_ref().map(binding_label)),
        );
        self.refilter();
    }

    /// Close without choosing. Returns the focus to restore.
    pub fn close(&mut self) -> Option<FocusId> {
        self.open = false;
        self.items.clear();
        self.rows.clear();
        self.matches.clear();
        self.restore_focus.take()
    }

    fn closed(&mut self) -> PaletteOutcome {
        PaletteOutcome::Closed {
            restore_focus: self.close(),
        }
    }

    /// Replace the query, as when opening with a prefilled search.
    pub fn set_query(&mut self, query: &str) {
        self.query.set_text(query);
        self.refilter();
    }

    /// Apply an edit to the search field and filter again when its text
    /// changed.
    pub fn edit(&mut self, command: TextEditCommand, now_ms: u64) -> TextEditOutcome {
        let outcome = self.query.apply_at(command, now_ms);
        if outcome.text_changed {
            self.refilter();
        }
        outcome
    }

    pub fn set_preedit(&mut self, text: impl Into<String>, cursor: Option<(usize, usize)>) {
        self.query.set_preedit(text, cursor);
    }

    fn recent_rank(&self, key: u64) -> Option<usize> {
        self.recent.iter().position(|k| *k == key)
    }

    fn refilter(&mut self) {
        self.matches.clear();
        self.positions.clear();
        self.rows.clear();
        self.matcher.set_query(self.query.text());
        if self.matcher.is_empty() {
            self.list_unfiltered();
        } else {
            self.list_matches();
        }
        self.selected = self
            .rows
            .iter()
            .position(|row| matches!(row, Row::Item(_)))
            .unwrap_or(0);
        self.scroll_top = 0;
    }

    /// Empty query: recents first, then every section in order.
    fn list_unfiltered(&mut self) {
        let mut shown_recent = false;
        for &key in &self.recent {
            let Some(item) = self.items.iter().position(|item| item.key == key) else {
                continue;
            };
            if !shown_recent {
                self.rows.push(Row::Header(None));
                shown_recent = true;
            }
            self.rows.push(Row::Item(self.matches.len() as u32));
            self.matches.push(Match {
                item: item as u32,
                score: 0,
                highlights: (0, 0),
            });
        }
        for (section_index, section) in self.sections.iter().enumerate() {
            let mut headed = false;
            for item in section.items.clone() {
                if self.recent.contains(&self.items[item].key) {
                    continue;
                }
                if !headed {
                    self.rows.push(Row::Header(Some(section_index as u32)));
                    headed = true;
                }
                self.rows.push(Row::Item(self.matches.len() as u32));
                self.matches.push(Match {
                    item: item as u32,
                    score: 0,
                    highlights: (0, 0),
                });
            }
        }
    }

    /// A query: each section's matches, best first.
    fn list_matches(&mut self) {
        for section_index in 0..self.sections.len() {
            let start = self.matches.len();
            for item in self.sections[section_index].items.clone() {
                let from = self.positions.len() as u32;
                let Some(mut score) = self
                    .matcher
                    .score(&self.items[item].title, &mut self.positions)
                else {
                    continue;
                };
                if let Some(rank) = self.recent_rank(self.items[item].key) {
                    score += (PALETTE_RECENT_LIMIT - rank) as i32 * RECENT_BOOST;
                }
                self.matches.push(Match {
                    item: item as u32,
                    score,
                    highlights: (from, self.positions.len() as u32),
                });
            }
            let items = &self.items;
            // Unstable sort allocates nothing; the item index makes it
            // deterministic. Shorter titles win ties: less went unmatched.
            self.matches[start..].sort_unstable_by(|a, b| {
                b.score
                    .cmp(&a.score)
                    .then_with(|| {
                        let len = |m: &Match| items[m.item as usize].title.len();
                        len(a).cmp(&len(b))
                    })
                    .then_with(|| a.item.cmp(&b.item))
            });
            if self.matches.len() > start {
                self.rows.push(Row::Header(Some(section_index as u32)));
                self.rows
                    .extend((start..self.matches.len()).map(|m| Row::Item(m as u32)));
            }
        }
    }

    fn item_at_row(&self, row: usize) -> Option<&PaletteItem> {
        match self.rows.get(row)? {
            Row::Item(m) => Some(&self.items[self.matches[*m as usize].item as usize]),
            Row::Header(_) => None,
        }
    }

    /// The row of the `n`th item (0-based), skipping headings.
    fn nth_item_row(&self, n: usize) -> Option<usize> {
        self.rows
            .iter()
            .enumerate()
            .filter(|(_, row)| matches!(row, Row::Item(_)))
            .nth(n)
            .map(|(i, _)| i)
    }

    fn move_selection(&mut self, delta: i32) {
        if self.rows.is_empty() {
            return;
        }
        let mut row = self.selected as i32;
        let last = self.rows.len() as i32 - 1;
        let step = delta.signum();
        let mut remaining = delta.abs();
        while remaining > 0 {
            let mut next = row + step;
            while (0..=last).contains(&next) && matches!(self.rows[next as usize], Row::Header(_)) {
                next += step;
            }
            if !(0..=last).contains(&next) {
                break;
            }
            row = next;
            remaining -= 1;
        }
        self.selected = row as usize;
        self.scroll_to_selected();
    }

    fn scroll_to_selected(&mut self) {
        // Keep the heading above the first item in view too.
        let top = if self.nth_item_row(0) == Some(self.selected) {
            0
        } else {
            self.selected
        };
        if top < self.scroll_top {
            self.scroll_top = top;
        } else if self.selected >= self.scroll_top + PALETTE_VISIBLE_ROWS {
            self.scroll_top = self.selected + 1 - PALETTE_VISIBLE_ROWS;
        }
    }

    fn scroll_by(&mut self, lines: i32) {
        let max = self.rows.len().saturating_sub(PALETTE_VISIBLE_ROWS) as i32;
        self.scroll_top = (self.scroll_top as i32 + lines).clamp(0, max) as usize;
    }

    fn activate_row(&mut self, row: usize) -> PaletteOutcome {
        let Some(item) = self.item_at_row(row) else {
            return PaletteOutcome::Handled;
        };
        let (key, action) = (item.key, item.action.clone());
        self.recent.retain(|k| *k != key);
        self.recent.insert(0, key);
        self.recent.truncate(PALETTE_RECENT_LIMIT);
        PaletteOutcome::Run {
            action,
            restore_focus: self.close(),
        }
    }

    /// Handle a key press while open: arrows, Page Up/Down, and Ctrl+N/P
    /// move the selection, Enter runs it, `mod+1`..`mod+9` run the nth
    /// result, and Escape closes. `None` means the key is not the
    /// palette's (typing goes to the search field).
    pub fn handle_key(&mut self, pressed: &Binding) -> Option<PaletteOutcome> {
        if !self.open {
            return None;
        }
        let m = pressed.mods;
        let plain = !(m.cmd || m.ctrl || m.alt || m.shift);
        let primary_only = (m.cmd || m.ctrl) && !(m.cmd && m.ctrl) && !m.alt && !m.shift;
        let key = pressed.key.as_str();
        let outcome = match key {
            "escape" if plain => self.closed(),
            "enter" if plain => self.activate_row(self.selected),
            "arrowdown" if plain => {
                self.move_selection(1);
                PaletteOutcome::Handled
            }
            "arrowup" if plain => {
                self.move_selection(-1);
                PaletteOutcome::Handled
            }
            "n" if m.ctrl && !m.cmd && !m.alt && !m.shift => {
                self.move_selection(1);
                PaletteOutcome::Handled
            }
            "p" if m.ctrl && !m.cmd && !m.alt && !m.shift => {
                self.move_selection(-1);
                PaletteOutcome::Handled
            }
            "pagedown" if plain => {
                self.move_selection(PALETTE_VISIBLE_ROWS as i32);
                PaletteOutcome::Handled
            }
            "pageup" if plain => {
                self.move_selection(-(PALETTE_VISIBLE_ROWS as i32));
                PaletteOutcome::Handled
            }
            _ if primary_only => {
                let digit = key.parse::<usize>().ok().filter(|d| (1..=9).contains(d))?;
                match self.nth_item_row(digit - 1) {
                    Some(row) => self.activate_row(row),
                    None => PaletteOutcome::Handled,
                }
            }
            _ => return None,
        };
        Some(outcome)
    }

    /// Handle an event one of the palette's elements emitted.
    pub fn apply(&mut self, event: PaletteEvent) -> Option<PaletteOutcome> {
        if !self.open {
            return None;
        }
        Some(match event {
            PaletteEvent::Activate(row) => self.activate_row(row),
            PaletteEvent::Scroll(lines) => {
                self.scroll_by(lines);
                PaletteOutcome::Handled
            }
            PaletteEvent::Dismiss => self.closed(),
        })
    }

    // ---- Rendering ---------------------------------------------------------

    /// The palette over a `window`-sized scrim, or `None` while closed.
    /// `focused` is whether [`PALETTE_INPUT`] has focus, for the caret.
    pub fn render(
        &self,
        window: (f32, f32),
        theme: &Theme,
        focused: bool,
        on_event: impl Fn(PaletteEvent) -> Action,
    ) -> Option<AnyElement> {
        if !self.open {
            return None;
        }
        let tc = &theme.colors;
        let m = &theme.metrics;
        let scale = m.ui_scale();
        let (width, height) = window;
        let margin = (Sz::MODAL_MARGIN * scale).round();
        let panel_w = (Sz::MODAL_LG * scale).min(width - margin).max(1.0);
        let row_h = m.ui_row_height.round();
        let shown = self.rows.len().clamp(1, PALETTE_VISIBLE_ROWS);
        let list_h = shown as f32 * row_h;

        let input = text_input(quark_ui::i18n::tr("quark-command-palette"), "")
            .field(&self.query)
            .placeholder(self.placeholder.clone())
            .focus_target(PALETTE_INPUT)
            .focused(focused)
            .search(true)
            .bare()
            .w_full()
            .h((Sz::INPUT * scale).round());

        let mut list = div()
            .flex_col()
            .w_full()
            .h(list_h)
            .overflow_hidden()
            .id("palette-results")
            .test_id("palette-results")
            .semantic_role(SemanticRole::ScrollArea)
            .accessibility_role(accesskit::Role::ListBox)
            .accessibility_id("palette-results")
            .accessibility_label(quark_ui::i18n::tr("quark-results"))
            .on_scroll(ScrollActionBuilder::new({
                let scroll = on_event(PaletteEvent::Scroll(1));
                let up = on_event(PaletteEvent::Scroll(-1));
                // The callback must be 'static, so the two actions are built
                // up front; a multi-line delta moves one row.
                move |lines| {
                    if lines < 0 {
                        up.clone()
                    } else {
                        scroll.clone()
                    }
                }
            }));
        if self.rows.is_empty() {
            list = list.child(
                div()
                    .h(row_h)
                    .px((Sp::MD * scale).round())
                    .items_center()
                    .flex_row()
                    .child(
                        text(quark_ui::i18n::tr("quark-no-results"))
                            .text_sm()
                            .color(tc.text_muted),
                    ),
            );
        }
        let end = (self.scroll_top + PALETTE_VISIBLE_ROWS).min(self.rows.len());
        for row in self.scroll_top..end {
            let element = match self.rows[row] {
                Row::Header(section) => {
                    let title = match section {
                        Some(s) => self.sections[s as usize].title.as_str(),
                        None => "Recent",
                    };
                    div()
                        .flex_row()
                        .items_center()
                        .flex_shrink_0()
                        .h(row_h)
                        .px((Sp::MD * scale).round())
                        .child(text(title).text_xs().medium().color(tc.text_muted))
                        .into_any()
                }
                Row::Item(m) => {
                    self.item_row(row, self.matches[m as usize], row_h, theme, &on_event)
                }
            };
            list = list.child(element);
        }

        let panel = div()
            .flex_col()
            .w(panel_w)
            .bg(tc.elevated_surface)
            .border(tc.border)
            .rounded(Rad::XL)
            .shadow_preset(Shadow::MODAL)
            .overflow_hidden()
            .on_click(NoopAction)
            .id("palette")
            .test_id("palette")
            .semantic_role(SemanticRole::Dialog)
            .focus_scope("quark.palette")
            .trap_focus(true)
            .accessibility_role(accesskit::Role::Dialog)
            .accessibility_id("palette")
            .accessibility_label(quark_ui::i18n::tr("quark-command-palette"))
            .child(
                div()
                    .flex_row()
                    .items_center()
                    .gap((Sp::SM * scale).round())
                    .px((Sp::MD * scale).round())
                    .border_b(tc.border_variant)
                    .child(svg_icon(quark_ui::icons::lucide::SEARCH, Ico::SM).color(tc.icon))
                    .child(div().flex_1().min_w(0.0).child(input)),
            )
            .child(div().p((Sp::XS * scale).round()).child(list));

        Some(
            div()
                .absolute()
                .top(0.0)
                .left(0.0)
                .w(width)
                .h(height)
                .z_index(400)
                .flex_col()
                .items_center()
                .pt((Sz::MODAL_TOP_OFFSET * scale).round())
                .bg(tc.overlay_scrim)
                .id("palette.backdrop")
                .test_id("palette-backdrop")
                .on_click(on_event(PaletteEvent::Dismiss))
                .block_mouse()
                .child(panel)
                .into_any(),
        )
    }

    fn item_row(
        &self,
        row: usize,
        found: Match,
        row_h: f32,
        theme: &Theme,
        on_event: &impl Fn(PaletteEvent) -> Action,
    ) -> AnyElement {
        let tc = &theme.colors;
        let scale = theme.metrics.ui_scale();
        let item = &self.items[found.item as usize];
        let selected = row == self.selected;
        let highlights = &self.positions[found.highlights.0 as usize..found.highlights.1 as usize];
        // Only the item's own shortcut: positional mod+1..9 hints next to
        // real bindings read as more bindings (New thread showed Ctrl+N and
        // the row under it Ctrl+2). Quick select still works unpainted.
        let hint = self.hints[found.item as usize].as_deref();
        let base = if selected { tc.text_strong } else { tc.text };

        let mut el = div()
            .flex_row()
            .items_center()
            .flex_shrink_0()
            .w_full()
            .h(row_h)
            .gap((Sp::SM * scale).round())
            .px((Sp::MD * scale).round())
            .rounded(Rad::MD)
            .bg(if selected {
                tc.sidebar_row_selected
            } else {
                Color::TRANSPARENT
            })
            .on_click(on_event(PaletteEvent::Activate(row)))
            .cursor(CursorHint::Pointer)
            .key(item.key.to_string())
            .test_id("palette-item")
            .semantic_role(SemanticRole::ListBoxOption)
            .accessibility_role(accesskit::Role::ListBoxOption)
            .accessibility_label(item.title.clone())
            .accessibility_selected(selected);
        if !selected {
            el = el.hover_bg(tc.sidebar_row_hover);
        }
        if let Some(svg) = item.icon {
            el = el.child(svg_icon(svg, Ico::SM).color(tc.icon));
        }
        el = el.child(highlighted_title(&item.title, highlights, base, tc.accent));
        if let Some(subtitle) = &item.subtitle {
            el = el.child(
                text(subtitle.as_str())
                    .text_xs()
                    .truncate()
                    .color(tc.text_muted),
            );
        }
        if let Some(hint) = hint {
            el = el.child(text(hint).text_xs().mono().color(tc.text_muted));
        }
        el.into_any()
    }
}

/// `title` as a row of text runs with the characters at byte offsets
/// `highlights` (ascending) in `accent`.
fn highlighted_title(title: &str, highlights: &[u32], base: Color, accent: Color) -> AnyElement {
    let mut row = div().flex_row().flex_1().min_w(0.0).overflow_hidden();
    if highlights.is_empty() {
        return row
            .child(text(title).text_sm().truncate().color(base))
            .into_any();
    }
    let mut cursor = 0;
    let mut i = 0;
    while i < highlights.len() {
        let start = highlights[i] as usize;
        let mut end = start + title[start..].chars().next().map_or(0, char::len_utf8);
        // Merge adjacent matched characters into one run.
        while i + 1 < highlights.len() && highlights[i + 1] as usize == end {
            i += 1;
            end += title[end..].chars().next().map_or(0, char::len_utf8);
        }
        if cursor < start {
            row = row.child(text(&title[cursor..start]).text_sm().color(base));
        }
        row = row.child(text(&title[start..end]).text_sm().semibold().color(accent));
        cursor = end;
        i += 1;
    }
    if cursor < title.len() {
        row = row.child(text(&title[cursor..]).text_sm().truncate().color(base));
    }
    row.into_any()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Candidates `query` matches, best first, by the order the palette
    /// shows them.
    fn ranked<'a>(query: &str, candidates: &[&'a str]) -> Vec<&'a str> {
        let mut matcher = FuzzyMatcher::new(query);
        let mut positions = Vec::new();
        let mut scored: Vec<(i32, usize, &str)> = candidates
            .iter()
            .enumerate()
            .filter_map(|(i, c)| Some((matcher.score(c, &mut positions)?, i, *c)))
            .collect();
        scored.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then(a.2.len().cmp(&b.2.len()))
                .then(a.1.cmp(&b.1))
        });
        scored.into_iter().map(|(_, _, c)| c).collect()
    }

    /// The matched characters of `candidate`, bracketed.
    fn highlighted(query: &str, candidate: &str) -> String {
        let mut positions = Vec::new();
        FuzzyMatcher::new(query)
            .score(candidate, &mut positions)
            .expect("matches");
        let mut out = String::new();
        for (offset, c) in candidate.char_indices() {
            if positions.contains(&(offset as u32)) {
                out.push('[');
                out.push(c);
                out.push(']');
            } else {
                out.push(c);
            }
        }
        out.replace("][", "")
    }

    // Catches a scorer that ignores word starts, runs, camelCase, gaps, or
    // case folding, or that matches out of order.
    #[test]
    fn fuzzy_ranking_prefers_word_starts_runs_and_tight_matches() {
        let cases: &[(&str, &[&str], &[&str])] = &[
            // Word starts beat letters buried mid-word.
            ("gs", &["bogus", "Git: Status"], &["Git: Status", "bogus"]),
            // A run beats the same letters spread out.
            (
                "term",
                &["the rematch", "Toggle terminal"],
                &["Toggle terminal", "the rematch"],
            ),
            // camelCase humps count as word starts.
            ("nt", &["intent", "newThread"], &["newThread", "intent"]),
            // Path segments count as word starts.
            (
                "mr",
                &["src/main.rs", "crates/quark/mirror.rs"],
                &["src/main.rs", "crates/quark/mirror.rs"],
            ),
            // Order matters; a query out of order does not match.
            ("ba", &["ab"], &[]),
            // Lowercase queries ignore case; an uppercase letter makes the
            // query case-sensitive.
            ("open", &["OPEN FILE"], &["OPEN FILE"]),
            ("Open", &["open file"], &[]),
            // Equal scores go to the shorter text.
            ("new", &["New window", "New"], &["New", "New window"]),
        ];
        for (query, candidates, expected) in cases {
            assert_eq!(&ranked(query, candidates), expected, "query {query:?}");
        }
    }

    // Catches highlights on the wrong occurrence of a letter, or offsets
    // that are char indices instead of byte offsets.
    #[test]
    fn highlights_mark_the_best_occurrence_by_byte_offset() {
        let cases = [
            ("gs", "Git: Status", "[G]it: [S]tatus"),
            ("term", "Toggle terminal", "Toggle [term]inal"),
            ("ém", "café menu", "caf[é] [m]enu"),
        ];
        for (query, candidate, expected) in cases {
            assert_eq!(highlighted(query, candidate), expected, "query {query:?}");
        }
    }
}
