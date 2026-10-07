//! Terminal-style prompt recall for an [`Editor`]: Arrow Up at the start of
//! the text walks back through earlier entries, Arrow Down at the end walks
//! forward, and stepping past the newest entry brings back the draft that
//! was there before recall started. The app owns the entries (one list per
//! conversation, say) and passes them in, oldest first, on every step.

use super::Editor;
use super::atoms::RichText;

/// Where recall stands. Browsing ends by itself once the text no longer
/// matches the recalled entry, so an edit or a send starts over.
#[derive(Debug, Clone, Default)]
pub struct PromptHistory {
    /// Index of the recalled entry.
    position: Option<usize>,
    /// What the editor held when recall started.
    draft: Option<RichText>,
}

impl PromptHistory {
    /// Whether `editor` still shows the entry recall put there.
    fn browsing(&self, editor: &Editor, entries: &[RichText]) -> Option<usize> {
        let at = self.position?;
        let entry = entries.get(at)?;
        (entry.text == editor.text() && entry.atoms == editor.atoms()).then_some(at)
    }

    fn caret_at(editor: &Editor, end: bool) -> bool {
        let at = if end { editor.byte_len() } else { 0 };
        editor.cursor() == at && editor.anchor() == at
    }

    /// Arrow Up: recall the entry before the current one. Starts from the
    /// caret at the start of the text, stashing what is there as the
    /// draft, and continues from a recalled entry with the caret at its
    /// end. Returns whether it took the key.
    pub fn back(&mut self, editor: &mut Editor, entries: &[RichText]) -> bool {
        let target = match self.browsing(editor, entries) {
            Some(at) if Self::caret_at(editor, true) || Self::caret_at(editor, false) => {
                // At the oldest entry the key is still taken, so the caret
                // does not jump.
                at.saturating_sub(1)
            }
            Some(_) => return false,
            None if Self::caret_at(editor, false) && !entries.is_empty() => {
                self.draft = Some(editor.rich_text());
                entries.len() - 1
            }
            None => return false,
        };
        self.recall(editor, entries, target);
        true
    }

    /// Arrow Down: recall the next entry, or past the newest restore the
    /// draft and stop browsing. Only while browsing with the caret at the
    /// end of the text. Returns whether it took the key.
    pub fn forward(&mut self, editor: &mut Editor, entries: &[RichText]) -> bool {
        let Some(at) = self.browsing(editor, entries) else {
            return false;
        };
        if !Self::caret_at(editor, true) {
            return false;
        }
        if at + 1 < entries.len() {
            self.recall(editor, entries, at + 1);
        } else {
            let draft = self.draft.take().unwrap_or_default();
            editor.set_rich_text(&draft);
            self.position = None;
        }
        true
    }

    fn recall(&mut self, editor: &mut Editor, entries: &[RichText], at: usize) {
        if let Some(entry) = entries.get(at) {
            editor.set_rich_text(entry);
            self.position = Some(at);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::TextEditCommand::*;
    use super::*;

    fn entries() -> Vec<RichText> {
        ["first", "second"].map(RichText::plain).to_vec()
    }

    #[test]
    fn up_walks_back_and_down_past_the_newest_restores_the_draft() {
        let entries = entries();
        let mut editor = Editor::default();
        editor.set_text("draft");
        editor.apply(CursorHome);
        let mut history = PromptHistory::default();
        let mut seen = Vec::new();
        for up in [true, true, true, false, false] {
            let taken = if up {
                history.back(&mut editor, &entries)
            } else {
                history.forward(&mut editor, &entries)
            };
            seen.push(format!("{taken}:{}", editor.text()));
        }
        assert_eq!(
            seen,
            [
                "true:second",
                "true:first",
                "true:first",
                "true:second",
                "true:draft"
            ]
        );
    }

    #[test]
    fn up_away_from_the_start_and_down_after_an_edit_are_left_to_the_caret() {
        let entries = entries();
        let mut editor = Editor::default();
        editor.set_text("draft");
        let mut history = PromptHistory::default();
        assert!(!history.back(&mut editor, &entries), "caret at the end");

        editor.apply(CursorHome);
        history.back(&mut editor, &entries);
        editor.apply(InsertText("!".into()));
        assert!(!history.forward(&mut editor, &entries), "edited recall");
    }
}
