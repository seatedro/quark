//! The diff model behind Quark's diff view, with no UI dependencies.
//!
//! - [`parse_unified`] reads unified diffs (git's format with renames,
//!   copies, modes, and binary markers, or plain `diff -u`), and
//!   [`write_unified`] writes git's format back. [`apply`] applies a file
//!   diff to its old text.
//! - [`diff_texts`] diffs two whole texts by line (Myers' algorithm, see
//!   [`myers`]), keeping both texts so collapsed context can be expanded.
//! - [`inline_diff`] finds the changed words of a pair of changed lines.
//! - [`DiffDocument`] stores files, hunks, and blocks as flat column
//!   tables; [`Projection`] turns it into display rows, unified or side by
//!   side, with gaps of unchanged lines collapsed per an [`Expansion`].

mod compute;
mod inline;
mod model;
pub mod myers;
mod patch;
mod projection;
mod text;

#[cfg(test)]
mod tests;

pub use compute::{DEFAULT_CONTEXT, diff_texts};
pub use inline::{InlineDiff, MAX_INLINE_LINE_BYTES, inline_diff};
pub use model::{
    BlockKind, BlockTable, DiffDocument, FileMeta, FileStatus, FileSummary, FileTable, HunkTable,
    IntegrityError, Side,
};
pub use patch::{ApplyError, PatchError, apply, parse_unified, write_unified};
pub use projection::{
    Expansion, GapId, GapRow, MIN_HIDDEN, Mode, NONE, Projection, ProjectionError, Reveal, RowKind,
};
pub use text::TextStore;
