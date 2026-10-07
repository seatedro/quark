//! Find in a block document: the matches of a query over every block's
//! plain text, in document order, with a current match that next and
//! previous step through.
//!
//! [`FindState`] works on the document model (block keys, revisions, and
//! text), not on layouts, so it covers blocks that were never materialized.
//! It rescans only the blocks whose revision changed since the last
//! [`FindState::update`], so keeping it current while text streams into
//! the last block costs a walk over the block keys plus one block's scan.
//!
//! Matching is case-insensitive by Unicode lowercase mapping, with `ß`
//! matching `ss` and final sigma matching sigma.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::ops::Range;

use quark::selection::BlockKey;
use quark_text::TextOffset;

use crate::action::{Action, FocusId};
use crate::element::{AnyElement, IntoAnyElement, div, text, text_input};
use crate::style::Styled;
use crate::text_input::TextField;
use crate::theme::Theme;

/// One match: a byte range of a block's text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FindMatch {
    pub block: BlockKey,
    pub range: Range<TextOffset>,
}

/// What [`FindState::verify_integrity`] found wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FindIntegrityError {
    /// A block's matches disagree with the match list, or the list holds
    /// matches no block has (`BlockKey(u64::MAX)`).
    MatchList {
        block: BlockKey,
    },
    Current,
}

/// The matches of one block at one revision.
#[derive(Debug, Clone)]
struct BlockScan {
    revision: u64,
    ranges: Vec<Range<usize>>,
    /// Index of the block's first match in [`FindState::matches`].
    first: usize,
}

/// A query and its matches over a block document. See the
/// [module docs](self).
#[derive(Debug, Clone, Default)]
pub struct FindState {
    query: String,
    folded: String,
    matches: Vec<FindMatch>,
    current: Option<usize>,
    scans: HashMap<BlockKey, BlockScan>,
    /// Last update's scans, emptied and kept for their capacity.
    spare: HashMap<BlockKey, BlockScan>,
}

impl FindState {
    pub fn new(query: &str) -> Self {
        let mut state = Self::default();
        state.set_query(query);
        state
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    /// Replaces the query; the next [`Self::update`] scans every block.
    pub fn set_query(&mut self, query: &str) {
        if query == self.query {
            return;
        }
        self.query.clear();
        self.query.push_str(query);
        self.folded.clear();
        fold_into(query, &mut self.folded, None);
        self.scans.clear();
        self.matches.clear();
        self.current = None;
    }

    /// Brings the matches up to date with `blocks`, each `(key, revision,
    /// text)` in document order. Blocks whose revision is unchanged keep
    /// their matches. The current match stays on the same text when it
    /// still matches, or moves to the next match after it. Returns whether
    /// the matches changed.
    pub fn update<'a>(
        &mut self,
        blocks: impl IntoIterator<Item = (BlockKey, u64, &'a str)>,
    ) -> bool {
        let current = self.current().cloned();
        let mut scans = std::mem::take(&mut self.spare);
        let old = std::mem::take(&mut self.matches);
        let mut changed = false;
        for (key, revision, text) in blocks {
            let scan = match self.scans.remove(&key) {
                Some(scan) if scan.revision == revision => scan,
                _ => {
                    changed = true;
                    BlockScan {
                        revision,
                        ranges: find_folded(text, &self.folded),
                        first: 0,
                    }
                }
            };
            let first = self.matches.len();
            changed |= scan.first != first && !scan.ranges.is_empty();
            self.matches.extend(scan.ranges.iter().map(|r| FindMatch {
                block: key,
                range: TextOffset::snap(text, r.start)..TextOffset::snap(text, r.end),
            }));
            scans.insert(key, BlockScan { first, ..scan });
        }
        // Blocks left in the old map have left the document.
        changed |= self.scans.values().any(|scan| !scan.ranges.is_empty());
        self.scans.clear();
        self.spare = std::mem::replace(&mut self.scans, scans);
        changed |= old.len() != self.matches.len();
        self.current = match current {
            None if self.matches.is_empty() => None,
            None => Some(0),
            Some(at) => self.locate(&at, &old),
        };
        debug_assert_eq!(self.verify_integrity(), Ok(()));
        changed
    }

    /// Every block's matches sit at its `first` index in the match list,
    /// the blocks' matches tile the list, and the current match exists.
    pub fn verify_integrity(&self) -> Result<(), FindIntegrityError> {
        let mut covered = 0;
        for (key, scan) in &self.scans {
            for (i, range) in scan.ranges.iter().enumerate() {
                let found = self.matches.get(scan.first + i);
                if found.is_none_or(|m| {
                    m.block != *key
                        || m.range.start.get() != range.start
                        || m.range.end.get() != range.end
                }) {
                    return Err(FindIntegrityError::MatchList { block: *key });
                }
            }
            covered += scan.ranges.len();
        }
        if covered != self.matches.len() {
            return Err(FindIntegrityError::MatchList {
                block: BlockKey(u64::MAX),
            });
        }
        if self.current.is_some_and(|i| i >= self.matches.len()) {
            return Err(FindIntegrityError::Current);
        }
        Ok(())
    }

    /// Index of `at` in the new matches, or of the first match after where
    /// it was, wrapping to the first.
    fn locate(&self, at: &FindMatch, old: &[FindMatch]) -> Option<usize> {
        if self.matches.is_empty() {
            return None;
        }
        if let Some(i) = self.matches.iter().position(|m| m == at) {
            return Some(i);
        }
        // The old matches after `at`, in order; the first one still present
        // is where the current match goes.
        let from = old.iter().position(|m| m == at).map_or(0, |i| i + 1);
        old[from..]
            .iter()
            .find_map(|m| self.matches.iter().position(|n| n == m))
            .or(Some(0))
    }

    /// Every match, in document order.
    pub fn matches(&self) -> &[FindMatch] {
        &self.matches
    }

    pub fn current_index(&self) -> Option<usize> {
        self.current
    }

    pub fn current(&self) -> Option<&FindMatch> {
        self.matches.get(self.current?)
    }

    /// Moves to the next match, wrapping past the last.
    pub fn next_match(&mut self) -> Option<&FindMatch> {
        let n = self.matches.len();
        self.current = (n > 0).then(|| self.current.map_or(0, |i| (i + 1) % n));
        self.current()
    }

    /// Moves to the previous match, wrapping before the first.
    pub fn prev_match(&mut self) -> Option<&FindMatch> {
        let n = self.matches.len();
        self.current = (n > 0).then(|| self.current.map_or(n - 1, |i| (i + n - 1) % n));
        self.current()
    }

    /// The byte ranges of `block`'s matches, each with whether it is the
    /// current match.
    pub fn block_matches(
        &self,
        block: BlockKey,
    ) -> impl Iterator<Item = (Range<usize>, bool)> + '_ {
        let scan = self.scans.get(&block);
        let first = scan.map_or(0, |s| s.first);
        scan.map_or(&[][..], |s| &s.ranges[..])
            .iter()
            .enumerate()
            .map(move |(i, r)| (r.clone(), self.current == Some(first + i)))
    }

    /// Hashes what highlighting `block` reads, for caching its painting.
    pub fn hash_block(&self, block: BlockKey, hasher: &mut impl Hasher) {
        for (range, current) in self.block_matches(block) {
            (range.start, range.end, current).hash(hasher);
        }
    }

    /// `"3 of 12"`, `"No matches"`, or empty without a query.
    pub fn status(&self) -> String {
        match (self.query.is_empty(), self.current) {
            (true, _) => String::new(),
            (false, Some(i)) => quark_i18n::tr_args(
                "quark-find-status",
                [
                    ("current", (i + 1).into()),
                    ("total", self.matches.len().into()),
                ],
            ),
            (false, None) => quark_i18n::tr("quark-find-no-matches"),
        }
    }
}

/// Folds one char for matching: lowercase, with `ß` as `ss` and final
/// sigma as sigma.
fn fold_char(c: char, out: &mut String) {
    match c {
        'ß' | 'ẞ' => out.push_str("ss"),
        'ς' => out.push('σ'),
        c => out.extend(c.to_lowercase()),
    }
}

/// Appends the folding of `text` to `out`. With `starts`, records each
/// char's `(folded byte, original byte)` start, plus a final entry for the
/// ends.
fn fold_into(text: &str, out: &mut String, mut starts: Option<&mut Vec<(usize, usize)>>) {
    for (at, c) in text.char_indices() {
        if let Some(starts) = starts.as_deref_mut() {
            starts.push((out.len(), at));
        }
        fold_char(c, out);
    }
    if let Some(starts) = starts {
        starts.push((out.len(), text.len()));
    }
}

/// Byte ranges of `text` whose folding equals `query` (already folded). A
/// match must start and end on whole chars of `text`: `s` does not match
/// half of `ß`.
fn find_folded(text: &str, query: &str) -> Vec<Range<usize>> {
    let mut found = Vec::new();
    if query.is_empty() || text.is_empty() {
        return found;
    }
    let mut folded = String::with_capacity(text.len());
    let mut starts = Vec::with_capacity(text.len() + 1);
    fold_into(text, &mut folded, Some(&mut starts));
    let original = |folded_at: usize| {
        starts
            .binary_search_by_key(&folded_at, |&(f, _)| f)
            .ok()
            .map(|i| starts[i].1)
    };
    let mut from = 0;
    while let Some(at) = folded[from..].find(query) {
        let start = from + at;
        let end = start + query.len();
        if let (Some(a), Some(b)) = (original(start), original(end)) {
            found.push(a..b);
            from = end;
        } else {
            from = start + folded[start..].chars().next().map_or(1, char::len_utf8);
        }
    }
    found
}

/// What the buttons of a [`find_bar`] emit.
pub struct FindBarActions {
    pub next: Action,
    pub prev: Action,
    pub close: Action,
}

/// A find bar: the query field, the match count, and previous, next, and
/// close buttons. The app owns `field` and routes its edits to the
/// document's query (see `Document::set_find_query`); Enter and
/// Shift+Enter in the field are the app's to bind to next and previous.
pub fn find_bar(
    find: Option<&FindState>,
    field: &TextField,
    focus: FocusId,
    focused: bool,
    actions: FindBarActions,
    theme: &Theme,
) -> AnyElement {
    let colors = &theme.colors;
    let size = theme.metrics.ui_small_font_size;
    let button = |id: &str, label: &str, glyph: &str, action: Action| {
        div()
            .accessibility_id(id)
            .accessibility_role(accesskit::Role::Button)
            .accessibility_label(label)
            .on_click(action)
            .w(size * 2.0)
            .h(size * 2.0)
            .rounded(size * 0.4)
            .items_center()
            .justify_center()
            .hover_bg(colors.element_background)
            .child(text(glyph).size(size).color(colors.text))
    };
    let status = find.map(FindState::status).unwrap_or_default();
    div()
        .accessibility_id("find.bar")
        .accessibility_role(accesskit::Role::Search)
        .accessibility_label(quark_i18n::tr("quark-find"))
        .flex_row()
        .items_center()
        .gap(size * 0.5)
        .p(size * 0.4)
        .rounded(size * 0.5)
        .bg(colors.panel)
        .child(
            text_input(quark_i18n::tr("quark-find"), "")
                .field(field)
                .placeholder(quark_i18n::tr("quark-find"))
                .search(true)
                .focus_target(focus)
                .focused(focused)
                .w(size * 18.0)
                .h(size * 2.2),
        )
        .child(
            div()
                .accessibility_id("find.status")
                .accessibility_role(accesskit::Role::Status)
                .accessibility_label(status.clone())
                .min_w(size * 6.0)
                .child(text(status).size(size).color(colors.text_muted)),
        )
        .child(button(
            "find.prev",
            &quark_i18n::tr("quark-find-previous"),
            "\u{2191}",
            actions.prev,
        ))
        .child(button(
            "find.next",
            &quark_i18n::tr("quark-find-next"),
            "\u{2193}",
            actions.next,
        ))
        .child(button(
            "find.close",
            &quark_i18n::tr("quark-find-close"),
            "\u{2715}",
            actions.close,
        ))
        .into_any()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `"text[a..b]"` for each match of `query` in `text`.
    fn found(text: &str, query: &str) -> Vec<String> {
        let mut folded = String::new();
        fold_into(query, &mut folded, None);
        find_folded(text, &folded)
            .into_iter()
            .map(|r| text[r].to_owned())
            .collect()
    }

    #[test]
    fn matching_folds_case_and_unicode_on_whole_chars() {
        let table: &[(&str, &str, &[&str])] = &[
            ("Rust rust RUST", "rust", &["Rust", "rust", "RUST"]),
            ("Straße STRASSE", "strasse", &["Straße", "STRASSE"]),
            ("ΟΔΟΣ οδος", "οδοσ", &["ΟΔΟΣ", "οδος"]),
            ("Ärger ärger", "ÄRGER", &["Ärger", "ärger"]),
            // `s` alone never lands inside `ß`.
            ("ß", "s", &[]),
            ("aaaa", "aa", &["aa", "aa"]),
            ("text", "", &[]),
        ];
        for (text, query, expected) in table {
            assert_eq!(found(text, query), *expected, "{query:?} in {text:?}");
        }
    }
}
