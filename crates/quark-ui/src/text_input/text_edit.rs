use unicode_segmentation::UnicodeSegmentation;

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
/// both byte offsets on grapheme boundaries.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TextField {
    text: String,
    cursor: usize,
    anchor: usize,
}

impl TextField {
    /// A field holding `text` with the caret at the end.
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        let len = text.len();
        Self {
            text,
            cursor: len,
            anchor: len,
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// Replace the text and put the caret at the end.
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
            .map(|(start, end)| &self.text[start..end])
    }

    pub fn apply(&mut self, cmd: TextEditCommand) -> TextEditOutcome {
        use TextEditCommand::*;
        let before = (self.cursor, self.anchor);
        let mut outcome = TextEditOutcome::default();
        match cmd {
            InsertText(value) | Paste(value) => outcome.text_changed = self.insert_text(&value),
            Backspace => outcome.text_changed = self.backspace(),
            DeleteForward => outcome.text_changed = self.delete_forward(),
            CursorLeft => self.cursor_left(false),
            CursorRight => self.cursor_right(false),
            CursorWordLeft => self.cursor_word_left(false),
            CursorWordRight => self.cursor_word_right(false),
            CursorHome => self.cursor_home(false),
            CursorEnd => self.cursor_end(false),
            SelectLeft => self.cursor_left(true),
            SelectRight => self.cursor_right(true),
            SelectWordLeft => self.cursor_word_left(true),
            SelectWordRight => self.cursor_word_right(true),
            SelectHome => self.cursor_home(true),
            SelectEnd => self.cursor_end(true),
            SelectAll => self.select_all(),
            Copy => outcome.clipboard_write = self.copy(),
            Cut => {
                outcome.clipboard_write = self.cut();
                outcome.text_changed = outcome.clipboard_write.is_some();
            }
            SetTextCursor(offset) => self.move_cursor(offset, false),
            ExtendTextSelection(offset) => self.move_cursor(offset, true),
            // Multiline and word/line deletion commands only apply to `Editor`.
            BackspaceWord | BackspaceLine | DeleteForwardWord | CursorUp | CursorDown
            | CursorSoftHome | CursorSoftEnd | SelectUp | SelectDown | SelectSoftHome
            | SelectSoftEnd => {}
        }
        outcome.selection_changed = (self.cursor, self.anchor) != before;
        outcome
    }

    /// Insert at the caret, replacing any selection. Returns true if the text changed.
    pub fn insert_text(&mut self, value: &str) -> bool {
        let deleted = self.delete_selection();
        self.clamp_cursor();
        self.text.insert_str(self.cursor, value);
        self.cursor += value.len();
        self.anchor = self.cursor;
        deleted || !value.is_empty()
    }

    /// Delete the selection, or the grapheme before the caret.
    pub fn backspace(&mut self) -> bool {
        if self.delete_selection() {
            return true;
        }
        if self.cursor == 0 {
            return false;
        }
        let prev = prev_grapheme_boundary(&self.text, self.cursor);
        self.text.drain(prev..self.cursor);
        self.cursor = prev;
        self.anchor = prev;
        true
    }

    /// Delete the selection, or the grapheme after the caret.
    pub fn delete_forward(&mut self) -> bool {
        if self.delete_selection() {
            return true;
        }
        if self.cursor >= self.text.len() {
            return false;
        }
        let next = next_grapheme_boundary(&self.text, self.cursor);
        self.text.drain(self.cursor..next);
        true
    }

    /// Delete the selection and collapse the caret. Returns true if something was deleted.
    pub fn delete_selection(&mut self) -> bool {
        self.clamp_cursor();
        let Some((start, end)) = self.selection_range() else {
            return false;
        };
        self.text.drain(start..end);
        self.cursor = start;
        self.anchor = start;
        true
    }

    /// Move the caret to `offset`, keeping the anchor when `extend_selection`.
    pub fn move_cursor(&mut self, offset: usize, extend_selection: bool) {
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
        self.anchor = 0;
        self.cursor = self.text.len();
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

    /// Insert clipboard contents, replacing any selection.
    pub fn paste(&mut self, value: &str) -> bool {
        self.insert_text(value)
    }

    /// Commit composed IME text at the caret.
    pub fn commit_ime(&mut self, value: &str) -> bool {
        self.insert_text(value)
    }

    /// Pull the caret and anchor back inside the text onto char boundaries,
    /// for when the owner replaced the text out from under them.
    fn clamp_cursor(&mut self) {
        self.cursor = floor_char_boundary(&self.text, self.cursor);
        self.anchor = floor_char_boundary(&self.text, self.anchor);
    }
}

fn floor_char_boundary(text: &str, offset: usize) -> usize {
    let mut offset = offset.min(text.len());
    while offset > 0 && !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

pub fn prev_grapheme_boundary(text: &str, offset: usize) -> usize {
    if offset == 0 {
        return 0;
    }
    let mut prev = 0;
    for (idx, _) in text.grapheme_indices(true) {
        if idx >= offset {
            break;
        }
        prev = idx;
    }
    prev
}

pub fn next_grapheme_boundary(text: &str, offset: usize) -> usize {
    for (idx, grapheme) in text.grapheme_indices(true) {
        if idx >= offset {
            return idx + grapheme.len();
        }
    }
    text.len()
}

/// Start of the word before `offset`. Words are ASCII alphanumeric runs.
pub fn prev_word_boundary(text: &str, offset: usize) -> usize {
    if offset == 0 {
        return 0;
    }
    let bytes = text.as_bytes();
    let mut pos = offset.min(bytes.len());
    // Skip whitespace/punctuation backwards
    while pos > 0 && !bytes[pos - 1].is_ascii_alphanumeric() {
        pos -= 1;
    }
    // Skip word chars backwards
    while pos > 0 && bytes[pos - 1].is_ascii_alphanumeric() {
        pos -= 1;
    }
    pos
}

/// Start of the word after `offset`. Words are ASCII alphanumeric runs.
pub fn next_word_boundary(text: &str, offset: usize) -> usize {
    let len = text.len();
    if offset >= len {
        return len;
    }
    let bytes = text.as_bytes();
    let mut pos = offset;
    // Skip word chars forward
    while pos < len && bytes[pos].is_ascii_alphanumeric() {
        pos += 1;
    }
    // Skip whitespace/punctuation forward
    while pos < len && !bytes[pos].is_ascii_alphanumeric() {
        pos += 1;
    }
    pos
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
}
