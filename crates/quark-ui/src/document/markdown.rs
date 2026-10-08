//! Markdown rows as document blocks.
//!
//! [`MarkdownBlocks`] turns each [`MarkdownDoc`] block into one
//! [`Block`], so every heading, paragraph, list item, quote,
//! table, code block, and rule is its own selectable block and selection
//! runs across rows. Converted blocks are kept between calls and reused
//! while their markdown and highlight are unchanged, so a streaming row
//! rebuilds only its growing tail and the leading blocks keep the layouts
//! already shaped for them.
//!
//! Blocks carry [`SpanTone`]s instead of theme colors; the document
//! element resolves them, so a theme change needs no reconversion.

use std::ops::Range;
use std::sync::Arc;

use quark::selection::BlockKey;
use quark_render::{FontKind, FontWeight};

use super::syntax::SyntaxHighlighter;
use super::{Block, BlockStyle, ImageState, ImageStore, SpanTone};
use crate::element::StyledSpan;
use crate::markdown::{BlockKind, ListMarker, MarkdownDoc, SpanFlags};

/// Code block font size relative to body text.
pub const CODE_SCALE: f32 = 0.92;
/// Table font size relative to body text.
pub const TABLE_SCALE: f32 = 0.93;

/// Font size scale and base weight of a heading level.
pub fn heading_style(level: u8) -> (f32, FontWeight) {
    match level {
        1 => (1.5, FontWeight::Bold),
        2 => (1.3, FontWeight::Bold),
        3 => (1.15, FontWeight::Semibold),
        _ => (1.0, FontWeight::Semibold),
    }
}

/// Hands out block keys that are never reused, so blocks of different
/// rows, or of one row before and after an edit, cannot collide.
#[derive(Debug, Clone, Default)]
pub struct BlockKeys {
    next: u64,
}

impl BlockKeys {
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts at `first`, for apps that keep other block keys below it.
    pub fn starting_at(first: u64) -> Self {
        Self { next: first }
    }

    pub fn allocate(&mut self) -> BlockKey {
        let key = BlockKey(self.next);
        // 2^64 keys at a billion per second last 584 years.
        self.next = self.next.wrapping_add(1);
        key
    }
}

struct Converted {
    hash: u64,
    highlight: u64,
    block: Block,
}

/// One row's markdown converted to document blocks, with the conversions
/// kept for reuse. Block `i` keeps its key for as long as the row has an
/// `i`-th block.
#[derive(Default)]
pub struct MarkdownBlocks {
    keys: Vec<BlockKey>,
    converted: Vec<Converted>,
}

impl MarkdownBlocks {
    pub fn new() -> Self {
        Self::default()
    }

    /// Keys of the blocks the last [`Self::blocks`] call returned.
    pub fn keys(&self) -> &[BlockKey] {
        &self.keys
    }

    /// Forgets every conversion and the highlights of every block, as when
    /// the row is removed.
    pub fn clear(&mut self, syntax: &mut SyntaxHighlighter) {
        for key in self.keys.drain(..) {
            syntax.forget(key);
        }
        self.converted.clear();
    }

    /// The blocks of `doc`. A block whose markdown hash and code highlight
    /// match the previous call is returned as before (its spans shared,
    /// not rebuilt). New blocks get keys from `keys`; blocks past the end
    /// of `doc` give up their keys and highlights.
    ///
    /// Image blocks take their pixels from `images`, which starts loading
    /// each image the first time it is asked for it; a block whose image
    /// state changed is rebuilt like one whose markdown did.
    pub fn blocks(
        &mut self,
        doc: &MarkdownDoc,
        syntax: &mut SyntaxHighlighter,
        images: &mut ImageStore,
        keys: &mut BlockKeys,
    ) -> Vec<Block> {
        for key in self.keys.drain(doc.len().min(self.keys.len())..) {
            syntax.forget(key);
        }
        self.converted.truncate(doc.len());
        while self.keys.len() < doc.len() {
            self.keys.push(keys.allocate());
        }
        for index in 0..doc.len() {
            let key = self.keys[index];
            let hash = doc.hash(index);
            let mut image = None;
            let highlight = match doc.kind(index) {
                BlockKind::CodeBlock => syntax.version(key, doc.lang(index), doc.text(index)),
                BlockKind::Image => {
                    let (state, version) = images.state(doc.image_src(index));
                    image = Some(state);
                    version
                }
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
                block: convert(doc, index, key, syntax, image),
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
    syntax: &SyntaxHighlighter,
    image: Option<ImageState>,
) -> Block {
    let style = block_style(doc, index);
    let block = match doc.kind(index) {
        BlockKind::Paragraph | BlockKind::Heading(_) => {
            Block::toned_prose(key, styled_spans(doc, doc.spans(index), style.weight))
        }
        BlockKind::CodeBlock => Block::toned_code(key, syntax.lines(key, doc.text(index)))
            .with_label(Some(Arc::from(doc.lang(index)))),
        BlockKind::Table => Block::table(key, table_rows(doc, index)),
        BlockKind::Rule => Block::rule(key),
        BlockKind::Image => Block::image(
            key,
            Arc::from(doc.image_src(index)),
            doc.text(index),
            image.unwrap_or(ImageState::Failed),
        ),
    };
    block.with_style(style)
}

/// The spans `spans` of `doc` as styled runs with their tones. Span weights
/// override the block's base weight in SelectableText, so the base
/// (heading weight) is folded into every span here.
fn styled_spans(
    doc: &MarkdownDoc,
    spans: Range<usize>,
    base: FontWeight,
) -> Vec<(StyledSpan, SpanTone)> {
    spans
        .map(|s| {
            let (text, flags, url) = doc.span(s);
            let code = flags.contains(SpanFlags::CODE);
            let image = flags.contains(SpanFlags::IMAGE);
            let span = StyledSpan {
                font_kind: if code { FontKind::Mono } else { FontKind::Ui },
                font_weight: if flags.contains(SpanFlags::BOLD) {
                    FontWeight::Bold
                } else {
                    base
                },
                italic: flags.contains(SpanFlags::ITALIC) || image,
                strikethrough: flags.contains(SpanFlags::STRIKE),
                link: url.cloned(),
                ..StyledSpan::plain(text)
            };
            let tone = if code {
                SpanTone::InlineCode
            } else if image {
                SpanTone::Muted
            } else {
                SpanTone::Plain
            };
            (span, tone)
        })
        .collect()
}

/// Indent, marker, size, and copy prefixes of block `index`.
fn block_style(doc: &MarkdownDoc, index: usize) -> BlockStyle {
    let quote = doc.quote_depth(index);
    let depth = doc.indent(index);
    let marker = doc.marker(index);
    let (scale, weight) = match doc.kind(index) {
        BlockKind::Heading(level) => heading_style(level),
        BlockKind::CodeBlock => (CODE_SCALE, FontWeight::Normal),
        BlockKind::Table => (TABLE_SCALE, FontWeight::Normal),
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
        // Consecutive list blocks sit closer together.
        tight: index > 0 && depth > 0 && doc.indent(index - 1) > 0,
        copy_prefix: shared(prefix),
        copy_line_prefix: shared(quote_prefix),
    }
}

/// `text` as a shared string, the shared empty one when it is empty.
fn shared(text: String) -> Arc<str> {
    if text.is_empty() {
        super::empty_str()
    } else {
        Arc::from(text)
    }
}

/// A table's cells as styled runs, row by row; the header is semibold.
fn table_rows(doc: &MarkdownDoc, block: usize) -> Vec<Vec<Vec<(StyledSpan, SpanTone)>>> {
    let columns = doc.table_columns(block).max(1);
    let mut rows: Vec<Vec<Vec<(StyledSpan, SpanTone)>>> = Vec::new();
    for cell in doc.cells(block) {
        let (row, col) = doc.cell_position(cell);
        if rows.len() <= row {
            rows.resize_with(row + 1, || vec![Vec::new(); columns]);
        }
        let weight = if row == 0 {
            FontWeight::Semibold
        } else {
            FontWeight::Normal
        };
        if let Some(slot) = rows[row].get_mut(col) {
            *slot = styled_spans(doc, doc.cell_spans(cell), weight);
        }
    }
    rows
}
