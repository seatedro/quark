use std::ops::Range;

use unicode_segmentation::UnicodeSegmentation;

use super::ime::{Composition, Preedit, compose, floor_char_boundary};
use super::undo::{Edit, EditKind, EditLog};
use super::view::HorizontalScroll;

/// A text editing command, independent of which widget has focus.
///
/// Platform input (keys, IME commits, clipboard reads) is translated into
/// these by the app and routed to the focused [`TextField`] or
/// [`super::Editor`]. IME commits arrive as `InsertText`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextEditCommand {
    InsertText(String),
    Backspace,
    BackspaceWord,
    BackspaceLine,
    DeleteForward,
    DeleteForwardWord,
    CursorLeft,
    CursorRight,
    CursorUp,
    CursorDown,
    CursorWordLeft,
    CursorWordRight,
    CursorHome,
    CursorEnd,
    CursorSoftHome,
    CursorSoftEnd,
    SelectLeft,
    SelectRight,
    SelectUp,
    SelectDown,
    SelectWordLeft,
    SelectWordRight,
    SelectHome,
    SelectEnd,
    SelectSoftHome,
    SelectSoftEnd,
    SelectAll,
    Copy,
    Cut,
    Paste(String),
    SetTextCursor(usize),
    ExtendTextSelection(usize),
    /// Select the word around a byte offset (double click).
    SelectWordAt(usize),
    /// Select the line around a byte offset (triple click).
    SelectLineAt(usize),
    Undo,
    Redo,
    /// Drop any IME composition without committing it (focus left the
    /// field, or the IME was turned off mid-composition).
    CancelPreedit,
}

/// What applying a [`TextEditCommand`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TextEditOutcome {
    /// The text was modified.
    pub text_changed: bool,
    /// The cursor or selection anchor moved.
    pub selection_changed: bool,
    /// Text the app should write to the system clipboard (copy or cut).
    pub clipboard_write: Option<String>,
}

/// Single-line text field model: text plus a caret and selection anchor,
/// both byte offsets on grapheme boundaries, an IME preedit, an undo log,
/// and the horizontal scroll its element keeps the caret visible with.
#[derive(Debug, Default)]
pub struct TextField {
    text: String,
    cursor: usize,
    anchor: usize,
    preedit: Option<Preedit>,
    history: EditLog,
    /// Last `now_ms` passed to [`TextField::apply_at`]; edits are stamped
    /// with it for undo coalescing.
    now_ms: u64,
    scroll: HorizontalScroll,
}

impl Clone for TextField {
    /// The clone gets its own scroll offset rather than sharing this one.
    fn clone(&self) -> Self {
        let scroll = HorizontalScroll::default();
        scroll.set(self.scroll.get());
        Self {
            text: self.text.clone(),
            cursor: self.cursor,
            anchor: self.anchor,
            preedit: self.preedit.clone(),
            history: self.history.clone(),
            now_ms: self.now_ms,
            scroll,
        }
    }
}

impl PartialEq for TextField {
    fn eq(&self, other: &Self) -> bool {
        (&self.text, self.cursor, self.anchor, &self.preedit)
            == (&other.text, other.cursor, other.anchor, &other.preedit)
    }
}

impl Eq for TextField {}

impl TextField {
    /// A field holding `text` with the caret at the end.
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        let len = text.len();
        Self {
            text,
            cursor: len,
            anchor: len,
            ..Self::default()
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// Replace the text and put the caret at the end. Clears undo history.
    pub fn set_text(&mut self, text: impl Into<String>) {
        *self = Self::new(text);
    }

    /// Byte offset of the caret.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Byte offset of the selection anchor. Equals `cursor` when nothing is selected.
    pub fn anchor(&self) -> usize {
        self.anchor
    }

    pub fn selection_range(&self) -> Option<(usize, usize)> {
        if self.cursor == self.anchor {
            None
        } else {
            Some((self.cursor.min(self.anchor), self.cursor.max(self.anchor)))
        }
    }

    pub fn selected_text(&self) -> Option<&str> {
        self.selection_range()
            .and_then(|(start, end)| self.text.get(start..end))
    }

    /// The scroll offset to hand to the painting element.
    pub fn scroll(&self) -> &HorizontalScroll {
        &self.scroll
    }

    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    /// [`TextField::apply`] at time `now_ms` (the app's clock, not wall
    /// time). A pause longer than [`super::COALESCE_PAUSE_MS`] since the
    /// previous edit starts a new undo step.
    pub fn apply_at(&mut self, cmd: TextEditCommand, now_ms: u64) -> TextEditOutcome {
        self.now_ms = now_ms;
        self.apply(cmd)
    }

    pub fn apply(&mut self, cmd: TextEditCommand) -> TextEditOutcome {
        use TextEditCommand::*;
        let before = (self.cursor, self.anchor);
        let mut outcome = TextEditOutcome::default();
        match cmd {
            InsertText(value) => outcome.text_changed = self.insert_text(&value),
            Paste(value) => outcome.text_changed = self.paste(&value),
            Backspace => outcome.text_changed = self.backspace(),
            BackspaceWord => outcome.text_changed = self.backspace_word(),
            BackspaceLine => outcome.text_changed = self.backspace_line(),
            DeleteForward => outcome.text_changed = self.delete_forward(),
            DeleteForwardWord => outcome.text_changed = self.delete_forward_word(),
            CursorLeft => self.cursor_left(false),
            CursorRight => self.cursor_right(false),
            CursorWordLeft => self.cursor_word_left(false),
            CursorWordRight => self.cursor_word_right(false),
            CursorHome | CursorSoftHome => self.cursor_home(false),
            CursorEnd | CursorSoftEnd => self.cursor_end(false),
            SelectLeft => self.cursor_left(true),
            SelectRight => self.cursor_right(true),
            SelectWordLeft => self.cursor_word_left(true),
            SelectWordRight => self.cursor_word_right(true),
            SelectHome | SelectSoftHome => self.cursor_home(true),
            SelectEnd | SelectSoftEnd => self.cursor_end(true),
            SelectAll | SelectLineAt(_) => self.select_all(),
            SelectWordAt(offset) => self.select_word_at(offset),
            Copy => outcome.clipboard_write = self.copy(),
            Cut => {
                outcome.clipboard_write = self.cut();
                outcome.text_changed = outcome.clipboard_write.is_some();
            }
            SetTextCursor(offset) => self.move_cursor(offset, false),
            ExtendTextSelection(offset) => self.move_cursor(offset, true),
            Undo => outcome.text_changed = self.undo(),
            Redo => outcome.text_changed = self.redo(),
            CancelPreedit => self.preedit = None,
            // Vertical movement only applies to `Editor`.
            CursorUp | CursorDown | SelectUp | SelectDown => {}
        }
        outcome.selection_changed = (self.cursor, self.anchor) != before;
        outcome
    }

    /// Replace `range` with `inserted`, record it for undo, and collapse
    /// the caret after the inserted text. Any composition is dropped.
    fn replace(&mut self, range: Range<usize>, inserted: &str, kind: EditKind) -> bool {
        self.preedit = None;
        if range.is_empty() && inserted.is_empty() {
            return false;
        }
        let Some(removed) = self.text.get(range.clone()).map(str::to_owned) else {
            return false;
        };
        let before = (self.anchor, self.cursor);
        self.text.replace_range(range.clone(), inserted);
        self.cursor = range.start + inserted.len();
        self.anchor = self.cursor;
        let edit = Edit {
            at: range.start,
            removed,
            inserted: inserted.to_owned(),
            before,
            after: (self.anchor, self.cursor),
        };
        self.history.record(edit, kind, self.now_ms);
        true
    }

    fn selection_or_caret(&mut self) -> Range<usize> {
        self.clamp_cursor();
        let (start, end) = self.selection_range().unwrap_or((self.cursor, self.cursor));
        start..end
    }

    /// Insert typed text at the caret, replacing any selection. Returns true
    /// if the text changed.
    pub fn insert_text(&mut self, value: &str) -> bool {
        let range = self.selection_or_caret();
        self.replace(range, value, EditKind::Typing)
    }

    /// Delete the selection, or the grapheme before the caret.
    pub fn backspace(&mut self) -> bool {
        if self.delete_selection() {
            return true;
        }
        let prev = prev_grapheme_boundary(&self.text, self.cursor);
        self.replace(prev..self.cursor, "", EditKind::Deleting)
    }

    /// Delete the selection, or the grapheme after the caret.
    pub fn delete_forward(&mut self) -> bool {
        if self.delete_selection() {
            return true;
        }
        let next = next_grapheme_boundary(&self.text, self.cursor);
        self.replace(self.cursor..next, "", EditKind::Deleting)
    }

    /// Delete the selection, or back to the start of the previous word.
    pub fn backspace_word(&mut self) -> bool {
        if self.delete_selection() {
            return true;
        }
        let start = prev_word_boundary(&self.text, self.cursor);
        self.replace(start..self.cursor, "", EditKind::Other)
    }

    /// Delete the selection, or forward to the start of the next word.
    pub fn delete_forward_word(&mut self) -> bool {
        if self.delete_selection() {
            return true;
        }
        let end = next_word_boundary(&self.text, self.cursor);
        self.replace(self.cursor..end, "", EditKind::Other)
    }

    /// Delete the selection, or everything before the caret.
    pub fn backspace_line(&mut self) -> bool {
        if self.delete_selection() {
            return true;
        }
        self.replace(0..self.cursor, "", EditKind::Other)
    }

    /// Delete the selection and collapse the caret. Returns true if something was deleted.
    pub fn delete_selection(&mut self) -> bool {
        let range = self.selection_or_caret();
        !range.is_empty() && self.replace(range, "", EditKind::Other)
    }

    /// Move the caret to `offset`, keeping the anchor when `extend_selection`.
    pub fn move_cursor(&mut self, offset: usize, extend_selection: bool) {
        self.history.break_coalescing();
        self.preedit = None;
        self.cursor = floor_char_boundary(&self.text, offset);
        if !extend_selection {
            self.anchor = self.cursor;
        }
    }

    pub fn cursor_left(&mut self, extend: bool) {
        if !extend && self.selection_range().is_some() {
            self.move_cursor(self.cursor.min(self.anchor), false);
            return;
        }
        if self.cursor == 0 {
            return;
        }
        self.move_cursor(prev_grapheme_boundary(&self.text, self.cursor), extend);
    }

    pub fn cursor_right(&mut self, extend: bool) {
        if !extend && self.selection_range().is_some() {
            self.move_cursor(self.cursor.max(self.anchor), false);
            return;
        }
        if self.cursor >= self.text.len() {
            return;
        }
        self.move_cursor(next_grapheme_boundary(&self.text, self.cursor), extend);
    }

    pub fn cursor_word_left(&mut self, extend: bool) {
        if !extend && self.selection_range().is_some() {
            self.move_cursor(self.cursor.min(self.anchor), false);
            return;
        }
        self.move_cursor(prev_word_boundary(&self.text, self.cursor), extend);
    }

    pub fn cursor_word_right(&mut self, extend: bool) {
        if !extend && self.selection_range().is_some() {
            self.move_cursor(self.cursor.max(self.anchor), false);
            return;
        }
        self.move_cursor(next_word_boundary(&self.text, self.cursor), extend);
    }

    pub fn cursor_home(&mut self, extend: bool) {
        self.move_cursor(0, extend);
    }

    pub fn cursor_end(&mut self, extend: bool) {
        self.move_cursor(self.text.len(), extend);
    }

    pub fn select_all(&mut self) {
        self.move_cursor(0, false);
        self.cursor = self.text.len();
    }

    /// Select the word (or whitespace or punctuation run) around `offset`.
    pub fn select_word_at(&mut self, offset: usize) {
        let range = word_range_at(&self.text, offset);
        self.move_cursor(range.start, false);
        self.cursor = range.end;
    }

    /// The selected text to put on the clipboard, if any.
    pub fn copy(&self) -> Option<String> {
        self.selected_text().map(str::to_owned)
    }

    /// Remove the selection and return it for the clipboard.
    pub fn cut(&mut self) -> Option<String> {
        let copied = self.copy()?;
        self.delete_selection();
        Some(copied)
    }

    /// Insert clipboard contents, replacing any selection. One undo step.
    pub fn paste(&mut self, value: &str) -> bool {
        let range = self.selection_or_caret();
        self.replace(range, value, EditKind::Other)
    }

    /// Show `text` as the IME composition at the caret without touching the
    /// committed text. Empty text cancels the composition. `cursor` is the
    /// IME's byte range inside `text`.
    pub fn set_preedit(&mut self, text: impl Into<String>, cursor: Option<(usize, usize)>) {
        self.preedit = Preedit::new(text, cursor);
    }

    pub fn preedit(&self) -> Option<&Preedit> {
        self.preedit.as_ref()
    }

    /// The text as painted while composing, or `None` when not composing.
    pub fn composition(&self) -> Option<Composition> {
        let preedit = self.preedit.as_ref()?;
        let (start, end) = self.selection_range().unwrap_or((self.cursor, self.cursor));
        Some(compose(&self.text, (start, end), preedit))
    }

    /// Commit composed IME text at the caret, replacing the composition and
    /// any selection. Each commit is its own undo step.
    pub fn commit_ime(&mut self, value: &str) -> bool {
        self.history.break_coalescing();
        let changed = self.insert_text(value);
        self.history.break_coalescing();
        changed
    }

    /// Revert the last undo step and restore the selection it started with.
    pub fn undo(&mut self) -> bool {
        self.preedit = None;
        match self.history.undo(&mut self.text) {
            Some((anchor, cursor)) => {
                self.set_selection(anchor, cursor);
                true
            }
            None => false,
        }
    }

    /// Reapply the last undone step.
    pub fn redo(&mut self) -> bool {
        self.preedit = None;
        match self.history.redo(&mut self.text) {
            Some((anchor, cursor)) => {
                self.set_selection(anchor, cursor);
                true
            }
            None => false,
        }
    }

    fn set_selection(&mut self, anchor: usize, cursor: usize) {
        self.anchor = floor_char_boundary(&self.text, anchor);
        self.cursor = floor_char_boundary(&self.text, cursor);
    }

    /// Pull the caret and anchor back inside the text onto char boundaries,
    /// for when the owner replaced the text out from under them.
    fn clamp_cursor(&mut self) {
        self.set_selection(self.anchor, self.cursor);
    }
}

pub fn prev_grapheme_boundary(text: &str, offset: usize) -> usize {
    let offset = floor_char_boundary(text, offset);
    text[..offset]
        .grapheme_indices(true)
        .next_back()
        .map_or(0, |(idx, _)| idx)
}

pub fn next_grapheme_boundary(text: &str, offset: usize) -> usize {
    let offset = floor_char_boundary(text, offset);
    text[offset..]
        .graphemes(true)
        .next()
        .map_or(text.len(), |g| offset + g.len())
}

/// A piece of text for word navigation: `(start, end, is_word)`.
type Piece = (usize, usize, bool);

/// Split one UAX #29 word-bound segment at ASCII punctuation (other than `_`
/// and `'`), so `foo.bar` stops at the dot as code editors do while `don't`
/// stays one word.
fn push_pieces(base: usize, segment: &str, out: &mut Vec<Piece>) {
    let is_word = |s: &str| s.chars().any(|c| c.is_alphanumeric() || c == '_');
    let mut start = 0;
    for (i, c) in segment.char_indices() {
        if c.is_ascii_punctuation() && c != '_' && c != '\'' {
            if start < i {
                out.push((base + start, base + i, is_word(&segment[start..i])));
            }
            out.push((base + i, base + i + 1, false));
            start = i + 1;
        }
    }
    if start < segment.len() {
        out.push((
            base + start,
            base + segment.len(),
            is_word(&segment[start..]),
        ));
    }
}

/// Start of the word before `offset`, by Unicode word boundaries. Adjacent
/// word segments (a run of CJK ideographs, say) count as one word; emoji
/// and punctuation separate words.
pub fn prev_word_boundary(text: &str, offset: usize) -> usize {
    let offset = floor_char_boundary(text, offset);
    let mut pieces = Vec::new();
    let mut word_start = None;
    for (idx, segment) in text[..offset].split_word_bound_indices().rev() {
        pieces.clear();
        push_pieces(idx, segment, &mut pieces);
        for &(start, _, is_word) in pieces.iter().rev() {
            if is_word {
                word_start = Some(start);
            } else if let Some(start) = word_start {
                return start;
            }
        }
    }
    0
}

/// Start of the word after `offset` (skips the rest of the current word,
/// then any separators).
pub fn next_word_boundary(text: &str, offset: usize) -> usize {
    let offset = floor_char_boundary(text, offset);
    let mut pieces = Vec::new();
    let mut left_word = false;
    for (idx, segment) in text[offset..].split_word_bound_indices() {
        pieces.clear();
        push_pieces(offset + idx, segment, &mut pieces);
        for &(start, _, is_word) in &pieces {
            if !is_word {
                left_word = true;
            } else if left_word {
                return start;
            }
        }
    }
    text.len()
}

/// End of the word after `offset` (skips separators, then the word).
pub fn next_word_end(text: &str, offset: usize) -> usize {
    let offset = floor_char_boundary(text, offset);
    let mut pieces = Vec::new();
    let mut word_end = None;
    for (idx, segment) in text[offset..].split_word_bound_indices() {
        pieces.clear();
        push_pieces(offset + idx, segment, &mut pieces);
        for &(_, end, is_word) in &pieces {
            if is_word {
                word_end = Some(end);
            } else if let Some(end) = word_end {
                return end;
            }
        }
    }
    text.len()
}

/// The word run around `offset` for double-click selection. Between a word
/// and a separator the word wins; a whitespace or punctuation run is
/// selected on its own when no word touches `offset`.
pub fn word_range_at(text: &str, offset: usize) -> Range<usize> {
    let offset = floor_char_boundary(text, offset);
    let line_start = text[..offset].rfind('\n').map_or(0, |i| i + 1);
    let line_end = text[offset..].find('\n').map_or(text.len(), |i| offset + i);
    let mut pieces = Vec::new();
    for (idx, segment) in text[line_start..line_end].split_word_bound_indices() {
        push_pieces(line_start + idx, segment, &mut pieces);
    }
    let containing = pieces.iter().position(|p| p.0 <= offset && offset < p.1);
    let before = pieces.iter().position(|p| p.1 == offset);
    let index = match (containing, before) {
        (Some(i), Some(b)) if !pieces[i].2 && pieces[b].2 => b,
        (Some(i), _) => i,
        (None, Some(b)) => b,
        (None, None) => return offset..offset,
    };
    let is_word = pieces[index].2;
    if !is_word {
        return pieces[index].0..pieces[index].1;
    }
    let mut first = index;
    while first > 0 && pieces[first - 1].2 {
        first -= 1;
    }
    let mut last = index;
    while last + 1 < pieces.len() && pieces[last + 1].2 {
        last += 1;
    }
    pieces[first].0..pieces[last].1
}

#[cfg(test)]
mod tests {
    use super::*;
    use TextEditCommand::*;

    fn field(text: &str, anchor: usize, cursor: usize) -> TextField {
        let mut f = TextField::new(text);
        f.move_cursor(anchor, false);
        f.move_cursor(cursor, true);
        f
    }

    #[test]
    fn backspace_removes_whole_grapheme_cluster() {
        // Family emoji is one grapheme made of several code points joined by ZWJ.
        let family = "👨\u{200d}👩\u{200d}👧";
        let mut f = TextField::new(format!("a{family}"));
        let out = f.apply(Backspace);
        assert!(out.text_changed);
        assert_eq!(f.text(), "a");
        assert_eq!(f.cursor(), 1);
    }

    #[test]
    fn cursor_moves_over_combining_marks() {
        let mut f = TextField::new("e\u{301}x");
        f.apply(CursorHome);
        f.apply(CursorRight);
        assert_eq!(f.cursor(), "e\u{301}".len());
        f.apply(DeleteForward);
        assert_eq!(f.text(), "e\u{301}");
        f.apply(CursorLeft);
        assert_eq!(f.cursor(), 0);
    }

    #[test]
    fn word_movement_skips_punctuation_and_spaces() {
        let mut f = TextField::new("foo.bar  baz");
        f.apply(CursorWordLeft);
        assert_eq!(f.cursor(), 9);
        f.apply(CursorWordLeft);
        assert_eq!(f.cursor(), 4);
        f.apply(CursorWordLeft);
        assert_eq!(f.cursor(), 0);
        f.apply(CursorWordRight);
        assert_eq!(f.cursor(), 4);
        f.apply(CursorWordRight);
        assert_eq!(f.cursor(), 9);
    }

    #[test]
    fn select_word_then_backspace_deletes_selection() {
        let mut f = TextField::new("hello world");
        let out = f.apply(SelectWordLeft);
        assert!(out.selection_changed && !out.text_changed);
        assert_eq!(f.selected_text(), Some("world"));
        f.apply(Backspace);
        assert_eq!(f.text(), "hello ");
        assert_eq!(f.selection_range(), None);
    }

    #[test]
    fn arrow_collapses_selection_to_its_edge() {
        let mut f = field("abcdef", 1, 4);
        f.apply(CursorLeft);
        assert_eq!((f.cursor(), f.anchor()), (1, 1));
        let mut f = field("abcdef", 4, 1);
        f.apply(CursorRight);
        assert_eq!((f.cursor(), f.anchor()), (4, 4));
    }

    #[test]
    fn paste_replaces_selection() {
        let mut f = field("one two three", 4, 7);
        let out = f.apply(Paste("2".into()));
        assert!(out.text_changed);
        assert_eq!(f.text(), "one 2 three");
        assert_eq!(f.cursor(), 5);
    }

    #[test]
    fn copy_and_cut_request_clipboard_writes() {
        let mut f = TextField::new("abc");
        assert_eq!(f.apply(Copy).clipboard_write, None);
        f.apply(SelectAll);
        let out = f.apply(Copy);
        assert_eq!(out.clipboard_write.as_deref(), Some("abc"));
        assert!(!out.text_changed);
        let out = f.apply(Cut);
        assert_eq!(out.clipboard_write.as_deref(), Some("abc"));
        assert!(out.text_changed);
        assert_eq!(f.text(), "");
    }

    #[test]
    fn select_home_and_end_extend_from_anchor() {
        let mut f = TextField::new("abcdef");
        f.apply(SetTextCursor(3));
        f.apply(SelectHome);
        assert_eq!(f.selected_text(), Some("abc"));
        f.apply(SelectEnd);
        assert_eq!(f.selected_text(), Some("def"));
        f.apply(ExtendTextSelection(5));
        assert_eq!(f.selected_text(), Some("de"));
    }

    #[test]
    fn set_cursor_clamps_to_char_boundary() {
        let mut f = TextField::new("é");
        f.apply(SetTextCursor(1));
        assert_eq!(f.cursor(), 0);
        f.apply(SetTextCursor(99));
        assert_eq!(f.cursor(), 2);
    }

    #[test]
    fn noop_commands_report_nothing() {
        let mut f = TextField::new("abc");
        assert_eq!(f.apply(DeleteForward), TextEditOutcome::default());
        assert_eq!(f.apply(CursorUp), TextEditOutcome::default());
        f.apply(CursorHome);
        assert_eq!(f.apply(Backspace), TextEditOutcome::default());
    }

    #[test]
    fn ime_commit_inserts_at_cursor() {
        let mut f = TextField::new("ab");
        f.apply(CursorLeft);
        assert!(f.commit_ime("日本"));
        assert_eq!(f.text(), "a日本b");
        assert_eq!(f.cursor(), 1 + "日本".len());
    }

    /// Steps for driving a field through time.
    enum Step {
        Type(&'static str, u64),
        Do(TextEditCommand, u64),
    }

    fn run(field: &mut TextField, steps: Vec<Step>) {
        for step in steps {
            match step {
                Step::Type(text, at) => {
                    for ch in text.chars() {
                        field.apply_at(InsertText(ch.to_string()), at);
                    }
                }
                Step::Do(cmd, at) => {
                    field.apply_at(cmd, at);
                }
            }
        }
    }

    #[test]
    fn one_undo_reverts_one_coalesced_step() {
        use Step::*;
        let cases = [
            (
                "a word is one step",
                "",
                vec![Type("hello world", 0)],
                "hello ",
            ),
            (
                "a pause splits typing",
                "",
                vec![Type("ab", 0), Type("cd", 5000)],
                "ab",
            ),
            (
                "a caret jump splits typing",
                "",
                vec![
                    Type("ab", 0),
                    Do(CursorLeft, 1),
                    Do(CursorRight, 2),
                    Type("cd", 3),
                ],
                "ab",
            ),
            (
                "repeated backspace is one step",
                "abc",
                vec![Do(Backspace, 0), Do(Backspace, 1), Do(Backspace, 2)],
                "abc",
            ),
            (
                "paste is its own step",
                "",
                vec![Type("ab", 0), Do(Paste("cd".into()), 1)],
                "ab",
            ),
        ];
        for (name, initial, steps, after_undo) in cases {
            let mut f = TextField::new(initial);
            run(&mut f, steps);
            f.apply_at(Undo, 10_000);
            assert_eq!(f.text(), after_undo, "{name}");
        }
    }

    #[test]
    fn undo_restores_the_replaced_selection() {
        let mut f = field("one two three", 4, 7);
        f.apply(InsertText("2".into()));
        assert_eq!(f.text(), "one 2 three");
        f.apply(Undo);
        assert_eq!(f.text(), "one two three");
        assert_eq!((f.anchor(), f.cursor()), (4, 7));
        f.apply(Redo);
        assert_eq!((f.text(), f.cursor()), ("one 2 three", 5));
    }

    #[test]
    fn preedit_leaves_text_alone_until_commit() {
        let mut f = field("ab", 1, 1);
        f.set_preedit("にほ", Some((6, 6)));
        assert_eq!(f.text(), "ab");
        let composition = f.composition().expect("composing");
        assert_eq!(composition.text, "aにほb");
        assert_eq!(composition.caret, Some(1 + "にほ".len()));

        f.set_preedit("", None);
        assert_eq!((f.text(), f.composition()), ("ab", None));

        f.set_preedit("にほん", None);
        f.commit_ime("日本");
        assert_eq!((f.text(), f.composition()), ("a日本b", None));
        f.apply(Undo);
        assert_eq!(f.text(), "ab");
    }

    #[test]
    fn word_navigation_over_cjk_and_emoji() {
        // Ideograph runs are one word; an emoji with a skin tone is one
        // separator grapheme and never split.
        let text = "hello 你好世界 👍🏽 wörld";
        let wörld = text.find('w').unwrap();
        let cjk = text.find('你').unwrap();
        let mut f = TextField::new(text);
        let mut stops = Vec::new();
        for _ in 0..4 {
            f.apply(CursorWordLeft);
            stops.push(f.cursor());
        }
        assert_eq!(stops, [wörld, cjk, 0, 0]);
        stops.clear();
        for _ in 0..3 {
            f.apply(CursorWordRight);
            stops.push(f.cursor());
        }
        assert_eq!(stops, [cjk, wörld, text.len()]);
        f.apply(BackspaceWord);
        assert_eq!(f.text(), "hello 你好世界 👍🏽 ");
    }

    #[test]
    fn double_click_selects_the_word_under_the_pointer() {
        let mut clicks = crate::text_input::ClickCounter::default();
        let mut f = TextField::new("hello wörld, 你好");
        assert_eq!(clicks.press(40.0, 5.0, 1000), 1);
        let count = clicks.press(41.0, 5.0, 1200);
        assert_eq!(count, 2);
        f.apply(SelectWordAt(8));
        assert_eq!(f.selected_text(), Some("wörld"));
        f.apply(SelectWordAt(f.text().len()));
        assert_eq!(f.selected_text(), Some("你好"));
    }

    #[derive(Debug, Clone)]
    enum Op {
        Cmd(TextEditCommand),
        Wait(u64),
    }

    fn op() -> impl proptest::strategy::Strategy<Value = Op> {
        use proptest::prelude::*;
        let insert = prop::sample::select(vec!["a", "b", " ", "你", "👍🏽", "e\u{301}", "\n"])
            .prop_map(|s| Op::Cmd(InsertText(s.to_owned())));
        let cmd = prop::sample::select(vec![
            Backspace,
            DeleteForward,
            BackspaceWord,
            DeleteForwardWord,
            CursorLeft,
            CursorRight,
            SelectLeft,
            SelectWordLeft,
            CursorHome,
            CursorEnd,
            SelectAll,
            Paste("xy z".into()),
            Cut,
            Undo,
            Redo,
        ])
        .prop_map(Op::Cmd);
        prop_oneof![3 => insert, 3 => cmd, 1 => (0u64..2500).prop_map(Op::Wait)]
    }

    proptest::proptest! {
        #[test]
        fn undo_all_restores_text_and_selection(
            initial in "[a-c 你]{0,8}",
            start in 0usize..12,
            ops in proptest::collection::vec(op(), 0..40),
        ) {
            let mut f = TextField::new(initial.clone());
            f.apply(SetTextCursor(start));
            let mut now = 0;
            // Selection just before the edit at the bottom of the undo stack.
            let mut expected = (f.anchor(), f.cursor());
            for op in ops {
                match op {
                    Op::Wait(ms) => now += ms,
                    Op::Cmd(cmd) => {
                        let is_history = matches!(cmd, Undo | Redo);
                        let bottom = !f.can_undo();
                        let before = (f.anchor(), f.cursor());
                        let out = f.apply_at(cmd, now);
                        if out.text_changed && bottom && !is_history {
                            expected = before;
                        }
                    }
                }
            }
            let last = f.text().to_owned();
            let mut steps = 0;
            while f.can_undo() {
                f.apply(Undo);
                steps += 1;
            }
            proptest::prop_assert_eq!(f.text(), initial.as_str());
            if steps > 0 {
                proptest::prop_assert_eq!((f.anchor(), f.cursor()), expected);
            }
            for _ in 0..steps {
                f.apply(Redo);
            }
            proptest::prop_assert_eq!(f.text(), last.as_str());
        }
    }
}
