//! Markdown messages as transcript blocks.
//!
//! [`MarkdownMessage`] turns each [`MarkdownDoc`] block into one
//! [`TranscriptBlock`] keyed by (message, block index), so every heading,
//! paragraph, list item, quote, table, code block, and rule is its own
//! selectable block and selection runs across messages. Converted blocks
//! are kept between calls and reused while their markdown and highlight
//! are unchanged, so a streaming message rebuilds only its growing tail and
//! the leading blocks keep the layouts already shaped for them.

use std::sync::Arc;

use quark::selection::BlockKey;
use quark_render::{FontKind, FontWeight};

use super::syntax::SyntaxHighlighter;
use super::{BlockStyle, TranscriptBlock};
use crate::element::StyledSpan;
use crate::markdown::{BlockKind, ListMarker, MarkdownDoc, heading_style, styled_spans};
use crate::theme::Theme;
use crate::virtual_list::RowKey;

/// Code block font size relative to body text.
const CODE_SCALE: f32 = 0.92;

/// Key of block `index` of message `row`: `row << 16 | index`. Rows must
/// stay below 2^48 and messages below 65,536 blocks.
pub fn markdown_block_key(row: RowKey, index: usize) -> BlockKey {
    BlockKey((row.0 << 16) | (index as u64 & 0xffff))
}

/// The message a [`markdown_block_key`] belongs to.
pub fn markdown_block_row(key: BlockKey) -> RowKey {
    RowKey(key.0 >> 16)
}

struct Converted {
    hash: u64,
    highlight: u64,
    block: TranscriptBlock,
}

/// One message's markdown converted to transcript blocks, with the
/// conversions kept for reuse. Build a new one (or call
/// [`MarkdownMessage::clear`]) when the theme changes, since block colors
/// come from it.
pub struct MarkdownMessage {
    row: RowKey,
    converted: Vec<Converted>,
}

impl MarkdownMessage {
    pub fn new(row: RowKey) -> Self {
        Self {
            row,
            converted: Vec::new(),
        }
    }

    /// Forgets every conversion.
    pub fn clear(&mut self) {
        self.converted.clear();
    }

    /// The blocks of `doc`. A block whose markdown hash and code highlight
    /// match the previous call is returned as before (its spans shared,
    /// not rebuilt).
    pub fn blocks(
        &mut self,
        doc: &MarkdownDoc,
        theme: &Theme,
        syntax: &mut SyntaxHighlighter,
    ) -> Vec<TranscriptBlock> {
        self.converted.truncate(doc.len());
        for index in 0..doc.len() {
            let key = markdown_block_key(self.row, index);
            let hash = doc.hash(index);
            let highlight = match doc.kind(index) {
                BlockKind::CodeBlock => syntax.version(key, doc.lang(index), doc.text(index)),
                _ => 0,
            };
            let fresh = self
                .converted
                .get(index)
                .is_some_and(|c| c.hash == hash && c.highlight == highlight);
            if fresh {
                continue;
            }
            let converted = Converted {
                hash,
                highlight,
                block: convert(doc, index, key, theme, syntax),
            };
            match self.converted.get_mut(index) {
                Some(slot) => *slot = converted,
                None => self.converted.push(converted),
            }
        }
        self.converted.iter().map(|c| c.block.clone()).collect()
    }
}

fn convert(
    doc: &MarkdownDoc,
    index: usize,
    key: BlockKey,
    theme: &Theme,
    syntax: &SyntaxHighlighter,
) -> TranscriptBlock {
    let style = block_style(doc, index);
    let block = match doc.kind(index) {
        BlockKind::Paragraph | BlockKind::Heading(_) => TranscriptBlock::prose(
            key,
            styled_spans(doc, doc.spans(index), style.weight, theme),
        ),
        BlockKind::CodeBlock => {
            TranscriptBlock::code(key, syntax.lines(key, doc.text(index), theme))
                .with_label(Some(Arc::from(doc.lang(index))))
        }
        BlockKind::Table => TranscriptBlock::code(key, table_lines(doc, index, theme)),
        BlockKind::Rule => TranscriptBlock::rule(key),
    };
    block.with_style(style)
}

/// Indent, marker, size, and copy prefixes of block `index`.
fn block_style(doc: &MarkdownDoc, index: usize) -> BlockStyle {
    let quote = doc.quote_depth(index);
    let depth = doc.indent(index);
    let marker = doc.marker(index);
    let (scale, weight) = match doc.kind(index) {
        BlockKind::Heading(level) => heading_style(level),
        BlockKind::CodeBlock | BlockKind::Table => (CODE_SCALE, FontWeight::Normal),
        _ => (1.0, FontWeight::Normal),
    };

    let quote_prefix = "> ".repeat(quote as usize);
    let mut prefix = quote_prefix.clone();
    if depth > 0 {
        let nesting = if marker == ListMarker::None {
            depth
        } else {
            depth - 1
        };
        prefix.push_str(&"  ".repeat(nesting as usize));
    }
    prefix.push_str(&match marker {
        ListMarker::None => String::new(),
        ListMarker::Bullet => "- ".to_owned(),
        ListMarker::Ordered(n) => format!("{n}. "),
        ListMarker::Task(true) => "- [x] ".to_owned(),
        ListMarker::Task(false) => "- [ ] ".to_owned(),
    });
    if let BlockKind::Heading(level) = doc.kind(index) {
        prefix.push_str(&"#".repeat(level as usize));
        prefix.push(' ');
    }

    BlockStyle {
        scale,
        weight,
        list_depth: depth,
        quote_depth: quote,
        marker: match marker {
            ListMarker::None => None,
            ListMarker::Bullet => Some(Arc::from("\u{2022}")),
            ListMarker::Ordered(n) => Some(Arc::from(format!("{n}."))),
            ListMarker::Task(true) => Some(Arc::from("[x]")),
            ListMarker::Task(false) => Some(Arc::from("[ ]")),
        },
        muted: quote > 0,
        // Consecutive list blocks sit closer, as in the markdown view.
        tight: index > 0 && depth > 0 && doc.indent(index - 1) > 0,
        copy_prefix: Arc::from(prefix),
        copy_line_prefix: Arc::from(quote_prefix),
    }
}

/// A table as aligned monospace pipe rows, header in bold. The text is
/// valid markdown, so copying a table pastes as one.
fn table_lines(doc: &MarkdownDoc, block: usize, theme: &Theme) -> Vec<Vec<StyledSpan>> {
    let columns = doc.table_columns(block).max(1);
    let mut rows: Vec<Vec<String>> = Vec::new();
    for cell in doc.cells(block) {
        let (row, col) = doc.cell_position(cell);
        if rows.len() <= row {
            rows.resize_with(row + 1, || vec![String::new(); columns]);
        }
        if let Some(slot) = rows[row].get_mut(col) {
            *slot = doc.cell_text(cell).replace('\n', " ");
        }
    }
    let mut widths = vec![3; columns];
    for row in &rows {
        for (width, text) in widths.iter_mut().zip(row) {
            *width = (*width).max(text.chars().count());
        }
    }

    let muted = theme.colors.text_muted;
    let span = |text: String, weight: FontWeight, color: Option<quark::Color>| StyledSpan {
        font_kind: FontKind::Mono,
        font_weight: weight,
        color,
        ..StyledSpan::plain(text)
    };
    let row_line = |row: &[String], weight: FontWeight| {
        let mut line = Vec::new();
        for (text, width) in row.iter().zip(&widths) {
            line.push(span("| ".to_owned(), FontWeight::Normal, Some(muted)));
            let pad = width - text.chars().count();
            line.push(span(format!("{text}{} ", " ".repeat(pad)), weight, None));
        }
        line.push(span("|".to_owned(), FontWeight::Normal, Some(muted)));
        line
    };

    let mut lines = Vec::with_capacity(rows.len() + 1);
    for (i, row) in rows.iter().enumerate() {
        if i == 0 {
            lines.push(row_line(row, FontWeight::Bold));
            let rule: String = widths
                .iter()
                .map(|w| format!("| {} ", "-".repeat(*w)))
                .chain(std::iter::once("|".to_owned()))
                .collect();
            lines.push(vec![span(rule, FontWeight::Normal, Some(muted))]);
        } else {
            lines.push(row_line(row, FontWeight::Normal));
        }
    }
    lines
}
