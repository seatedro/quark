//! The editor's side of spell checking: what was sent, what came back.
//!
//! Up to [`super::highlight::WHOLE_TEXT_MAX_BYTES`] the checker gets the
//! whole text; past that, the lines around the view, so a long document is
//! never copied whole for it.

use std::ops::Range;
use std::sync::Arc;

use super::Editor;
use super::highlight::WHOLE_TEXT_MAX_BYTES;
use crate::text_input::spell::{SpellChecker, SpellResult};

/// A misspelled word, for a context menu of corrections.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpellingIssue {
    /// Bytes of the word in the editor's text.
    pub range: Range<usize>,
    pub word: String,
    /// Likely corrections, best first.
    pub suggestions: Vec<String>,
}

/// One request to the checker: the text revision and where the text sent
/// starts and ends in the editor's.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Request {
    rev: u64,
    range: Range<usize>,
}

#[derive(Debug, Default)]
pub(super) struct Spelling {
    checker: Option<SpellChecker>,
    /// Bumped on every text change.
    rev: u64,
    /// Bumped on every request; the checker echoes it back.
    seq: u64,
    /// The last request sent, under `seq`.
    sent: Option<Request>,
    /// Misspelled words (bytes of the editor's text) of the request
    /// `checked`; kept, slightly stale, until a newer result arrives.
    misspelled: Vec<Range<usize>>,
    checked: Option<Request>,
}

impl Clone for Spelling {
    /// A clone checks with its own clone of the checker and starts over.
    fn clone(&self) -> Self {
        Self {
            checker: self.checker.clone(),
            ..Self::default()
        }
    }
}

impl Spelling {
    pub(super) fn text_changed(&mut self) {
        self.rev += 1;
    }
}

impl Editor {
    /// Check this field's spelling with `checker` (a clone of one other
    /// fields use shares their worker and dictionary), or stop with `None`.
    /// Off by default.
    pub fn set_spellcheck(&mut self, checker: Option<SpellChecker>) {
        self.spelling = Spelling {
            checker,
            rev: self.spelling.rev,
            ..Spelling::default()
        };
    }

    /// The bytes the checker should see: all of a short text, else the
    /// lines around the last flush's view.
    fn spelling_range(&self) -> Range<usize> {
        let rope = self.buffer.rope();
        if rope.len() <= WHOLE_TEXT_MAX_BYTES {
            return 0..rope.len();
        }
        let lines = self.window.line_range();
        rope.line_to_byte(lines.start)..rope.line_to_byte(lines.end)
    }

    /// Send the text to the checker if it changed (or the view moved past
    /// what was checked), and take any result.
    pub(super) fn refresh_spelling(&mut self) {
        if self.spelling.checker.is_none() {
            return;
        }
        let wanted = Request {
            rev: self.spelling.rev,
            range: self.spelling_range(),
        };
        let covered = self.spelling.sent.as_ref().is_some_and(|sent| {
            sent.rev == wanted.rev
                && sent.range.start <= wanted.range.start
                && wanted.range.end <= sent.range.end
        });
        if !covered {
            let rope = self.buffer.rope();
            let text: Arc<str> = match rope.as_single_chunk() {
                Some(text) => Arc::from(text),
                None if wanted.range.len() == rope.len() => self.buffer.snapshot().flat(),
                None => Arc::from(rope.slice(wanted.range.clone()).as_ref()),
            };
            self.spelling.seq += 1;
            // The checker itself, never a clone: a clone is another client
            // with its own results.
            if let Some(checker) = &self.spelling.checker {
                checker.request(self.spelling.seq, text);
            }
            self.spelling.sent = Some(wanted);
        }
        let result = self.spelling.checker.as_ref().and_then(SpellChecker::poll);
        if let Some(result) = result {
            self.accept_spelling(result);
        }
    }

    fn accept_spelling(&mut self, result: SpellResult) {
        let Some(sent) = &self.spelling.sent else {
            return;
        };
        if result.rev == self.spelling.seq && sent.rev == self.spelling.rev {
            let base = sent.range.start;
            self.spelling.misspelled = result
                .misspelled
                .into_iter()
                .map(|r| r.start + base..r.end + base)
                .collect();
            self.spelling.checked = Some(sent.clone());
        }
    }

    /// Block until the checker has checked the current text (tests, or an
    /// app that wants marks before the first paint). Returns at once
    /// without a checker.
    pub fn wait_for_spelling(&mut self) {
        self.refresh_spelling();
        while self.spelling.checked.as_ref().map(|c| c.rev) != Some(self.spelling.rev) {
            let Some(result) = self.spelling.checker.as_ref().and_then(SpellChecker::wait) else {
                return;
            };
            self.accept_spelling(result);
        }
    }

    /// The misspelled words as of the last check, as byte ranges. For a
    /// long text, only those around the view are known.
    pub fn misspellings(&self) -> &[Range<usize>] {
        &self.spelling.misspelled
    }

    /// The misspelled word at byte `at` (a right click), with corrections.
    pub fn spelling_at(&self, at: usize) -> Option<SpellingIssue> {
        let checker = self.spelling.checker.as_ref()?;
        let range = self
            .spelling
            .misspelled
            .iter()
            .find(|r| r.start <= at && at <= r.end)?
            .clone();
        let rope = self.buffer.rope();
        if range.end > rope.len()
            || !rope.is_char_boundary(range.start)
            || !rope.is_char_boundary(range.end)
        {
            return None;
        }
        let word = rope.slice_to_string(range.clone());
        Some(SpellingIssue {
            suggestions: checker.dictionary().suggest(&word),
            range,
            word,
        })
    }

    /// Accept `word` in every field sharing this checker's dictionary
    /// (calling its add hook), and check this field again.
    pub fn learn_word(&mut self, word: &str) {
        if let Some(checker) = &self.spelling.checker {
            checker.dictionary().learn(word);
            self.spelling.rev += 1;
        }
    }
}
