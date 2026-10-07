//! Markdown parsed into a column-oriented block model with pulldown-cmark,
//! plus an incremental parser for streaming sources. The transcript
//! renders it (see `crate::transcript::MarkdownMessage`).

mod incremental;
mod model;

#[cfg(test)]
mod tests;

pub use incremental::IncrementalMarkdown;
pub use model::{BlockKind, IntegrityError, ListMarker, MarkdownDoc, NO_LINK, SpanFlags};
