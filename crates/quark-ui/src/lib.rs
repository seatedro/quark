//! Element tree, layout, styling, theming, and text input for Quark.
//!
//! A view builds a fresh tree of elements every frame with builder calls
//! ([`element::div`], [`element::text`], [`element::text_input`]); Taffy
//! lays it out, and paint emits a `quark::Scene` that `quark-render` draws.
//! Retained state lives in the app (text field models, scroll handles, list
//! state) and in per-window tables keyed by stable identity (animations,
//! the [`element::ElementCache`]). `quark-app`'s `UiApp` runs this loop in a
//! window.
//!
//! - [`element`]: elements, layout, input routing, scrolling, and cache
//!   boundaries ([`element::cached`]).
//! - [`style`], [`design`], [`theme`], [`palette`]: the [`style::Styled`]
//!   builder methods, spacing and radius tokens, and themes.
//! - [`text_input`] holds [`text_input::TextField`] and [`text_input::Editor`]
//!   editing models, IME preedit, undo, and composer pieces (atoms,
//!   triggers, completion, prompt history).
//! - [`virtual_list`], [`document`], [`markdown`]: variable-height
//!   virtual lists, a selectable block document, find, and streaming
//!   markdown.
//! - [`accessibility`]: the AccessKit tree elements publish each frame.
//! - [`animation`]: style transitions on the window's animation table.
//!
//! Elements are plain values styled through [`style::Styled`]:
//!
//! ```
//! use quark_ui::Action;
//! use quark_ui::design::{Rad, Sp};
//! use quark_ui::element::{AnyElement, IntoAnyElement, div, text};
//! use quark_ui::style::Styled;
//! use quark_ui::theme::Theme;
//!
//! #[derive(Debug, PartialEq)]
//! struct Save;
//!
//! fn toolbar(theme: &Theme) -> AnyElement {
//!     let colors = &theme.colors;
//!     div()
//!         .flex_row()
//!         .gap(Sp::SM)
//!         .p(Sp::MD)
//!         .bg(colors.surface)
//!         .child(
//!             div()
//!                 .test_id("toolbar.save")
//!                 .on_click(Action::new(Save))
//!                 .px(Sp::LG)
//!                 .rounded(Rad::XL)
//!                 .bg(colors.accent)
//!                 .hover_bg(colors.accent_strong)
//!                 .child(text("Save").color(colors.text_strong).semibold()),
//!         )
//!         .into_any()
//! }
//!
//! let _toolbar = toolbar(&Theme::default_dark());
//! ```
//!
//! Text fields are models the app owns, driven by [`text_input::TextEditCommand`]s:
//!
//! ```
//! use quark_ui::text_input::{TextEditCommand, TextField};
//!
//! let mut field = TextField::new("");
//! field.apply(TextEditCommand::InsertText("hello".into()));
//! field.apply(TextEditCommand::SelectAll);
//! let cut = field.apply(TextEditCommand::Cut);
//! assert_eq!(cut.clipboard_write.as_deref(), Some("hello"));
//! assert_eq!(field.text(), "");
//!
//! field.apply(TextEditCommand::Undo);
//! assert_eq!(field.text(), "hello");
//! ```
//!
//! A [`element::cached`] boundary replays its subtree's layout and paint
//! until its key or inputs hash changes, so an unchanged row costs no
//! building:
//!
//! ```
//! use quark_ui::element::{AnyElement, IntoAnyElement, cached, div, inputs_hash, text};
//! use quark_ui::style::Styled;
//!
//! struct Row {
//!     id: u64,
//!     revision: u64,
//!     label: String,
//! }
//!
//! fn rows(rows: &[Row], selected: Option<u64>) -> AnyElement {
//!     div()
//!         .flex_col()
//!         .children(rows.iter().map(|row| {
//!             let is_selected = selected == Some(row.id);
//!             let label = row.label.clone();
//!             // The hash covers everything the closure reads.
//!             cached(row.id, inputs_hash(&(row.revision, is_selected)), move || {
//!                 let line = text(label);
//!                 if is_selected { line.semibold() } else { line }
//!             })
//!             .into_any()
//!         }))
//!         .into_any()
//! }
//!
//! let _list = rows(&[Row { id: 1, revision: 0, label: "first".into() }], Some(1));
//! ```
//!
//! A [`virtual_list::VariableList`] keeps row heights and the scroll
//! position, so a view builds only the rows in its window, and
//! [`document::FindState`] searches block text by key, whether or not the
//! block is on screen:
//!
//! ```
//! use quark::BlockKey;
//! use quark_ui::document::FindState;
//! use quark_ui::virtual_list::{RowKey, VariableList};
//!
//! // 1,000 rows estimated at 20 points in a 100 point viewport. A new list
//! // is pinned to the bottom, like a chat transcript.
//! let mut list = VariableList::new(20.0, 100.0);
//! let keys: Vec<RowKey> = (0..1000).map(RowKey).collect();
//! list.extend(&keys).unwrap();
//! let window = list.window(0.0);
//! assert_eq!(window.range, 995..1000);
//!
//! let mut find = FindState::new("rust");
//! find.update([
//!     (BlockKey(1), 0, "Rust is a language."),
//!     (BlockKey(2), 0, "Quark is written in rust."),
//! ]);
//! assert_eq!(find.matches().len(), 2);
//! assert_eq!(find.status(), "1 of 2");
//! ```

/// A profiler scope plus a `tracing` span for the rest of the block, both
/// compiled only with the `profile` feature.
#[allow(unused_macros)]
macro_rules! profile_scope {
    ($name:literal) => {
        #[cfg(feature = "profile")]
        profiling::scope!($name);
        #[cfg(feature = "profile")]
        let _profile_span = tracing::trace_span!($name).entered();
    };
}

pub mod accessibility;
pub mod action;
pub mod animation;
pub mod design;
pub mod document;
pub mod element;
pub mod hud;
pub mod icons;
#[cfg(feature = "devtools")]
pub mod inspector;
pub mod key_context;
pub mod markdown;
pub mod palette;
pub mod style;
pub mod text_input;
pub mod theme;
pub mod virtual_list;

#[cfg(any(test, feature = "test-alloc"))]
#[doc(hidden)]
pub mod test_alloc;

#[cfg(test)]
#[global_allocator]
static ALLOCATOR: test_alloc::Counting = test_alloc::Counting;

pub use action::{Action, ActionPayload, FocusId};
/// Localized messages and formats; see [`quark_i18n`].
pub use quark_i18n as i18n;
/// Grammar stores for code block highlighting.
#[cfg(feature = "syntax")]
pub use quark_syntax;
