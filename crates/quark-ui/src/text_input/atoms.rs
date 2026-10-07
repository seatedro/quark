//! Atomic inline spans: ranges of a text buffer that edit as one unit.
//!
//! An atom's label is ordinary text in the buffer (so layout, caret math,
//! and assistive tech read it as written), and the buffer keeps a sorted
//! list of the atoms' ranges beside it. The caret never rests inside an
//! atom, and an edit that touches part of one takes all of it. The app
//! keys its own payload (a file, a skill, a terminal excerpt) by the
//! atom's [`AtomId`].

use std::ops::Range;
use std::sync::{Arc, Mutex};

use quark_text::offset;

/// The app's key for an atom's payload: `kind` says which of its tables
/// (files, skills, ...) and `key` the entry there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AtomId {
    pub kind: u32,
    pub key: u64,
}

impl AtomId {
    pub const fn new(kind: u32, key: u64) -> Self {
        Self { kind, key }
    }
}

/// One atom: `range` (bytes, on grapheme boundaries) of its text holds the
/// label. `export` replaces the label when the text leaves the app (a copy
/// to the system clipboard), such as a Markdown link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineAtom {
    pub range: Range<usize>,
    pub id: AtomId,
    pub export: Arc<str>,
}

impl InlineAtom {
    fn shifted(&self, by: isize) -> Self {
        let shift = |o: usize| o.saturating_add_signed(by);
        Self {
            range: shift(self.range.start)..shift(self.range.end),
            ..self.clone()
        }
    }
}

/// Text with atoms in it: what copy, history, and drafts carry. Atom ranges
/// are relative to `text`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RichText {
    pub text: String,
    pub atoms: Vec<InlineAtom>,
}

impl RichText {
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            atoms: Vec::new(),
        }
    }

    /// `label` as a single atom followed by `after` (often a space, so
    /// typing continues outside it).
    pub fn atom(label: &str, id: AtomId, export: impl Into<Arc<str>>, after: &str) -> Self {
        Self {
            text: format!("{label}{after}"),
            atoms: vec![InlineAtom {
                range: 0..label.len(),
                id,
                export: export.into(),
            }],
        }
    }

    /// The text with every atom's label replaced by its export, as written
    /// to the system clipboard.
    pub fn export(&self) -> String {
        let mut out = String::with_capacity(self.text.len());
        let mut at = 0;
        for atom in &self.atoms {
            out.push_str(offset::slice(&self.text, at..atom.range.start));
            out.push_str(&atom.export);
            at = atom.range.end;
        }
        out.push_str(offset::suffix(&self.text, at));
        out
    }

    /// Checks that the atoms are sorted, disjoint, non-empty, and on
    /// grapheme boundaries of `text`.
    pub fn verify_integrity(&self) -> Result<(), AtomIntegrityError> {
        verify(&self.text, &self.atoms)
    }
}

/// A broken [`RichText`] or buffer atom list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AtomIntegrityError {
    /// Atom `index` is empty or reaches past the text.
    OutOfBounds { index: usize },
    /// Atom `index` starts or ends inside a grapheme.
    OffBoundary { index: usize },
    /// Atom `index` starts before atom `index - 1` ends.
    Overlap { index: usize },
}

impl std::fmt::Display for AtomIntegrityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OutOfBounds { index } => write!(f, "atom {index} is empty or out of bounds"),
            Self::OffBoundary { index } => write!(f, "atom {index} is off a grapheme boundary"),
            Self::Overlap { index } => write!(f, "atom {index} overlaps the one before it"),
        }
    }
}

impl std::error::Error for AtomIntegrityError {}

fn verify(text: &str, atoms: &[InlineAtom]) -> Result<(), AtomIntegrityError> {
    let mut prev_end = 0;
    for (index, atom) in atoms.iter().enumerate() {
        let Range { start, end } = atom.range;
        if start >= end || end > text.len() {
            return Err(AtomIntegrityError::OutOfBounds { index });
        }
        let on_boundary = |o: usize| offset::TextOffset::snap(text, o) == o;
        if !on_boundary(start) || !on_boundary(end) {
            return Err(AtomIntegrityError::OffBoundary { index });
        }
        if index > 0 && start < prev_end {
            return Err(AtomIntegrityError::Overlap { index });
        }
        prev_end = end;
    }
    Ok(())
}

/// The buffer's atoms, sorted by position and disjoint.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct AtomList(Vec<InlineAtom>);

impl AtomList {
    pub(super) fn as_slice(&self) -> &[InlineAtom] {
        &self.0
    }

    pub(super) fn clear(&mut self) {
        self.0.clear();
    }

    pub(super) fn set(&mut self, atoms: &[InlineAtom]) {
        self.0.clear();
        self.0.extend_from_slice(atoms);
    }

    pub(super) fn verify_integrity(&self, text: &str) -> Result<(), AtomIntegrityError> {
        verify(text, &self.0)
    }

    /// The atom `at` falls strictly inside of.
    pub(super) fn containing(&self, at: usize) -> Option<&InlineAtom> {
        self.0
            .iter()
            .find(|atom| atom.range.start < at && at < atom.range.end)
    }

    /// `range` grown so that it takes every atom it touches whole. An
    /// empty range inside an atom moves to the atom's end instead, so an
    /// insertion lands after it.
    pub(super) fn expand(&self, range: Range<usize>) -> Range<usize> {
        if range.is_empty() {
            let at = self
                .containing(range.start)
                .map_or(range.start, |a| a.range.end);
            return at..at;
        }
        let mut out = range;
        for atom in &self.0 {
            if atom.range.start < out.end && out.start < atom.range.end {
                out.start = out.start.min(atom.range.start);
                out.end = out.end.max(atom.range.end);
            }
        }
        out
    }

    /// Atoms wholly inside `range`, relative to its start.
    pub(super) fn slice(&self, range: Range<usize>) -> Vec<InlineAtom> {
        self.0
            .iter()
            .filter(|a| range.start <= a.range.start && a.range.end <= range.end)
            .map(|a| a.shifted(-(range.start as isize)))
            .collect()
    }

    /// Mirror a text replacement: `removed_len` bytes at `at` became
    /// `inserted_len` bytes holding `inserted` (relative atoms). Atoms in
    /// the removed bytes are returned relative to `at`; the caller has
    /// already grown the range so none straddles its edges.
    pub(super) fn splice(
        &mut self,
        at: usize,
        removed_len: usize,
        inserted: &[InlineAtom],
        inserted_len: usize,
    ) -> Vec<InlineAtom> {
        let end = at + removed_len;
        let first = self.0.partition_point(|a| a.range.end <= at);
        let last = self.0.partition_point(|a| a.range.start < end);
        let removed = self
            .0
            .splice(
                first..last.max(first),
                inserted.iter().map(|a| a.shifted(at as isize)),
            )
            .map(|a| a.shifted(-(at as isize)))
            .collect();
        let delta = inserted_len as isize - removed_len as isize;
        let after = first + inserted.len();
        for atom in self.0.iter_mut().skip(after) {
            *atom = atom.shifted(delta);
        }
        removed
    }
}

/// The app's own clipboard for text with atoms. A copy writes the exported
/// text to the system clipboard and keeps the [`RichText`] here; pasting
/// that same text back gets the atoms again, while other apps see only the
/// export. Clones share the slot, so give every editor that should paste
/// another's atoms the same one ([`super::Editor::set_rich_clipboard`]).
#[derive(Debug, Clone, Default)]
pub struct RichClipboard(Arc<Mutex<Option<RichText>>>);

impl RichClipboard {
    /// Remember `rich` as the source of the exported text just copied.
    pub(super) fn store(&self, rich: Option<RichText>) {
        if let Ok(mut slot) = self.0.lock() {
            *slot = rich;
        }
    }

    /// The atoms behind `pasted`, if it is the text last copied here.
    pub fn resolve(&self, pasted: &str) -> Option<RichText> {
        let slot = self.0.lock().ok()?;
        slot.as_ref()
            .filter(|rich| rich.export() == pasted)
            .cloned()
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::super::{Editor, InputHooks, Insertion, TextEditCommand};
    use super::*;
    use TextEditCommand::*;

    const FILE: AtomId = AtomId::new(1, 7);

    /// An editor from `marked`: `[..]` is an atom exporting `<label>`
    /// wrapped in a link, `|` the caret.
    fn editor(marked: &str) -> Editor {
        let mut rich = RichText::default();
        let (mut open, mut caret) = (0, 0);
        for ch in marked.chars() {
            match ch {
                '|' => caret = rich.text.len(),
                '[' => open = rich.text.len(),
                ']' => {
                    let label = offset::suffix(&rich.text, open).to_owned();
                    rich.atoms.push(InlineAtom {
                        range: open..rich.text.len(),
                        id: FILE,
                        export: format!("[{label}](file)").into(),
                    });
                }
                _ => rich.text.push(ch),
            }
        }
        let mut editor = Editor::default();
        editor.set_rich_text(&rich);
        editor.apply(SetTextCursor(caret));
        editor
    }

    /// The editor's text with atoms in brackets, the caret as `|`, and the
    /// anchor as `^` when something is selected.
    fn dump(editor: &Editor) -> String {
        let text = editor.text();
        let mut out = String::new();
        for (i, ch) in text.char_indices().chain([(text.len(), '\0')]) {
            if editor.atoms().iter().any(|a| a.range.end == i) {
                out.push(']');
            }
            if editor.anchor() == i && editor.anchor() != editor.cursor() {
                out.push('^');
            }
            if editor.cursor() == i {
                out.push('|');
            }
            if editor.atoms().iter().any(|a| a.range.start == i) {
                out.push('[');
            }
            if ch != '\0' {
                out.push(ch);
            }
        }
        out
    }

    #[test]
    fn an_atom_edits_and_moves_as_one_unit() {
        let cases = [
            ("left over it", "a[@x.rs]|b", CursorLeft, "a|[@x.rs]b"),
            ("right over it", "a|[@x.rs]b", CursorRight, "a[@x.rs]|b"),
            ("select over it", "a|[@x.rs]b", SelectRight, "a^[@x.rs]|b"),
            ("backspace deletes it", "a[@x.rs]|b", Backspace, "a|b"),
            ("delete deletes it", "a|[@x.rs]b", DeleteForward, "a|b"),
            ("word delete takes it", "a [@x.rs]|", BackspaceWord, "a |"),
            (
                "click lands on the near edge",
                "|a[@x.rs]b",
                SetTextCursor(3),
                "a|[@x.rs]b",
            ),
            (
                "click lands on the far edge",
                "|a[@x.rs]b",
                SetTextCursor(6),
                "a[@x.rs]|b",
            ),
            (
                "double click selects it",
                "|a [@x.rs]",
                SelectWordAt(5),
                "a ^[@x.rs]|",
            ),
            (
                "a selection touching it takes it",
                "a[@x.rs]|b",
                ExtendTextSelection(4),
                "a[@x.rs]^|b",
            ),
        ];
        for (name, before, cmd, after) in cases {
            let mut e = editor(before);
            if let ExtendTextSelection(_) = cmd {
                e.apply(cmd);
                e.apply(Backspace);
                assert_eq!(dump(&e), "a|b", "{name}");
                continue;
            }
            e.apply(cmd);
            assert_eq!(dump(&e), after, "{name}");
        }
    }

    #[test]
    fn undo_of_an_atom_deletion_restores_the_atom() {
        let mut e = editor("see [@x.rs] now|");
        e.apply(SetTextCursor(9));
        e.apply(Backspace);
        e.apply(InsertText("Y".into()));
        assert_eq!(dump(&e), "see Y| now");
        e.apply(Undo);
        e.apply(Undo);
        assert_eq!(dump(&e), "see [@x.rs]| now");
        e.apply(Redo);
        assert_eq!(dump(&e), "see | now");
    }

    #[test]
    fn copied_atoms_export_as_text_and_paste_back_as_atoms() {
        let clipboard = RichClipboard::default();
        let mut source = editor("see [@x.rs]|");
        source.set_rich_clipboard(clipboard.clone());
        source.apply(SelectAll);
        let exported = source.apply(Copy).clipboard_write;
        assert_eq!(exported.as_deref(), Some("see [@x.rs](file)"));

        let mut target = editor("> |");
        target.set_rich_clipboard(clipboard);
        target.apply(Paste("see [@x.rs](file)".into()));
        assert_eq!(dump(&target), "> see [@x.rs]|");
        // Text from elsewhere that happens to look similar stays text.
        target.apply(Paste(" [@x.rs](other)".into()));
        assert_eq!(dump(&target), "> see [@x.rs] [@x.rs](other)|");
    }

    /// Takes pastes over `limit` bytes as attachments.
    struct Attach {
        limit: usize,
        taken: Vec<String>,
    }

    impl InputHooks for Attach {
        fn paste(&mut self, pasted: Insertion) -> Insertion {
            match pasted {
                Insertion::Text(text) if text.len() > self.limit => {
                    self.taken.push(text);
                    Insertion::Nothing
                }
                other => other,
            }
        }
    }

    #[test]
    fn a_paste_hook_can_take_the_paste_instead_of_the_text() {
        let mut hooks = Attach {
            limit: 4,
            taken: Vec::new(),
        };
        let mut e = editor("|");
        let big = e.apply_with(Paste("too long".into()), &mut hooks);
        e.apply_with(Paste("ok".into()), &mut hooks);
        assert_eq!(
            (dump(&e), hooks.taken),
            ("ok|".into(), vec!["too long".into()])
        );
        assert!(!big.text_changed);
    }

    fn command() -> impl Strategy<Value = TextEditCommand> {
        let raw = 0usize..24;
        prop_oneof![
            prop::sample::select(vec!["a", " ", "\u{e9}"]).prop_map(|s| InsertText(s.into())),
            raw.clone().prop_map(SetTextCursor),
            raw.clone().prop_map(ExtendTextSelection),
            raw.prop_map(SelectWordAt),
            prop::sample::select(vec![
                Backspace,
                DeleteForward,
                BackspaceWord,
                DeleteForwardWord,
                CursorLeft,
                CursorRight,
                SelectLeft,
                SelectWordRight,
                CursorHome,
                Cut,
                Undo,
                Redo,
            ]),
        ]
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(128))]

        // Catches an edit or motion leaving the caret or anchor inside an
        // atom, or undo losing an atom's range (the buffer's integrity
        // check fails inside `apply` if the list goes bad).
        #[test]
        fn carets_never_rest_inside_atoms_and_undo_all_restores_them(
            commands in prop::collection::vec(command(), 1..32),
        ) {
            let start = "a [@x.rs] b[#12]";
            let mut e = editor(start);
            let initial = e.rich_text();
            for cmd in commands {
                e.apply(cmd.clone());
                for at in [e.cursor().get(), e.anchor().get()] {
                    prop_assert!(
                        !e.atoms().iter().any(|a| a.range.start < at && at < a.range.end),
                        "after {:?}: {}", cmd, dump(&e)
                    );
                }
            }
            while e.can_undo() {
                e.apply(Undo);
            }
            prop_assert_eq!(e.rich_text(), initial);
        }
    }
}
