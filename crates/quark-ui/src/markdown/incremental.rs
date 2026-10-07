//! Parsing a streaming markdown source without reparsing its finished part.
//!
//! A closed top-level fenced code block followed by a blank line ends every
//! open construct, so the source before that point parses the same alone as
//! inside the whole document. [`IncrementalMarkdown`] keeps the parse of the
//! source up to the last such point and, while the source keeps that
//! prefix, parses only what follows. Code-heavy answers stream mostly
//! inside and after fences, so most of their source is parsed once.
//! (Ported from T3 Code's `markdown-incremental.ts`.)

use super::model::{MarkdownDoc, parse_source};

/// One per streaming source.
#[derive(Debug, Default, Clone)]
pub struct IncrementalMarkdown {
    /// Source up to the last split point, and its parse.
    prefix_source: String,
    prefix: MarkdownDoc,
}

impl IncrementalMarkdown {
    pub fn new() -> Self {
        Self::default()
    }

    /// Bytes of source whose parse the next [`Self::parse`] reuses when the
    /// source still starts with them.
    pub fn reused_len(&self) -> usize {
        self.prefix_source.len()
    }

    /// The parse of `source`, equal to [`MarkdownDoc::parse`] of it.
    pub fn parse(&mut self, source: &str) -> MarkdownDoc {
        // A streaming `\r` can become half of a `\r\n`, and a BOM is only
        // stripped at the start of a document; both make a split unsafe.
        if source.contains(['\r', '\u{feff}']) {
            self.reset();
            return MarkdownDoc::parse(source);
        }
        if !source.starts_with(&self.prefix_source) {
            self.reset();
        }
        let offset = self.prefix_source.len();
        let tail = parse_source(&source[offset..]);
        // Reference definitions are document-wide: no split is valid. A
        // prefix is only kept from sources without any, so the tail tells.
        if tail.has_definitions {
            if offset == 0 {
                return tail.doc;
            }
            self.reset();
            return MarkdownDoc::parse(source);
        }
        let doc = if offset == 0 {
            tail.doc
        } else {
            let mut doc = self.prefix.clone();
            doc.append(&tail.doc);
            doc
        };
        if let Some(boundary) = tail.boundary.map(|b| offset + b)
            && boundary > offset
        {
            self.prefix_source = source[..boundary].to_owned();
            self.prefix = MarkdownDoc::parse(&self.prefix_source);
        }
        doc
    }

    fn reset(&mut self) {
        self.prefix_source.clear();
        self.prefix = MarkdownDoc::default();
    }
}
