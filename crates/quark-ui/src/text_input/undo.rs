//! Edit log behind undo and redo, shared by [`super::TextField`] and
//! [`super::Editor`].
//!
//! Every text change is recorded as one [`Edit`]. Consecutive edits of the
//! same kind coalesce into one undo step while they stay contiguous, arrive
//! within [`COALESCE_PAUSE_MS`] of each other, and (for typing) do not start
//! a new word. Time comes from the caller as `now_ms`.

use quark_text::TextOffset;

use super::atoms::{AtomList, InlineAtom};
use super::styles::{StyleList, StyleSpan};

/// A pause longer than this between edits starts a new undo step.
pub const COALESCE_PAUSE_MS: u64 = 1000;
/// Oldest steps are dropped beyond this many.
const MAX_STEPS: usize = 500;

/// `(anchor, cursor)`.
pub type Selection = (TextOffset, TextOffset);

/// One replacement: `removed` at byte `at` became `inserted`. Replaying
/// checks that the text still holds `removed` (or `inserted`) there before
/// touching it, so `at` never slices text it was not recorded against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub at: usize,
    pub removed: String,
    pub inserted: String,
    /// Atoms in `removed` and `inserted`, relative to `at`.
    pub removed_atoms: Vec<InlineAtom>,
    pub inserted_atoms: Vec<InlineAtom>,
    /// Style runs over `removed` and `inserted`, relative to `at`. A
    /// formatting change is an edit whose `inserted` text equals `removed`.
    pub removed_styles: Vec<StyleSpan>,
    pub inserted_styles: Vec<StyleSpan>,
    pub before: Selection,
    pub after: Selection,
}

/// How an edit may coalesce with the previous one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditKind {
    /// Typed text; merges with directly following typed text.
    Typing,
    /// Single-grapheme deletes; merge while they stay adjacent.
    Deleting,
    /// Always its own step (paste, cut, word deletes, selection deletes).
    Other,
}

#[derive(Debug, Clone)]
struct Step {
    edits: Vec<Edit>,
    kind: EditKind,
    last_ms: u64,
}

#[derive(Debug, Clone, Default)]
pub struct EditLog {
    undo: Vec<Step>,
    redo: Vec<Step>,
    /// Whether the next edit may merge into the last step.
    open: bool,
}

impl EditLog {
    pub fn record(&mut self, edit: Edit, kind: EditKind, now_ms: u64) {
        self.redo.clear();
        if self.open
            && let Some(step) = self.undo.last_mut()
            && step.kind == kind
            && now_ms.saturating_sub(step.last_ms) <= COALESCE_PAUSE_MS
            && step
                .edits
                .last()
                .is_some_and(|prev| continues(prev, &edit, kind))
        {
            step.edits.push(edit);
            step.last_ms = now_ms;
            return;
        }
        self.undo.push(Step {
            edits: vec![edit],
            kind,
            last_ms: now_ms,
        });
        if self.undo.len() > MAX_STEPS {
            self.undo.remove(0);
        }
        self.open = kind != EditKind::Other;
    }

    /// The next edit starts a new step (the caret jumped, focus moved, ...).
    pub fn break_coalescing(&mut self) {
        self.open = false;
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Revert the last step in `doc`; returns the selection to restore. If
    /// the text no longer matches the log (changed outside it), the log is
    /// cleared and nothing happens.
    pub fn undo(&mut self, doc: Doc<'_>) -> Option<Selection> {
        let Doc {
            text,
            atoms,
            styles,
        } = doc;
        self.open = false;
        let step = self.undo.pop()?;
        let mut applied = 0;
        for edit in step.edits.iter().rev() {
            if !replay(text, atoms, styles, edit, false) {
                break;
            }
            applied += 1;
        }
        if applied < step.edits.len() {
            // Put back what was reverted so `text` matches the log's view
            // of history no worse than before, then drop the history.
            for edit in step.edits.iter().rev().take(applied).rev() {
                replay(text, atoms, styles, edit, true);
            }
            self.clear();
            return None;
        }
        let selection = step.edits.first().map(|edit| edit.before);
        self.redo.push(step);
        selection
    }

    /// Reapply the last undone step; returns the selection to restore.
    pub fn redo(&mut self, doc: Doc<'_>) -> Option<Selection> {
        let Doc {
            text,
            atoms,
            styles,
        } = doc;
        self.open = false;
        let step = self.redo.pop()?;
        let mut applied = 0;
        for edit in &step.edits {
            if !replay(text, atoms, styles, edit, true) {
                break;
            }
            applied += 1;
        }
        if applied < step.edits.len() {
            for edit in step.edits.iter().take(applied).rev() {
                replay(text, atoms, styles, edit, false);
            }
            self.clear();
            return None;
        }
        let selection = step.edits.last().map(|edit| edit.after);
        self.undo.push(step);
        selection
    }
}

/// Apply `edit` forward (`removed` becomes `inserted`) or backward, to the
/// text and its atoms together.
fn replay(
    text: &mut String,
    atoms: &mut AtomList,
    styles: &mut StyleList,
    edit: &Edit,
    forward: bool,
) -> bool {
    let (expected, with, with_atoms, with_styles) = if forward {
        (
            &edit.removed,
            &edit.inserted,
            &edit.inserted_atoms,
            &edit.inserted_styles,
        )
    } else {
        (
            &edit.inserted,
            &edit.removed,
            &edit.removed_atoms,
            &edit.removed_styles,
        )
    };
    if !replace_checked(text, edit.at, expected, with) {
        return false;
    }
    atoms.splice(edit.at, expected.len(), with_atoms, with.len());
    styles.splice(edit.at, expected.len(), with_styles, with.len());
    true
}

/// The parts of a buffer an edit changes.
pub struct Doc<'a> {
    pub text: &'a mut String,
    pub atoms: &'a mut AtomList,
    pub styles: &'a mut StyleList,
}

/// Whether `next` directly continues `prev` as one step of `kind`.
fn continues(prev: &Edit, next: &Edit, kind: EditKind) -> bool {
    if next.before != prev.after {
        return false;
    }
    match kind {
        EditKind::Typing => {
            let contiguous = next.removed.is_empty() && next.at == prev.at + prev.inserted.len();
            // "hello world" undoes as "world" then "hello ": a word starts a step.
            let starts_word = prev.inserted.ends_with(char::is_whitespace)
                && next.inserted.starts_with(|c: char| !c.is_whitespace());
            contiguous && !starts_word
        }
        EditKind::Deleting => {
            let backward = next.at + next.removed.len() == prev.at;
            let forward = next.at == prev.at;
            next.inserted.is_empty() && prev.inserted.is_empty() && (backward || forward)
        }
        EditKind::Other => false,
    }
}

/// Replace `expected` at `at` with `with`, only if `text` really holds
/// `expected` there.
fn replace_checked(text: &mut String, at: usize, expected: &str, with: &str) -> bool {
    let end = at + expected.len();
    if text.get(at..end) != Some(expected) {
        return false;
    }
    text.replace_range(at..end, with);
    true
}
