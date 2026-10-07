//! [`MarkdownTranscript`]: a transcript of markdown messages that owns
//! everything between the app's markdown strings and the screen.
//!
//! Without it an app keeps the markdown sources, a [`MarkdownMessage`] per
//! message, the converted messages, the [`Transcript`], and a
//! [`SyntaxHighlighter`] in step by hand: parse, convert, update, store,
//! and route arriving highlights back to their messages every frame. It
//! also measures the rows outside the window on a background thread, so
//! their heights become exact while the app idles.

use std::collections::HashMap;
use std::sync::Arc;

use quark::selection::BlockKey;

use super::background::BackgroundMeasure;
use super::markdown::{BlockKeys, MarkdownMessage};
use super::syntax::SyntaxHighlighter;
use super::{
    BlockMeasurer, FindMatch, TextGeometry, Transcript, TranscriptElement, TranscriptEvent,
    TranscriptMessage, TranscriptRole, TranscriptStyle,
};
use crate::action::Action;
use crate::markdown::{IncrementalMarkdown, MarkdownDoc};
use crate::theme::Theme;
use crate::virtual_list::{RowError, RowKey, ScrollAlign};

/// A message to add to a [`MarkdownTranscript`].
#[derive(Debug, Clone)]
pub struct MarkdownEntry {
    pub row: RowKey,
    pub role: TranscriptRole,
    pub author: Arc<str>,
    pub markdown: String,
}

/// What is kept per message to convert it again cheaply.
struct Entry {
    role: TranscriptRole,
    author: Arc<str>,
    source: String,
    parser: IncrementalMarkdown,
    doc: MarkdownDoc,
    markdown: MarkdownMessage,
}

/// A virtualized transcript of markdown messages, keyed by the app's
/// [`RowKey`]s. It allocates every block key itself, so apps never pick
/// block keys, and streaming sources are parsed incrementally.
///
/// Each frame: [`Self::poll_highlights`], [`Self::prepare`], then
/// [`Self::element`]; pass the element's events to [`Self::handle`]. While
/// [`Self::is_measuring`], keep drawing frames so background heights land.
pub struct MarkdownTranscript {
    transcript: Transcript,
    messages: HashMap<RowKey, TranscriptMessage>,
    entries: HashMap<RowKey, Entry>,
    /// Owner of every block key handed out, for routing highlights.
    block_rows: HashMap<BlockKey, RowKey>,
    syntax: SyntaxHighlighter,
    keys: BlockKeys,
    background: BackgroundMeasure,
}

impl MarkdownTranscript {
    pub fn new(style: TranscriptStyle) -> Self {
        Self {
            transcript: Transcript::new(style),
            messages: HashMap::new(),
            entries: HashMap::new(),
            block_rows: HashMap::new(),
            syntax: SyntaxHighlighter::new(),
            keys: BlockKeys::new(),
            background: BackgroundMeasure::default(),
        }
    }

    /// Scroll, selection, and geometry state.
    pub fn transcript(&self) -> &Transcript {
        &self.transcript
    }

    /// For scrolling and selection changes. Document edits go through the
    /// facade, which keeps its messages in step with the transcript.
    pub fn transcript_mut(&mut self) -> &mut Transcript {
        &mut self.transcript
    }

    /// The converted messages, as a [`super::TranscriptSource`].
    pub fn messages(&self) -> &HashMap<RowKey, TranscriptMessage> {
        &self.messages
    }

    pub fn highlighter(&self) -> &SyntaxHighlighter {
        &self.syntax
    }

    /// The markdown source of `row`.
    pub fn markdown(&self, row: RowKey) -> Option<&str> {
        self.entries.get(&row).map(|e| e.source.as_str())
    }

    pub fn len(&self) -> usize {
        self.transcript.len()
    }

    pub fn is_empty(&self) -> bool {
        self.transcript.is_empty()
    }

    /// Appends a message at the end.
    pub fn push(&mut self, entry: MarkdownEntry) -> Result<(), RowError> {
        self.extend([entry])
    }

    /// Appends many messages, as when loading history.
    pub fn extend(
        &mut self,
        entries: impl IntoIterator<Item = MarkdownEntry>,
    ) -> Result<(), RowError> {
        let messages = self.adopt(entries)?;
        self.transcript.extend(&messages)?;
        self.store(messages);
        Ok(())
    }

    /// Inserts older messages before the first one; the rows on screen and
    /// the selection stay put.
    pub fn prepend(
        &mut self,
        entries: impl IntoIterator<Item = MarkdownEntry>,
    ) -> Result<(), RowError> {
        let messages = self.adopt(entries)?;
        self.transcript.prepend(&messages)?;
        self.store(messages);
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

    pub fn remove(&mut self, row: RowKey) -> Result<(), RowError> {
        self.transcript.remove(row)?;
        self.messages.remove(&row);
        self.background.forget(row);
        if let Some(mut entry) = self.entries.remove(&row) {
            for key in entry.markdown.keys() {
                self.block_rows.remove(key);
            }
            entry.markdown.clear(&mut self.syntax);
        }
        Ok(())
    }

    /// Takes finished code highlights and rebuilds their messages. Returns
    /// whether any message changed (draw a frame).
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

    /// See [`Transcript::prepare`]. Also applies the row heights measured
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
        let style = *self.transcript.style();
        self.background.configure(measurer, style, width);
        self.background.apply(&mut self.transcript);
        self.transcript
            .prepare(width, height, now_ms, &self.messages, measurer);
        self.background.request(&self.transcript, &self.messages);
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
        self.background.finish(&mut self.transcript, &self.messages)
    }

    #[cfg(test)]
    pub(super) fn background_mut(&mut self) -> (&mut BackgroundMeasure, &mut Transcript) {
        (&mut self.background, &mut self.transcript)
    }

    /// See [`Transcript::element`].
    pub fn element(
        &mut self,
        theme: &Theme,
        on_event: impl Fn(TranscriptEvent) -> Action + 'static,
    ) -> TranscriptElement {
        self.transcript.element(&self.messages, theme, on_event)
    }

    pub fn handle(&mut self, event: TranscriptEvent) {
        self.transcript.handle(event);
    }

    /// See [`Transcript::set_find_query`].
    pub fn set_find_query(&mut self, query: &str) {
        self.transcript.set_find_query(query, &self.messages);
    }

    /// See [`Transcript::find_next`].
    pub fn find_next(&mut self, align: ScrollAlign) -> Option<FindMatch> {
        self.transcript.find_next(align)
    }

    /// See [`Transcript::find_prev`].
    pub fn find_prev(&mut self, align: ScrollAlign) -> Option<FindMatch> {
        self.transcript.find_prev(align)
    }

    /// See [`Transcript::close_find`].
    pub fn close_find(&mut self) {
        self.transcript.close_find();
    }

    /// See [`Transcript::selected_text`].
    pub fn selected_text(&self) -> String {
        self.transcript.selected_text(&self.messages)
    }

    /// Parses and converts new messages without adding them anywhere.
    fn adopt(
        &mut self,
        entries: impl IntoIterator<Item = MarkdownEntry>,
    ) -> Result<Vec<TranscriptMessage>, RowError> {
        let entries: Vec<MarkdownEntry> = entries.into_iter().collect();
        for (i, entry) in entries.iter().enumerate() {
            let repeated = entries[..i].iter().any(|e| e.row == entry.row);
            if repeated || self.entries.contains_key(&entry.row) {
                return Err(RowError::DuplicateKey(entry.row));
            }
        }
        let mut messages = Vec::with_capacity(entries.len());
        for new in entries {
            let mut parser = IncrementalMarkdown::new();
            let doc = parser.parse(&new.markdown);
            let mut entry = Entry {
                role: new.role,
                author: new.author,
                source: new.markdown,
                parser,
                doc,
                markdown: MarkdownMessage::new(),
            };
            messages.push(self.convert(new.row, &mut entry));
            self.entries.insert(new.row, entry);
        }
        Ok(messages)
    }

    fn store(&mut self, messages: Vec<TranscriptMessage>) {
        self.messages
            .extend(messages.into_iter().map(|m| (m.key, m)));
    }

    /// Converts `entry`'s parsed markdown and records who owns its keys.
    fn convert(&mut self, row: RowKey, entry: &mut Entry) -> TranscriptMessage {
        let before = entry.markdown.keys().len();
        // Blocks past the new end give their keys up.
        for key in &entry.markdown.keys()[entry.doc.len().min(before)..] {
            self.block_rows.remove(key);
        }
        let blocks = entry
            .markdown
            .blocks(&entry.doc, &mut self.syntax, &mut self.keys);
        let keys = entry.markdown.keys();
        for key in &keys[before.min(keys.len())..] {
            self.block_rows.insert(*key, row);
        }
        TranscriptMessage {
            key: row,
            role: entry.role,
            author: entry.author.clone(),
            blocks,
        }
    }

    fn refresh(&mut self, row: RowKey) -> Result<(), RowError> {
        let Some(mut entry) = self.entries.remove(&row) else {
            return Err(RowError::UnknownKey(row));
        };
        let message = self.convert(row, &mut entry);
        self.entries.insert(row, entry);
        self.background.forget(row);
        let result = self.transcript.update(&message);
        self.messages.insert(row, message);
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
