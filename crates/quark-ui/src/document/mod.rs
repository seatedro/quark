//! A virtualized document of rows of text blocks, with document-wide text
//! selection, find, images, and background measuring. Chat transcripts,
//! logs, and long markdown views are all built on it.
//!
//! [`Document`] is app-owned state: the row heights and scroll model
//! ([`VariableList`]), the document order of every text block
//! ([`BlockOrder`]), the selection, and the geometry of the rows currently
//! materialized. The row content itself stays in the app's model and is
//! read through [`DocumentSource`].
//!
//! The document draws only blocks. Anything around them (a chat message's
//! author line, a tinted background per role) is row chrome: each row's
//! [`RowChrome`] reserves a header band and carries app data, and the
//! app's [`RowDecorator`] draws the header and background from it.
//!
//! Each frame the app calls [`Document::prepare`] with a
//! [`BlockMeasurer`] (normally [`TextMeasurer`] over the frame's shared
//! `LayoutCache`), which measures only the rows in the overscanned window,
//! then builds [`Document::element`] from the result. Pointer and wheel
//! input comes back as [`DocumentEvent`]s in the element's local
//! coordinates, which the app passes to [`Document::handle`].
//!
//! Rows outside the window are measured on a background thread when the
//! measurer offers a [`MeasureSpec`] ([`MarkdownDocument`] does this by
//! default), so their heights become exact without costing the UI thread.
//!
//! Selection endpoints are `(BlockKey, byte)` pairs, so a selection
//! survives its rows scrolling out of the window, history being prepended,
//! and text streaming into the last row.

mod adornment;
mod background;
mod element;
mod facade;
mod find;
mod images;
mod markdown;
mod measure;
mod syntax;
mod table;
#[cfg(test)]
mod tests;

pub use adornment::{
    AdornmentAccessibility, AdornmentCx, AdornmentKey, AdornmentSlot, RowAdornment,
};
pub use background::MeasureSpec;

use adornment::AdornmentShape;
pub use element::{CopyCode, DocumentElement, DocumentEvent};
pub use facade::{MarkdownDocument, MarkdownEntry};
pub use find::{FindBarActions, FindIntegrityError, FindMatch, FindState, find_bar};
pub use images::{DecodedImage, ImageLoader, ImageState, ImageStore, LoadedImage};
pub use markdown::{BlockKeys, CODE_SCALE, MarkdownBlocks, heading_style};
pub use measure::{TextGeometry, TextMeasurer};
pub use syntax::SyntaxHighlighter;
pub use table::{TableCell, TableCells, TableGeometry, TableMetrics};

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use quark::selection::{
    BlockKey, BlockOrder, FULL_INTEGRITY_CHECKS, Selection, SelectionPoint, SelectionText,
    count_integrity_steps,
};
use quark_render::FontWeight;
use quark_render::scene::Rect;
use quark_text::FontEpoch;

use crate::element::{
    AnyElement, Binding, LineHeight, ScrollHandle, ScrollbarVisibility, StyledSpan, join_code_lines,
};
use crate::theme::Theme;
use crate::virtual_list::{RowError, RowIntegrityError, RowKey, ScrollAlign, VariableList};
use quark::Color;
use quark::focus::FocusId;

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
        /// A toolbar row above the lines holds the label, Copy, and the
        /// wrap toggle; see [`Document::set_code_toolbar`].
        toolbar: bool,
        /// Lines wrap at the column instead of scrolling sideways; see
        /// [`Document::set_code_wrap`].
        wrap: bool,
    },
    /// A grid of cells with a header row; see [`Block::table`]. Its text is
    /// the table as markdown, which selection indexes and copy reads.
    Table(TableCells),
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
            Self::Rule | Self::Image { .. } | Self::Table(_) => None,
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

/// How a block sits in its row: size, indent, list marker, quote bars,
/// and the markdown prefixes copy restores. Display never draws the
/// prefixes; the marker is painted in a gutter outside the selectable text.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockStyle {
    /// Font size relative to the document's.
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
            copy_prefix: empty_str(),
            copy_line_prefix: empty_str(),
        }
    }
}

thread_local! {
    static EMPTY_STR: Arc<str> = Arc::from("");
}

/// A shared empty string, so plain blocks' styles allocate no prefixes:
/// a streamed answer rebuilds its last block's style on every chunk.
pub(crate) fn empty_str() -> Arc<str> {
    EMPTY_STR.with(Arc::clone)
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

/// One selectable text block of a row. Markdown rendering produces a list
/// of these per row.
///
/// Every constructor and `with_` method gives the block a new
/// [`revision`](Self::revision), and clones share it; the element caches a
/// row while the revisions of its blocks stay the same. Change a block by
/// building a new one (or through a `with_` method), not by assigning its
/// fields.
#[derive(Debug, Clone)]
pub struct Block {
    pub key: BlockKey,
    pub content: BlockContent,
    pub style: BlockStyle,
    text: Arc<str>,
    /// One per content span, when any span takes its color from the theme.
    tones: Option<Arc<[SpanTone]>>,
    revision: u64,
}

impl Block {
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
                toolbar: false,
                wrap: false,
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
    fn same_layout(&self, other: &Block) -> bool {
        let content = match (&self.content, &other.content) {
            (BlockContent::Prose(a), BlockContent::Prose(b)) => Arc::ptr_eq(a, b),
            (
                BlockContent::Code {
                    spans: a,
                    label: la,
                    toolbar: ta,
                    wrap: wa,
                    ..
                },
                BlockContent::Code {
                    spans: b,
                    label: lb,
                    toolbar: tb,
                    wrap: wb,
                    ..
                },
            ) => Arc::ptr_eq(a, b) && la.is_some() == lb.is_some() && (ta, wa) == (tb, wb),
            (BlockContent::Table(a), BlockContent::Table(b)) => Arc::ptr_eq(&a.cells, &b.cells),
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

/// How the document shows code blocks, besides the blocks themselves.
#[derive(Debug, Clone, Copy)]
struct CodePresentation<'a> {
    toolbar: bool,
    wrapped: &'a HashSet<BlockKey>,
}

impl CodePresentation<'_> {
    /// `block` as the document lays it out: a code block with the
    /// document's toolbar and its wrap state. Its revision stays the
    /// source block's; the row hash covers both settings.
    fn present<'b>(&self, block: &'b Block) -> Cow<'b, Block> {
        let BlockContent::Code { toolbar, wrap, .. } = &block.content else {
            return Cow::Borrowed(block);
        };
        let want = (
            *toolbar || self.toolbar,
            *wrap || self.wrapped.contains(&block.key),
        );
        if (*toolbar, *wrap) == want {
            return Cow::Borrowed(block);
        }
        let mut block = block.clone();
        if let BlockContent::Code { toolbar, wrap, .. } = &mut block.content {
            (*toolbar, *wrap) = want;
        }
        Cow::Owned(block)
    }
}

/// One row of the document.
#[derive(Debug, Clone)]
pub struct DocumentRow {
    pub key: RowKey,
    pub chrome: RowChrome,
    pub blocks: Vec<Block>,
    /// App elements between the blocks; see [`RowAdornment`]. Keys are
    /// unique within the row.
    pub adornments: Vec<RowAdornment>,
}

impl DocumentRow {
    /// A row without adornments.
    pub fn new(key: RowKey, chrome: RowChrome, blocks: Vec<Block>) -> Self {
        Self {
            key,
            chrome,
            blocks,
            adornments: Vec::new(),
        }
    }

    pub fn with_adornments(mut self, adornments: Vec<RowAdornment>) -> Self {
        self.adornments = adornments;
        self
    }
}

/// App data for the chrome around one row's blocks. The document reserves
/// the header band and hands the rest to the [`RowDecorator`], which draws
/// the header and background from it. The default is no chrome.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RowChrome {
    /// Height of the band above the blocks the header is drawn in; zero
    /// for none. It counts toward the row's height.
    pub header_height: f32,
    /// The row's accessible name. The header is hidden from assistive tech
    /// while it is set, so the name is not read twice.
    pub label: Option<Arc<str>>,
    /// App-defined kind the decorator branches on, as a chat message's
    /// role.
    pub kind: u32,
}

impl Hash for RowChrome {
    fn hash<H: Hasher>(&self, state: &mut H) {
        (self.header_height.to_bits(), &self.label, self.kind).hash(state);
    }
}

/// Draws row chrome; set one with [`Document::set_decorator`]. Rows are
/// cached, so what a decorator draws must depend only on the chrome, the
/// row width, and the theme.
pub trait RowDecorator {
    /// Painted behind the whole row.
    fn background(&self, chrome: &RowChrome, theme: &Theme) -> Option<Color> {
        let _ = (chrome, theme);
        None
    }

    /// A bar along the row's leading edge, over the background, as its
    /// color and width in points: the accent of an alert or error row.
    fn leading_edge(&self, chrome: &RowChrome, theme: &Theme) -> Option<(Color, f32)> {
        let _ = (chrome, theme);
        None
    }

    /// The element filling the header band, `width` wide and
    /// `chrome.header_height` tall. Called only when the row is rebuilt.
    fn header(&self, chrome: &RowChrome, width: f32, theme: &Theme) -> Option<AnyElement> {
        let _ = (chrome, width, theme);
        None
    }
}

/// A shared [`RowDecorator`], compared by identity.
#[derive(Clone)]
struct Decorator(Rc<dyn RowDecorator>);

impl std::fmt::Debug for Decorator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Decorator")
    }
}

/// The app's row store, read by key.
pub trait DocumentSource {
    fn row(&self, key: RowKey) -> Option<&DocumentRow>;
}

impl DocumentSource for HashMap<RowKey, DocumentRow> {
    fn row(&self, key: RowKey) -> Option<&DocumentRow> {
        self.get(&key)
    }
}

/// Layout of a row: `pad_y`, the row's [`RowChrome::header_height`], the
/// blocks separated by `block_gap`, and `pad_y` again. Blocks are inset
/// `pad_x` on both sides.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DocumentStyle {
    pub font_size: f32,
    pub pad_x: f32,
    pub pad_y: f32,
    pub block_gap: f32,
    /// Pixels per wheel line.
    pub line_scroll: f32,
    /// Height of the band at the top and bottom edges where a drag
    /// autoscrolls.
    pub edge: f32,
    /// Extra pixels above and below the viewport that are materialized.
    pub overscan: f32,
    /// Line height of body text: prose, list items, failed image alt
    /// text, and table cells. Headings scale it with their text
    /// ([`LineHeight::scaled`]); code blocks keep their own. An invalid
    /// value falls back to [`LineHeight::PARAGRAPH`].
    pub line_height: LineHeight,
}

impl DocumentStyle {
    /// Proportions for a body font of `font_size` physical pixels.
    pub fn for_font_size(font_size: f32) -> Self {
        Self {
            font_size,
            pad_x: (font_size * 1.2).round(),
            pad_y: (font_size * 0.6).round(),
            block_gap: (font_size * 0.6).round(),
            line_scroll: (font_size * 3.0).round(),
            edge: (font_size * 2.0).round(),
            overscan: (font_size * 20.0).round(),
            line_height: LineHeight::PARAGRAPH,
        }
    }

    /// The same proportions with body text at `line_height`.
    pub fn with_line_height(mut self, line_height: LineHeight) -> Self {
        self.line_height = line_height;
        self
    }

    /// The line height of a block `scale` times the body size.
    pub fn block_line_height(&self, scale: f32) -> LineHeight {
        self.line_height
            .valid_or(LineHeight::PARAGRAPH)
            .scaled(scale)
    }
}

impl Default for DocumentStyle {
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

    /// Width of the block's content from its left inset when it does not
    /// wrap (code), or `None` when it fits any width. Content wider than
    /// the column scrolls horizontally.
    fn natural_width(&self) -> Option<f32> {
        None
    }

    /// The grid of a table block, when the measurer lays tables out as
    /// one; the element places the cells by it. Blocks without one show
    /// a table's markdown text.
    fn table_metrics(&self) -> Option<&TableMetrics> {
        None
    }
}

/// Measures blocks; [`TextMeasurer`] does it with the shared text layouts
/// the block elements paint.
pub trait BlockMeasurer {
    type Geometry: BlockGeometry;
    fn measure(&mut self, block: &Block, width: f32) -> Self::Geometry;

    /// Takes the document-wide settings blocks are measured under (body
    /// line height). The document calls it before measuring, on the UI
    /// thread and the background one alike, so geometry always matches
    /// what its element paints. A measurer whose geometry depends on them
    /// must reflect them in [`Self::settings_key`]. The default ignores
    /// them.
    fn apply_style(&mut self, style: &DocumentStyle) {
        let _ = style;
    }

    /// Identifies the settings geometry depends on besides the block and
    /// width (fonts, font size, scale factor). When it changes, every block
    /// and row is measured again.
    fn settings_key(&self) -> MeasureKey {
        MeasureKey::default()
    }

    /// How a background thread can measure exactly as this measurer does.
    /// `None` (the default) keeps rows outside the window estimated until
    /// they scroll in.
    fn background_spec(&self) -> Option<MeasureSpec> {
        None
    }
}

/// What a [`BlockMeasurer`]'s geometry depends on besides the block and
/// width.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MeasureKey {
    /// Measurer-defined settings; [`TextMeasurer`] hashes the font size,
    /// scale factor, and body line height.
    pub settings: u64,
    /// The fonts text is shaped with, for measurers that shape text. It
    /// changes with the font settings, a loaded font, or a replaced text
    /// system.
    pub fonts: Option<FontEpoch>,
}

/// Geometry of a block as measured, with what it was measured from.
#[derive(Debug, Clone)]
struct Measured<G> {
    block: Block,
    width: u32,
    settings: MeasureKey,
    geometry: G,
}

/// `block`'s geometry from `cache` when it was measured from the same
/// content, style, width, and measurer settings; otherwise measures it and
/// caches the result.
fn measure_cached<G: BlockGeometry, M: BlockMeasurer<Geometry = G>>(
    cache: &mut HashMap<BlockKey, Measured<G>>,
    measurer: &mut M,
    block: &Block,
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
    pub top: f32,
    pub height: f32,
    /// This row's entries in [`Document::visible_blocks`].
    pub blocks: std::ops::Range<usize>,
    /// This row's entries in [`Document::visible_adornments`].
    pub adornments: std::ops::Range<usize>,
}

/// A materialized adornment, in viewport coordinates.
#[derive(Debug, Clone)]
pub struct VisibleAdornment {
    pub key: AdornmentKey,
    pub row: RowKey,
    /// Position of the adornment in its row's `adornments`.
    pub index: usize,
    pub rect: Rect,
    /// Top of the adornment below its row's top.
    pub offset_in_row: f32,
}

/// A materialized block, in viewport coordinates.
#[derive(Debug, Clone)]
pub struct VisibleBlock<G> {
    pub key: BlockKey,
    pub row: RowKey,
    /// Position of the block in its row's `blocks`.
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
pub enum DocumentIntegrityError {
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

/// Keyboard commands a document responds to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentCommand {
    Copy,
    SelectAll,
    /// Open find (see [`Document::set_find_query`] and [`find_bar`]).
    Find,
}

/// The command a pressed key triggers. Bound to `mod+c`, `mod+a`, and
/// `mod+f`, so Cmd and Ctrl both work and one table serves every platform.
pub fn key_command(pressed: &Binding) -> Option<DocumentCommand> {
    [
        ("mod+c", DocumentCommand::Copy),
        ("mod+a", DocumentCommand::SelectAll),
        ("mod+f", DocumentCommand::Find),
    ]
    .into_iter()
    .find(|(pattern, _)| pattern.parse::<Binding>().is_ok_and(|p| p.matches(pressed)))
    .map(|(_, command)| command)
}

/// Document state. `G` is the block geometry the measurer produces.
#[derive(Debug, Clone)]
pub struct Document<G = TextGeometry> {
    list: VariableList,
    order: BlockOrder,
    block_row: HashMap<BlockKey, RowKey>,
    row_blocks: HashMap<RowKey, Vec<BlockKey>>,
    selection: Option<Selection>,
    drag: Option<Drag>,
    content_below: bool,
    decorator: Option<Decorator>,
    style: DocumentStyle,
    size: (f32, f32),
    rows: Vec<VisibleRow>,
    blocks: Vec<VisibleBlock<G>>,
    adornments: Vec<VisibleAdornment>,
    /// A row materialized even outside the window, while it holds focus.
    kept_row: Option<RowKey>,
    /// A row the next prepare keeps at its place in the viewport.
    held_row: Option<RowKey>,
    /// A row every prepare keeps at a requested place until a scroll or
    /// another request replaces it.
    anchor: Option<RowAnchor>,
    /// Geometry of the blocks measured for the current window; materialize
    /// reuses it instead of measuring every visible block every frame.
    measured: HashMap<BlockKey, Measured<G>>,
    /// Last frame's `measured` map, empty, kept for its capacity.
    measured_spare: HashMap<BlockKey, Measured<G>>,
    /// The measurer settings the row heights were measured under.
    measure_key: Option<MeasureKey>,
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
    /// Horizontal scroll of each block wider than its column, kept while
    /// the block is in the document.
    scroll_handles: HashMap<BlockKey, ScrollHandle>,
    /// Code blocks show a toolbar row with Copy and a wrap toggle.
    code_toolbar: bool,
    /// Code blocks whose lines wrap, kept while the block is in the
    /// document.
    wrapped: HashSet<BlockKey>,
    /// When the list's auto-hiding scrollbar shows; see
    /// [`DocumentElement::scrollbar_auto_hide`](element::DocumentElement::scrollbar_auto_hide).
    scrollbar: ScrollbarVisibility,
    /// Elements built so far; rows whose scroll is still moving hash it so
    /// they rebuild every frame until it settles.
    elements_built: u64,
}

/// A row kept at a fixed place in the viewport: its top sits
/// `viewport_offset` points below the viewport's top. See
/// [`Document::anchor_row`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RowAnchor {
    pub row: RowKey,
    pub viewport_offset: f32,
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

impl<G: BlockGeometry> Document<G> {
    pub fn new(style: DocumentStyle) -> Self {
        let estimate = style.pad_y * 2.0 + style.font_size * 2.0;
        Self {
            list: VariableList::new(estimate, 0.0),
            order: BlockOrder::new(),
            block_row: HashMap::new(),
            row_blocks: HashMap::new(),
            selection: None,
            drag: None,
            content_below: false,
            decorator: None,
            style,
            size: (0.0, 0.0),
            rows: Vec::new(),
            blocks: Vec::new(),
            adornments: Vec::new(),
            kept_row: None,
            held_row: None,
            anchor: None,
            measured: HashMap::new(),
            measured_spare: HashMap::new(),
            measure_key: None,
            painted: HashMap::new(),
            painted_spare: HashMap::new(),
            row_builds: HashMap::new(),
            row_builds_spare: HashMap::new(),
            find: None,
            find_stale: false,
            reveal: None,
            scroll_handles: HashMap::new(),
            code_toolbar: false,
            wrapped: HashSet::new(),
            scrollbar: ScrollbarVisibility::new(),
            elements_built: 0,
        }
    }

    pub fn style(&self) -> &DocumentStyle {
        &self.style
    }

    /// Changes the document's style, such as its body line height. Every
    /// row is measured again as it becomes visible; the row at the top of
    /// the viewport keeps its place.
    pub fn set_style(&mut self, style: DocumentStyle) {
        if style != self.style {
            self.style = style;
            self.list.invalidate_all();
        }
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

    /// Draws every row's chrome from now on; see [`RowDecorator`].
    pub fn set_decorator(&mut self, decorator: impl RowDecorator + 'static) {
        self.decorator = Some(Decorator(Rc::new(decorator)));
    }

    /// Gives every code block a toolbar row with its label, a Copy button
    /// (emitting [`CopyCode`] with the block's whole source), and a wrap
    /// toggle. Off by default.
    pub fn set_code_toolbar(&mut self, toolbar: bool) {
        if self.code_toolbar != toolbar {
            self.code_toolbar = toolbar;
            self.list.invalidate_all();
        }
    }

    /// Wraps the lines of code block `block` at the column, or lets them
    /// scroll sideways again. Remembered while the block stays in the
    /// document; its row is remeasured on the next prepare.
    pub fn set_code_wrap(&mut self, block: BlockKey, wrap: bool) {
        let changed = if wrap {
            self.wrapped.insert(block)
        } else {
            self.wrapped.remove(&block)
        };
        if changed && let Some(row) = self.block_row.get(&block) {
            let _ = self.list.invalidate(*row);
        }
    }

    /// Whether the lines of code block `block` wrap.
    pub fn is_code_wrapped(&self, block: BlockKey) -> bool {
        self.wrapped.contains(&block)
    }

    fn code_presentation(&self) -> CodePresentation<'_> {
        CodePresentation {
            toolbar: self.code_toolbar,
            wrapped: &self.wrapped,
        }
    }

    // -- Document changes --

    /// Appends a row at the end.
    pub fn push(&mut self, row: &DocumentRow) -> Result<(), RowError> {
        self.list.append(row.key)?;
        let blocks = row
            .blocks
            .iter()
            .map(|block| block.key)
            .filter(|key| self.order.append(*key))
            .collect();
        self.adopt(row.key, blocks);
        self.mark_new_content();
        self.debug_check_row(row.key);
        Ok(())
    }

    /// Appends many rows as one batch, checking integrity once.
    pub fn extend<'a>(
        &mut self,
        rows: impl IntoIterator<Item = &'a DocumentRow>,
    ) -> Result<(), RowError> {
        let rows: Vec<&DocumentRow> = rows.into_iter().collect();
        let keys: Vec<RowKey> = rows.iter().map(|m| m.key).collect();
        self.list.extend(&keys)?;
        let mut fresh = Vec::new();
        let mut seen = HashSet::new();
        for row in rows {
            let blocks: Vec<BlockKey> = row
                .blocks
                .iter()
                .map(|block| block.key)
                .filter(|key| !self.order.contains(*key) && seen.insert(*key))
                .collect();
            fresh.extend_from_slice(&blocks);
            self.adopt(row.key, blocks);
        }
        self.order.extend(fresh);
        self.mark_new_content();
        self.debug_check();
        Ok(())
    }

    /// Inserts older rows before the first one. The rows on screen and
    /// the selection stay where they are.
    pub fn prepend<'a>(
        &mut self,
        rows: impl IntoIterator<Item = &'a DocumentRow>,
    ) -> Result<(), RowError> {
        let rows: Vec<&DocumentRow> = rows.into_iter().collect();
        let keys: Vec<RowKey> = rows.iter().map(|m| m.key).collect();
        self.list.prepend(&keys)?;
        let mut fresh = Vec::new();
        let mut seen = HashSet::new();
        for row in &rows {
            let blocks: Vec<BlockKey> = row
                .blocks
                .iter()
                .map(|block| block.key)
                .filter(|key| !self.order.contains(*key) && seen.insert(*key))
                .collect();
            fresh.extend_from_slice(&blocks);
            self.adopt(row.key, blocks);
        }
        self.order.prepend(fresh);
        self.debug_check();
        Ok(())
    }

    /// The row's blocks or text changed, as while streaming. New
    /// blocks join the document order, removed ones leave it (shrinking the
    /// selection inward), and the row is remeasured on the next prepare.
    pub fn update(&mut self, row: &DocumentRow) -> Result<(), RowError> {
        let key = row.key;
        let old = self
            .row_blocks
            .remove(&key)
            .ok_or(RowError::UnknownKey(key))?;
        let wanted: HashSet<BlockKey> = row.blocks.iter().map(|b| b.key).collect();
        for block in old.iter().filter(|b| !wanted.contains(b)) {
            self.forget_block(*block);
        }
        let mut kept: Vec<BlockKey> = Vec::with_capacity(row.blocks.len());
        for block in row.blocks.iter().map(|b| b.key) {
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
        // Edits to older rows (a highlight arriving, a status change)
        // are not new content to jump to.
        if self.list.rows().keys().last() == Some(&key) {
            self.mark_new_content();
        }
        self.debug_check_row(key);
        Ok(())
    }

    pub fn remove(&mut self, key: RowKey) -> Result<(), RowError> {
        self.list.remove(key)?;
        // The rows on screen stay put; nothing is left to keep in place.
        if self.anchor.is_some_and(|a| a.row == key) {
            self.anchor = None;
        }
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
        self.scroll_handles.remove(&block);
        self.wrapped.remove(&block);
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

    /// Whether `block` is in the document as a block of `row`. A row can
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
            self.content_below = true;
        }
    }

    // -- Scrolling --

    pub fn scroll_offset(&self) -> f32 {
        self.list.scroll_offset()
    }

    pub fn max_scroll_offset(&self) -> f32 {
        self.list.max_scroll_offset()
    }

    /// A user scroll; landing at the bottom pins the view there. Cancels
    /// a [`Self::anchor_row`].
    pub fn set_scroll_offset(&mut self, offset: f32) -> f32 {
        self.anchor = None;
        self.adjust_scroll(offset)
    }

    /// Moves the view without cancelling an anchor: the document's own
    /// corrections.
    fn adjust_scroll(&mut self, offset: f32) -> f32 {
        let offset = self.list.set_scroll_offset(offset);
        if self.list.is_stuck_to_bottom() {
            self.content_below = false;
        }
        offset
    }

    /// Keeps `row`'s top `viewport_offset` points below the viewport's top,
    /// from the next prepare on: through rows prepended or appended,
    /// streaming, and remeasurement (new widths, fonts, line heights, and
    /// heights measured in the background), each corrected before the
    /// frame is built. A row outside the window is located by its
    /// estimated offset, measured, and placed in the same prepare. The view
    /// never scrolls past either end, so a row too near one sits as close
    /// as it can.
    ///
    /// A user scroll ([`Self::set_scroll_offset`], wheel, drag
    /// autoscroll), a reveal, another anchor, or removing the row cancels
    /// it; removal leaves the view where it is. Fails for a row that is not
    /// in the document, or a nonfinite offset.
    pub fn anchor_row(&mut self, row: RowKey, viewport_offset: f32) -> Result<(), RowError> {
        if self.list.rows().index_of(row).is_none() || !viewport_offset.is_finite() {
            return Err(RowError::UnknownKey(row));
        }
        self.anchor = Some(RowAnchor {
            row,
            viewport_offset,
        });
        self.reveal = None;
        Ok(())
    }

    /// The anchor in effect, if any.
    pub fn anchor(&self) -> Option<RowAnchor> {
        self.anchor
    }

    /// Stops keeping the anchored row in place; the view stays where it is.
    pub fn clear_anchor(&mut self) {
        self.anchor = None;
    }

    pub fn scroll_by(&mut self, delta: f32) -> f32 {
        self.set_scroll_offset(self.scroll_offset() + delta)
    }

    pub fn is_stuck_to_bottom(&self) -> bool {
        self.list.is_stuck_to_bottom()
    }

    /// Content arrived at the end while the view was scrolled away from
    /// it; cleared once the view reaches the bottom. Apps show a "jump to
    /// latest" affordance from it.
    pub fn has_content_below(&self) -> bool {
        self.content_below
    }

    /// Scrolls to the end and sticks there as content arrives.
    pub fn scroll_to_bottom(&mut self) {
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
    pub fn selected_text(&self, source: &impl DocumentSource) -> String {
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
    pub fn set_find_query(&mut self, query: &str, source: &impl DocumentSource) {
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
        self.anchor = None;
        if !in_view {
            let _ = self.list.scroll_to(row, align);
            if self.list.is_stuck_to_bottom() {
                self.content_below = false;
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
    fn refresh_find(&mut self, source: &impl DocumentSource) {
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
        source: &impl DocumentSource,
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
    /// to the document's top left.
    pub fn handle(&mut self, event: DocumentEvent) {
        match event {
            DocumentEvent::PointerDown { x, y } => {
                self.drag = Some(Drag {
                    pointer: (x, y),
                    last_ms: None,
                });
                self.selection = self.point_at(x, y).map(Selection::collapsed);
            }
            DocumentEvent::PointerDrag { x, y } => {
                if let Some(drag) = &mut self.drag {
                    drag.pointer = (x, y);
                }
                self.extend_selection_to(x, y);
            }
            DocumentEvent::PointerUp => self.drag = None,
            DocumentEvent::Wheel(lines) => {
                self.scroll_by(lines as f32 * self.style.line_scroll);
            }
            DocumentEvent::ScrollTo(offset) => {
                self.set_scroll_offset(offset);
            }
            DocumentEvent::SetCodeWrap { block, wrap } => self.set_code_wrap(block, wrap),
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
            // Content scrolled left sits under the pointer further right.
            block
                .geometry
                .hit(x - rect.x + self.scroll_x(block.key), y - rect.y)
        };
        Some(SelectionPoint::new(block.key, byte))
    }

    /// Whether `focus` is one of the document's own focus targets: the
    /// sideways scroll of a wide code block or table, which a press or Tab
    /// focuses so arrow keys reach it. Apps route copy and select-all to
    /// the document while focus is on nothing or on one of these.
    pub fn owns_focus(&self, focus: FocusId) -> bool {
        self.scroll_handles
            .keys()
            .any(|key| FocusId::from_key(&scroll_area_id(*key)) == focus)
    }

    /// How far block `key`'s content is scrolled left: a wide code block
    /// or table keeps its sideways scroll by key while it stays in the
    /// document. Zero for blocks that fit their column.
    pub fn scroll_x(&self, key: BlockKey) -> f32 {
        self.scroll_handles.get(&key).map_or(0.0, |h| h.offset().0)
    }

    /// Whether block `key` scrolls sideways and its offset may still change
    /// during the next frame (a requested jump, a smooth scroll, a fling).
    fn scroll_unsettled(&self, key: BlockKey) -> bool {
        self.scroll_handles
            .get(&key)
            .is_some_and(|h| !h.is_settled())
    }

    // -- Frame --

    /// Advances autoscroll to `now_ms`, measures the rows entering the
    /// window at `width`, and rebuilds the materialized rows. A width
    /// change, or a change of the measurer's [`MeasureKey`] (such as new
    /// fonts), remeasures every row as it becomes visible.
    pub fn prepare<M: BlockMeasurer<Geometry = G>>(
        &mut self,
        width: f32,
        height: f32,
        now_ms: u64,
        source: &impl DocumentSource,
        measurer: &mut M,
    ) {
        if height != self.size.1 {
            self.list.set_viewport_height(height);
        }
        self.size = (width, height);
        measurer.apply_style(&self.style);
        self.autoscroll(now_ms);

        let held = self.held_row.take().and_then(|row| {
            let top = self.list.rows().offset_of(row)? - self.list.scroll_offset();
            Some((row, top))
        });
        self.measure_window(source, measurer);
        if let Some((row, top)) = held {
            // Rows measured on the way can move it again; two passes
            // settle it, as for a reveal.
            for _ in 0..2 {
                let Some(offset) = self.list.rows().offset_of(row) else {
                    break;
                };
                let delta = offset - self.list.scroll_offset() - top;
                if delta.abs() < 0.5 {
                    break;
                }
                self.adjust_scroll(self.list.scroll_offset() + delta);
                self.measure_window(source, measurer);
            }
        }
        self.place_anchor(source, measurer);
        if self.list.is_stuck_to_bottom() {
            self.content_below = false;
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

    /// Scrolls the anchored row to its place, measuring the rows that
    /// come into the window. Rows measured on the way can move it again,
    /// so it takes a few passes; one that cannot move further (the view
    /// is at an end) stops early.
    fn place_anchor<M: BlockMeasurer<Geometry = G>>(
        &mut self,
        source: &impl DocumentSource,
        measurer: &mut M,
    ) {
        let Some(anchor) = self.anchor else {
            return;
        };
        for _ in 0..3 {
            let Some(top) = self.list.rows().offset_of(anchor.row) else {
                self.anchor = None;
                return;
            };
            let before = self.list.scroll_offset();
            let target = top - anchor.viewport_offset;
            if (target - before).abs() < 0.5 {
                return;
            }
            self.adjust_scroll(target);
            let moved = self.list.scroll_offset() != before;
            self.measure_window(source, measurer);
            if !moved {
                return;
            }
        }
    }

    /// Measures the unmeasured rows of the overscanned window.
    fn measure_window<M: BlockMeasurer<Geometry = G>>(
        &mut self,
        source: &impl DocumentSource,
        measurer: &mut M,
    ) {
        let key = measurer.settings_key();
        if self.measure_key != Some(key) {
            // Every height so far is from the old fonts or sizes; they stay
            // as estimates until each row is measured again.
            self.list.invalidate_all();
            self.measure_key = Some(key);
        }
        let style = self.style;
        let width = self.size.0;
        let block_row = &self.block_row;
        let code = CodePresentation {
            toolbar: self.code_toolbar,
            wrapped: &self.wrapped,
        };
        let cache = &mut self.measured;
        self.list
            .measure_visible(width, style.overscan, |key, width| {
                let key = RowKey(key);
                let blocks = owned_blocks(source, block_row, key);
                let (header, adornments) = source.row(key).map_or((0.0, &[][..]), |r| {
                    (r.chrome.header_height, r.adornments.as_slice())
                });
                let block_width = block_width(&style, width);
                lay_out_row(&style, header, blocks, adornments, |item, _| match item {
                    RowItem::Block { block, .. } => {
                        let block = code.present(block);
                        measure_cached(cache, measurer, &block, block_width).height()
                    }
                    RowItem::Adornment { height, .. } => height,
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
        source: &impl DocumentSource,
        measurer: &mut M,
    ) {
        let style = self.style;
        let block_width = block_width(&style, self.size.0);
        let window = self.list.window(style.overscan);
        let scroll = self.list.scroll_offset();
        let rows = self.list.rows();
        self.rows.clear();
        self.blocks.clear();
        self.adornments.clear();
        let mut kept = std::mem::take(&mut self.measured_spare);
        // A kept row outside the window is materialized too, in order, so
        // the focused control in it stays in the tree.
        let extra = self
            .kept_row
            .and_then(|row| rows.index_of(row))
            .filter(|index| !window.range.contains(index));
        let above = extra.filter(|index| *index < window.range.start);
        let below = extra.filter(|index| *index >= window.range.end);
        for index in above.into_iter().chain(window.range).chain(below) {
            let key = rows.keys()[index];
            let top = rows.offset_of_index(index) - scroll;
            let height = rows.height_of(key).unwrap_or(0.0);
            let first = self.blocks.len();
            let first_adornment = self.adornments.len();
            let (header, adornments) = source.row(key).map_or((0.0, &[][..]), |r| {
                (r.chrome.header_height, r.adornments.as_slice())
            });
            let (blocks, visible_adornments) = (&mut self.blocks, &mut self.adornments);
            let (measured, handles) = (&mut self.measured, &mut self.scroll_handles);
            let code = CodePresentation {
                toolbar: self.code_toolbar,
                wrapped: &self.wrapped,
            };
            let owned = owned_blocks(source, &self.block_row, key);
            lay_out_row(&style, header, owned, adornments, |item, y| match item {
                RowItem::Block { index: i, block } => {
                    let block = code.present(block);
                    let block = &*block;
                    let geometry = measure_cached(measured, measurer, block, block_width);
                    if let Some(entry) = measured.remove(&block.key) {
                        kept.insert(block.key, entry);
                    }
                    let block_height = geometry.height();
                    let column = block_width - block.style.inset(style.font_size);
                    if geometry.natural_width().is_some_and(|w| w > column) {
                        handles.entry(block.key).or_default();
                    }
                    blocks.push(VisibleBlock {
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
                    block_height
                }
                RowItem::Adornment { index: i, height } => {
                    visible_adornments.push(VisibleAdornment {
                        key: adornments[i].key,
                        row: key,
                        index: i,
                        rect: Rect {
                            x: style.pad_x,
                            y: top + y,
                            width: block_width,
                            height,
                        },
                        offset_in_row: y,
                    });
                    height
                }
            });
            self.rows.push(VisibleRow {
                key,
                index,
                top,
                height,
                blocks: first..self.blocks.len(),
                adornments: first_adornment..self.adornments.len(),
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

    /// Adornments of the materialized rows, in document order.
    pub fn visible_adornments(&self) -> &[VisibleAdornment] {
        &self.adornments
    }

    /// Keeps `row` materialized, and so in the element and accessibility
    /// tree, while it scrolls out of the window: set it while focus is on a
    /// control in one of its adornments, so the control survives a scroll
    /// until focus moves on, and clear it then.
    pub fn keep_materialized(&mut self, row: Option<RowKey>) {
        self.kept_row = row;
    }

    /// Keeps `row`'s top where it is in the viewport through the next
    /// prepare, whatever happens to the rows around it and to its own
    /// height: call it with the row whose disclosure was just toggled, so
    /// the control stays under the pointer even while the view follows
    /// the bottom. Scrolling away from the bottom this way stops following
    /// it. The view never scrolls past the end, so a row that shrinks
    /// near the end still moves down.
    pub fn hold_in_place(&mut self, row: RowKey) {
        self.held_row = Some(row);
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

    /// What `row`'s height depends on, as a snapshot another thread can
    /// measure: adornments go as their slots and heights only.
    fn row_snapshot(&self, source: &impl DocumentSource, row: RowKey) -> RowSnapshot {
        let content = source.row(row);
        RowSnapshot {
            header: content.map_or(0.0, |r| r.chrome.header_height),
            adornments: content.map_or_else(Vec::new, |r| {
                r.adornments.iter().map(|a| (a.slot, a.height)).collect()
            }),
            blocks: owned_blocks(source, &self.block_row, row)
                .map(|(_, block)| self.code_presentation().present(block).into_owned())
                .collect(),
        }
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
    /// of the row) plus the rows without blocks around it; per-row
    /// edits call it through `debug_assert!`.
    pub fn verify_row(&self, row: RowKey) -> Result<(), DocumentIntegrityError> {
        let rows = self.list.rows();
        let index = rows
            .index_of(row)
            .ok_or(DocumentIntegrityError::UnknownRow { row })?;
        rows.verify_row(index)
            .map_err(DocumentIntegrityError::Rows)?;
        if self.row_blocks.len() != rows.len() || self.block_row.len() != self.order.len() {
            return Err(DocumentIntegrityError::BlockOrder);
        }
        let blocks = self
            .row_blocks
            .get(&row)
            .ok_or(DocumentIntegrityError::UnknownRow { row })?;
        count_integrity_steps(blocks.len());
        let mut expected = self
            .last_block_before(row)
            .and_then(|b| self.order.position(b))
            .map_or(0, |p| p + 1);
        for block in blocks {
            if self.block_row.get(block) != Some(&row) {
                return Err(DocumentIntegrityError::BlockRow { block: *block });
            }
            if self.order.position(*block) != Some(expected) {
                return Err(DocumentIntegrityError::BlockOrder);
            }
            expected += 1;
        }
        let next_first = rows.keys()[index + 1..]
            .iter()
            .find_map(|key| self.row_blocks.get(key)?.first().copied());
        let next_pos = next_first.map_or(Some(self.order.len() as u32), |b| self.order.position(b));
        if next_pos != Some(expected) {
            return Err(DocumentIntegrityError::BlockOrder);
        }
        self.verify_selection()
    }

    fn verify_selection(&self) -> Result<(), DocumentIntegrityError> {
        match self.selection {
            Some(selection) if selection.ordered(&self.order).is_none() => {
                Err(DocumentIntegrityError::SelectionOutsideDocument)
            }
            _ => Ok(()),
        }
    }

    /// Checks every row, block, and the selection. O(rows + blocks); batch
    /// edits call it through `debug_assert!`, as do per-row ones with
    /// `integrity-checks`.
    pub fn verify_integrity(&self) -> Result<(), DocumentIntegrityError> {
        count_integrity_steps(self.order.len());
        self.list
            .rows()
            .verify_integrity()
            .map_err(DocumentIntegrityError::Rows)?;
        let rows = self.list.rows().keys();
        if self.row_blocks.len() != rows.len() {
            return Err(DocumentIntegrityError::BlockOrder);
        }
        // Walk the rows' blocks against the order in place, without
        // collecting them.
        let order = self.order.keys();
        let mut at = 0;
        for row in rows {
            let blocks = self
                .row_blocks
                .get(row)
                .ok_or(DocumentIntegrityError::UnknownRow { row: *row })?;
            for block in blocks {
                if self.block_row.get(block) != Some(row) {
                    return Err(DocumentIntegrityError::BlockRow { block: *block });
                }
                if order.get(at) != Some(block) {
                    return Err(DocumentIntegrityError::BlockOrder);
                }
                at += 1;
            }
        }
        if at != order.len() || self.block_row.len() != at {
            return Err(DocumentIntegrityError::BlockOrder);
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

/// The blocks of `row`'s content that the document holds as `row`'s, with
/// their positions in the row's `blocks`.
fn owned_blocks<'a>(
    source: &'a impl DocumentSource,
    block_row: &'a HashMap<BlockKey, RowKey>,
    row: RowKey,
) -> impl Iterator<Item = (usize, &'a Block)> + Clone + 'a {
    source
        .row(row)
        .map_or(&[][..], |m| m.blocks.as_slice())
        .iter()
        .enumerate()
        .filter(move |(_, block)| block_row.get(&block.key) == Some(&row))
}

/// A row's height inputs, copied for a background thread.
#[derive(Debug, Clone)]
pub(super) struct RowSnapshot {
    pub header: f32,
    pub adornments: Vec<(AdornmentSlot, f32)>,
    pub blocks: Vec<Block>,
}

/// One item of a row's flow, as [`lay_out_row`] meets it.
#[derive(Clone, Copy)]
enum RowItem<'a> {
    /// The block at `index` in the row's `blocks`.
    Block { index: usize, block: &'a Block },
    /// The adornment at `index` in the row's adornments.
    Adornment { index: usize, height: f32 },
}

/// Lays a row out top to bottom: `pad_y`, the `header` band, its blocks
/// and adornments in flow order separated by the block gap (half of it
/// before a [`BlockStyle::tight`] block), and `pad_y` again. `item` gets
/// each item and its top below the row's top and returns its height.
/// Returns the row's height. The UI thread and the background measurer
/// both lay rows out through it, so their heights agree to the bit.
///
/// Adornments of zero height take no space and are skipped. One whose
/// [`AdornmentSlot::Before`] block is not among `blocks` goes to the end.
fn lay_out_row<'a, A: AdornmentShape>(
    style: &DocumentStyle,
    header: f32,
    blocks: impl Iterator<Item = (usize, &'a Block)> + Clone,
    adornments: &[A],
    mut item: impl FnMut(RowItem<'a>, f32) -> f32,
) -> f32 {
    let mut y = style.pad_y + header;
    let mut placed = false;
    let mut next = |entry: RowItem<'a>, tight: bool| {
        if placed {
            y += if tight {
                (style.block_gap * 0.5).round()
            } else {
                style.block_gap
            };
        }
        placed = true;
        y += item(entry, y);
    };
    let shaped = adornments
        .iter()
        .enumerate()
        .filter(|(_, a)| a.height() > 0.0)
        .map(|(index, a)| (index, a.slot(), a.height()));
    for (index, _, height) in shaped.clone().filter(|a| a.1 == AdornmentSlot::Start) {
        next(RowItem::Adornment { index, height }, false);
    }
    for (index, block) in blocks.clone() {
        let before = AdornmentSlot::Before(block.key);
        for (index, _, height) in shaped.clone().filter(|a| a.1 == before) {
            next(RowItem::Adornment { index, height }, false);
        }
        next(RowItem::Block { index, block }, block.style.tight);
    }
    let at_end = |slot: AdornmentSlot| match slot {
        AdornmentSlot::End => true,
        AdornmentSlot::Before(key) => !blocks.clone().any(|(_, b)| b.key == key),
        AdornmentSlot::Start => false,
    };
    for (index, _, height) in shaped.filter(|a| at_end(a.1)) {
        next(RowItem::Adornment { index, height }, false);
    }
    y + style.pad_y
}

fn block_width(style: &DocumentStyle, width: f32) -> f32 {
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

/// The stable id of the sideways scroll area of wide block `key`.
fn scroll_area_id(key: BlockKey) -> String {
    format!("document.block:{}:scroll", key.0)
}

/// Block text read from the app's model by way of the owning row.
struct SourceText<'a, S> {
    source: &'a S,
    block_row: &'a HashMap<BlockKey, RowKey>,
}

impl<'a, S: DocumentSource> SourceText<'a, S> {
    fn block(&self, key: BlockKey) -> Option<&'a Block> {
        let row = self.block_row.get(&key)?;
        self.source
            .row(*row)?
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
