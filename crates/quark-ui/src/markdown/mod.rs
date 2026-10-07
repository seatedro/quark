//! Markdown: a column-oriented block model parsed with pulldown-cmark and a
//! view that renders it as selectable text.

mod model;
mod view;

#[cfg(test)]
mod tests;

pub use model::{BlockKind, IntegrityError, ListMarker, MarkdownDoc, NO_LINK, SpanFlags};
pub use view::{MarkdownView, block_key, cell_key, markdown_doc_view, markdown_view};
pub(crate) use view::{heading_style, styled_spans};
