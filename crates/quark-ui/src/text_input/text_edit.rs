use std::ops::Range;

use quark_text::TextOffset;

use super::buffer::{TextBuffer, WordForward};
use super::ime::{Composition, Preedit};
use super::view::HorizontalScroll;

/// A text editing command, independent of which widget has focus.
///
/// Platform input (keys, IME commits, clipboard reads) is translated into
/// these by the app and routed to the focused [`TextField`] or
/// [`super::Editor`]. IME commits arrive as `InsertText`. Byte offsets are
/// raw (a pointer hit on an earlier frame, or the app's own); the target
/// snaps each onto a grapheme boundary of its current text.
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

/// Single-line text field model: a [`TextBuffer`] (text, caret, anchor,
/// IME preedit, undo log) plus the horizontal scroll its element keeps the
/// caret visible with. Home and End go to the ends of the whole text, and
/// word movement stops at word starts.
#[derive(Debug)]
pub struct TextField {
    buffer: TextBuffer,
    scroll: HorizontalScroll,
}

impl Default for TextField {
    fn default() -> Self {
        Self::new("")
    }
}

impl Clone for TextField {
    /// The clone gets its own scroll offset rather than sharing this one.
    fn clone(&self) -> Self {
        let scroll = HorizontalScroll::default();
        scroll.set(self.scroll.get());
        Self {
            buffer: self.buffer.clone(),
            scroll,
        }
    }
}

impl PartialEq for TextField {
    fn eq(&self, other: &Self) -> bool {
        self.buffer == other.buffer
    }
}

impl Eq for TextField {}

impl TextField {
    /// A field holding `text` with the caret at the end.
    pub fn new(text: impl Into<String>) -> Self {
        let mut buffer = TextBuffer::new(false, WordForward::NextStart);
        buffer.set_text(&text.into());
        Self {
            buffer,
            scroll: HorizontalScroll::default(),
        }
    }

    pub fn text(&self) -> &str {
        self.buffer.text()
    }

    /// Replace the text and put the caret at the end. Clears undo history.
    pub fn set_text(&mut self, text: impl Into<String>) {
        self.buffer.set_text(&text.into());
    }

    pub fn cursor(&self) -> TextOffset {
        self.buffer.cursor()
    }

    /// The selection anchor. Equals `cursor` when nothing is selected.
    pub fn anchor(&self) -> TextOffset {
        self.buffer.anchor()
    }

    /// The selection in order, or `None` when nothing is selected.
    pub fn selection_range(&self) -> Option<Range<TextOffset>> {
        Some(self.buffer.selection()).filter(|range| !range.is_empty())
    }

    pub fn selected_text(&self) -> Option<&str> {
        self.buffer.selected_text()
    }

    /// The scroll offset to hand to the painting element.
    pub fn scroll(&self) -> &HorizontalScroll {
        &self.scroll
    }

    pub fn can_undo(&self) -> bool {
        self.buffer.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.buffer.can_redo()
    }

    /// [`TextField::apply`] at time `now_ms` (the app's clock, not wall
    /// time). A pause longer than [`super::COALESCE_PAUSE_MS`] since the
    /// previous edit starts a new undo step.
    pub fn apply_at(&mut self, cmd: TextEditCommand, now_ms: u64) -> TextEditOutcome {
        self.buffer.set_now(now_ms);
        self.apply(cmd)
    }

    /// Apply `cmd`. Vertical movement does nothing in a single-line field.
    pub fn apply(&mut self, cmd: TextEditCommand) -> TextEditOutcome {
        self.buffer.apply(cmd)
    }

    /// Show `text` as the IME composition at the caret without touching the
    /// committed text. Empty text cancels the composition. `cursor` is the
    /// IME's byte range inside `text`.
    pub fn set_preedit(&mut self, text: impl Into<String>, cursor: Option<(usize, usize)>) {
        self.buffer.set_preedit(text, cursor);
    }

    pub fn preedit(&self) -> Option<&Preedit> {
        self.buffer.preedit()
    }

    /// The text as painted while composing, or `None` when not composing.
    pub fn composition(&self) -> Option<Composition> {
        self.buffer.composition()
    }

    /// Commit composed IME text at the caret, replacing the composition and
    /// any selection. Each commit is its own undo step.
    pub fn commit_ime(&mut self, value: &str) -> bool {
        self.buffer.commit_ime(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use TextEditCommand::*;

    fn field(text: &str, anchor: usize, cursor: usize) -> TextField {
        let mut f = TextField::new(text);
        f.apply(SetTextCursor(anchor));
        f.apply(ExtendTextSelection(cursor));
        f
    }

    /// `(anchor, cursor)` as bytes.
    fn selection(f: &TextField) -> (usize, usize) {
        (f.anchor().get(), f.cursor().get())
    }

    #[test]
    fn backspace_removes_whole_grapheme_cluster() {
        // Family emoji is one grapheme made of several code points joined by ZWJ.
        let family = "👨\u{200d}👩\u{200d}👧";
        let mut f = TextField::new(format!("a{family}"));
        let out = f.apply(Backspace);
        assert!(out.text_changed);
        assert_eq!(f.text(), "a");
        assert_eq!(f.cursor().get(), 1);
    }

    #[test]
    fn cursor_moves_over_combining_marks() {
        let mut f = TextField::new("e\u{301}x");
        f.apply(CursorHome);
        f.apply(CursorRight);
        assert_eq!(f.cursor().get(), "e\u{301}".len());
        f.apply(DeleteForward);
        assert_eq!(f.text(), "e\u{301}");
        f.apply(CursorLeft);
        assert_eq!(f.cursor().get(), 0);
    }

    #[test]
    fn word_movement_skips_punctuation_and_spaces() {
        let mut f = TextField::new("foo.bar  baz");
        f.apply(CursorWordLeft);
        assert_eq!(f.cursor().get(), 9);
        f.apply(CursorWordLeft);
        assert_eq!(f.cursor().get(), 4);
        f.apply(CursorWordLeft);
        assert_eq!(f.cursor().get(), 0);
        f.apply(CursorWordRight);
        assert_eq!(f.cursor().get(), 4);
        f.apply(CursorWordRight);
        assert_eq!(f.cursor().get(), 9);
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
        assert_eq!(selection(&f), (1, 1));
        let mut f = field("abcdef", 4, 1);
        f.apply(CursorRight);
        assert_eq!(selection(&f), (4, 4));
    }

    #[test]
    fn paste_replaces_selection() {
        let mut f = field("one two three", 4, 7);
        let out = f.apply(Paste("2".into()));
        assert!(out.text_changed);
        assert_eq!(f.text(), "one 2 three");
        assert_eq!(f.cursor().get(), 5);
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
        assert_eq!(f.cursor().get(), 0);
        f.apply(SetTextCursor(99));
        assert_eq!(f.cursor().get(), 2);
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
        assert_eq!(f.cursor().get(), 1 + "日本".len());
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
        assert_eq!(selection(&f), (4, 7));
        f.apply(Redo);
        assert_eq!((f.text(), f.cursor().get()), ("one 2 three", 5));
    }

    #[test]
    fn preedit_leaves_text_alone_until_commit() {
        let mut f = field("ab", 1, 1);
        f.set_preedit("にほ", Some((6, 6)));
        assert_eq!(f.text(), "ab");
        let composition = f.composition().expect("composing");
        assert_eq!(composition.text, "aにほb");
        assert_eq!(
            composition.caret.map(TextOffset::get),
            Some(1 + "にほ".len())
        );

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
            stops.push(f.cursor().get());
        }
        assert_eq!(stops, [wörld, cjk, 0, 0]);
        stops.clear();
        for _ in 0..3 {
            f.apply(CursorWordRight);
            stops.push(f.cursor().get());
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
            let mut expected = selection(&f);
            for op in ops {
                match op {
                    Op::Wait(ms) => now += ms,
                    Op::Cmd(cmd) => {
                        let is_history = matches!(cmd, Undo | Redo);
                        let bottom = !f.can_undo();
                        let before = selection(&f);
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
                proptest::prop_assert_eq!(selection(&f), expected);
            }
            for _ in 0..steps {
                f.apply(Redo);
            }
            proptest::prop_assert_eq!(f.text(), last.as_str());
        }
    }
}
