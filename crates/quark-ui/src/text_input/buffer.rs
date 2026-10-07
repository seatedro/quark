//! The editing core shared by [`super::TextField`] and [`super::Editor`]:
//! text, caret and anchor, IME preedit, and undo history, driven by
//! [`TextEditCommand`]s.
//!
//! Caret and anchor are always grapheme boundaries of the current text.
//! Commands carry raw byte indices (they come from pointer hits on an
//! earlier frame, the app, or assistive tech), and every one is snapped
//! onto the current text before it is used.

use std::ops::Range;

use quark_text::TextOffset;
use quark_text::offset;

use super::atoms::{AtomList, InlineAtom, RichClipboard, RichText};
use super::hooks::{InputHooks, Insertion, NoHooks};
use super::ime::{Composition, Preedit, compose};
use super::text_edit::{TextEditCommand, TextEditOutcome};
use super::undo::{Edit, EditKind, EditLog};

/// Where word-wise forward movement and deletion stop.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum WordForward {
    /// The start of the next word (single-line fields).
    #[default]
    NextStart,
    /// The end of the current or next word (the editor).
    NextEnd,
}

#[derive(Debug, Clone, Default)]
pub(super) struct TextBuffer {
    text: String,
    /// Atomic spans of `text`; the caret never rests inside one.
    atoms: AtomList,
    /// Where copies of text with atoms keep them for pasting back.
    rich_clipboard: RichClipboard,
    cursor: TextOffset,
    anchor: TextOffset,
    preedit: Option<Preedit>,
    /// Set whenever `preedit` changes, until taken.
    preedit_dirty: bool,
    /// Bumped whenever `preedit` changes.
    preedit_rev: u32,
    history: EditLog,
    /// Last `now_ms` from the app; edits are stamped with it for undo
    /// coalescing.
    now_ms: u64,
    /// Home, End, line delete, and line selection work on the logical line
    /// rather than the whole text.
    multiline: bool,
    word_forward: WordForward,
    read_only: bool,
}

impl PartialEq for TextBuffer {
    fn eq(&self, other: &Self) -> bool {
        (
            &self.text,
            &self.atoms,
            self.cursor,
            self.anchor,
            &self.preedit,
        ) == (
            &other.text,
            &other.atoms,
            other.cursor,
            other.anchor,
            &other.preedit,
        )
    }
}

impl TextBuffer {
    /// An empty buffer of the given flavor.
    pub(super) fn new(multiline: bool, word_forward: WordForward) -> Self {
        Self {
            multiline,
            word_forward,
            ..Self::default()
        }
    }

    pub(super) fn text(&self) -> &str {
        &self.text
    }

    pub(super) fn cursor(&self) -> TextOffset {
        self.cursor
    }

    pub(super) fn anchor(&self) -> TextOffset {
        self.anchor
    }

    /// The selection in order; empty at the caret when nothing is selected.
    pub(super) fn selection(&self) -> Range<TextOffset> {
        self.cursor.min(self.anchor)..self.cursor.max(self.anchor)
    }

    pub(super) fn selected_text(&self) -> Option<&str> {
        let selected = offset::slice(&self.text, self.selection());
        (!selected.is_empty()).then_some(selected)
    }

    pub(super) fn atoms(&self) -> &[InlineAtom] {
        self.atoms.as_slice()
    }

    /// `range` of the text with the atoms wholly inside it.
    pub(super) fn rich_slice(&self, range: Range<TextOffset>) -> RichText {
        let range = range.start.get()..range.end.get();
        RichText {
            text: offset::slice(&self.text, range.clone()).to_owned(),
            atoms: self.atoms.slice(range),
        }
    }

    pub(super) fn set_rich_clipboard(&mut self, clipboard: RichClipboard) {
        self.rich_clipboard = clipboard;
    }

    /// Replace the text and atoms programmatically, caret at the end. Atoms
    /// that do not fit `rich.text` are dropped. Clears undo history.
    pub(super) fn set_rich_text(&mut self, rich: &RichText) {
        self.set_text(&rich.text);
        if rich.verify_integrity().is_ok() {
            self.atoms.set(&rich.atoms);
        }
    }

    fn debug_check_atoms(&self) {
        debug_assert_eq!(self.atoms.verify_integrity(&self.text), Ok(()));
    }

    pub(super) fn now_ms(&self) -> u64 {
        self.now_ms
    }

    pub(super) fn set_now(&mut self, now_ms: u64) {
        self.now_ms = now_ms;
    }

    pub(super) fn set_read_only(&mut self, read_only: bool) {
        self.read_only = read_only;
    }

    /// Replace the text programmatically with the caret at the end.
    /// Clears undo history and any composition.
    pub(super) fn set_text(&mut self, text: &str) {
        self.text.clear();
        self.text.push_str(text);
        self.atoms.clear();
        self.cursor = TextOffset::end(&self.text);
        self.anchor = self.cursor;
        self.history.clear();
        self.clear_preedit();
    }

    /// Append text programmatically (streaming). A caret at the end
    /// follows it. Earlier undo steps stay valid because their offsets
    /// are untouched.
    pub(super) fn append(&mut self, text: &str) {
        let at_end = self.cursor == self.text.len() && self.anchor == self.cursor;
        self.text.push_str(text);
        if at_end {
            self.cursor = TextOffset::end(&self.text);
            self.anchor = self.cursor;
        }
        self.history.break_coalescing();
    }

    pub(super) fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    pub(super) fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    pub(super) fn preedit(&self) -> Option<&Preedit> {
        self.preedit.as_ref()
    }

    /// Show `text` as the IME composition at the caret without touching the
    /// committed text. Empty text cancels it. `cursor` is the IME's byte
    /// range inside `text`. Returns whether the composition changed.
    pub(super) fn set_preedit(
        &mut self,
        text: impl Into<String>,
        cursor: Option<(usize, usize)>,
    ) -> bool {
        let preedit = Preedit::new(text, cursor);
        if preedit == self.preedit {
            return false;
        }
        self.preedit = preedit;
        self.preedit_changed();
        true
    }

    fn clear_preedit(&mut self) {
        if self.preedit.take().is_some() {
            self.preedit_changed();
        }
    }

    fn preedit_changed(&mut self) {
        self.preedit_dirty = true;
        self.preedit_rev = self.preedit_rev.wrapping_add(1);
    }

    /// Whether the preedit changed since the last call.
    pub(super) fn take_preedit_dirty(&mut self) -> bool {
        std::mem::take(&mut self.preedit_dirty)
    }

    /// The text as painted while composing, or `None` when not composing.
    pub(super) fn composition(&self) -> Option<Composition> {
        let preedit = self.preedit.as_ref()?;
        Some(compose(&self.text, self.selection(), preedit))
    }

    /// Commit composed IME text over the selection. Each commit is its own
    /// undo step.
    pub(super) fn commit_ime(&mut self, value: &str) -> bool {
        self.history.break_coalescing();
        let changed = self.replace(self.selection(), value, &[], EditKind::Typing);
        self.history.break_coalescing();
        changed
    }

    /// Replace `range` with `inserted` (holding `inserted_atoms`, relative
    /// to it), record it for undo, and collapse the caret after the
    /// inserted text. The one place user edits change the text. A range
    /// that touches part of an atom takes all of it, and an insertion
    /// inside one lands after it. Any composition is dropped.
    fn replace(
        &mut self,
        range: Range<TextOffset>,
        inserted: &str,
        inserted_atoms: &[InlineAtom],
        kind: EditKind,
    ) -> bool {
        if self.read_only {
            return false;
        }
        self.clear_preedit();
        let range = offset::ordered(&self.text, range);
        let range = self.atoms.expand(range.start.get()..range.end.get());
        let range =
            TextOffset::snap(&self.text, range.start)..TextOffset::snap(&self.text, range.end);
        if range.is_empty() && inserted.is_empty() {
            return false;
        }
        let before = (self.anchor, self.cursor);
        let removed = offset::replace(&mut self.text, range.clone(), inserted);
        let at = range.start.get();
        let removed_atoms = self
            .atoms
            .splice(at, removed.len(), inserted_atoms, inserted.len());
        self.debug_check_atoms();
        // Rounding up keeps the caret after inserted text that merges into
        // the grapheme after it (a combining mark typed before another).
        self.cursor = TextOffset::snap_up(&self.text, at + inserted.len());
        self.anchor = self.cursor;
        let edit = Edit {
            at,
            removed,
            inserted: inserted.to_owned(),
            removed_atoms,
            inserted_atoms: inserted_atoms.to_vec(),
            before,
            after: (self.anchor, self.cursor),
        };
        self.history.record(edit, kind, self.now_ms);
        true
    }

    /// Replace `range` with `insertion`; each is its own undo step.
    fn insert(&mut self, range: Range<TextOffset>, insertion: &Insertion) -> bool {
        self.history.break_coalescing();
        match insertion {
            Insertion::Text(text) => self.replace(range, text, &[], EditKind::Other),
            Insertion::Rich(rich) if rich.verify_integrity().is_ok() => {
                self.replace(range, &rich.text, &rich.atoms, EditKind::Other)
            }
            Insertion::Rich(rich) => self.replace(range, &rich.text, &[], EditKind::Other),
            Insertion::Nothing => false,
        }
    }

    /// [`Self::insert`] reporting an outcome.
    pub(super) fn insert_at(
        &mut self,
        range: Option<Range<TextOffset>>,
        insertion: &Insertion,
    ) -> TextEditOutcome {
        let before = (self.cursor, self.anchor, self.preedit_rev);
        let range = range.unwrap_or_else(|| self.selection());
        let text_changed = self.insert(range, insertion);
        self.outcome(before, text_changed)
    }

    fn outcome(
        &self,
        before: (TextOffset, TextOffset, u32),
        text_changed: bool,
    ) -> TextEditOutcome {
        TextEditOutcome {
            text_changed,
            selection_changed: (self.cursor, self.anchor) != (before.0, before.1),
            clipboard_write: None,
            preedit_changed: self.preedit_rev != before.2,
        }
    }

    /// The selection exported for the system clipboard, keeping its atoms
    /// in the app's clipboard.
    fn copy_selection(&self) -> Option<String> {
        let selection = self.selection();
        if selection.is_empty() {
            return None;
        }
        let rich = self.rich_slice(selection);
        if rich.atoms.is_empty() {
            self.rich_clipboard.store(None);
            return Some(rich.text);
        }
        let exported = rich.export();
        self.rich_clipboard.store(Some(rich));
        Some(exported)
    }

    fn delete_selection(&mut self) -> bool {
        let selection = self.selection();
        !selection.is_empty() && self.replace(selection, "", &[], EditKind::Other)
    }

    /// Delete the selection, or from the caret to `target`.
    fn delete_to(&mut self, target: TextOffset, kind: EditKind) -> bool {
        self.delete_selection() || self.replace(self.cursor..target, "", &[], kind)
    }

    /// Move the caret to `at`, keeping the anchor when `extend`. A target
    /// inside an atom goes on past it in the direction of travel. Ends the
    /// current undo step and any composition.
    pub(super) fn move_to(&mut self, at: TextOffset, extend: bool) {
        let at = at.within(&self.text);
        let at = match self.atoms.containing(at.get()) {
            Some(atom) if at < self.cursor => atom.range.start,
            Some(atom) => atom.range.end,
            None => at.get(),
        };
        self.place(TextOffset::snap(&self.text, at), extend);
    }

    /// [`Self::move_to`] for a pointer: a point inside an atom goes to its
    /// nearer edge.
    fn move_to_point(&mut self, raw: usize, extend: bool) {
        let at = TextOffset::snap(&self.text, raw);
        let at = match self.atoms.containing(at.get()) {
            Some(atom) if at.get() - atom.range.start < atom.range.end - at.get() => {
                atom.range.start
            }
            Some(atom) => atom.range.end,
            None => at.get(),
        };
        self.place(TextOffset::snap(&self.text, at), extend);
    }

    fn place(&mut self, at: TextOffset, extend: bool) {
        self.history.break_coalescing();
        self.clear_preedit();
        self.cursor = at;
        if !extend {
            self.anchor = self.cursor;
        }
    }

    /// Select `range`, grown to take whole any atom it touches.
    fn select(&mut self, range: Range<TextOffset>) {
        let range = self.atoms.expand(range.start.get()..range.end.get());
        self.place(TextOffset::snap(&self.text, range.start), false);
        self.place(TextOffset::snap(&self.text, range.end), true);
    }

    /// Move the caret to `target(self, cursor)`, or, when not extending,
    /// collapse a selection to its start (`forward` false) or end.
    fn step(&mut self, forward: bool, extend: bool, target: fn(&Self, TextOffset) -> TextOffset) {
        let selection = self.selection();
        let to = if !extend && !selection.is_empty() {
            if forward {
                selection.end
            } else {
                selection.start
            }
        } else {
            target(self, self.cursor)
        };
        self.move_to(to, extend);
    }

    fn word_forward(&self, at: TextOffset) -> TextOffset {
        match self.word_forward {
            WordForward::NextStart => offset::next_word_start(&self.text, at),
            WordForward::NextEnd => offset::next_word_end(&self.text, at),
        }
    }

    /// The line holding `at` without its line ending: the logical line
    /// when multiline, else the whole text.
    fn line_bounds(&self, at: TextOffset) -> Range<TextOffset> {
        if self.multiline {
            offset::line_start(&self.text, at)..offset::line_end(&self.text, at)
        } else {
            TextOffset::ZERO..TextOffset::end(&self.text)
        }
    }

    fn undo(&mut self) -> bool {
        if self.read_only {
            return false;
        }
        self.clear_preedit();
        let restored = self.history.undo(&mut self.text, &mut self.atoms);
        self.debug_check_atoms();
        self.restore(restored)
    }

    fn redo(&mut self) -> bool {
        if self.read_only {
            return false;
        }
        self.clear_preedit();
        let restored = self.history.redo(&mut self.text, &mut self.atoms);
        self.debug_check_atoms();
        self.restore(restored)
    }

    fn restore(&mut self, selection: Option<(TextOffset, TextOffset)>) -> bool {
        let Some((anchor, cursor)) = selection else {
            return false;
        };
        self.anchor = anchor.within(&self.text);
        self.cursor = cursor.within(&self.text);
        true
    }

    /// Apply `cmd`. Vertical movement needs a layout, so it is a no-op
    /// here; [`super::Editor`] handles it (and visual line ends) itself.
    pub(super) fn apply(&mut self, cmd: TextEditCommand) -> TextEditOutcome {
        self.apply_with(cmd, &mut NoHooks)
    }

    /// [`Self::apply`] with the app's paste policy.
    pub(super) fn apply_with(
        &mut self,
        cmd: TextEditCommand,
        hooks: &mut dyn InputHooks,
    ) -> TextEditOutcome {
        use TextEditCommand::*;
        let before = (self.cursor, self.anchor, self.preedit_rev);
        let cursor = self.cursor;
        let mut outcome = TextEditOutcome::default();
        let text_changed = match cmd {
            InsertText(value) => self.replace(self.selection(), &value, &[], EditKind::Typing),
            Paste(value) => {
                let pasted = match self.rich_clipboard.resolve(&value) {
                    Some(rich) => Insertion::Rich(rich),
                    None => Insertion::Text(value),
                };
                let insertion = hooks.paste(pasted);
                self.insert(self.selection(), &insertion)
            }
            Backspace => {
                let target = offset::prev_grapheme(&self.text, cursor);
                self.delete_to(target, EditKind::Deleting)
            }
            DeleteForward => {
                let target = offset::next_grapheme(&self.text, cursor);
                self.delete_to(target, EditKind::Deleting)
            }
            BackspaceWord => {
                let target = offset::prev_word_start(&self.text, cursor);
                self.delete_to(target, EditKind::Other)
            }
            DeleteForwardWord => self.delete_to(self.word_forward(cursor), EditKind::Other),
            BackspaceLine => self.delete_to(self.line_bounds(cursor).start, EditKind::Other),
            Cut => {
                outcome.clipboard_write = self.copy_selection();
                outcome.clipboard_write.is_some() && self.delete_selection()
            }
            Undo => self.undo(),
            Redo => self.redo(),
            SetPreedit { text, cursor } => {
                self.set_preedit(text, cursor);
                false
            }
            Copy => {
                outcome.clipboard_write = self.copy_selection();
                false
            }
            other => {
                self.apply_motion(other);
                false
            }
        };
        outcome.text_changed = text_changed;
        outcome.selection_changed = (self.cursor, self.anchor) != (before.0, before.1);
        outcome.preedit_changed = self.preedit_rev != before.2;
        outcome
    }

    /// The commands that move the caret or selection without editing.
    fn apply_motion(&mut self, cmd: TextEditCommand) {
        use TextEditCommand::*;
        let line = |b: &Self| b.line_bounds(b.cursor);
        match cmd {
            CursorLeft => self.step(false, false, |b, at| offset::prev_grapheme(&b.text, at)),
            SelectLeft => self.step(false, true, |b, at| offset::prev_grapheme(&b.text, at)),
            CursorRight => self.step(true, false, |b, at| offset::next_grapheme(&b.text, at)),
            SelectRight => self.step(true, true, |b, at| offset::next_grapheme(&b.text, at)),
            CursorWordLeft => self.step(false, false, |b, at| offset::prev_word_start(&b.text, at)),
            SelectWordLeft => self.step(false, true, |b, at| offset::prev_word_start(&b.text, at)),
            CursorWordRight => self.step(true, false, Self::word_forward),
            SelectWordRight => self.step(true, true, Self::word_forward),
            CursorHome | CursorSoftHome => self.move_to(line(self).start, false),
            SelectHome | SelectSoftHome => self.move_to(line(self).start, true),
            CursorEnd | CursorSoftEnd => self.move_to(line(self).end, false),
            SelectEnd | SelectSoftEnd => self.move_to(line(self).end, true),
            SelectAll => self.select(TextOffset::ZERO..TextOffset::end(&self.text)),
            SelectWordAt(raw) => {
                let at = TextOffset::snap(&self.text, raw);
                self.select(offset::word_range_at(&self.text, at));
            }
            SelectLineAt(raw) => {
                let at = TextOffset::snap(&self.text, raw);
                let range = if self.multiline {
                    offset::line_range_at(&self.text, at)
                } else {
                    TextOffset::ZERO..TextOffset::end(&self.text)
                };
                self.select(range);
            }
            SetTextCursor(raw) => self.move_to_point(raw, false),
            ExtendTextSelection(raw) => self.move_to_point(raw, true),
            CancelPreedit => self.clear_preedit(),
            CursorUp | CursorDown | SelectUp | SelectDown => {}
            InsertText(_)
            | Paste(_)
            | Backspace
            | BackspaceWord
            | BackspaceLine
            | DeleteForward
            | DeleteForwardWord
            | Undo
            | Redo
            | Cut
            | Copy
            | SetPreedit { .. } => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use quark_text::TextSystem;
    use unicode_segmentation::UnicodeSegmentation;

    use super::super::{Editor, EditorMode, TextField};
    use super::*;
    use TextEditCommand::*;

    const PIECES: &[&str] = &[
        "a",
        " ",
        "\n",
        "\r\n",
        ".",
        "\u{e9}",
        "e\u{301}",
        "\u{65e5}",
        "\u{1f44d}\u{1f3fd}",
        "\u{1f469}\u{200d}\u{1f4bb}",
    ];

    fn command() -> impl Strategy<Value = TextEditCommand> {
        let raw = 0usize..64;
        let insert = prop::sample::select(PIECES).prop_map(|s| InsertText(s.to_owned()));
        let positioned = prop_oneof![
            raw.clone().prop_map(SetTextCursor),
            raw.clone().prop_map(ExtendTextSelection),
            raw.clone().prop_map(SelectWordAt),
            raw.prop_map(SelectLineAt),
        ];
        let plain = prop::sample::select(vec![
            Backspace,
            BackspaceWord,
            BackspaceLine,
            DeleteForward,
            DeleteForwardWord,
            CursorLeft,
            SelectRight,
            CursorWordLeft,
            SelectWordRight,
            CursorUp,
            SelectDown,
            CursorSoftHome,
            SelectSoftEnd,
            CursorHome,
            SelectEnd,
            SelectAll,
            Cut,
            Undo,
            Redo,
        ]);
        prop_oneof![2 => insert, 3 => positioned, 3 => plain]
    }

    fn is_boundary(text: &str, at: TextOffset) -> bool {
        at == text.len() || text.grapheme_indices(true).any(|(i, _)| at == i)
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]

        // Catches any command path slicing or storing a raw offset: a
        // byte inside a char, past the end, or inside a cluster panicked
        // (select_line_at, masked fields) or left the caret mid-grapheme.
        #[test]
        fn raw_offsets_through_field_and_editor_leave_carets_on_graphemes(
            initial in prop::collection::vec(prop::sample::select(PIECES), 0..12),
            commands in prop::collection::vec(command(), 1..24),
        ) {
            let initial = initial.concat();
            let mut text_system = TextSystem::vendored_only(&Default::default());
            let mut field = TextField::new(initial.as_str());
            let mut editor = Editor::new(EditorMode::CodeInput);
            editor.sync_size(90.0, 40.0);
            editor.set_text(&initial);
            for command in commands {
                field.apply(command.clone());
                editor.flush(&mut text_system);
                editor.apply(command.clone());
                for (name, text, cursor, anchor) in [
                    ("field", field.text(), field.cursor(), field.anchor()),
                    ("editor", editor.text(), editor.cursor(), editor.anchor()),
                ] {
                    prop_assert!(
                        is_boundary(text, cursor) && is_boundary(text, anchor),
                        "{} after {:?}: {:?} anchor {} cursor {}", name, command, text, anchor, cursor
                    );
                }
            }
        }
    }
}
