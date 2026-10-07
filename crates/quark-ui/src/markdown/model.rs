//! Markdown parsed into flat columns: leaf blocks, inline spans, links, and
//! table cells. Containers (lists, block quotes) are not nodes; each leaf
//! block records the list depth and quote depth it sits in, which is all the
//! view needs to indent it and draw quote bars.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::ops::Range;
use std::sync::Arc;

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

/// What a leaf block renders as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BlockKind {
    Paragraph,
    /// Level 1 to 6.
    Heading(u8),
    /// Fenced or indented code. The block text is the code, without fences
    /// and without the final line ending.
    CodeBlock,
    /// Cells live in the cell columns; the block's spans are empty.
    Table,
    Rule,
}

/// The list marker shown before a block. Only the first block of a list
/// item carries one; later blocks of the item have [`ListMarker::None`] and
/// the item's indent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ListMarker {
    None,
    Bullet,
    Ordered(u64),
    /// Task list item; `true` when checked.
    Task(bool),
}

/// Inline style bits of a span.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct SpanFlags(u8);

impl SpanFlags {
    pub const NONE: Self = Self(0);
    pub const BOLD: Self = Self(1);
    pub const ITALIC: Self = Self(1 << 1);
    pub const STRIKE: Self = Self(1 << 2);
    pub const CODE: Self = Self(1 << 3);
    pub const LINK: Self = Self(1 << 4);
    /// Image alt text standing in for the image.
    pub const IMAGE: Self = Self(1 << 5);

    pub fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    fn with(self, other: Self, on: bool) -> Self {
        if on { Self(self.0 | other.0) } else { self }
    }
}

/// `span_link` value for spans outside any link.
pub const NO_LINK: u32 = u32::MAX;

/// Parsed markdown. Every `Range<u32>` indexes `text` (byte ranges) or the
/// span/cell columns. Spans of a block tile its text range in order.
#[derive(Debug, Default, Clone)]
pub struct MarkdownDoc {
    /// Arena of every block's display text plus code block languages.
    text: String,

    block_kind: Vec<BlockKind>,
    block_text: Vec<Range<u32>>,
    block_spans: Vec<Range<u32>>,
    block_quote: Vec<u8>,
    block_indent: Vec<u8>,
    block_marker: Vec<ListMarker>,
    /// Code block language (first word of the fence info); empty otherwise.
    block_lang: Vec<Range<u32>>,
    block_cells: Vec<Range<u32>>,
    block_columns: Vec<u32>,
    block_hash: Vec<u64>,

    span_range: Vec<Range<u32>>,
    span_flags: Vec<SpanFlags>,
    span_link: Vec<u32>,

    link_url: Vec<Arc<str>>,

    cell_row: Vec<u32>,
    cell_col: Vec<u32>,
    cell_text: Vec<Range<u32>>,
    cell_spans: Vec<Range<u32>>,
}

/// Integrity violations found by [`MarkdownDoc::verify_integrity`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntegrityError {
    ColumnLengths,
    SpansDoNotTile { block: usize },
    CellSpansDoNotTile { cell: usize },
    BadLink { span: usize },
    BadRange,
}

fn urange(r: &Range<u32>) -> Range<usize> {
    r.start as usize..r.end as usize
}

fn len_u32(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

impl MarkdownDoc {
    pub fn parse(source: &str) -> Self {
        let options =
            Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
        let mut builder = Builder::default();
        for event in Parser::new_ext(source, options) {
            builder.event(event);
        }
        builder.finish()
    }

    pub fn len(&self) -> usize {
        self.block_kind.len()
    }

    pub fn is_empty(&self) -> bool {
        self.block_kind.is_empty()
    }

    pub fn kind(&self, block: usize) -> BlockKind {
        self.block_kind[block]
    }

    /// Display text of a block (table cells concatenated for tables).
    pub fn text(&self, block: usize) -> &str {
        self.slice(&self.block_text[block])
    }

    pub fn quote_depth(&self, block: usize) -> u8 {
        self.block_quote[block]
    }

    /// List nesting depth; 0 outside lists.
    pub fn indent(&self, block: usize) -> u8 {
        self.block_indent[block]
    }

    pub fn marker(&self, block: usize) -> ListMarker {
        self.block_marker[block]
    }

    pub fn lang(&self, block: usize) -> &str {
        self.slice(&self.block_lang[block])
    }

    /// Hash of everything the block renders from. Unchanged leading blocks
    /// of a growing document keep their hash.
    pub fn hash(&self, block: usize) -> u64 {
        self.block_hash[block]
    }

    /// Span indices of a block.
    pub fn spans(&self, block: usize) -> Range<usize> {
        urange(&self.block_spans[block])
    }

    /// Span text, byte range within its block or cell text, flags, and URL.
    pub fn span(&self, span: usize) -> (&str, SpanFlags, Option<&Arc<str>>) {
        let link = self.span_link[span];
        (
            self.slice(&self.span_range[span]),
            self.span_flags[span],
            (link != NO_LINK).then(|| &self.link_url[link as usize]),
        )
    }

    pub fn table_columns(&self, block: usize) -> usize {
        self.block_columns[block] as usize
    }

    /// Cell indices of a table block, row-major.
    pub fn cells(&self, block: usize) -> Range<usize> {
        urange(&self.block_cells[block])
    }

    /// `(row, column)`; row 0 is the header.
    pub fn cell_position(&self, cell: usize) -> (usize, usize) {
        (self.cell_row[cell] as usize, self.cell_col[cell] as usize)
    }

    pub fn cell_text(&self, cell: usize) -> &str {
        self.slice(&self.cell_text[cell])
    }

    pub fn cell_spans(&self, cell: usize) -> Range<usize> {
        urange(&self.cell_spans[cell])
    }

    /// Source-accurate plain text for copying a whole block: list markers
    /// and heading hashes kept, quote prefixes on every line, code without
    /// fences, tables as pipe rows. Inline markup is dropped, matching what
    /// selection copies.
    pub fn plain_text(&self, block: usize) -> String {
        let mut body = String::new();
        match self.kind(block) {
            BlockKind::Heading(level) => {
                body.extend(std::iter::repeat_n('#', level as usize));
                body.push(' ');
                body.push_str(self.text(block));
            }
            BlockKind::Rule => body.push_str("---"),
            BlockKind::Table => self.table_plain_text(block, &mut body),
            BlockKind::Paragraph | BlockKind::CodeBlock => body.push_str(self.text(block)),
        }
        let marker = match self.marker(block) {
            ListMarker::None => String::new(),
            ListMarker::Bullet => "- ".to_owned(),
            ListMarker::Ordered(n) => format!("{n}. "),
            ListMarker::Task(true) => "- [x] ".to_owned(),
            ListMarker::Task(false) => "- [ ] ".to_owned(),
        };
        let quote = "> ".repeat(self.quote_depth(block) as usize);
        let mut out = String::with_capacity(body.len() + marker.len() + quote.len());
        for (i, line) in body.split('\n').enumerate() {
            if i > 0 {
                out.push('\n');
            }
            out.push_str(&quote);
            if i == 0 {
                out.push_str(&marker);
            }
            out.push_str(line);
        }
        out
    }

    fn table_plain_text(&self, block: usize, out: &mut String) {
        let columns = self.table_columns(block);
        let mut row = None;
        for cell in self.cells(block) {
            let (r, _) = self.cell_position(cell);
            if row != Some(r) {
                if let Some(prev) = row {
                    out.push_str(" |\n");
                    if prev == 0 {
                        out.push_str(&"| --- ".repeat(columns));
                        out.push_str("|\n");
                    }
                }
                out.push_str("| ");
                row = Some(r);
            } else {
                out.push_str(" | ");
            }
            out.push_str(self.cell_text(cell));
        }
        if let Some(r) = row {
            out.push_str(" |");
            if r == 0 {
                out.push('\n');
                out.push_str(&"| --- ".repeat(columns));
                out.push('|');
            }
        }
    }

    fn slice(&self, range: &Range<u32>) -> &str {
        self.text.get(urange(range)).unwrap_or_default()
    }

    pub fn verify_integrity(&self) -> Result<(), IntegrityError> {
        let n = self.block_kind.len();
        let blocks_ok = [
            self.block_text.len(),
            self.block_spans.len(),
            self.block_quote.len(),
            self.block_indent.len(),
            self.block_marker.len(),
            self.block_lang.len(),
            self.block_cells.len(),
            self.block_columns.len(),
            self.block_hash.len(),
        ]
        .iter()
        .all(|&len| len == n);
        let spans_ok = self.span_flags.len() == self.span_range.len()
            && self.span_link.len() == self.span_range.len();
        let cells_ok = [
            self.cell_col.len(),
            self.cell_text.len(),
            self.cell_spans.len(),
        ]
        .iter()
        .all(|&len| len == self.cell_row.len());
        if !(blocks_ok && spans_ok && cells_ok) {
            return Err(IntegrityError::ColumnLengths);
        }
        let in_text = |r: &Range<u32>| {
            r.start <= r.end
                && self.text.is_char_boundary(r.start as usize)
                && self.text.is_char_boundary(r.end as usize)
                && (r.end as usize) <= self.text.len()
        };
        let tiles = |text: &Range<u32>, spans: &Range<u32>| {
            let mut at = text.start;
            for s in urange(spans) {
                match self.span_range.get(s) {
                    Some(r) if r.start == at && r.end > r.start => at = r.end,
                    _ => return false,
                }
            }
            spans.is_empty() || at == text.end
        };
        for block in 0..n {
            if !in_text(&self.block_text[block]) || !in_text(&self.block_lang[block]) {
                return Err(IntegrityError::BadRange);
            }
            if !tiles(&self.block_text[block], &self.block_spans[block]) {
                return Err(IntegrityError::SpansDoNotTile { block });
            }
        }
        for cell in 0..self.cell_row.len() {
            if !in_text(&self.cell_text[cell]) {
                return Err(IntegrityError::BadRange);
            }
            if !tiles(&self.cell_text[cell], &self.cell_spans[cell]) {
                return Err(IntegrityError::CellSpansDoNotTile { cell });
            }
        }
        for (span, &link) in self.span_link.iter().enumerate() {
            let flagged = self.span_flags[span].contains(SpanFlags::LINK);
            if (link != NO_LINK) != flagged
                || (link != NO_LINK && link as usize >= self.link_url.len())
            {
                return Err(IntegrityError::BadLink { span });
            }
        }
        Ok(())
    }
}

/// An open leaf block or table cell collecting text.
#[derive(Debug, Clone, Copy)]
struct Open {
    kind: BlockKind,
    text_start: u32,
    span_start: u32,
    marker: ListMarker,
}

/// Parser state. Inline styles are depth counters because they nest.
#[derive(Default)]
struct Builder {
    doc: MarkdownDoc,
    open: Option<Open>,
    lang: Range<u32>,
    quote: u8,
    /// Next ordinal per open list; `None` for bullet lists.
    lists: Vec<Option<u64>>,
    pending_marker: Option<ListMarker>,
    bold: u32,
    italic: u32,
    strike: u32,
    image: u32,
    image_text_start: u32,
    links: Vec<u32>,
    table: Option<TableState>,
}

#[derive(Debug, Clone, Copy)]
struct TableState {
    text_start: u32,
    cell_start: u32,
    columns: u32,
    row: u32,
    col: u32,
    cell: Option<(u32, u32)>,
}

impl Builder {
    fn event(&mut self, event: Event) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => {
                if self.table.is_none() || self.in_cell() {
                    self.ensure_open();
                    self.push_text(&text, SpanFlags::NONE);
                }
            }
            Event::Code(text) => {
                self.ensure_open();
                self.push_text(&text, SpanFlags::CODE);
            }
            Event::Html(text) | Event::InlineHtml(text) => {
                self.ensure_open();
                self.push_text(text.trim_end_matches('\n'), SpanFlags::NONE);
            }
            Event::InlineMath(text) | Event::DisplayMath(text) => {
                self.ensure_open();
                self.push_text(&text, SpanFlags::CODE);
            }
            Event::FootnoteReference(name) => {
                self.ensure_open();
                self.push_text(&format!("[^{name}]"), SpanFlags::NONE);
            }
            // Soft breaks keep the author's line structure, as chat and
            // comment renderers do.
            Event::SoftBreak | Event::HardBreak => {
                self.ensure_open();
                self.push_text("\n", SpanFlags::NONE);
            }
            Event::Rule => {
                self.close_open();
                self.open_block(BlockKind::Rule);
                self.close_open();
            }
            Event::TaskListMarker(checked) => match &mut self.open {
                Some(open) if open.marker != ListMarker::None => {
                    open.marker = ListMarker::Task(checked)
                }
                _ => self.pending_marker = Some(ListMarker::Task(checked)),
            },
        }
    }

    fn start(&mut self, tag: Tag) {
        match tag {
            Tag::Paragraph => {
                if !self.in_cell() {
                    self.close_open();
                    self.open_block(BlockKind::Paragraph);
                }
            }
            Tag::Heading { level, .. } => {
                self.close_open();
                self.open_block(BlockKind::Heading(heading_level(level)));
            }
            Tag::CodeBlock(kind) => {
                self.close_open();
                let lang = match kind {
                    CodeBlockKind::Fenced(info) => info
                        .split_whitespace()
                        .next()
                        .unwrap_or_default()
                        .to_owned(),
                    CodeBlockKind::Indented => String::new(),
                };
                let start = len_u32(self.doc.text.len());
                self.doc.text.push_str(&lang);
                self.lang = start..len_u32(self.doc.text.len());
                self.open_block(BlockKind::CodeBlock);
            }
            Tag::BlockQuote(_) => {
                self.close_open();
                self.quote = self.quote.saturating_add(1);
            }
            Tag::List(start) => {
                self.close_open();
                self.lists.push(start);
            }
            Tag::Item => {
                self.close_open();
                let marker = match self.lists.last_mut() {
                    Some(Some(n)) => {
                        let marker = ListMarker::Ordered(*n);
                        *n += 1;
                        marker
                    }
                    _ => ListMarker::Bullet,
                };
                self.pending_marker = Some(marker);
            }
            Tag::Table(aligns) => {
                self.close_open();
                self.open_block(BlockKind::Table);
                self.table = Some(TableState {
                    text_start: len_u32(self.doc.text.len()),
                    cell_start: len_u32(self.doc.cell_row.len()),
                    columns: len_u32(aligns.len()),
                    row: 0,
                    col: 0,
                    cell: None,
                });
            }
            Tag::TableCell => {
                let text = len_u32(self.doc.text.len());
                let spans = len_u32(self.doc.span_range.len());
                if let Some(table) = &mut self.table {
                    table.cell = Some((text, spans));
                }
            }
            Tag::Emphasis => self.italic += 1,
            Tag::Strong => self.bold += 1,
            Tag::Strikethrough => self.strike += 1,
            Tag::Link { dest_url, .. } => {
                self.ensure_open();
                self.links.push(len_u32(self.doc.link_url.len()));
                self.doc.link_url.push(Arc::from(dest_url.as_ref()));
            }
            Tag::Image { .. } => {
                self.ensure_open();
                self.image += 1;
                self.push_text("[", SpanFlags::NONE);
                self.image_text_start = len_u32(self.doc.text.len());
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::CodeBlock | TagEnd::HtmlBlock => {
                if !self.in_cell() {
                    self.close_open();
                }
            }
            TagEnd::BlockQuote(_) => {
                self.close_open();
                self.quote = self.quote.saturating_sub(1);
            }
            TagEnd::List(_) => {
                self.close_open();
                self.lists.pop();
            }
            TagEnd::Item => {
                self.close_open();
                // An empty item still shows its marker.
                if self.pending_marker.is_some() {
                    self.open_block(BlockKind::Paragraph);
                    self.close_open();
                }
            }
            TagEnd::TableCell => self.close_cell(),
            TagEnd::TableHead | TagEnd::TableRow => {
                if let Some(table) = &mut self.table {
                    table.row += 1;
                    table.col = 0;
                }
            }
            TagEnd::Table => {
                if let Some(table) = self.table.take() {
                    let block = self.doc.block_kind.len();
                    if let Some(open) = self.open.take() {
                        self.push_block(open);
                        self.doc.block_text[block] = table.text_start..len_u32(self.doc.text.len());
                        self.doc.block_cells[block] =
                            table.cell_start..len_u32(self.doc.cell_row.len());
                        self.doc.block_columns[block] = table.columns;
                        self.doc.block_hash[block] = self.table_hash(block);
                    }
                }
            }
            TagEnd::Emphasis => self.italic = self.italic.saturating_sub(1),
            TagEnd::Strong => self.bold = self.bold.saturating_sub(1),
            TagEnd::Strikethrough => self.strike = self.strike.saturating_sub(1),
            TagEnd::Link => {
                self.links.pop();
            }
            TagEnd::Image => {
                if len_u32(self.doc.text.len()) == self.image_text_start {
                    self.push_text("image", SpanFlags::NONE);
                }
                self.push_text("]", SpanFlags::NONE);
                self.image = self.image.saturating_sub(1);
            }
            _ => {}
        }
    }

    fn in_cell(&self) -> bool {
        self.table.is_some_and(|t| t.cell.is_some())
    }

    /// Text outside any block (tight list items, HTML) starts a paragraph.
    fn ensure_open(&mut self) {
        if self.open.is_none() {
            self.open_block(BlockKind::Paragraph);
        }
    }

    fn open_block(&mut self, kind: BlockKind) {
        if kind != BlockKind::CodeBlock {
            self.lang = 0..0;
        }
        self.open = Some(Open {
            kind,
            text_start: len_u32(self.doc.text.len()),
            span_start: len_u32(self.doc.span_range.len()),
            marker: self.pending_marker.take().unwrap_or(ListMarker::None),
        });
    }

    fn close_open(&mut self) {
        if self.table.is_some() {
            return;
        }
        if let Some(open) = self.open.take() {
            if open.kind == BlockKind::CodeBlock {
                self.trim_final_newline(open);
            }
            self.push_block(open);
            let block = self.doc.block_kind.len() - 1;
            self.doc.block_hash[block] = self.block_hash(block);
        }
    }

    fn push_block(&mut self, open: Open) {
        let d = &mut self.doc;
        d.block_kind.push(open.kind);
        d.block_text.push(open.text_start..len_u32(d.text.len()));
        d.block_spans
            .push(open.span_start..len_u32(d.span_range.len()));
        d.block_quote.push(self.quote);
        d.block_indent
            .push(len_u32(self.lists.len()).min(255) as u8);
        d.block_marker.push(open.marker);
        d.block_lang.push(std::mem::replace(&mut self.lang, 0..0));
        d.block_cells.push(0..0);
        d.block_columns.push(0);
        d.block_hash.push(0);
    }

    /// Fenced code always ends with a line ending; it is not part of the code.
    fn trim_final_newline(&mut self, open: Open) {
        let d = &mut self.doc;
        if d.text.len() > open.text_start as usize && d.text.ends_with('\n') {
            d.text.pop();
            let end = len_u32(d.text.len());
            if let Some(last) = d.span_range.last_mut() {
                last.end = end;
                if last.start == last.end {
                    d.span_range.pop();
                    d.span_flags.pop();
                    d.span_link.pop();
                }
            }
        }
    }

    fn close_cell(&mut self) {
        let Some(table) = &mut self.table else {
            return;
        };
        let Some((text_start, span_start)) = table.cell.take() else {
            return;
        };
        let d = &mut self.doc;
        d.cell_row.push(table.row);
        d.cell_col.push(table.col);
        d.cell_text.push(text_start..len_u32(d.text.len()));
        d.cell_spans.push(span_start..len_u32(d.span_range.len()));
        table.col += 1;
    }

    fn push_text(&mut self, text: &str, extra: SpanFlags) {
        if text.is_empty() {
            return;
        }
        let link = self.links.last().copied().unwrap_or(NO_LINK);
        let flags = SpanFlags(extra.0)
            .with(SpanFlags::BOLD, self.bold > 0)
            .with(SpanFlags::ITALIC, self.italic > 0)
            .with(SpanFlags::STRIKE, self.strike > 0)
            .with(SpanFlags::LINK, link != NO_LINK)
            .with(SpanFlags::IMAGE, self.image > 0)
            .with(
                SpanFlags::CODE,
                self.open.is_some_and(|o| o.kind == BlockKind::CodeBlock),
            );
        let d = &mut self.doc;
        let start = len_u32(d.text.len());
        d.text.push_str(text);
        let end = len_u32(d.text.len());
        // Extend the previous span when the style continues, but never
        // across a block or cell boundary.
        let floor = match (self.table.and_then(|t| t.cell), self.open) {
            (Some((_, spans)), _) => spans,
            (None, Some(open)) => open.span_start,
            (None, None) => len_u32(d.span_range.len()),
        };
        let n = d.span_range.len();
        if n > floor as usize
            && d.span_flags[n - 1] == flags
            && d.span_link[n - 1] == link
            && d.span_range[n - 1].end == start
        {
            d.span_range[n - 1].end = end;
        } else {
            d.span_range.push(start..end);
            d.span_flags.push(flags);
            d.span_link.push(link);
        }
    }

    fn hash_spans(&self, spans: Range<usize>, base: u32, h: &mut DefaultHasher) {
        for s in spans {
            let r = &self.doc.span_range[s];
            (r.start - base, r.end - base).hash(h);
            self.doc.span_flags[s].hash(h);
            let link = self.doc.span_link[s];
            if link != NO_LINK {
                self.doc.link_url[link as usize].hash(h);
            }
        }
    }

    fn block_hash(&self, block: usize) -> u64 {
        let d = &self.doc;
        let mut h = DefaultHasher::new();
        d.block_kind[block].hash(&mut h);
        d.block_quote[block].hash(&mut h);
        d.block_indent[block].hash(&mut h);
        d.block_marker[block].hash(&mut h);
        d.lang(block).hash(&mut h);
        d.text(block).hash(&mut h);
        self.hash_spans(d.spans(block), d.block_text[block].start, &mut h);
        h.finish()
    }

    fn table_hash(&self, block: usize) -> u64 {
        let d = &self.doc;
        let mut h = DefaultHasher::new();
        self.block_hash(block).hash(&mut h);
        for cell in d.cells(block) {
            d.cell_position(cell).hash(&mut h);
            d.cell_text(cell).hash(&mut h);
            self.hash_spans(d.cell_spans(cell), d.cell_text[cell].start, &mut h);
        }
        h.finish()
    }

    fn finish(mut self) -> MarkdownDoc {
        self.table = None;
        self.close_open();
        debug_assert_eq!(self.doc.verify_integrity(), Ok(()));
        self.doc
    }
}

fn heading_level(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

#[cfg(test)]
impl MarkdownDoc {
    /// One line per block: indent, quote depth, kind, marker, then the text
    /// with styled spans as `[flags:text]` (flags b i s c l img). Tables
    /// list cells as `rRcC:` entries.
    pub(crate) fn dump(&self) -> String {
        let mut out = String::new();
        for block in 0..self.len() {
            if block > 0 {
                out.push('\n');
            }
            out.push_str(&"  ".repeat(self.indent(block).saturating_sub(1) as usize));
            out.push_str(&">".repeat(self.quote_depth(block) as usize));
            let kind = match self.kind(block) {
                BlockKind::Paragraph => "p".to_owned(),
                BlockKind::Heading(n) => format!("h{n}"),
                BlockKind::CodeBlock => format!("code({})", self.lang(block)),
                BlockKind::Table => format!("table{}", self.table_columns(block)),
                BlockKind::Rule => "hr".to_owned(),
            };
            out.push_str(&kind);
            match self.marker(block) {
                ListMarker::None => {}
                ListMarker::Bullet => out.push_str(" -"),
                ListMarker::Ordered(n) => out.push_str(&format!(" {n}.")),
                ListMarker::Task(c) => out.push_str(if c { " [x]" } else { " [ ]" }),
            }
            out.push(':');
            if self.kind(block) == BlockKind::Table {
                for cell in self.cells(block) {
                    let (r, c) = self.cell_position(cell);
                    out.push_str(&format!(" r{r}c{c}:"));
                    self.dump_spans(self.cell_spans(cell), &mut out);
                }
            } else {
                out.push(' ');
                self.dump_spans(self.spans(block), &mut out);
            }
        }
        out
    }

    fn dump_spans(&self, spans: Range<usize>, out: &mut String) {
        for s in spans {
            let (text, flags, url) = self.span(s);
            let text = text.replace('\n', "\\n");
            let names: Vec<&str> = [
                (SpanFlags::BOLD, "b"),
                (SpanFlags::ITALIC, "i"),
                (SpanFlags::STRIKE, "s"),
                (SpanFlags::CODE, "c"),
                (SpanFlags::IMAGE, "img"),
            ]
            .into_iter()
            .filter(|(f, _)| flags.contains(*f))
            .map(|(_, n)| n)
            .collect();
            let mut names = names.join(",");
            if let Some(url) = url {
                if !names.is_empty() {
                    names.push(',');
                }
                names.push_str(&format!("l={url}"));
            }
            if names.is_empty() {
                out.push_str(&text);
            } else {
                out.push_str(&format!("[{names}:{text}]"));
            }
        }
    }
}
