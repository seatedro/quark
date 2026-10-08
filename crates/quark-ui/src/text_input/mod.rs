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
//! For chat inputs the editor also keeps atomic inline spans
//! ([`InlineAtom`]: mention chips that edit as one unit), lets the app
//! intercept pastes and drops ([`InputHooks`]), finds trigger characters at
//! the caret ([`TriggerRule`]) to drive a [`Completion`] popup, and recalls
//! earlier prompts ([`PromptHistory`]).
//!
//! Positions are [`TextOffset`]s on grapheme boundaries; raw byte indices
//! (in [`TextEditCommand`]s) are snapped onto the target's text on entry.

// Byte slicing of strings lives in `quark_text::offset`.
#![deny(clippy::string_slice)]
#![cfg_attr(not(test), deny(clippy::indexing_slicing))]

mod anchor;
mod atoms;
mod buffer;
mod completion;
mod editor;
mod history;
mod hooks;
mod ime;
mod input_element;
mod keys;
mod pointer;
mod spell;
mod styles;
mod text_edit;
mod trigger;
mod undo;
mod view;

pub use anchor::{
    CaretAnchor, CaretGeometry, CaretPlacement, CaretPopup, CaretPopupLayout, caret_popup,
    place_at_caret,
};
pub use atoms::{AtomId, AtomIntegrityError, InlineAtom, RichClipboard, RichText};
pub use completion::{
    Answer, Completion, CompletionItem, CompletionKey, CompletionProvider, CompletionQuery,
    completion_list,
};
pub use history::PromptHistory;
pub use hooks::{InputHooks, Insertion, NoHooks};
pub use trigger::{TriggerBoundary, TriggerMatch, TriggerRule, find_trigger};

pub use editor::{
    CursorState, Editor, EditorMode, SelectionRect, SpellingIssue, SyntaxHighlighter, SyntaxSpan,
    SyntaxTokenKind, TextDecoration,
};
pub use ime::{Composition, Preedit, compose};
pub use input_element::{CursorSnapshot, TextEditorElement, text_editor_element};
pub use keys::{AsBinding, command_for_binding};
pub(crate) use pointer::text_pointer_drag;
pub use pointer::{AUTOSCROLL_STEP_MS, TextPointer, TextPointerEvent};
pub use quark_text::TextOffset;
pub use spell::{SpellChecker, SpellDictionary, SpellError, SpellResult};
pub use styles::{InlineStyle, RichExport, StyleIntegrityError, StyleSpan, TextFormat};
pub use text_edit::{TextEditCommand, TextEditOutcome, TextField};
pub use undo::COALESCE_PAUSE_MS;
pub use view::{
    CARET_BLINK_MS, ClickCounter, HorizontalScroll, MULTI_CLICK_MS, caret_blink, reveal_offset,
};

/// `"{prefix}:{target:?}"`, the id a text field's semantic and
/// accessibility nodes share, interned so a steady frame formats it into a
/// reused buffer and allocates nothing.
pub(crate) fn target_key(prefix: &str, target: crate::FocusId) -> std::sync::Arc<str> {
    use std::fmt::Write;
    thread_local! {
        static BUF: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
    }
    BUF.with(|buf| {
        let mut buf = buf.borrow_mut();
        buf.clear();
        let _ = write!(buf, "{prefix}:{target:?}");
        quark::intern(&buf)
    })
}
