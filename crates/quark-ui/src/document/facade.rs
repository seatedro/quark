//! [`MarkdownDocument`]: a document of markdown rows that owns
//! everything between the app's markdown strings and the screen.
//!
//! Without it an app keeps the markdown sources, a [`MarkdownBlocks`] per
//! row, the converted rows, the [`Document`], and a
//! [`SyntaxHighlighter`] in step by hand: parse, convert, update, store,
//! and route arriving highlights back to their rows every frame. It
//! also measures the rows outside the window on a background thread, so
//! their heights become exact while the app idles.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use quark::selection::BlockKey;

use super::background::BackgroundMeasure;
use super::images::{ImageLoader, ImageStore};
use super::markdown::{BlockKeys, MarkdownBlocks};
use super::syntax::SyntaxHighlighter;
use super::{
    BlockMeasurer, Document, DocumentElement, DocumentEvent, DocumentRow, DocumentStyle, FindMatch,
    RowAdornment, RowChrome, RowDecorator, TextGeometry,
};
use crate::action::Action;
use crate::markdown::{BlockKind, IncrementalMarkdown, MarkdownDoc};
use crate::theme::Theme;
use crate::virtual_list::{RowError, RowKey, ScrollAlign};

/// A row to add to a [`MarkdownDocument`].
#[derive(Debug, Clone)]
pub struct MarkdownEntry {
    pub row: RowKey,
    pub chrome: RowChrome,
    pub markdown: String,
}

/// What is kept per row to convert it again cheaply.
struct Entry {
    chrome: RowChrome,
    adornments: Vec<RowAdornment>,
    source: String,
    parser: IncrementalMarkdown,
    doc: MarkdownDoc,
    markdown: MarkdownBlocks,
}

/// A virtualized document of markdown rows, keyed by the app's
/// [`RowKey`]s. It allocates every block key itself, so apps never pick
/// block keys, and streaming sources are parsed incrementally.
///
/// Each frame: [`Self::poll_highlights`], [`Self::prepare`], then
/// [`Self::element`]; pass the element's events to [`Self::handle`]. While
/// [`Self::is_measuring`], keep drawing frames so background heights land.
pub struct MarkdownDocument {
    document: Document,
    rows: HashMap<RowKey, DocumentRow>,
    entries: HashMap<RowKey, Entry>,
    /// Owner of every block key handed out, for routing highlights.
    block_rows: HashMap<BlockKey, RowKey>,
    syntax: SyntaxHighlighter,
    images: ImageStore,
    keys: BlockKeys,
    background: BackgroundMeasure,
}

impl MarkdownDocument {
    pub fn new(style: DocumentStyle) -> Self {
        Self {
            document: Document::new(style),
            rows: HashMap::new(),
            entries: HashMap::new(),
            block_rows: HashMap::new(),
            syntax: SyntaxHighlighter::new(),
            images: ImageStore::new(),
            keys: BlockKeys::new(),
            background: BackgroundMeasure::default(),
        }
    }

    /// Scroll, selection, and geometry state.
    pub fn document(&self) -> &Document {
        &self.document
    }

    /// For scrolling and selection changes. Document edits go through the
    /// facade, which keeps its rows in step with the document.
    pub fn document_mut(&mut self) -> &mut Document {
        &mut self.document
    }

    /// The converted rows, as a [`super::DocumentSource`].
    pub fn rows(&self) -> &HashMap<RowKey, DocumentRow> {
        &self.rows
    }

    /// See [`Document::set_decorator`].
    pub fn set_decorator(&mut self, decorator: impl RowDecorator + 'static) {
        self.document.set_decorator(decorator);
    }

    pub fn highlighter(&self) -> &SyntaxHighlighter {
        &self.syntax
    }

    /// Highlights code blocks with `store`'s grammars; see
    /// [`SyntaxHighlighter::set_grammar_store`].
    #[cfg(feature = "syntax")]
    pub fn set_grammar_store(&mut self, store: quark_syntax::GrammarStore) {
        self.syntax.set_grammar_store(store);
    }

    /// The markdown source of `row`.
    pub fn markdown(&self, row: RowKey) -> Option<&str> {
        self.entries.get(&row).map(|e| e.source.as_str())
    }

    pub fn len(&self) -> usize {
        self.document.len()
    }

    pub fn is_empty(&self) -> bool {
        self.document.is_empty()
    }

    /// Appends a row at the end.
    pub fn push(&mut self, entry: MarkdownEntry) -> Result<(), RowError> {
        self.extend([entry])
    }

    /// Appends many rows, as when loading history.
    pub fn extend(
        &mut self,
        entries: impl IntoIterator<Item = MarkdownEntry>,
    ) -> Result<(), RowError> {
        let rows = self.adopt(entries)?;
        self.document.extend(&rows)?;
        self.store(rows);
        Ok(())
    }

    /// Inserts older rows before the first one; the rows on screen and
    /// the selection stay put.
    pub fn prepend(
        &mut self,
        entries: impl IntoIterator<Item = MarkdownEntry>,
    ) -> Result<(), RowError> {
        let rows = self.adopt(entries)?;
        self.document.prepend(&rows)?;
        self.store(rows);
        Ok(())
    }

    /// Replaces the markdown of `row`, as each streamed chunk arrives. Only
    /// the blocks that changed are rebuilt and remeasured.
    pub fn set_markdown(&mut self, row: RowKey, markdown: &str) -> Result<(), RowError> {
        let entry = self
            .entries
            .get_mut(&row)
            .ok_or(RowError::UnknownKey(row))?;
        if entry.source == markdown {
            return Ok(());
        }
        entry.source.clear();
        entry.source.push_str(markdown);
        entry.doc = entry.parser.parse(&entry.source);
        self.refresh(row)
    }

    /// Replaces the chrome of `row`: its author line once an answer has
    /// finished streaming, say. The row is rebuilt on the next prepare.
    pub fn set_chrome(&mut self, row: RowKey, chrome: RowChrome) -> Result<(), RowError> {
        let entry = self
            .entries
            .get_mut(&row)
            .ok_or(RowError::UnknownKey(row))?;
        if entry.chrome == chrome {
            return Ok(());
        }
        entry.chrome = chrome;
        self.refresh(row)
    }

    /// Replaces the adornments of `row`: a tool card's header changing
    /// state, an action bar appearing under a finished answer. The row is
    /// rebuilt and remeasured on the next prepare; the row on screen keeps
    /// its place, so collapsing a card above the view moves nothing.
    pub fn set_adornments(
        &mut self,
        row: RowKey,
        adornments: Vec<RowAdornment>,
    ) -> Result<(), RowError> {
        let entry = self
            .entries
            .get_mut(&row)
            .ok_or(RowError::UnknownKey(row))?;
        if entry.adornments == adornments {
            return Ok(());
        }
        entry.adornments = adornments;
        self.refresh(row)
    }

    pub fn remove(&mut self, row: RowKey) -> Result<(), RowError> {
        self.document.remove(row)?;
        self.rows.remove(&row);
        self.background.forget(row);
        if let Some(mut entry) = self.entries.remove(&row) {
            for key in entry.markdown.keys() {
                self.block_rows.remove(key);
            }
            entry.markdown.clear(&mut self.syntax);
        }
        Ok(())
    }

    /// Takes finished code highlights and rebuilds their rows. Returns
    /// whether any row changed (draw a frame).
    pub fn poll_highlights(&mut self) -> bool {
        let keys = self.syntax.poll();
        self.refresh_highlighted(keys)
    }

    /// Waits for every pending highlight and applies it. For tests and
    /// screenshots.
    pub fn finish_highlights(&mut self) -> bool {
        let keys = self.syntax.finish_pending();
        self.refresh_highlighted(keys)
    }

    /// Sets how image URLs become bytes; see [`ImageStore`]. Without a
    /// loader, image blocks show their alt text.
    pub fn set_image_loader(&mut self, loader: ImageLoader) {
        self.images.set_loader(loader);
        self.refresh_all_images();
    }

    /// The intrinsic size of the image at `src`, when the app knows it
    /// before the pixels: its block reserves that height while it loads.
    pub fn hint_image_size(&mut self, src: &str, width: u32, height: u32) {
        self.images.hint_size(src, width, height);
        self.refresh_images(&[Arc::from(src)]);
    }

    /// Takes decoded images and rebuilds the rows showing them.
    /// Returns whether any row changed (draw a frame). Rows above the
    /// first visible one that change height keep the view in place.
    pub fn poll_images(&mut self) -> bool {
        let srcs = self.images.poll();
        self.refresh_images(&srcs)
    }

    /// Waits for every pending image and applies it. For tests and
    /// screenshots.
    pub fn finish_images(&mut self) -> bool {
        let srcs = self.images.finish_pending();
        self.refresh_images(&srcs)
    }

    /// Images are loading; keep drawing frames to pick them up.
    pub fn is_loading_images(&self) -> bool {
        self.images.is_loading()
    }

    /// Rebuilds the rows with an image block showing one of `srcs`.
    fn refresh_images(&mut self, srcs: &[Arc<str>]) -> bool {
        if srcs.is_empty() {
            return false;
        }
        let mut rows: Vec<RowKey> = self
            .entries
            .iter()
            .filter(|(_, e)| {
                (0..e.doc.len()).any(|b| {
                    e.doc.kind(b) == BlockKind::Image
                        && srcs.iter().any(|s| **s == *e.doc.image_src(b))
                })
            })
            .map(|(row, _)| *row)
            .collect();
        rows.sort_unstable();
        for row in &rows {
            // The row is in `entries`, so it exists.
            let _ = self.refresh(*row);
        }
        !rows.is_empty()
    }

    fn refresh_all_images(&mut self) {
        let mut rows: Vec<RowKey> = self
            .entries
            .iter()
            .filter(|(_, e)| (0..e.doc.len()).any(|b| e.doc.kind(b) == BlockKind::Image))
            .map(|(row, _)| *row)
            .collect();
        rows.sort_unstable();
        for row in &rows {
            let _ = self.refresh(*row);
        }
    }

    /// See [`Document::prepare`]. Also applies the row heights measured
    /// in the background since the last frame and requests more, when the
    /// measurer offers a [`super::MeasureSpec`] ([`super::TextMeasurer`]
    /// does). A width or font change drops the requests in flight.
    pub fn prepare<M: BlockMeasurer<Geometry = TextGeometry>>(
        &mut self,
        width: f32,
        height: f32,
        now_ms: u64,
        measurer: &mut M,
    ) {
        let style = *self.document.style();
        self.background.configure(measurer, style, width);
        self.background.apply(&mut self.document);
        self.document
            .prepare(width, height, now_ms, &self.rows, measurer);
        self.background.request(&self.document, &self.rows);
    }

    /// Rows are being measured in the background; draw another frame to
    /// pick their heights up.
    pub fn is_measuring(&self) -> bool {
        self.background.is_busy()
    }

    /// Measures every row still holding an estimate under the last
    /// prepare's width and fonts, and applies the heights, blocking until
    /// done. Positions of materialized rows update on the next prepare. For
    /// tests and screenshots. Returns whether any height changed.
    pub fn finish_measures(&mut self) -> bool {
        self.background.finish(&mut self.document, &self.rows)
    }

    #[cfg(test)]
    pub(super) fn background_mut(&mut self) -> (&mut BackgroundMeasure, &mut Document) {
        (&mut self.background, &mut self.document)
    }

    /// See [`Document::element`].
    pub fn element(
        &mut self,
        theme: &Theme,
        on_event: impl Fn(DocumentEvent) -> Action + 'static,
    ) -> DocumentElement {
        self.document.element(&self.rows, theme, on_event)
    }

    pub fn handle(&mut self, event: DocumentEvent) {
        self.document.handle(event);
    }

    /// See [`Document::set_find_query`].
    pub fn set_find_query(&mut self, query: &str) {
        self.document.set_find_query(query, &self.rows);
    }

    /// See [`Document::find_next`].
    pub fn find_next(&mut self, align: ScrollAlign) -> Option<FindMatch> {
        self.document.find_next(align)
    }

    /// See [`Document::find_prev`].
    pub fn find_prev(&mut self, align: ScrollAlign) -> Option<FindMatch> {
        self.document.find_prev(align)
    }

    /// See [`Document::reveal_current_match`].
    pub fn reveal_current_match(&mut self, align: ScrollAlign) -> Option<FindMatch> {
        self.document.reveal_current_match(align)
    }

    /// See [`Document::close_find`].
    pub fn close_find(&mut self) {
        self.document.close_find();
    }

    /// See [`Document::selected_text`].
    pub fn selected_text(&self) -> String {
        self.document.selected_text(&self.rows)
    }

    /// Parses and converts new rows without adding them anywhere.
    fn adopt(
        &mut self,
        entries: impl IntoIterator<Item = MarkdownEntry>,
    ) -> Result<Vec<DocumentRow>, RowError> {
        let entries: Vec<MarkdownEntry> = entries.into_iter().collect();
        // A whole history arrives as one batch, so repeats are found with a
        // set; the first repeated key is reported, and nothing is adopted.
        let mut seen = HashSet::with_capacity(entries.len());
        for entry in &entries {
            if !seen.insert(entry.row) || self.entries.contains_key(&entry.row) {
                return Err(RowError::DuplicateKey(entry.row));
            }
        }
        let mut rows = Vec::with_capacity(entries.len());
        for new in entries {
            let mut parser = IncrementalMarkdown::new();
            let doc = parser.parse(&new.markdown);
            let mut entry = Entry {
                chrome: new.chrome,
                adornments: Vec::new(),
                source: new.markdown,
                parser,
                doc,
                markdown: MarkdownBlocks::new(),
            };
            rows.push(self.convert(new.row, &mut entry));
            self.entries.insert(new.row, entry);
        }
        Ok(rows)
    }

    fn store(&mut self, rows: Vec<DocumentRow>) {
        self.rows.extend(rows.into_iter().map(|m| (m.key, m)));
    }

    /// Converts `entry`'s parsed markdown and records who owns its keys.
    fn convert(&mut self, row: RowKey, entry: &mut Entry) -> DocumentRow {
        let before = entry.markdown.keys().len();
        // Blocks past the new end give their keys up.
        for key in &entry.markdown.keys()[entry.doc.len().min(before)..] {
            self.block_rows.remove(key);
        }
        let blocks = entry.markdown.blocks(
            &entry.doc,
            &mut self.syntax,
            &mut self.images,
            &mut self.keys,
        );
        let keys = entry.markdown.keys();
        for key in &keys[before.min(keys.len())..] {
            self.block_rows.insert(*key, row);
        }
        DocumentRow {
            key: row,
            chrome: entry.chrome.clone(),
            blocks,
            adornments: entry.adornments.clone(),
        }
    }

    fn refresh(&mut self, row: RowKey) -> Result<(), RowError> {
        let Some(mut entry) = self.entries.remove(&row) else {
            return Err(RowError::UnknownKey(row));
        };
        let content = self.convert(row, &mut entry);
        self.entries.insert(row, entry);
        self.background.forget(row);
        let result = self.document.update(&content);
        self.rows.insert(row, content);
        result
    }

    fn refresh_highlighted(&mut self, keys: Vec<BlockKey>) -> bool {
        let mut rows: Vec<RowKey> = keys
            .iter()
            .filter_map(|key| self.block_rows.get(key).copied())
            .collect();
        rows.sort_unstable();
        rows.dedup();
        for row in &rows {
            // The row was in `block_rows`, so it exists.
            let _ = self.refresh(*row);
        }
        !rows.is_empty()
    }
}
