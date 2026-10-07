//! A virtualized chat transcript with document-wide text selection.
//!
//! [`Transcript`] is app-owned state: the row heights and scroll model
//! ([`VariableList`]), the document order of every text block
//! ([`BlockOrder`]), the selection, and the geometry of the rows currently
//! materialized. The message text itself stays in the app's model and is
//! read through [`TranscriptSource`].
//!
//! Each frame the app calls [`Transcript::prepare`] with a
//! [`BlockMeasurer`] (normally [`TextMeasurer`] over the frame's shared
//! `LayoutCache`), which measures only the rows in the overscanned window,
//! then builds [`Transcript::element`] from the result. Pointer and wheel
//! input comes back as [`TranscriptEvent`]s in the element's local
//! coordinates, which the app passes to [`Transcript::handle`].
//!
//! Rows outside the window are measured on a background thread when the
//! measurer offers a [`MeasureSpec`] ([`MarkdownTranscript`] does this by
//! default), so their heights become exact without costing the UI thread.
//!
//! Selection endpoints are `(BlockKey, byte)` pairs, so a selection
//! survives its rows scrolling out of the window, history being prepended,
//! and text streaming into the last message.

mod background;
mod element;
mod facade;
mod find;
mod images;
mod markdown;
mod measure;
mod syntax;
#[cfg(test)]
mod tests;

pub use background::MeasureSpec;
pub use element::{TranscriptElement, TranscriptEvent};
pub use facade::{MarkdownEntry, MarkdownTranscript};
pub use find::{FindBarActions, FindMatch, FindState, find_bar};
pub use images::{DecodedImage, ImageLoader, ImageState, ImageStore, LoadedImage};
pub use markdown::{BlockKeys, CODE_SCALE, MarkdownMessage, heading_style};
pub use measure::{TextGeometry, TextMeasurer};
pub use syntax::SyntaxHighlighter;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use quark::selection::{
    BlockKey, BlockOrder, FULL_INTEGRITY_CHECKS, Selection, SelectionPoint, SelectionText,
    count_integrity_steps,
};
use quark_render::FontWeight;
use quark_render::scene::Rect;

use crate::element::{Binding, StyledSpan, join_code_lines};
use crate::theme::Theme;
use crate::virtual_list::{RowError, RowIntegrityError, RowKey, ScrollAlign, VariableList};
use quark::Color;

/// What separates blocks in copied text.
pub const BLOCK_SEPARATOR: &str = "\n\n";
/// What separates a [`BlockStyle::tight`] block from the one before it.
pub const TIGHT_SEPARATOR: &str = "\n";

/// Autoscroll speed per pixel of pointer travel past the edge zone, in
/// pixels per millisecond.
const AUTOSCROLL_GAIN: f32 = 0.02;
/// Fastest autoscroll, in pixels per millisecond.
const AUTOSCROLL_MAX: f32 = 4.0;
/// Longest frame gap autoscroll integrates over, so a stalled frame does
/// not jump the view.
const AUTOSCROLL_MAX_DT_MS: u64 = 50;

/// Height of a rule block, in multiples of its font size.
const RULE_HEIGHT: f32 = 1.0;
/// Height of an image placeholder whose size is unknown, in multiples of
/// the font size.
const IMAGE_PLACEHOLDER_HEIGHT: f32 = 8.0;
/// Width of one quote level's bar column, in multiples of the font size.
const QUOTE_STEP: f32 = 1.0;
/// Width of one list level's marker gutter, in multiples of the font size.
const LIST_STEP: f32 = 1.75;

/// The styled content of one block. The concatenation of the span texts
/// is the plain text that selection offsets index and copy reads.
#[derive(Debug, Clone)]
pub enum BlockContent {
    /// Wrapped text, painted by `SelectableText`.
    Prose(Arc<[StyledSpan]>),
    /// Unwrapped monospace lines, painted by `CodeBlock`, with an optional
    /// label (the fence language) above them. `spans` are the lines joined
    /// by [`join_code_lines`], a plain `\n` span between lines.
    Code {
        spans: Arc<[StyledSpan]>,
        line_count: usize,
        label: Option<Arc<str>>,
    },
    /// A horizontal rule. Its text is `---`, so copy keeps it.
    Rule,
    /// An image scaled to the block's width (never past its own width),
    /// or a placeholder while it loads. The block's text is the alt text,
    /// which copy and assistive tech read.
    Image { src: Arc<str>, state: ImageState },
}

impl BlockContent {
    fn spans(&self) -> Option<&Arc<[StyledSpan]>> {
        match self {
            Self::Prose(spans) | Self::Code { spans, .. } => Some(spans),
            Self::Rule | Self::Image { .. } => None,
        }
    }
}

/// Where a span's color comes from when it depends on the theme. Resolved
/// each time the element is built, so a theme change needs no rebuild of
/// the blocks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum SpanTone {
    /// The span's own `color`, or the block's text color when `None`.
    #[default]
    Plain,
    /// The muted text color (image alt text, table rules).
    Muted,
    /// Inline code: a pill in the element background color behind it.
    InlineCode,
    /// A syntax highlight class.
    Syntax(SyntaxTone),
}

/// Syntax highlight classes, each with its own theme color.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SyntaxTone {
    Keyword,
    String,
    Comment,
    Function,
    Type,
    Number,
    Property,
    Operator,
}

/// The theme colors [`SpanTone`]s resolve to.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Palette {
    muted: Color,
    pill: Color,
    syntax: [Color; 8],
}

impl Palette {
    fn new(theme: &Theme) -> Self {
        let c = &theme.colors;
        Self {
            muted: c.text_muted,
            pill: c.element_background,
            syntax: [
                c.syntax_keyword,
                c.syntax_string,
                c.syntax_comment,
                c.syntax_function,
                c.syntax_type,
                c.syntax_number,
                c.syntax_property,
                c.syntax_operator,
            ],
        }
    }

    fn hash_into(&self, hasher: &mut impl std::hash::Hasher) {
        use std::hash::Hash;
        for c in [self.muted, self.pill].iter().chain(&self.syntax) {
            (c.r, c.g, c.b, c.a).hash(hasher);
        }
    }

    fn paint(&self, span: &StyledSpan, tone: SpanTone) -> StyledSpan {
        let mut span = span.clone();
        match tone {
            SpanTone::Plain => {}
            SpanTone::Muted => span.color = Some(self.muted),
            SpanTone::InlineCode => span.pill = Some(self.pill),
            SpanTone::Syntax(tone) => span.color = Some(self.syntax[tone as usize]),
        }
        span
    }
}

/// How a block sits in its message: size, indent, list marker, quote bars,
/// and the markdown prefixes copy restores. Display never draws the
/// prefixes; the marker is painted in a gutter outside the selectable text.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockStyle {
    /// Font size relative to the transcript's.
    pub scale: f32,
    /// Base weight of prose.
    pub weight: FontWeight,
    /// List nesting depth; each level indents one marker gutter.
    pub list_depth: u8,
    /// Quote nesting depth; each level indents one bar column.
    pub quote_depth: u8,
    /// Drawn right-aligned in the innermost list gutter on the first line.
    pub marker: Option<Arc<str>>,
    /// Paints prose in the muted text color (block quotes).
    pub muted: bool,
    /// Half the block gap above and a single line break before it in
    /// copied text, for consecutive list items.
    pub tight: bool,
    /// Copied before the block's first line when the selection covers the
    /// block's start, as `"- "`, `"> 1. "`, or `"## "`.
    pub copy_prefix: Arc<str>,
    /// Copied after every line break inside the block, as `"> "`.
    pub copy_line_prefix: Arc<str>,
}

impl Default for BlockStyle {
    fn default() -> Self {
        Self {
            scale: 1.0,
            weight: FontWeight::Normal,
            list_depth: 0,
            quote_depth: 0,
            marker: None,
            muted: false,
            tight: false,
            copy_prefix: Arc::from(""),
            copy_line_prefix: Arc::from(""),
        }
    }
}

impl BlockStyle {
    /// Left inset of the block's content, in pixels at `font_size`.
    pub fn inset(&self, font_size: f32) -> f32 {
        (self.quote_depth as f32 * QUOTE_STEP + self.list_depth as f32 * LIST_STEP) * font_size
    }
}

/// A revision no block has had yet. Revisions are process-wide so a block
/// rebuilt under a reused key never repeats an earlier revision.
fn next_revision() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// One selectable text block of a message. Markdown rendering produces a
/// list of these per message.
///
/// Every constructor and `with_` method gives the block a new
/// [`revision`](Self::revision), and clones share it; the element caches a
/// row while the revisions of its blocks stay the same. Change a block by
/// building a new one (or through a `with_` method), not by assigning its
/// fields.
#[derive(Debug, Clone)]
pub struct TranscriptBlock {
    pub key: BlockKey,
    pub content: BlockContent,
    pub style: BlockStyle,
    text: Arc<str>,
    /// One per content span, when any span takes its color from the theme.
    tones: Option<Arc<[SpanTone]>>,
    revision: u64,
}

impl TranscriptBlock {
    pub fn plain(key: BlockKey, text: impl Into<String>) -> Self {
        Self::prose(key, vec![StyledSpan::plain(text)])
    }

    pub fn prose(key: BlockKey, spans: Vec<StyledSpan>) -> Self {
        let text: String = spans.iter().map(|span| span.text.as_str()).collect();
        Self {
            key,
            content: BlockContent::Prose(spans.into()),
            style: BlockStyle::default(),
            text: text.into(),
            tones: None,
            revision: next_revision(),
        }
    }

    /// Prose whose spans take theme colors by [`SpanTone`].
    pub fn toned_prose(key: BlockKey, spans: Vec<(StyledSpan, SpanTone)>) -> Self {
        let (spans, tones): (Vec<StyledSpan>, Vec<SpanTone>) = spans.into_iter().unzip();
        Self::prose(key, spans).with_tones(tones)
    }

    pub fn rule(key: BlockKey) -> Self {
        Self {
            key,
            content: BlockContent::Rule,
            style: BlockStyle::default(),
            text: Arc::from("---"),
            tones: None,
            revision: next_revision(),
        }
    }

    /// An image block showing `src` in `state`, with `alt` as its text.
    pub fn image(key: BlockKey, src: Arc<str>, alt: &str, state: ImageState) -> Self {
        Self {
            key,
            content: BlockContent::Image { src, state },
            style: BlockStyle::default(),
            text: Arc::from(alt),
            tones: None,
            revision: next_revision(),
        }
    }

    pub fn with_style(mut self, style: BlockStyle) -> Self {
        self.style = style;
        self.revision = next_revision();
        self
    }

    /// Sets the label of a code block; other blocks are unchanged.
    pub fn with_label(mut self, label: Option<Arc<str>>) -> Self {
        if let BlockContent::Code { label: slot, .. } = &mut self.content {
            *slot = label.filter(|l| !l.is_empty());
        }
        self.revision = next_revision();
        self
    }

    /// Each inner `Vec` is one source line.
    pub fn code(key: BlockKey, lines: Vec<Vec<StyledSpan>>) -> Self {
        let line_count = lines.len();
        let spans = join_code_lines(&lines);
        let text: String = spans.iter().map(|span| span.text.as_str()).collect();
        Self {
            key,
            content: BlockContent::Code {
                spans: spans.into(),
                line_count,
                label: None,
            },
            style: BlockStyle::default(),
            text: text.into(),
            tones: None,
            revision: next_revision(),
        }
    }

    /// Code whose spans take theme colors by [`SpanTone`].
    pub fn toned_code(key: BlockKey, lines: Vec<Vec<(StyledSpan, SpanTone)>>) -> Self {
        let mut tones = Vec::new();
        let lines = lines
            .into_iter()
            .enumerate()
            .map(|(i, line)| {
                if i > 0 {
                    // The `\n` span join_code_lines puts between lines.
                    tones.push(SpanTone::Plain);
                }
                line.into_iter()
                    .map(|(span, tone)| {
                        tones.push(tone);
                        span
                    })
                    .collect()
            })
            .collect();
        Self::code(key, lines).with_tones(tones)
    }

    /// `tones` holds one tone per content span; a block whose tones are all
    /// plain stores none.
    fn with_tones(mut self, tones: Vec<SpanTone>) -> Self {
        debug_assert_eq!(
            Some(tones.len()),
            self.content.spans().map(|spans| spans.len())
        );
        self.tones = tones
            .iter()
            .any(|tone| *tone != SpanTone::Plain)
            .then(|| tones.into());
        self.revision = next_revision();
        self
    }

    /// The plain text selection and copy operate on.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Changes whenever the block's content or style does; equal for clones.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The tone of each content span, or `None` when all are plain.
    pub fn tones(&self) -> Option<&[SpanTone]> {
        self.tones.as_deref()
    }

    /// Whether `other` lays out exactly like this block: the same shared
    /// content and the same style. Colors do not affect layout.
    fn same_layout(&self, other: &TranscriptBlock) -> bool {
        let content = match (&self.content, &other.content) {
            (BlockContent::Prose(a), BlockContent::Prose(b)) => Arc::ptr_eq(a, b),
            (
                BlockContent::Code {
                    spans: a,
                    label: la,
                    ..
                },
                BlockContent::Code {
                    spans: b,
                    label: lb,
                    ..
                },
            ) => Arc::ptr_eq(a, b) && la.is_some() == lb.is_some(),
            (BlockContent::Rule, BlockContent::Rule) => true,
            (BlockContent::Image { state: a, .. }, BlockContent::Image { state: b, .. }) => {
                image_height_class(a) == image_height_class(b)
            }
            _ => false,
        };
        content && self.style == other.style
    }

    /// The content spans with their tones resolved against `palette`.
    fn painted_spans(&self, palette: &Palette) -> Option<Arc<[StyledSpan]>> {
        let spans = self.content.spans()?;
        Some(match &self.tones {
            None => spans.clone(),
            Some(tones) => spans
                .iter()
                .zip(tones.iter())
                .map(|(span, tone)| palette.paint(span, *tone))
                .collect(),
        })
    }
}

/// What an image block's height depends on: its intrinsic size, or
/// whether it shows a placeholder or its alt text.
fn image_height_class(state: &ImageState) -> (Option<(u32, u32)>, bool) {
    (state.size(), matches!(state, ImageState::Failed))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TranscriptRole {
    User,
    Assistant,
    System,
}

/// One row of the transcript.
#[derive(Debug, Clone)]
pub struct TranscriptMessage {
    pub key: RowKey,
    pub role: TranscriptRole,
    pub author: Arc<str>,
    pub blocks: Vec<TranscriptBlock>,
}

/// The app's message store, read by key.
pub trait TranscriptSource {
    fn message(&self, key: RowKey) -> Option<&TranscriptMessage>;
}

impl TranscriptSource for HashMap<RowKey, TranscriptMessage> {
    fn message(&self, key: RowKey) -> Option<&TranscriptMessage> {
        self.get(&key)
    }
}

/// Layout of a row: `pad_y`, a header line of `header_height` for the
/// author, the blocks separated by `block_gap`, and `pad_y` again. Blocks
/// are inset `pad_x` on both sides.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TranscriptStyle {
    pub font_size: f32,
    pub pad_x: f32,
    pub pad_y: f32,
    pub header_height: f32,
    pub block_gap: f32,
    /// Pixels per wheel line.
    pub line_scroll: f32,
    /// Height of the band at the top and bottom edges where a drag
    /// autoscrolls.
    pub edge: f32,
    /// Extra pixels above and below the viewport that are materialized.
    pub overscan: f32,
}

impl TranscriptStyle {
    /// Proportions for a body font of `font_size` physical pixels.
    pub fn for_font_size(font_size: f32) -> Self {
        Self {
            font_size,
            pad_x: (font_size * 1.2).round(),
            pad_y: (font_size * 0.6).round(),
            header_height: (font_size * 1.6).round(),
            block_gap: (font_size * 0.6).round(),
            line_scroll: (font_size * 3.0).round(),
            edge: (font_size * 2.0).round(),
            overscan: (font_size * 20.0).round(),
        }
    }
}

impl Default for TranscriptStyle {
    fn default() -> Self {
        Self::for_font_size(14.0)
    }
}

/// Measured geometry of one block at one width. Kept between frames for
/// blocks that stay materialized, so it should be cheap to clone.
pub trait BlockGeometry: Clone {
    fn height(&self) -> f32;
    /// Byte offset nearest to a point relative to the block's top left.
    fn hit(&self, x: f32, y: f32) -> usize;

    /// Appends rectangles covering the text in `range`, relative to the
    /// block's top left, for highlights and scrolling to a match. The
    /// default appends none.
    fn range_rects(&self, range: std::ops::Range<usize>, out: &mut Vec<Rect>) {
        let _ = (range, out);
    }
}

/// Measures blocks; [`TextMeasurer`] does it with the shared text layouts
/// the block elements paint.
pub trait BlockMeasurer {
    type Geometry: BlockGeometry;
    fn measure(&mut self, block: &TranscriptBlock, width: f32) -> Self::Geometry;

    /// Identifies the settings geometry depends on besides the block and
    /// width (font size, scale factor). Geometry measured under another key
    /// is measured again.
    fn settings_key(&self) -> u64 {
        0
    }

    /// How a background thread can measure exactly as this measurer does.
    /// `None` (the default) keeps rows outside the window estimated until
    /// they scroll in.
    fn background_spec(&self) -> Option<MeasureSpec> {
        None
    }
}

/// Geometry of a block as measured, with what it was measured from.
#[derive(Debug, Clone)]
struct Measured<G> {
    block: TranscriptBlock,
    width: u32,
    settings: u64,
    geometry: G,
}

/// `block`'s geometry from `cache` when it was measured from the same
/// content, style, width, and measurer settings; otherwise measures it and
/// caches the result.
fn measure_cached<G: BlockGeometry, M: BlockMeasurer<Geometry = G>>(
    cache: &mut HashMap<BlockKey, Measured<G>>,
    measurer: &mut M,
    block: &TranscriptBlock,
    width: f32,
) -> G {
    let settings = measurer.settings_key();
    if let Some(m) = cache.get(&block.key)
        && m.width == width.to_bits()
        && m.settings == settings
        && m.block.same_layout(block)
    {
        return m.geometry.clone();
    }
    let geometry = measurer.measure(block, width);
    cache.insert(
        block.key,
        Measured {
            block: block.clone(),
            width: width.to_bits(),
            settings,
            geometry: geometry.clone(),
        },
    );
    geometry
}

/// A materialized row, in viewport coordinates.
#[derive(Debug, Clone)]
pub struct VisibleRow {
    pub key: RowKey,
    /// Position among all rows.
    pub index: usize,
    pub role: TranscriptRole,
    pub top: f32,
    pub height: f32,
    /// This row's entries in [`Transcript::visible_blocks`].
    pub blocks: std::ops::Range<usize>,
}

/// A materialized block, in viewport coordinates.
#[derive(Debug, Clone)]
pub struct VisibleBlock<G> {
    pub key: BlockKey,
    pub row: RowKey,
    /// Position of the block in its message's `blocks`.
    pub index: usize,
    pub rect: Rect,
    /// Top of the block below its row's top; `rect.y` is the row's top
    /// plus this, so it does not change while the row scrolls.
    pub offset_in_row: f32,
    pub text_len: usize,
    pub geometry: G,
}

#[derive(Debug, Clone, Copy)]
struct Drag {
    /// Latest pointer position in viewport coordinates.
    pointer: (f32, f32),
    /// Clock of the last autoscroll step, while autoscrolling.
    last_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TranscriptIntegrityError {
    Rows(RowIntegrityError),
    /// The blocks of the rows, in row order, are not the block order.
    BlockOrder,
    BlockRow {
        block: BlockKey,
    },
    UnknownRow {
        row: RowKey,
    },
    SelectionOutsideDocument,
}

/// Keyboard commands a transcript responds to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptCommand {
    Copy,
    SelectAll,
    /// Open find (see [`Transcript::set_find_query`] and [`find_bar`]).
    Find,
}

/// The command a pressed key triggers. Bound to `mod+c`, `mod+a`, and
/// `mod+f`, so Cmd and Ctrl both work and one table serves every platform.
pub fn key_command(pressed: &Binding) -> Option<TranscriptCommand> {
    [
        ("mod+c", TranscriptCommand::Copy),
        ("mod+a", TranscriptCommand::SelectAll),
        ("mod+f", TranscriptCommand::Find),
    ]
    .into_iter()
    .find(|(pattern, _)| pattern.parse::<Binding>().is_ok_and(|p| p.matches(pressed)))
    .map(|(_, command)| command)
}

/// Transcript state. `G` is the block geometry the measurer produces.
#[derive(Debug, Clone)]
pub struct Transcript<G = TextGeometry> {
    list: VariableList,
    order: BlockOrder,
    block_row: HashMap<BlockKey, RowKey>,
    row_blocks: HashMap<RowKey, Vec<BlockKey>>,
    selection: Option<Selection>,
    drag: Option<Drag>,
    unseen: bool,
    style: TranscriptStyle,
    size: (f32, f32),
    rows: Vec<VisibleRow>,
    blocks: Vec<VisibleBlock<G>>,
    /// Geometry of the blocks measured for the current window; materialize
    /// reuses it instead of measuring every visible block every frame.
    measured: HashMap<BlockKey, Measured<G>>,
    /// Last frame's `measured` map, empty, kept for its capacity.
    measured_spare: HashMap<BlockKey, Measured<G>>,
    /// Theme-resolved spans of the blocks the last element painted, and
    /// last frame's map kept for its capacity.
    painted: HashMap<BlockKey, element::PaintedSpans>,
    painted_spare: HashMap<BlockKey, element::PaintedSpans>,
    /// What the last element built each row's cached subtree from, by the
    /// hash of its inputs, and last frame's map kept for its capacity.
    row_builds: HashMap<RowKey, element::RowEntry>,
    row_builds_spare: HashMap<RowKey, element::RowEntry>,
    /// The find query and its matches, while find is open.
    find: Option<FindState>,
    /// The document changed since find last scanned it.
    find_stale: bool,
    /// A match to bring into view once the next prepare has its geometry.
    reveal: Option<Reveal>,
}

#[derive(Debug, Clone)]
struct Reveal {
    block: BlockKey,
    range: std::ops::Range<usize>,
    align: ScrollAlign,
    /// The range was out of view: align it even once the row scroll has
    /// brought it into view.
    align_always: bool,
}

impl<G: BlockGeometry> Transcript<G> {
    pub fn new(style: TranscriptStyle) -> Self {
        let estimate = style.pad_y * 2.0 + style.header_height + style.font_size * 2.0;
        Self {
            list: VariableList::new(estimate, 0.0),
            order: BlockOrder::new(),
            block_row: HashMap::new(),
            row_blocks: HashMap::new(),
            selection: None,
            drag: None,
            unseen: false,
            style,
            size: (0.0, 0.0),
            rows: Vec::new(),
            blocks: Vec::new(),
            measured: HashMap::new(),
            measured_spare: HashMap::new(),
            painted: HashMap::new(),
            painted_spare: HashMap::new(),
            row_builds: HashMap::new(),
            row_builds_spare: HashMap::new(),
            find: None,
            find_stale: false,
            reveal: None,
        }
    }

    pub fn style(&self) -> &TranscriptStyle {
        &self.style
    }

    pub fn len(&self) -> usize {
        self.list.rows().len()
    }

    pub fn is_empty(&self) -> bool {
        self.list.rows().is_empty()
    }

    pub fn list(&self) -> &VariableList {
        &self.list
    }

    // -- Document changes --

    /// Appends a message at the end.
    pub fn push(&mut self, message: &TranscriptMessage) -> Result<(), RowError> {
        self.list.append(message.key)?;
        let blocks = message
            .blocks
            .iter()
            .map(|block| block.key)
            .filter(|key| self.order.append(*key))
            .collect();
        self.adopt(message.key, blocks);
        self.mark_new_content();
        self.debug_check_row(message.key);
        Ok(())
    }

    /// Appends many messages as one batch, checking integrity once.
    pub fn extend<'a>(
        &mut self,
        messages: impl IntoIterator<Item = &'a TranscriptMessage>,
    ) -> Result<(), RowError> {
        let messages: Vec<&TranscriptMessage> = messages.into_iter().collect();
        let keys: Vec<RowKey> = messages.iter().map(|m| m.key).collect();
        self.list.extend(&keys)?;
        let mut fresh = Vec::new();
        let mut seen = HashSet::new();
        for message in messages {
            let blocks: Vec<BlockKey> = message
                .blocks
                .iter()
                .map(|block| block.key)
                .filter(|key| !self.order.contains(*key) && seen.insert(*key))
                .collect();
            fresh.extend_from_slice(&blocks);
            self.adopt(message.key, blocks);
        }
        self.order.extend(fresh);
        self.mark_new_content();
        self.debug_check();
        Ok(())
    }

    /// Inserts older messages before the first one. The rows on screen and
    /// the selection stay where they are.
    pub fn prepend<'a>(
        &mut self,
        messages: impl IntoIterator<Item = &'a TranscriptMessage>,
    ) -> Result<(), RowError> {
        let messages: Vec<&TranscriptMessage> = messages.into_iter().collect();
        let keys: Vec<RowKey> = messages.iter().map(|m| m.key).collect();
        self.list.prepend(&keys)?;
        let mut fresh = Vec::new();
        let mut seen = HashSet::new();
        for message in &messages {
            let blocks: Vec<BlockKey> = message
                .blocks
                .iter()
                .map(|block| block.key)
                .filter(|key| !self.order.contains(*key) && seen.insert(*key))
                .collect();
            fresh.extend_from_slice(&blocks);
            self.adopt(message.key, blocks);
        }
        self.order.prepend(fresh);
        self.debug_check();
        Ok(())
    }

    /// The message's blocks or text changed, as while streaming. New
    /// blocks join the document order, removed ones leave it (shrinking the
    /// selection inward), and the row is remeasured on the next prepare.
    pub fn update(&mut self, message: &TranscriptMessage) -> Result<(), RowError> {
        let key = message.key;
        let old = self
            .row_blocks
            .remove(&key)
            .ok_or(RowError::UnknownKey(key))?;
        let wanted: HashSet<BlockKey> = message.blocks.iter().map(|b| b.key).collect();
        for block in old.iter().filter(|b| !wanted.contains(b)) {
            self.forget_block(*block);
        }
        let mut kept: Vec<BlockKey> = Vec::with_capacity(message.blocks.len());
        for block in message.blocks.iter().map(|b| b.key) {
            if self.block_row.get(&block) == Some(&key) {
                kept.push(block);
                continue;
            }
            if self.order.contains(block) {
                // Owned by another row; a block belongs to one row.
                continue;
            }
            let inserted = match kept.last().copied().or_else(|| self.last_block_before(key)) {
                Some(after) => self.order.insert_after(after, block),
                None => self.order.prepend([block]) == 1,
            };
            if inserted {
                kept.push(block);
            }
        }
        self.adopt(key, kept);
        self.list.invalidate(key)?;
        // Edits to older messages (a highlight arriving, a status change)
        // are not new content to jump to.
        if self.list.rows().keys().last() == Some(&key) {
            self.mark_new_content();
        }
        self.debug_check_row(key);
        Ok(())
    }

    pub fn remove(&mut self, key: RowKey) -> Result<(), RowError> {
        self.list.remove(key)?;
        for block in self.row_blocks.remove(&key).unwrap_or_default() {
            self.forget_block(block);
        }
        self.debug_check();
        Ok(())
    }

    fn adopt(&mut self, row: RowKey, blocks: Vec<BlockKey>) {
        self.find_stale = true;
        for block in &blocks {
            self.block_row.insert(*block, row);
        }
        self.row_blocks.insert(row, blocks);
    }

    fn forget_block(&mut self, block: BlockKey) {
        self.find_stale = true;
        self.block_row.remove(&block);
        let Some(pos) = self.order.remove(block) else {
            return;
        };
        // No text source here: an endpoint that moves to the end of the
        // previous block gets `usize::MAX`, which selection reads as the
        // end of that block.
        self.selection = self
            .selection
            .and_then(|s| s.after_remove(block, pos, &self.order, &NoText));
    }

    /// Whether `block` is in the document as a block of `row`. A message can
    /// list blocks that are not: keys another row owns already, or repeats.
    /// Those are neither measured, drawn, nor selectable.
    fn owns(&self, row: RowKey, block: BlockKey) -> bool {
        self.block_row.get(&block) == Some(&row)
    }

    /// Last block of the nearest earlier row that has blocks.
    fn last_block_before(&self, row: RowKey) -> Option<BlockKey> {
        let index = self.list.rows().index_of(row)?;
        self.list.rows().keys()[..index]
            .iter()
            .rev()
            .find_map(|key| self.row_blocks.get(key)?.last().copied())
    }

    fn mark_new_content(&mut self) {
        if !self.list.is_stuck_to_bottom() {
            self.unseen = true;
        }
    }

    // -- Scrolling --

    pub fn scroll_offset(&self) -> f32 {
        self.list.scroll_offset()
    }

    pub fn max_scroll_offset(&self) -> f32 {
        self.list.max_scroll_offset()
    }

    /// A user scroll; landing at the bottom pins the view there.
    pub fn set_scroll_offset(&mut self, offset: f32) -> f32 {
        let offset = self.list.set_scroll_offset(offset);
        if self.list.is_stuck_to_bottom() {
            self.unseen = false;
        }
        offset
    }

    pub fn scroll_by(&mut self, delta: f32) -> f32 {
        self.set_scroll_offset(self.scroll_offset() + delta)
    }

    pub fn is_stuck_to_bottom(&self) -> bool {
        self.list.is_stuck_to_bottom()
    }

    /// New content arrived while the view was scrolled up; show a "jump to
    /// latest" affordance.
    pub fn has_unseen(&self) -> bool {
        self.unseen
    }

    /// Scrolls to the newest content and pins there.
    pub fn jump_to_latest(&mut self) {
        self.set_scroll_offset(f32::MAX);
    }

    // -- Selection --

    pub fn selection(&self) -> Option<Selection> {
        self.selection
    }

    pub fn set_selection(&mut self, selection: Option<Selection>) {
        self.selection = selection
            .filter(|s| self.order.contains(s.anchor.block) && self.order.contains(s.focus.block));
    }

    /// Selects every block from the start of the first to the end of the
    /// last, including text that streams into the last one later.
    pub fn select_all(&mut self) {
        let keys = self.order.keys();
        if let (Some(first), Some(last)) = (keys.first(), keys.last()) {
            self.selection = Some(Selection::new(
                SelectionPoint::new(*first, 0),
                SelectionPoint::new(*last, usize::MAX),
            ));
        }
    }

    /// The selected text from the app's model, blocks joined with blank
    /// lines. Empty when nothing is selected. A block whose start is
    /// selected gets its [`BlockStyle::copy_prefix`], and every line break
    /// inside a block its `copy_line_prefix`, so list markers, heading
    /// hashes, and quote marks survive the copy.
    pub fn selected_text(&self, source: &impl TranscriptSource) -> String {
        let mut out = String::new();
        let Some((start, end)) = self.selection.and_then(|s| s.ordered(&self.order)) else {
            return out;
        };
        if start == end {
            return out;
        }
        let texts = SourceText {
            source,
            block_row: &self.block_row,
        };
        let keys = self.order.range(start.block, end.block);
        let last = keys.len().saturating_sub(1);
        for (i, key) in keys.iter().enumerate() {
            let Some(block) = texts.block(*key) else {
                continue;
            };
            let text = block.text();
            let from = if i == 0 {
                text.floor_char_boundary(start.byte)
            } else {
                0
            };
            let to = if i == last {
                text.floor_char_boundary(end.byte)
            } else {
                text.len()
            };
            if i > 0 {
                out.push_str(if block.style.tight {
                    TIGHT_SEPARATOR
                } else {
                    BLOCK_SEPARATOR
                });
            }
            if from >= to {
                continue;
            }
            if from == 0 {
                out.push_str(&block.style.copy_prefix);
            }
            let line_prefix = &block.style.copy_line_prefix;
            for (n, line) in text[from..to].split('\n').enumerate() {
                if n > 0 {
                    out.push('\n');
                    out.push_str(line_prefix);
                }
                out.push_str(line);
            }
        }
        out
    }

    /// Selected byte range within `block`, clamped to `len`, or `None`
    /// when the block is outside the selection.
    pub fn block_selection(&self, block: BlockKey, len: usize) -> Option<(usize, usize)> {
        let (start, end) = self.selection?.ordered(&self.order)?;
        let pos = self.order.position(block)?;
        let start_pos = self.order.position(start.block)?;
        let end_pos = self.order.position(end.block)?;
        if pos < start_pos || pos > end_pos {
            return None;
        }
        let lo = if block == start.block { start.byte } else { 0 };
        let hi = if block == end.block { end.byte } else { len };
        let (lo, hi) = (lo.min(len), hi.min(len));
        (lo < hi).then_some((lo, hi))
    }

    // -- Find --

    /// Opens find with `query`, or changes its query, and scans the
    /// document. The current match is the first one; call
    /// [`Self::find_next`] to scroll to it. Matches stay current as the
    /// document changes: each prepare rescans the blocks that changed.
    pub fn set_find_query(&mut self, query: &str, source: &impl TranscriptSource) {
        self.find
            .get_or_insert_with(FindState::default)
            .set_query(query);
        self.refresh_find(source);
    }

    /// Closes find; highlights go away on the next element.
    pub fn close_find(&mut self) {
        self.find = None;
        self.reveal = None;
    }

    /// The open find, if any.
    pub fn find(&self) -> Option<&FindState> {
        self.find.as_ref()
    }

    /// Moves to the next match, wrapping past the last, and scrolls it into
    /// view at `align` on the next prepare.
    pub fn find_next(&mut self, align: ScrollAlign) -> Option<FindMatch> {
        let found = self.find.as_mut()?.next_match().cloned()?;
        self.reveal(
            found.block,
            found.range.start.get()..found.range.end.get(),
            align,
        );
        Some(found)
    }

    /// Scrolls the current match into view at `align` on the next prepare,
    /// as after typing into a find field.
    pub fn reveal_current_match(&mut self, align: ScrollAlign) -> Option<FindMatch> {
        let found = self.find.as_ref()?.current().cloned()?;
        self.reveal(
            found.block,
            found.range.start.get()..found.range.end.get(),
            align,
        );
        Some(found)
    }

    /// Moves to the previous match, wrapping before the first, and scrolls
    /// it into view at `align` on the next prepare.
    pub fn find_prev(&mut self, align: ScrollAlign) -> Option<FindMatch> {
        let found = self.find.as_mut()?.prev_match().cloned()?;
        self.reveal(
            found.block,
            found.range.start.get()..found.range.end.get(),
            align,
        );
        Some(found)
    }

    /// Scrolls so bytes `range` of `block` sit at `align` in the viewport,
    /// unless they are already in full view. The block's row is scrolled
    /// to now; the next prepare, which has the block's geometry, places the
    /// range itself.
    pub fn reveal(&mut self, block: BlockKey, range: std::ops::Range<usize>, align: ScrollAlign) {
        let Some(row) = self.block_row.get(&block).copied() else {
            return;
        };
        let in_view = self
            .blocks
            .iter()
            .find(|b| b.key == block)
            .and_then(|b| self.range_extent(b, range.clone()))
            .is_some_and(|(top, bottom)| top >= 0.0 && bottom <= self.size.1);
        if !in_view {
            let _ = self.list.scroll_to(row, align);
            if self.list.is_stuck_to_bottom() {
                self.unseen = false;
            }
        }
        self.reveal = Some(Reveal {
            block,
            range,
            align,
            align_always: !in_view,
        });
    }

    /// Top and bottom of `range` of a materialized block, in viewport
    /// coordinates.
    fn range_extent(
        &self,
        block: &VisibleBlock<G>,
        range: std::ops::Range<usize>,
    ) -> Option<(f32, f32)> {
        let mut rects = Vec::new();
        block.geometry.range_rects(range, &mut rects);
        let top = rects.iter().map(|r| r.y).reduce(f32::min)?;
        let bottom = rects.iter().map(|r| r.y + r.height).reduce(f32::max)?;
        Some((block.rect.y + top, block.rect.y + bottom))
    }

    /// Rescans the blocks that changed since the last scan.
    fn refresh_find(&mut self, source: &impl TranscriptSource) {
        self.find_stale = false;
        let Some(find) = &mut self.find else {
            return;
        };
        let block_row = &self.block_row;
        let blocks = self.list.rows().keys().iter().flat_map(|row| {
            owned_blocks(source, block_row, *row)
                .map(|(_, block)| (block.key, block.revision(), block.text()))
        });
        find.update(blocks);
    }

    /// Places a pending reveal now that the window has geometry.
    fn place_reveal<M: BlockMeasurer<Geometry = G>>(
        &mut self,
        source: &impl TranscriptSource,
        measurer: &mut M,
    ) {
        let Some(reveal) = self.reveal.take() else {
            return;
        };
        // Measuring rows that scroll in can move the range again; two
        // passes settle it.
        let mut align_always = reveal.align_always;
        for _ in 0..2 {
            let Some((top, bottom)) = self
                .blocks
                .iter()
                .find(|b| b.key == reveal.block)
                .and_then(|b| self.range_extent(b, reveal.range.clone()))
            else {
                return;
            };
            let height = self.size.1;
            let delta = match reveal.align {
                ScrollAlign::Top => top,
                ScrollAlign::Center => (top + bottom - height) * 0.5,
                ScrollAlign::Bottom => bottom - height,
            };
            let in_view = top >= 0.0 && bottom <= height;
            if (in_view && !align_always) || delta.abs() < 0.5 {
                return;
            }
            align_always = false;
            self.scroll_by(delta);
            self.measure_window(source, measurer);
            self.materialize(source, measurer);
        }
    }

    // -- Input --

    /// Applies one input event from the element. Coordinates are relative
    /// to the transcript's top left.
    pub fn handle(&mut self, event: TranscriptEvent) {
        match event {
            TranscriptEvent::PointerDown { x, y } => {
                self.drag = Some(Drag {
                    pointer: (x, y),
                    last_ms: None,
                });
                self.selection = self.point_at(x, y).map(Selection::collapsed);
            }
            TranscriptEvent::PointerDrag { x, y } => {
                if let Some(drag) = &mut self.drag {
                    drag.pointer = (x, y);
                }
                self.extend_selection_to(x, y);
            }
            TranscriptEvent::PointerUp => self.drag = None,
            TranscriptEvent::Wheel(lines) => {
                self.scroll_by(lines as f32 * self.style.line_scroll);
            }
            TranscriptEvent::JumpToLatest => self.jump_to_latest(),
        }
    }

    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// A drag is autoscrolling; draw another frame.
    pub fn wants_frame(&self) -> bool {
        self.drag
            .is_some_and(|drag| self.autoscroll_velocity(drag.pointer.1) != 0.0)
    }

    fn extend_selection_to(&mut self, x: f32, y: f32) {
        let Some(point) = self.point_at(x, y) else {
            return;
        };
        self.selection = Some(match self.selection {
            Some(selection) => Selection::new(selection.anchor, point),
            None => Selection::collapsed(point),
        });
    }

    /// Signed autoscroll speed in pixels per millisecond for a pointer at
    /// `y`: zero inside the viewport away from the edges, growing with the
    /// distance into or past an edge band.
    fn autoscroll_velocity(&self, y: f32) -> f32 {
        let (edge, height) = (self.style.edge, self.size.1);
        let past = if y < edge {
            y - edge
        } else if y > height - edge {
            y - (height - edge)
        } else {
            0.0
        };
        (past * AUTOSCROLL_GAIN).clamp(-AUTOSCROLL_MAX, AUTOSCROLL_MAX)
    }

    /// Selection point under a viewport point: the nearest visible block
    /// vertically, the start of it when above, the end when below, else a
    /// hit test of its layout. Points outside the viewport clamp to it.
    pub fn point_at(&self, x: f32, y: f32) -> Option<SelectionPoint> {
        let y = y.clamp(0.0, (self.size.1 - 1.0).max(0.0));
        // Edits since the last prepare may have taken materialized blocks
        // out of the document; a point must never land in one.
        let block = self
            .blocks
            .iter()
            .filter(|b| self.owns(b.row, b.key))
            .min_by(|a, b| {
                vertical_distance(&a.rect, y).total_cmp(&vertical_distance(&b.rect, y))
            })?;
        let rect = block.rect;
        let byte = if y < rect.y {
            0
        } else if y >= rect.y + rect.height {
            block.text_len
        } else {
            block.geometry.hit(x - rect.x, y - rect.y)
        };
        Some(SelectionPoint::new(block.key, byte))
    }

    // -- Frame --

    /// Advances autoscroll to `now_ms`, measures the rows entering the
    /// window at `width`, and rebuilds the materialized rows. A width
    /// change remeasures every row as it becomes visible.
    pub fn prepare<M: BlockMeasurer<Geometry = G>>(
        &mut self,
        width: f32,
        height: f32,
        now_ms: u64,
        source: &impl TranscriptSource,
        measurer: &mut M,
    ) {
        if height != self.size.1 {
            self.list.set_viewport_height(height);
        }
        self.size = (width, height);
        self.autoscroll(now_ms);

        self.measure_window(source, measurer);
        if self.list.is_stuck_to_bottom() {
            self.unseen = false;
        }
        self.materialize(source, measurer);
        if self.find_stale {
            self.refresh_find(source);
        }
        self.place_reveal(source, measurer);

        // Rows moved under a held pointer (autoscroll, streaming); keep the
        // selection end under it.
        if let Some(drag) = self.drag {
            self.extend_selection_to(drag.pointer.0, drag.pointer.1);
        }
    }

    /// Measures the unmeasured rows of the overscanned window.
    fn measure_window<M: BlockMeasurer<Geometry = G>>(
        &mut self,
        source: &impl TranscriptSource,
        measurer: &mut M,
    ) {
        let style = self.style;
        let width = self.size.0;
        let block_row = &self.block_row;
        let cache = &mut self.measured;
        self.list
            .measure_visible(width, style.overscan, |key, width| {
                let blocks = owned_blocks(source, block_row, RowKey(key)).map(|(_, b)| b);
                row_height(&style, width, blocks, |block, block_width| {
                    measure_cached(cache, measurer, block, block_width).height()
                })
            });
    }

    fn autoscroll(&mut self, now_ms: u64) {
        let Some(mut drag) = self.drag else {
            return;
        };
        let velocity = self.autoscroll_velocity(drag.pointer.1);
        if velocity == 0.0 {
            drag.last_ms = None;
        } else {
            if let Some(last) = drag.last_ms {
                let dt = now_ms.saturating_sub(last).min(AUTOSCROLL_MAX_DT_MS);
                self.scroll_by(velocity * dt as f32);
            }
            drag.last_ms = Some(now_ms);
        }
        self.drag = Some(drag);
    }

    fn materialize<M: BlockMeasurer<Geometry = G>>(
        &mut self,
        source: &impl TranscriptSource,
        measurer: &mut M,
    ) {
        let style = self.style;
        let block_width = block_width(&style, self.size.0);
        let window = self.list.window(style.overscan);
        let scroll = self.list.scroll_offset();
        let rows = self.list.rows();
        self.rows.clear();
        self.blocks.clear();
        let mut kept = std::mem::take(&mut self.measured_spare);
        for index in window.range {
            let key = rows.keys()[index];
            let top = rows.offset_of_index(index) - scroll;
            let height = rows.height_of(key).unwrap_or(0.0);
            let message = source.message(key);
            let first = self.blocks.len();
            let mut y = style.pad_y + style.header_height;
            for (n, (i, block)) in owned_blocks(source, &self.block_row, key).enumerate() {
                y += gap_before(&style, n, block);
                let geometry = measure_cached(&mut self.measured, measurer, block, block_width);
                if let Some(entry) = self.measured.remove(&block.key) {
                    kept.insert(block.key, entry);
                }
                let block_height = geometry.height();
                self.blocks.push(VisibleBlock {
                    key: block.key,
                    row: key,
                    index: i,
                    rect: Rect {
                        x: style.pad_x,
                        y: top + y,
                        width: block_width,
                        height: block_height,
                    },
                    offset_in_row: y,
                    text_len: block.text().len(),
                    geometry,
                });
                y += block_height;
            }
            self.rows.push(VisibleRow {
                key,
                index,
                role: message.map_or(TranscriptRole::System, |m| m.role),
                top,
                height,
                blocks: first..self.blocks.len(),
            });
        }
        // Only the window's geometry is kept.
        self.measured_spare = std::mem::replace(&mut self.measured, kept);
        self.measured_spare.clear();
    }

    /// Rows materialized by the last prepare, top to bottom.
    pub fn visible_rows(&self) -> &[VisibleRow] {
        &self.rows
    }

    /// Blocks of the materialized rows, in document order.
    pub fn visible_blocks(&self) -> &[VisibleBlock<G>] {
        &self.blocks
    }

    pub fn viewport_size(&self) -> (f32, f32) {
        self.size
    }

    // -- Background measurement --

    /// Up to `limit` rows still holding an estimate, nearest to the window
    /// first, alternating above and below it, skipping rows `skip` names.
    /// `skipped` is how many unmeasured rows `skip` can name at most; when
    /// every unmeasured row is among them the scan is skipped.
    fn unmeasured_near_window(
        &self,
        limit: usize,
        skipped: usize,
        skip: impl Fn(RowKey) -> bool,
    ) -> Vec<RowKey> {
        let rows = self.list.rows();
        let mut found = Vec::new();
        if limit == 0 || rows.len() - rows.measured_count() <= skipped {
            return found;
        }
        let window = self.list.window(self.style.overscan).range;
        let (mut above, mut below) = (window.start, window.start);
        let take = |index: usize, found: &mut Vec<RowKey>| {
            let key = rows.keys()[index];
            if !rows.is_measured_at(index) && !skip(key) {
                found.push(key);
            }
        };
        while found.len() < limit && (above > 0 || below < rows.len()) {
            if below < rows.len() {
                take(below, &mut found);
                below += 1;
            }
            if above > 0 && found.len() < limit {
                above -= 1;
                take(above, &mut found);
            }
        }
        found
    }

    /// The blocks `row` lays out, as a snapshot another thread can measure.
    fn row_snapshot(&self, source: &impl TranscriptSource, row: RowKey) -> Vec<TranscriptBlock> {
        owned_blocks(source, &self.block_row, row)
            .map(|(_, block)| block.clone())
            .collect()
    }

    /// Records a height measured off the UI thread for a row still holding
    /// an estimate. The first visible row keeps its place, or the view stays
    /// pinned to the bottom. Returns whether the height was taken.
    fn set_background_height(&mut self, row: RowKey, height: f32) -> bool {
        if self.list.rows().is_measured(row) != Some(false) {
            return false;
        }
        self.list.set_height(row, height).is_ok()
    }

    /// A copy that builds its next element from scratch, for comparing
    /// cached frames against fresh ones.
    #[cfg(test)]
    fn without_element_memory(&self) -> Self {
        let mut copy = self.clone();
        copy.row_builds.clear();
        copy.painted.clear();
        copy
    }

    // -- Integrity --

    /// Checks row `row` only: its blocks are owned by it and sit together
    /// in the block order, right after the previous row's blocks and right
    /// before the next row's, plus the map sizes and the selection. O(blocks
    /// of the row) plus the rows without blocks around it; per-message
    /// edits call it through `debug_assert!`.
    pub fn verify_row(&self, row: RowKey) -> Result<(), TranscriptIntegrityError> {
        let rows = self.list.rows();
        let index = rows
            .index_of(row)
            .ok_or(TranscriptIntegrityError::UnknownRow { row })?;
        rows.verify_row(index)
            .map_err(TranscriptIntegrityError::Rows)?;
        if self.row_blocks.len() != rows.len() || self.block_row.len() != self.order.len() {
            return Err(TranscriptIntegrityError::BlockOrder);
        }
        let blocks = self
            .row_blocks
            .get(&row)
            .ok_or(TranscriptIntegrityError::UnknownRow { row })?;
        count_integrity_steps(blocks.len());
        let mut expected = self
            .last_block_before(row)
            .and_then(|b| self.order.position(b))
            .map_or(0, |p| p + 1);
        for block in blocks {
            if self.block_row.get(block) != Some(&row) {
                return Err(TranscriptIntegrityError::BlockRow { block: *block });
            }
            if self.order.position(*block) != Some(expected) {
                return Err(TranscriptIntegrityError::BlockOrder);
            }
            expected += 1;
        }
        let next_first = rows.keys()[index + 1..]
            .iter()
            .find_map(|key| self.row_blocks.get(key)?.first().copied());
        let next_pos = next_first.map_or(Some(self.order.len() as u32), |b| self.order.position(b));
        if next_pos != Some(expected) {
            return Err(TranscriptIntegrityError::BlockOrder);
        }
        self.verify_selection()
    }

    fn verify_selection(&self) -> Result<(), TranscriptIntegrityError> {
        match self.selection {
            Some(selection) if selection.ordered(&self.order).is_none() => {
                Err(TranscriptIntegrityError::SelectionOutsideDocument)
            }
            _ => Ok(()),
        }
    }

    /// Checks every row, block, and the selection. O(rows + blocks); batch
    /// edits call it through `debug_assert!`, as do per-message ones with
    /// `integrity-checks`.
    pub fn verify_integrity(&self) -> Result<(), TranscriptIntegrityError> {
        count_integrity_steps(self.order.len());
        self.list
            .rows()
            .verify_integrity()
            .map_err(TranscriptIntegrityError::Rows)?;
        let rows = self.list.rows().keys();
        if self.row_blocks.len() != rows.len() {
            return Err(TranscriptIntegrityError::BlockOrder);
        }
        // Walk the rows' blocks against the order in place, without
        // collecting them.
        let order = self.order.keys();
        let mut at = 0;
        for row in rows {
            let blocks = self
                .row_blocks
                .get(row)
                .ok_or(TranscriptIntegrityError::UnknownRow { row: *row })?;
            for block in blocks {
                if self.block_row.get(block) != Some(row) {
                    return Err(TranscriptIntegrityError::BlockRow { block: *block });
                }
                if order.get(at) != Some(block) {
                    return Err(TranscriptIntegrityError::BlockOrder);
                }
                at += 1;
            }
        }
        if at != order.len() || self.block_row.len() != at {
            return Err(TranscriptIntegrityError::BlockOrder);
        }
        self.verify_selection()
    }

    fn debug_check(&self) {
        debug_assert_eq!(self.verify_integrity(), Ok(()));
    }

    fn debug_check_row(&self, row: RowKey) {
        if FULL_INTEGRITY_CHECKS {
            self.debug_check();
        } else {
            debug_assert_eq!(self.verify_row(row), Ok(()));
        }
    }
}

/// The blocks of `row`'s message that the document holds as `row`'s, with
/// their positions in the message.
fn owned_blocks<'a>(
    source: &'a impl TranscriptSource,
    block_row: &'a HashMap<BlockKey, RowKey>,
    row: RowKey,
) -> impl Iterator<Item = (usize, &'a TranscriptBlock)> + 'a {
    source
        .message(row)
        .map_or(&[][..], |m| m.blocks.as_slice())
        .iter()
        .enumerate()
        .filter(move |(_, block)| block_row.get(&block.key) == Some(&row))
}

/// Height of a row of `width` holding `blocks`, given each block's height
/// at the blocks' width. The UI thread and the background measurer both
/// use it, so their heights agree to the bit.
fn row_height<'a>(
    style: &TranscriptStyle,
    width: f32,
    blocks: impl Iterator<Item = &'a TranscriptBlock>,
    mut block_height: impl FnMut(&TranscriptBlock, f32) -> f32,
) -> f32 {
    let block_width = block_width(style, width);
    let mut height = style.pad_y * 2.0 + style.header_height;
    for (n, block) in blocks.enumerate() {
        height += gap_before(style, n, block);
        height += block_height(block, block_width);
    }
    height
}

/// Space above the `index`-th block of a message.
fn gap_before(style: &TranscriptStyle, index: usize, block: &TranscriptBlock) -> f32 {
    match index {
        0 => 0.0,
        _ if block.style.tight => (style.block_gap * 0.5).round(),
        _ => style.block_gap,
    }
}

fn block_width(style: &TranscriptStyle, width: f32) -> f32 {
    (width - style.pad_x * 2.0).max(1.0)
}

fn vertical_distance(rect: &Rect, y: f32) -> f32 {
    if y < rect.y {
        rect.y - y
    } else if y >= rect.y + rect.height {
        y - (rect.y + rect.height)
    } else {
        0.0
    }
}

/// Block text read from the app's model by way of the owning row.
struct SourceText<'a, S> {
    source: &'a S,
    block_row: &'a HashMap<BlockKey, RowKey>,
}

impl<'a, S: TranscriptSource> SourceText<'a, S> {
    fn block(&self, key: BlockKey) -> Option<&'a TranscriptBlock> {
        let row = self.block_row.get(&key)?;
        self.source
            .message(*row)?
            .blocks
            .iter()
            .find(|block| block.key == key)
    }
}

struct NoText;

impl SelectionText for NoText {
    fn text(&self, _key: BlockKey) -> Option<&str> {
        None
    }
}
