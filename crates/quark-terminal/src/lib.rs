//! A terminal element for Quark.
//!
//! Terminal emulation is libghostty-vt, Ghostty's VT library, built from a
//! pinned Ghostty commit with Zig and linked statically (see build.rs): the
//! parser, screen and scrollback state, reflow on resize, selection, and
//! the key, mouse, focus, and paste encoders. [`Pty`] runs the program on a
//! Unix PTY or Windows ConPTY with a reader thread. [`terminal_view`]
//! paints the viewport with `quark-text`: one cache boundary per row keyed
//! by its content, runs of same-styled cells, 256-color and truecolor SGR,
//! bold, italic, faint, inverse, five underline styles, strikethrough,
//! wide characters and emoji through the bundled fallback fonts, and the
//! cursor in each DECSCUSR shape, blinking when the program asks.
//!
//! Scrollback scrolls with a [`quark_ui::element::ScrollHandle`] over the
//! full history (wheel, fling, scrollbar), while only the visible rows are
//! ever built. Drag selects; double and triple click select words and
//! lines; the copy shortcut copies. Ctrl+click (Cmd+click on macOS) opens
//! OSC 8 hyperlinks. OSC 52 clipboard writes are off until
//! [`TerminalState::allow_clipboard_write`]. Titles, bells, and exits come
//! out as [`TerminalSignal`]s. Screen readers get one `Terminal` node with
//! the visible text and the cursor as its caret, and an IME composition as
//! a `Mark` inside it.
//!
//! # Wiring
//!
//! The app owns a [`TerminalState`]. Its `view` calls
//! [`TerminalState::set_viewport`] and [`TerminalState::prepare`], then
//! builds [`terminal_view`]. PTY output arrives on another thread: spawn
//! with a callback that wakes the app (its `Waker`), and call
//! [`TerminalState::read_pty`] from `UiApp::wake`. Keys and raw
//! pointer events come from the app's input hook (`UiApp::event`): key
//! presses to [`TerminalState::key_press`], text (IME commits included) to
//! [`TerminalState::text_input`], IME compositions to
//! [`TerminalState::set_preedit`], and pointer input to
//! [`TerminalState::pointer`] for mouse reporting.
//!
//! The view registers the terminal as an IME target, so quark-app turns IME
//! on while it has focus and keeps the candidate window at the cursor. A
//! composition is painted at the cursor and sends nothing until the IME
//! commits it; keys pressed meanwhile are the IME's. Losing focus (the
//! window's, through [`TerminalState::focus_changed`], or to another
//! element, seen by the next [`terminal_view`]) drops it unsent.
//!
//! # Platforms
//!
//! Linux, macOS, and Windows on x86-64 with the MSVC toolchain. Building
//! for another Windows target fails unless a prebuilt libghostty-vt is
//! supplied (see build.rs).

mod grid;
pub mod input;
mod pty;
mod state;
#[allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    unsafe_op_in_unsafe_fn,
    unnecessary_transmutes,
    clippy::all
)]
mod sys;
mod view;
pub mod vt;

#[cfg(test)]
mod tests;
// build.rs's manifest parsing, here so its unit tests run with the crate's.
// Counts allocations for the budget tests.
#[cfg(test)]
#[global_allocator]
static ALLOCATOR: quark_ui::test_alloc::Counting = quark_ui::test_alloc::Counting;

#[cfg(test)]
#[path = "../build/ghostty_deps.rs"]
#[allow(dead_code)]
mod ghostty_deps;

pub use grid::{CellStyle, Colors, Cursor, CursorShape, Grid, GridRow, Rgb, Run, Underline};
pub use input::KeyPress;
pub use pty::{INPUT_QUEUE, Pty, PtyCommand, PtyEvent, PtyGeometry};
pub use state::{
    PointerInput, Preedit, TerminalEvent, TerminalOutcome, TerminalSignal, TerminalState,
    TerminalStyle,
};
pub use view::{TerminalEnv, terminal_view};
pub use vt::UnsafePaste;
