//! Text editing model and input element.
//!
//! [`TextField`] is the single-line editing model and [`Editor`] the
//! multiline editor. Both consume [`TextEditCommand`]s and report a
//! [`TextEditOutcome`]; the app decides which one has focus and acts on the
//! outcome (persisting, writing the clipboard). Both keep an undo log and an
//! IME [`Preedit`].
//!
//! [`TextEditorElement`] paints an [`Editor`] snapshot (text, selection,
//! cursor, gutter) as an element.

mod editor;
mod ime;
mod input_element;
mod keys;
mod text_edit;
mod undo;
mod view;

pub use editor::{
    CursorState, Editor, EditorMode, SelectionRect, SyntaxHighlighter, SyntaxSpan, SyntaxTokenKind,
};
pub use ime::{Composition, Preedit, compose};
pub use input_element::{CursorSnapshot, TextEditorElement, text_editor_element};
pub use keys::command_for_binding;
pub use text_edit::{
    TextEditCommand, TextEditOutcome, TextField, next_grapheme_boundary, next_word_boundary,
    next_word_end, prev_grapheme_boundary, prev_word_boundary, word_range_at,
};
pub use undo::COALESCE_PAUSE_MS;
pub use view::{ClickCounter, HorizontalScroll, MULTI_CLICK_MS, reveal_offset};
