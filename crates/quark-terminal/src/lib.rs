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
//! the visible text and the cursor as its caret.
//!
//! # Wiring
//!
//! The app owns a [`TerminalState`]. Its `view` calls
//! [`TerminalState::set_viewport`] and [`TerminalState::prepare`], then
//! builds [`terminal_view`]. PTY output arrives on another thread: spawn
//! with a callback that sends each [`PtyEvent`] through the app's
//! `UiSender`, and pass it to [`TerminalState::handle_pty`]. Keys and raw
//! pointer events come from the app's input hook (`UiApp::event`): key
//! presses to [`TerminalState::key_press`], text to
//! [`TerminalState::text_input`], and pointer input to
//! [`TerminalState::pointer`] for mouse reporting.
//!
//! # Platforms
//!
//! Linux and macOS. On Windows the crate builds without the terminal (only
//! [`Pty`]): libghostty-vt's MSVC build is not wired up yet.

#[cfg(ghostty_vt)]
mod grid;
#[cfg(ghostty_vt)]
pub mod input;
mod pty;
#[cfg(ghostty_vt)]
mod state;
#[cfg(ghostty_vt)]
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
#[cfg(ghostty_vt)]
mod view;
#[cfg(ghostty_vt)]
pub mod vt;

#[cfg(all(test, ghostty_vt))]
mod tests;

#[cfg(ghostty_vt)]
pub use grid::{CellStyle, Colors, Cursor, CursorShape, Grid, GridRow, Rgb, Run, Underline};
#[cfg(ghostty_vt)]
pub use input::KeyPress;
pub use pty::{Pty, PtyCommand, PtyEvent, PtyGeometry};
#[cfg(ghostty_vt)]
pub use state::{
    PointerInput, TerminalEvent, TerminalOutcome, TerminalSignal, TerminalState, TerminalStyle,
};
#[cfg(ghostty_vt)]
pub use view::{TerminalEnv, terminal_view};
#[cfg(ghostty_vt)]
pub use vt::UnsafePaste;

/// Whether this build has the terminal (libghostty-vt). False on Windows.
pub const AVAILABLE: bool = cfg!(ghostty_vt);
