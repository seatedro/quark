//! Text editing model and input element.
//!
//! [`TextField`] is the single-line editing model and [`Editor`] the
//! glyphon-backed multiline editor. Both consume [`TextEditCommand`]s and
//! report a [`TextEditOutcome`]; the app decides which one has focus and acts
//! on the outcome (persisting, writing the clipboard).
//!
//! `input_element.rs` is carried over from diffy with history and joins this
//! module once it is decoupled.

mod editor;
mod text_edit;

pub use editor::{
    CursorState, Editor, EditorMode, SelectionRect, SyntaxHighlighter, SyntaxSpan, SyntaxTokenKind,
};
pub use text_edit::{
    TextEditCommand, TextEditOutcome, TextField, next_grapheme_boundary, next_word_boundary,
    prev_grapheme_boundary, prev_word_boundary,
};
