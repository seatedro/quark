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
//!
//! Positions are [`TextOffset`]s on grapheme boundaries; raw byte indices
//! (in [`TextEditCommand`]s) are snapped onto the target's text on entry.

// Byte slicing of strings lives in `quark_text::offset`.
#![deny(clippy::string_slice)]
#![cfg_attr(not(test), deny(clippy::indexing_slicing))]

mod buffer;
mod editor;
mod ime;
mod input_element;
mod keys;
mod pointer;
mod text_edit;
mod undo;
mod view;

pub use editor::{
    CursorState, Editor, EditorMode, SelectionRect, SyntaxHighlighter, SyntaxSpan, SyntaxTokenKind,
};
pub use ime::{Composition, Preedit, compose};
pub use input_element::{CursorSnapshot, TextEditorElement, text_editor_element};
pub use keys::command_for_binding;
pub(crate) use pointer::text_pointer_drag;
pub use pointer::{AUTOSCROLL_STEP_MS, TextPointer, TextPointerEvent};
pub use quark_text::TextOffset;
pub use text_edit::{TextEditCommand, TextEditOutcome, TextField};
pub use undo::COALESCE_PAUSE_MS;
pub use view::{
    CARET_BLINK_MS, ClickCounter, HorizontalScroll, MULTI_CLICK_MS, caret_blink, reveal_offset,
};
