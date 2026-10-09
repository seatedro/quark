use std::mem;
use std::ops::Range;
use std::sync::Arc;

use cosmic_text::{
    Attrs, AttrsList, AttrsOwned, Buffer, BufferLine, CacheKey, CacheKeyFlags, Family, FontSystem,
    LineEnding, Metrics, PhysicalGlyph, Shaping, Wrap, fontdb,
};
use quark::scene::FontStyle;
use quark::{FontKind, FontWeight, Rect};
use unicode_segmentation::UnicodeSegmentation;

use crate::fonts::{FamilyId, FamilyNames, FontFamily};
use crate::offset::{TextOffset, ToTextOffset};
use crate::source::TextSource;

/// Line height multiplier quark has always used for text.
pub const DEFAULT_LINE_HEIGHT_FACTOR: f32 = 1.35;

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum TextError {
    #[error("font size must be finite and positive, got {0}")]
    InvalidFontSize(f32),
    #[error("line height must be finite and positive, got {0}")]
    InvalidLineHeight(f32),
    #[error("scale factor must be finite and positive, got {0}")]
    InvalidScaleFactor(f32),
    #[error("wrap width must not be NaN")]
    InvalidWrapWidth,
    #[error("span {index} range {start}..{end} is out of bounds or not on a char boundary")]
    InvalidSpan {
        index: usize,
        start: usize,
        end: usize,
    },
    #[error("text longer than u32::MAX bytes")]
    TextTooLong,
}

/// A broken [`TextLayout`] glyph, line, or run invariant, reported by
/// [`TextLayout::verify_integrity`].
#[derive(Debug, Clone, thiserror::Error, PartialEq)]
pub enum IntegrityError {
    #[error("layout has no lines")]
    NoLines,
    #[error("layout size {width}x{height} is not finite and non-negative")]
    Size { width: f32, height: f32 },
    #[error("line {line} glyphs {start}..{end} do not continue from glyph {expected}")]
    LineGlyphs {
        line: usize,
        start: usize,
        end: usize,
        expected: usize,
    },
    #[error(
        "line {line} bytes {start}..{end} are out of bounds, off a char boundary, or out of order"
    )]
    LineBytes {
        line: usize,
        start: usize,
        end: usize,
    },
    #[error("glyph {glyph} bytes {start}..{end} are out of bounds or off a char boundary")]
    GlyphBytes {
        glyph: usize,
        start: usize,
        end: usize,
    },
    #[error("glyph {glyph} is outside the glyph range of its line {line}")]
    GlyphLine { glyph: usize, line: usize },
    #[error("glyph {glyph} has span {span} but there are {spans} spans")]
    GlyphSpan {
        glyph: usize,
        span: u32,
        spans: usize,
    },
    #[error("run {run} glyphs {start}..{end} do not continue from glyph {expected} on one line")]
    RunGlyphs {
        run: usize,
        start: usize,
        end: usize,
        expected: usize,
    },
}

/// Shaping-relevant style for a whole text block. Colors are deliberately
/// absent: they do not affect layout, so the renderer maps [`GlyphRun::span`]
/// to colors at paint time and color changes never invalidate a layout.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextStyle {
    pub font_kind: FontKind,
    pub font_weight: FontWeight,
    pub font_size: f32,
    pub line_height: f32,
    /// A font family by name in place of `font_kind`'s generic one, for
    /// text that keeps its own font whatever the app's settings (a
    /// terminal's). Spans that set their own kind still use that kind's
    /// family.
    pub family: Option<FamilyId>,
    /// Extra advance after every glyph, in ems. A terminal sets it so
    /// glyphs land on its whole-pixel cell grid.
    pub letter_spacing: f32,
    /// Rasterize glyph outlines slightly emboldened (color glyphs are
    /// left alone), like Ghostty's `font-thicken`.
    pub thicken: bool,
    /// Blend glyph coverage as Ghostty's `linear-corrected` does, for text
    /// over a background of this sRGB-encoded luminance (0 to 255): the
    /// weight sRGB-space blending gives, without its color fringes.
    /// Linear blending, the default, draws dark text on light backgrounds
    /// thinner and light text on dark ones heavier.
    pub linear_correction: Option<u8>,
}

impl TextStyle {
    pub fn new(font_size: f32) -> Self {
        Self {
            font_kind: FontKind::Ui,
            font_weight: FontWeight::Normal,
            font_size,
            line_height: font_size * DEFAULT_LINE_HEIGHT_FACTOR,
            family: None,
            letter_spacing: 0.0,
            thicken: false,
            linear_correction: None,
        }
    }

    pub fn kind(mut self, font_kind: FontKind) -> Self {
        self.font_kind = font_kind;
        self
    }

    pub fn weight(mut self, font_weight: FontWeight) -> Self {
        self.font_weight = font_weight;
        self
    }

    pub fn line_height(mut self, line_height: f32) -> Self {
        self.line_height = line_height;
        self
    }

    /// A family by static name; see [`Self::font_family`].
    pub fn family(mut self, family: Option<&'static str>) -> Self {
        self.family = family.map(FamilyId::from_static);
        self
    }

    /// The family text draws in. [`FontFamily::SystemUi`] and
    /// [`FontFamily::UiMonospace`] set the kind; a named family keeps it,
    /// for spans that re-kind and for a family the system lacks.
    pub fn font_family(mut self, family: FontFamily) -> Self {
        match family {
            FontFamily::SystemUi => (self.font_kind, self.family) = (FontKind::Ui, None),
            FontFamily::UiMonospace => (self.font_kind, self.family) = (FontKind::Mono, None),
            FontFamily::Named(id) => self.family = Some(id),
        }
        self
    }

    pub fn letter_spacing(mut self, ems: f32) -> Self {
        self.letter_spacing = ems;
        self
    }

    pub fn thicken(mut self, thicken: bool) -> Self {
        self.thicken = thicken;
        self
    }

    pub fn linear_correction(mut self, background_luminance: Option<u8>) -> Self {
        self.linear_correction = background_luminance;
        self
    }
}

/// Attribute override for a byte range of the text. Later spans win where
/// they overlap. Glyphs record the index of the span that styled them.
#[derive(Debug, Clone, PartialEq)]
pub struct TextSpan {
    pub range: Range<usize>,
    pub weight: Option<FontWeight>,
    pub style: Option<FontStyle>,
    /// Font family override, e.g. an inline code run in UI text.
    pub kind: Option<FontKind>,
    /// Font size override in logical pixels, e.g. inline code a little
    /// smaller than the prose around it. The line keeps the style's line
    /// height, and the span's glyphs sit on the line's baseline.
    pub size: Option<f32>,
    /// Extra advance after every glyph of the span, in ems of its size, in
    /// place of the style's: room around an inline code pill.
    pub letter_spacing: Option<f32>,
}

/// Everything a layout depends on. Sizes are logical pixels.
#[derive(Debug, Clone)]
pub struct TextParams {
    pub text: Arc<str>,
    pub spans: Arc<[TextSpan]>,
    pub style: TextStyle,
    pub wrap_width: Option<f32>,
    pub scale_factor: f32,
}

/// The empty span list every [`TextParams::new`] shares, so building
/// params allocates nothing for spans.
fn no_spans() -> Arc<[TextSpan]> {
    static EMPTY: std::sync::LazyLock<Arc<[TextSpan]>> =
        std::sync::LazyLock::new(|| Arc::from(Vec::new()));
    EMPTY.clone()
}

/// [`TextParams`] borrowed: what a cache lookup needs, so a hit costs no
/// allocation. The cache builds owned params only on a miss.
#[derive(Debug, Clone, Copy)]
pub struct TextQuery<'a> {
    pub text: &'a str,
    pub spans: &'a [TextSpan],
    pub style: TextStyle,
    pub wrap_width: Option<f32>,
    pub scale_factor: f32,
}

impl<'a> TextQuery<'a> {
    pub fn new(text: &'a str, style: TextStyle) -> Self {
        Self {
            text,
            spans: &[],
            style,
            wrap_width: None,
            scale_factor: 1.0,
        }
    }

    pub fn wrap_width(mut self, wrap_width: Option<f32>) -> Self {
        self.wrap_width = wrap_width;
        self
    }

    pub fn scale_factor(mut self, scale_factor: f32) -> Self {
        self.scale_factor = scale_factor;
        self
    }

    /// Owned params with copies of the text and spans.
    pub fn to_params(&self) -> TextParams {
        let spans = if self.spans.is_empty() {
            no_spans()
        } else {
            Arc::from(self.spans)
        };
        TextParams {
            text: Arc::from(self.text),
            spans,
            style: self.style,
            wrap_width: self.wrap_width,
            scale_factor: self.scale_factor,
        }
    }

    pub(crate) fn validate(&self) -> Result<(), TextError> {
        let style = &self.style;
        if !(style.font_size.is_finite() && style.font_size > 0.0) {
            return Err(TextError::InvalidFontSize(style.font_size));
        }
        if !(style.line_height.is_finite() && style.line_height > 0.0) {
            return Err(TextError::InvalidLineHeight(style.line_height));
        }
        if !(self.scale_factor.is_finite() && self.scale_factor > 0.0) {
            return Err(TextError::InvalidScaleFactor(self.scale_factor));
        }
        if self.wrap_width.is_some_and(f32::is_nan) {
            return Err(TextError::InvalidWrapWidth);
        }
        if u32::try_from(self.text.len()).is_err() {
            return Err(TextError::TextTooLong);
        }
        for (index, span) in self.spans.iter().enumerate() {
            let Range { start, end } = span.range;
            let valid = start <= end
                && end <= self.text.len()
                && self.text.is_char_boundary(start)
                && self.text.is_char_boundary(end);
            if !valid {
                return Err(TextError::InvalidSpan { index, start, end });
            }
        }
        Ok(())
    }
}

impl TextParams {
    /// The borrowed form of these params.
    pub fn query(&self) -> TextQuery<'_> {
        TextQuery {
            text: &self.text,
            spans: &self.spans,
            style: self.style,
            wrap_width: self.wrap_width,
            scale_factor: self.scale_factor,
        }
    }

    pub fn new(text: impl Into<Arc<str>>, style: TextStyle) -> Self {
        Self {
            text: text.into(),
            spans: no_spans(),
            style,
            wrap_width: None,
            scale_factor: 1.0,
        }
    }

    pub fn spans(mut self, spans: impl Into<Arc<[TextSpan]>>) -> Self {
        self.spans = spans.into();
        self
    }

    pub fn wrap_width(mut self, wrap_width: Option<f32>) -> Self {
        self.wrap_width = wrap_width;
        self
    }

    pub fn scale_factor(mut self, scale_factor: f32) -> Self {
        self.scale_factor = scale_factor;
        self
    }

    fn validate(&self) -> Result<(), TextError> {
        self.query().validate()
    }
}

/// Caret geometry in logical pixels relative to the layout origin.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Caret {
    pub x: f32,
    pub y: f32,
    pub height: f32,
    pub line: usize,
}

/// One visual line (a paragraph produces one or more when wrapped).
#[derive(Debug, Clone, PartialEq)]
pub struct LineInfo {
    /// Absolute bytes. Visual lines of a paragraph tile it; the paragraph's
    /// line ending is excluded.
    pub byte_range: Range<usize>,
    pub top: f32,
    pub height: f32,
    pub baseline: f32,
    pub width: f32,
    pub glyph_range: Range<usize>,
    /// Paragraph base direction.
    pub rtl: bool,
}

/// Consecutive glyphs on one visual row sharing a span and direction.
/// A line has one row, except where wrapping between glyphs split a
/// cluster over several rows (see [`TextLayout::lines`]).
#[derive(Debug, Clone, PartialEq)]
pub struct GlyphRun {
    pub line: usize,
    /// 0 = base style, `i + 1` = `params.spans[i]`.
    pub span: u32,
    pub rtl: bool,
    pub glyphs: Range<usize>,
    /// The row's top, height, and baseline in physical pixels below the
    /// layout origin, unrounded, as cosmic-text laid the row out: what the
    /// renderer culls rows by and rounds baselines from.
    pub phys_top: f32,
    pub phys_height: f32,
    pub phys_baseline: f32,
}

/// A layout's glyphs: one [`Glyph`] each, stored line by line in
/// cosmic-text's order (left to right for LTR, logical order within RTL
/// runs, so `x` is not sorted). The name stays from when glyphs were stored
/// a column per field.
pub type GlyphColumns = [Glyph];

/// One glyph of a [`TextLayout`]. Logical fields are in logical pixels;
/// `phys_*` are physical pixels, unrounded. One record per glyph rather
/// than a vector per field, so a new layout allocates its glyphs once.
///
/// Draw a glyph where [`TextLayout::physical_glyph`] puts it: the renderer
/// truncates `phys_dy` and rounds the row's baseline separately, so adding
/// them first moves glyphs on fractional baselines by a pixel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Glyph {
    pub x: f32,
    pub advance: f32,
    pub line: u32,
    pub byte_start: u32,
    pub byte_end: u32,
    /// Unicode bidi embedding level; odd = RTL.
    pub level: u8,
    pub span: u32,
    pub font_id: fontdb::ID,
    pub glyph_id: u16,
    pub font_size: f32,
    pub font_weight: fontdb::Weight,
    pub flags: CacheKeyFlags,
    /// Right of the shaped buffer's origin, which sits
    /// [`TextLayout::buffer_x`] right of the layout origin.
    pub phys_dx: f32,
    /// Below the glyph's row baseline ([`GlyphRun::phys_baseline`]).
    pub phys_dy: f32,
}

impl Glyph {
    pub fn rtl(&self) -> bool {
        self.level % 2 == 1
    }

    /// The text bytes the glyph's cluster covers.
    pub fn bytes(&self) -> Range<usize> {
        self.byte_start as usize..self.byte_end as usize
    }
}

fn clear_reserve<T>(column: &mut Vec<T>, len: usize) {
    column.clear();
    column.reserve(len);
}

/// One visual line: absolute bytes (line ending excluded), logical-pixel
/// metrics, and its glyph range. One vector of these, not a column each:
/// lines are few, and a new layout then allocates once for them.
#[derive(Debug, Clone, Copy)]
struct Line {
    byte_start: u32,
    byte_end: u32,
    top: f32,
    height: f32,
    baseline: f32,
    width: f32,
    glyph_start: u32,
    glyph_end: u32,
    rtl: bool,
}

impl Line {
    fn glyphs(&self) -> Range<usize> {
        self.glyph_start as usize..self.glyph_end as usize
    }
}

/// Consecutive glyphs of one line sharing a span and direction; see
/// [`GlyphRun`].
#[derive(Debug, Clone, Copy)]
struct Run {
    line: u32,
    span: u32,
    rtl: bool,
    glyph_start: u32,
    glyph_end: u32,
    /// Its row, in physical pixels; see [`GlyphRun`].
    top: f32,
    height: f32,
    baseline: f32,
}

/// Lay `buffer` out again at its widest line when a line runs right to
/// left, and return how far right (physical pixels) the buffer must be drawn
/// so no line starts left of zero. cosmic-text aligns an RTL line against
/// the buffer width (the wrap width, or each paragraph's own widest line
/// without one), so it would paint outside the measured box.
///
/// When every line fits the buffer width, relaying out at the widest line
/// keeps the wrapping (cosmic-text sums widths in the order it measures
/// them), so each RTL line ends at the widest line's right edge. When a
/// cluster wider than the wrap overflows a line, a wider buffer would let
/// other lines take more words, so the layout keeps the wrap width and is
/// shifted right instead, by the most any RTL line overflows it.
fn fit_rtl_lines(fs: &mut FontSystem, buffer: &mut Buffer) -> f32 {
    if !buffer.layout_runs().any(|run| run.rtl) {
        return 0.0;
    }
    let w = buffer
        .layout_runs()
        .fold(0.0_f32, |widest, run| widest.max(run.line_w));
    if buffer.size().0.is_none_or(|wrap| w <= wrap) {
        buffer.set_size(fs, Some(w), None);
        for line_i in 0..buffer.lines.len() {
            buffer.line_layout(fs, line_i);
        }
    }
    // Every RTL paragraph now aligns its lines' right edges to this width.
    let align = buffer.size().0.unwrap_or(w);
    buffer
        .layout_runs()
        .filter(|run| run.rtl)
        .fold(0.0_f32, |shift, run| shift.max(run.line_w - align))
}

/// Immutable result of shaping + layout, shared via `Arc` by measurement,
/// hit-testing, selection, and painting.
#[derive(Debug)]
pub struct TextLayout {
    source: TextSource,
    style: TextStyle,
    wrap_width: Option<f32>,
    scale_factor: f32,
    width: f32,
    height: f32,
    glyphs: Vec<Glyph>,
    lines: Vec<Line>,
    runs: Vec<Run>,
    /// Physical x at which `buffer` is drawn relative to the layout origin.
    buffer_x: f32,
    // Kept so the renderer can hand it to glyphon's TextRenderer, which only
    // accepts Buffers. Shaped at physical size; draw it with `scale: 1.0`.
    buffer: Buffer,
    /// Lines a longer text left in `buffer`, kept for their storage when
    /// the layout is rebuilt; the buffer lays out every line it holds.
    spare_lines: Vec<BufferLine>,
}

/// What shaping depends on besides the params and the font system: the
/// [`crate::TextSystem`]'s derived font facts and settings.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ShapeEnv<'a> {
    pub(crate) synth: SyntheticItalic,
    /// The color emoji family emoji clusters ask for first.
    pub(crate) emoji: Option<&'static str>,
    pub(crate) ligatures: bool,
    pub(crate) names: &'a FamilyNames,
}

/// Temporaries of a layout build. The [`TextSystem`](crate::TextSystem)
/// keeps them, so a build allocates only what its result keeps.
#[derive(Debug, Default)]
pub(crate) struct LayoutScratch {
    paragraphs: Vec<(Range<usize>, LineEnding)>,
    /// The paragraph of each visual line.
    line_paragraph: Vec<usize>,
    /// The font features every span gets.
    features: cosmic_text::FontFeatures,
    /// Faces for text-presentation characters, kept across builds.
    pub(crate) text_faces: crate::fonts::TextFaces,
    /// Shaping storage: a built layout keeps only its lines' layout (what
    /// painting reads) and returns the shaping its lines were laid out from
    /// here, so the next build shapes into it instead of allocating a span,
    /// word, and glyph vector per word.
    shapes: Vec<cosmic_text::ShapeLine>,
}

/// Most shaped lines [`LayoutScratch`] keeps for reuse, and the most
/// storage one may hold: past either, a line's shaping is dropped rather
/// than kept for the life of the text system.
const SPARE_SHAPES: usize = 32;
const SPARE_SHAPE_BYTES: usize = 64 << 10;

impl TextLayout {
    /// A layout with no text, for [`Self::rebuild`] to fill.
    pub(crate) fn empty() -> Self {
        Self {
            source: TextSource::empty(),
            style: TextStyle::new(1.0),
            wrap_width: None,
            scale_factor: 1.0,
            width: 0.0,
            height: 0.0,
            glyphs: Vec::new(),
            lines: Vec::new(),
            runs: Vec::new(),
            buffer_x: 0.0,
            buffer: Buffer::new_empty(Metrics::new(1.0, 1.0)),
            spare_lines: Vec::new(),
        }
    }

    pub(crate) fn build(
        fs: &mut FontSystem,
        scratch: &mut LayoutScratch,
        params: &TextParams,
        env: &ShapeEnv,
    ) -> Result<Self, TextError> {
        Self::build_with(fs, scratch, params, env, |_| true)
    }

    /// [`Self::build`] keeping only the shaped runs `keep` accepts, so tests
    /// can reproduce a shaper that yields no lines.
    fn build_with(
        fs: &mut FontSystem,
        scratch: &mut LayoutScratch,
        params: &TextParams,
        env: &ShapeEnv,
        keep: impl Fn(&cosmic_text::LayoutRun) -> bool,
    ) -> Result<Self, TextError> {
        params.validate()?;
        let mut layout = Self::empty();
        layout.copy_inputs(&params.query());
        layout.rebuild_with(fs, scratch, env, keep);
        Ok(layout)
    }

    /// How many glyphs the columns hold without growing.
    pub(crate) fn glyph_capacity(&self) -> usize {
        self.glyphs.capacity()
    }

    /// Bytes this layout keeps, at capacity: itself, its glyph, line, and
    /// run columns, cosmic-text's shaped lines (with those kept for reuse),
    /// and its text and spans.
    pub fn storage_bytes(&self) -> usize {
        fn cap<T>(column: &Vec<T>) -> usize {
            column.capacity() * size_of::<T>()
        }
        let glyphs = cap(&self.glyphs);
        let (lines, runs) = (cap(&self.lines), cap(&self.runs));
        let buffer = cap(&self.buffer.lines)
            + cap(&self.spare_lines)
            + self
                .buffer
                .lines
                .iter()
                .chain(&self.spare_lines)
                .map(BufferLine::storage_bytes)
                .sum::<usize>();
        let inputs = self.source.storage_bytes();
        size_of::<Self>() + glyphs + lines + runs + buffer + inputs
    }

    /// Copies `query`'s text and spans, into this layout's own source when
    /// nothing else holds it.
    pub(crate) fn copy_inputs(&mut self, query: &TextQuery) {
        self.source.set(query.text, query.spans);
        self.set_settings(query);
    }

    /// Takes `source` as its text and spans, shared, with `query`'s
    /// settings.
    pub(crate) fn share_source(&mut self, source: &TextSource, query: &TextQuery) {
        self.source = source.clone();
        self.set_settings(query);
    }

    /// Lets go of its source, so its owner can write over it while this
    /// layout waits to be rebuilt.
    pub(crate) fn detach_source(&mut self) {
        self.source = TextSource::empty();
    }

    /// `pooled` refilled by `fill`, or a new layout when there is none or
    /// something reached it after all. Only [`Arc::get_mut`] decides that
    /// nothing else holds a layout.
    pub(crate) fn refill(pooled: Option<Arc<Self>>, fill: impl FnOnce(&mut Self)) -> Arc<Self> {
        if let Some(mut layout) = pooled
            && let Some(own) = Arc::get_mut(&mut layout)
        {
            fill(own);
            return layout;
        }
        let mut layout = Self::empty();
        fill(&mut layout);
        Arc::new(layout)
    }

    fn set_settings(&mut self, query: &TextQuery) {
        self.style = query.style;
        self.wrap_width = query.wrap_width;
        self.scale_factor = query.scale_factor;
    }

    /// Shapes and lays out the inputs [`Self::copy_inputs`] or
    /// [`Self::share_source`] set, which must be valid params, reusing this
    /// layout's buffer, lines, and columns.
    pub(crate) fn rebuild(
        &mut self,
        fs: &mut FontSystem,
        scratch: &mut LayoutScratch,
        env: &ShapeEnv,
    ) {
        self.rebuild_with(fs, scratch, env, |_| true);
    }

    fn rebuild_with(
        &mut self,
        fs: &mut FontSystem,
        scratch: &mut LayoutScratch,
        env: &ShapeEnv,
        keep: impl Fn(&cosmic_text::LayoutRun) -> bool,
    ) {
        let ShapeEnv {
            synth,
            emoji,
            ligatures,
            names,
        } = *env;
        // A reference count, not a copy: the fields below are borrowed
        // mutably while the text is read.
        let source = self.source.clone();
        let (text, spans) = (source.as_str(), source.spans());
        let style = self.style;
        let scale = self.scale_factor;
        let wrap = self.wrap_width.map(|w| (w * scale).max(1.0));
        let Self {
            buffer,
            spare_lines,
            glyphs,
            lines,
            runs,
            ..
        } = &mut *self;

        split_paragraphs(text, &mut scratch.paragraphs);
        let paragraphs = &scratch.paragraphs;
        set_font_features(&mut scratch.features, ligatures);
        // One features vector moves between the base and each span's
        // attributes: an `Attrs` owns its features, so a clone per span
        // would allocate whenever ligatures are off.
        // A named family another system interned draws in the generic one.
        let named = style.family.and_then(|id| names.name(id));
        let own_family = named.map_or_else(|| family(style.font_kind), Family::Name);
        let synth = synth.for_family(fs, named);
        let mut base =
            base_attrs(&style, own_family).font_features(mem::take(&mut scratch.features));
        while buffer.lines.len() > paragraphs.len() {
            spare_lines.extend(buffer.lines.pop());
        }
        for (line_i, (range, ending)) in paragraphs.iter().enumerate() {
            let mut attrs = AttrsList::new(&base);
            for (i, span) in spans.iter().enumerate() {
                let start = span.range.start.max(range.start);
                let end = span.range.end.min(range.end);
                if start < end {
                    let span_attrs = span_attrs(&style, own_family, span, i, synth, scale)
                        .font_features(mem::take(&mut base.font_features));
                    attrs.add_span(start - range.start..end - range.start, &span_attrs);
                    base.font_features = span_attrs.font_features;
                }
            }
            let paragraph = text.get(range.clone()).unwrap_or_default();
            if let Some(emoji) = emoji {
                emoji_spans(&mut attrs, paragraph, emoji);
                if !paragraph.is_ascii() {
                    let key = (
                        style.family,
                        style.font_kind == FontKind::Mono,
                        font_weight_value(style.font_weight),
                    );
                    let faces = scratch.text_faces.faces(fs.db(), key, own_family, emoji);
                    text_spans(&mut attrs, paragraph, fs, &mut scratch.text_faces, faces);
                }
            }
            let reused = match buffer.lines.get_mut(line_i) {
                Some(line) => Some(line),
                None => spare_lines.pop().map(|line| {
                    buffer.lines.push(line);
                    &mut buffer.lines[line_i]
                }),
            };
            match reused {
                Some(line) => {
                    line.set_text(paragraph, *ending, attrs);
                    // `set_text` keeps the old shaping when text and
                    // attributes are unchanged, but the fonts they resolve
                    // to may have changed since. Resetting keeps the shaping
                    // storage for the next shape.
                    line.reset();
                }
                None => buffer.lines.push(BufferLine::new(
                    paragraph,
                    *ending,
                    attrs,
                    Shaping::Advanced,
                )),
            }
            if let Some(shape) = scratch.shapes.pop()
                && let Some(unused) = buffer.lines[line_i].lend_shape_storage(shape)
            {
                scratch.shapes.push(unused);
            }
        }
        scratch.features = base.font_features;
        buffer.set_wrap(fs, Wrap::WordOrGlyph);
        buffer.set_metrics_and_size(
            fs,
            Metrics::new(style.font_size * scale, style.line_height * scale),
            wrap,
            None,
        );
        for line_i in 0..buffer.lines.len() {
            buffer.line_layout(fs, line_i);
        }
        let buffer_x = fit_rtl_lines(fs, buffer);

        let inv = 1.0 / scale;
        let line_paragraph = &mut scratch.line_paragraph;
        line_paragraph.clear();
        let (glyph_count, row_count, run_count) = buffer
            .layout_runs()
            .filter(|run| keep(run))
            .fold((0, 0, 0), |(g, r, n), run| {
                (g + run.glyphs.len(), r + 1, n + run_breaks(run.glyphs))
            });
        // Room for a glyph per char lets a later text of as many chars,
        // with fewer ligatures, refill this layout without growing it.
        clear_reserve(glyphs, glyph_count.max(text.chars().count()));
        // One more for the line made below when no run was kept.
        clear_reserve(lines, row_count.max(1));
        clear_reserve(runs, run_count);
        let mut width = 0.0_f32;
        let mut height = 0.0_f32;

        for run in buffer.layout_runs().filter(|run| keep(run)) {
            let para_start = paragraphs[run.line_i].0.start;
            let min_start = run
                .glyphs
                .iter()
                .map(|g| para_start + g.start)
                .min()
                .unwrap_or(usize::MAX);
            let first_of_paragraph = line_paragraph.last() != Some(&run.line_i);
            // Glyph wrapping can split one multi-glyph cluster over several
            // visual lines that then share a start byte. Line starts must
            // strictly increase for `line_for_byte`, so fold such a line into
            // the previous one; its glyphs keep their own painted positions.
            let continuation = !first_of_paragraph
                && lines.last().is_some_and(|prev| {
                    min_start == usize::MAX || min_start <= prev.byte_start as usize
                });
            let line_index = if continuation {
                lines.len() as u32 - 1
            } else {
                lines.len() as u32
            };
            let glyph_start = glyphs.len() as u32;
            glyphs.extend(run.glyphs.iter().map(|g| Glyph {
                x: (g.x + buffer_x) * inv,
                advance: g.w * inv,
                line: line_index,
                byte_start: (para_start + g.start) as u32,
                byte_end: (para_start + g.end) as u32,
                level: g.level.number(),
                span: g.metadata as u32,
                font_id: g.font_id,
                glyph_id: g.glyph_id,
                font_size: g.font_size,
                font_weight: g.font_weight,
                flags: g.cache_key_flags,
                // The sums cosmic-text's `LayoutGlyph::physical` makes, in
                // its order, so offsets cross subpixel bins where its do.
                phys_dx: g.x + g.font_size * g.x_offset,
                phys_dy: g.y - g.font_size * g.y_offset,
            }));
            push_row_runs(runs, glyphs, glyph_start as usize, line_index, &run);
            // RTL lines end at the shifted buffer's right edge, which is the
            // widest RTL line's own; LTR lines start at the shift.
            let shift = if run.rtl { 0.0 } else { buffer_x };
            width = width.max((shift + run.line_w) * inv);
            height = height.max((run.line_top + run.line_height) * inv);
            if let Some(last) = lines.last_mut().filter(|_| continuation) {
                last.glyph_end = glyphs.len() as u32;
                last.width = last.width.max(run.line_w * inv);
                continue;
            }
            let byte_start = if first_of_paragraph || min_start == usize::MAX {
                para_start
            } else {
                min_start
            };
            lines.push(Line {
                byte_start: byte_start as u32,
                byte_end: 0, // fixed up below
                top: run.line_top * inv,
                height: run.line_height * inv,
                baseline: run.line_y * inv,
                width: run.line_w * inv,
                glyph_start,
                glyph_end: glyphs.len() as u32,
                rtl: run.rtl,
            });
            line_paragraph.push(run.line_i);
        }

        // cosmic-text yields a run for every laid-out paragraph, but `hit`,
        // `caret`, and `line_at_y` read the last line, so one line is made
        // structural here instead of trusted.
        if lines.is_empty() {
            lines.push(Line {
                byte_start: 0,
                byte_end: 0,
                top: 0.0,
                height: style.line_height,
                baseline: style.line_height.min(style.font_size),
                width: 0.0,
                glyph_start: 0,
                glyph_end: glyphs.len() as u32,
                rtl: false,
            });
            line_paragraph.push(0);
            height = height.max(style.line_height);
        }

        for i in 0..line_paragraph.len() {
            let same_paragraph_next = line_paragraph.get(i + 1) == Some(&line_paragraph[i]);
            lines[i].byte_end = if same_paragraph_next {
                lines[i + 1].byte_start
            } else {
                paragraphs[line_paragraph[i]].0.end as u32
            };
        }

        // Painting reads only the lines' layout, and the layout is never
        // laid out again, so its shaping goes back for the next build.
        for line in buffer.lines.iter_mut().chain(spare_lines.iter_mut()) {
            if let Some(shape) = line.take_shape()
                && scratch.shapes.len() < SPARE_SHAPES
                && shape.storage_bytes() <= SPARE_SHAPE_BYTES
            {
                scratch.shapes.push(shape);
            }
        }
        self.width = width;
        self.height = height;
        self.buffer_x = buffer_x;
        debug_assert_eq!(self.verify_integrity(), Ok(()));
    }

    /// Checks the column invariants hit-testing, carets, and painting rely
    /// on: equal column lengths, lines and runs tiling the glyphs in order,
    /// line starts strictly increasing, and every byte offset in bounds and on
    /// a char boundary. [`TextSystem::layout`](crate::TextSystem::layout)
    /// calls this through `debug_assert!`, so release builds skip it.
    pub fn verify_integrity(&self) -> Result<(), IntegrityError> {
        let g = &self.glyphs;
        let l = &self.lines;
        let r = &self.runs;
        let glyph_count = g.len();
        let line_count = l.len();
        let run_count = r.len();
        // `hit` and `caret` index line `count - 1` unconditionally.
        if line_count == 0 {
            return Err(IntegrityError::NoLines);
        }
        if !(self.width.is_finite() && self.width >= 0.0)
            || !(self.height.is_finite() && self.height >= 0.0)
        {
            return Err(IntegrityError::Size {
                width: self.width,
                height: self.height,
            });
        }

        let text = self.text();
        let in_text = |start: usize, end: usize| {
            start <= end
                && end <= text.len()
                && text.is_char_boundary(start)
                && text.is_char_boundary(end)
        };
        let mut next_glyph = 0;
        for line in 0..line_count {
            let (gs, ge) = (l[line].glyph_start as usize, l[line].glyph_end as usize);
            if gs != next_glyph || ge < gs || ge > glyph_count {
                return Err(IntegrityError::LineGlyphs {
                    line,
                    start: gs,
                    end: ge,
                    expected: next_glyph,
                });
            }
            next_glyph = ge;
            let (bs, be) = (l[line].byte_start as usize, l[line].byte_end as usize);
            // `line_for_byte` binary-searches line starts.
            let after_previous = line == 0 || bs > l[line - 1].byte_start as usize;
            let before_next = l
                .get(line + 1)
                .is_none_or(|next| be <= next.byte_start as usize);
            if !in_text(bs, be) || !after_previous || !before_next {
                return Err(IntegrityError::LineBytes {
                    line,
                    start: bs,
                    end: be,
                });
            }
        }
        if next_glyph != glyph_count {
            return Err(IntegrityError::LineGlyphs {
                line: line_count,
                start: glyph_count,
                end: glyph_count,
                expected: next_glyph,
            });
        }

        for (glyph, record) in g.iter().enumerate() {
            let (start, end) = (record.byte_start as usize, record.byte_end as usize);
            if !in_text(start, end) {
                return Err(IntegrityError::GlyphBytes { glyph, start, end });
            }
            let line = record.line as usize;
            let on_line = l.get(line).is_some_and(|l| l.glyphs().contains(&glyph));
            if !on_line {
                return Err(IntegrityError::GlyphLine { glyph, line });
            }
            if record.span as usize > self.spans().len() {
                return Err(IntegrityError::GlyphSpan {
                    glyph,
                    span: record.span,
                    spans: self.spans().len(),
                });
            }
        }

        let mut next_glyph = 0;
        for (run, record) in r.iter().enumerate() {
            let (gs, ge) = (record.glyph_start as usize, record.glyph_end as usize);
            let line = record.line as usize;
            let same_line = gs < ge
                && ge <= glyph_count
                && line < line_count
                && (gs..ge).all(|i| g[i].line as usize == line);
            if gs != next_glyph || !same_line {
                return Err(IntegrityError::RunGlyphs {
                    run,
                    start: gs,
                    end: ge,
                    expected: next_glyph,
                });
            }
            next_glyph = ge;
        }
        if next_glyph != glyph_count {
            return Err(IntegrityError::RunGlyphs {
                run: run_count,
                start: glyph_count,
                end: glyph_count,
                expected: next_glyph,
            });
        }
        Ok(())
    }

    pub fn text(&self) -> &str {
        self.source.as_str()
    }

    /// The text and spans, shared: hold this to keep the text of what was
    /// painted without copying it.
    pub fn source(&self) -> &TextSource {
        &self.source
    }

    /// The inputs this layout was laid out from, borrowed: look it up again
    /// at another wrap width without copying the text.
    pub fn query(&self) -> TextQuery<'_> {
        TextQuery {
            text: self.source.as_str(),
            spans: self.source.spans(),
            style: self.style,
            wrap_width: self.wrap_width,
            scale_factor: self.scale_factor,
        }
    }

    /// Logical width of the widest piece of text between two line break
    /// opportunities (UAX #14, where wrapping breaks first), without its
    /// trailing spaces: the narrowest wrap width that splits no word. Below
    /// it, wrapping falls back to breaking between glyphs.
    pub fn min_content_width(&self) -> f32 {
        let text = self.text();
        // No-break spaces do not end a word.
        let breaking_space =
            |c: char| c.is_whitespace() && !matches!(c, '\u{a0}' | '\u{2007}' | '\u{202f}');
        // `(end, end without trailing spaces, width)` per segment, in text
        // order; glyphs are not in text order, so each one finds its
        // segment by binary search.
        let mut start = 0;
        let mut segments: Vec<(usize, usize, f32)> = unicode_linebreak::linebreaks(text)
            .map(|(end, _)| {
                let segment = text.get(start..end).unwrap_or_default();
                let trimmed = start + segment.trim_end_matches(breaking_space).len();
                let item = (end, trimmed, 0.0);
                start = end;
                item
            })
            .collect();
        let glyphs = &self.glyphs;
        for glyph in glyphs {
            let byte = glyph.byte_start as usize;
            let at = segments.partition_point(|&(end, _, _)| end <= byte);
            if let Some((_, trimmed, width)) = segments.get_mut(at)
                && byte < *trimmed
            {
                *width += glyph.advance;
            }
        }
        segments.iter().fold(0.0, |widest, s| widest.max(s.2))
    }

    pub fn spans(&self) -> &[TextSpan] {
        self.source.spans()
    }

    pub fn style(&self) -> TextStyle {
        self.style
    }

    pub fn wrap_width(&self) -> Option<f32> {
        self.wrap_width
    }

    pub fn scale_factor(&self) -> f32 {
        self.scale_factor
    }

    /// `(width, height)` in logical pixels.
    pub fn size(&self) -> (f32, f32) {
        (self.width, self.height)
    }

    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    pub fn line(&self, i: usize) -> Option<LineInfo> {
        (i < self.line_count()).then(|| self.line_info(i))
    }

    pub fn lines(&self) -> impl ExactSizeIterator<Item = LineInfo> + '_ {
        (0..self.line_count()).map(|i| self.line_info(i))
    }

    fn line_info(&self, i: usize) -> LineInfo {
        let l = &self.lines[i];
        LineInfo {
            byte_range: l.byte_start as usize..l.byte_end as usize,
            top: l.top,
            height: l.height,
            baseline: l.baseline,
            width: l.width,
            glyph_range: l.glyphs(),
            rtl: l.rtl,
        }
    }

    pub fn glyphs(&self) -> &[Glyph] {
        &self.glyphs
    }

    pub fn glyph_count(&self) -> usize {
        self.glyphs.len()
    }

    /// Glyph `i`, in storage order (see [`GlyphColumns`]).
    pub fn glyph(&self, i: usize) -> Option<Glyph> {
        self.glyphs.get(i).copied()
    }

    /// Every glyph, in storage order.
    pub fn glyph_iter(&self) -> impl ExactSizeIterator<Item = Glyph> + '_ {
        self.glyphs.iter().copied()
    }

    /// Runs in glyph order, row by row from the top.
    pub fn glyph_runs(&self) -> impl ExactSizeIterator<Item = GlyphRun> + '_ {
        self.runs.iter().map(|r| GlyphRun {
            line: r.line as usize,
            span: r.span,
            rtl: r.rtl,
            glyphs: r.glyph_start as usize..r.glyph_end as usize,
            phys_top: r.top,
            phys_height: r.height,
            phys_baseline: r.baseline,
        })
    }

    /// Rasterization key and integer pixel position (baseline included) of
    /// glyph `i`, with the layout origin at `origin` in physical pixels:
    /// exactly where the renderer's buffer path draws it. That path places
    /// each row's glyphs with cosmic-text's `LayoutGlyph::physical`, which
    /// truncates the glyph's own vertical offset from the origin, then adds
    /// the row's baseline rounded on its own.
    pub fn physical_glyph(&self, i: usize, origin: (f32, f32)) -> Option<PhysicalGlyph> {
        let glyph = self.glyphs.get(i)?;
        let run = &self.runs[self.runs.partition_point(|r| r.glyph_end as usize <= i)];
        Some(self.place(glyph, run.baseline, origin))
    }

    /// [`Self::physical_glyph`] of each of `run`'s glyphs, in order, without
    /// looking its row up again.
    pub fn physical_run(
        &self,
        run: &GlyphRun,
        origin: (f32, f32),
    ) -> impl ExactSizeIterator<Item = PhysicalGlyph> + '_ {
        let baseline = run.phys_baseline;
        self.glyphs[run.glyphs.clone()]
            .iter()
            .map(move |glyph| self.place(glyph, baseline, origin))
    }

    fn place(&self, g: &Glyph, baseline: f32, origin: (f32, f32)) -> PhysicalGlyph {
        // The renderer's text area origin, summed as it sums it.
        let left = origin.0 + self.buffer_x;
        let (cache_key, x, y) = CacheKey::new(
            g.font_id,
            g.glyph_id,
            g.font_size,
            (g.phys_dx + left, (g.phys_dy + origin.1).trunc()),
            g.font_weight,
            g.flags,
        );
        PhysicalGlyph {
            cache_key,
            x,
            y: y + baseline.round() as i32,
        }
    }

    /// The shaped cosmic-text buffer (physical pixels), for glyphon.
    /// Draw it [`Self::buffer_x`] right of the layout origin.
    pub fn buffer(&self) -> &Buffer {
        &self.buffer
    }

    /// Physical x at which [`Self::buffer`] is drawn relative to the layout
    /// origin. Nonzero only when a right-to-left line overflows the wrap
    /// width; the glyph columns already include it.
    pub fn buffer_x(&self) -> f32 {
        self.buffer_x
    }

    /// Grapheme boundary nearest to the logical point. Points outside the
    /// text clamp to the nearest line and line edge.
    pub fn hit(&self, x: f32, y: f32) -> TextOffset {
        let line = self.line_at_y(y);
        let g = &self.glyphs;
        let range = self.glyph_range(line);
        if range.is_empty() {
            // An empty line can start inside a grapheme ("\n\r\n" is "\n\r"
            // plus "\n" to the layout but "\n" plus "\r\n" as graphemes).
            return TextOffset::snap(self.text(), self.lines[line].byte_start as usize);
        }
        // Glyph storage order is not visual (RTL runs are stored logically),
        // so scan for the containing glyph and the visual extremes.
        let (mut left, mut right, mut inside) = (range.start, range.start, None);
        let mut nearest = (f32::INFINITY, range.start);
        for i in range {
            let (x0, x1) = (g[i].x, g[i].x + g[i].advance);
            if x0 < g[left].x {
                left = i;
            }
            if x1 > g[right].x + g[right].advance {
                right = i;
            }
            if inside.is_none() && x >= x0 && x < x1 {
                inside = Some(i);
            }
            let distance = if x < x0 { x0 - x } else { (x - x1).max(0.0) };
            if distance < nearest.0 {
                nearest = (distance, i);
            }
        }
        let byte = if x < g[left].x {
            self.visual_left_byte(left)
        } else if x >= g[right].x + g[right].advance {
            self.visual_right_byte(right)
        } else {
            // Falls back to the nearest glyph when x is in a gap between glyphs.
            let i = inside.unwrap_or(nearest.1);
            let (x0, x1) = self.cluster_extent(i);
            let w = x1 - x0;
            let mut frac = if w > 0.0 { (x - x0) / w } else { 0.0 };
            if g[i].rtl() {
                frac = 1.0 - frac;
            }
            self.cluster_byte_at(i, frac.clamp(0.0, 1.0))
        };
        let start = self.lines[line].byte_start as usize;
        let end = self.lines[line].byte_end as usize;
        TextOffset::snap(self.text(), byte.clamp(start, end))
    }

    /// Caret position for an offset (a raw index is clamped and snapped down
    /// to a grapheme boundary). At a soft wrap the caret goes to the start
    /// of the next line.
    pub fn caret(&self, offset: impl ToTextOffset) -> Caret {
        let byte = offset.to_offset(self.text()).get();
        let line = self.line_for_byte(byte);
        Caret {
            x: self.x_for_byte(line, byte),
            y: self.lines[line].top,
            height: self.lines[line].height,
            line,
        }
    }

    /// Highlight rectangles (logical pixels) covering `range` (either
    /// order), one or more per line; RTL/mixed runs may produce several per
    /// line. Visits only the lines the range touches.
    pub fn selection_rects(&self, range: Range<impl ToTextOffset>) -> std::vec::IntoIter<Rect> {
        let len = self.text().len();
        // Snap like `caret` so arbitrary offsets never slice inside a char
        // and rect edges line up with carets.
        let range = crate::offset::ordered(self.text(), range);
        let (a, b) = (range.start.get(), range.end.get());
        let mut rects = Vec::new();
        if a == b {
            return rects.into_iter();
        }
        let g = &self.glyphs;
        for line in self.line_for_byte(a)..self.line_count() {
            let start = self.lines[line].byte_start as usize;
            if start >= b {
                break;
            }
            // Include the line ending so an empty line inside the selection shows.
            let end_with_break = self
                .lines
                .get(line + 1)
                .map_or(len, |next| next.byte_start as usize);
            if start >= b || end_with_break <= a {
                continue;
            }
            let top = self.lines[line].top;
            let height = self.lines[line].height;
            let glyph_range = self.glyph_range(line);
            if glyph_range.is_empty() {
                if a <= start && b > start {
                    let width = self.style.font_size * 0.3;
                    rects.push(Rect {
                        x: 0.0,
                        y: top,
                        width,
                        height,
                    });
                }
                continue;
            }
            let mut spans: Vec<(f32, f32)> = Vec::new();
            for i in glyph_range {
                let (gs, ge) = (g[i].byte_start as usize, g[i].byte_end as usize);
                let (os, oe) = (gs.max(a), ge.min(b));
                if os >= oe {
                    continue;
                }
                if os == gs && oe == ge {
                    spans.push((g[i].x, g[i].x + g[i].advance));
                } else {
                    let p = self.x_in_cluster(i, os);
                    let q = self.x_in_cluster(i, oe);
                    spans.push((p.min(q), p.max(q)));
                }
            }
            spans.sort_by(|p, q| p.0.total_cmp(&q.0));
            let mut current: Option<(f32, f32)> = None;
            for (x0, x1) in spans {
                current = match current {
                    Some((cx0, cx1)) if x0 <= cx1 + 0.5 => Some((cx0, x1.max(cx1))),
                    Some((cx0, cx1)) => {
                        rects.push(rect_span(cx0, cx1, top, height));
                        Some((x0, x1))
                    }
                    None => Some((x0, x1)),
                };
            }
            if let Some((cx0, cx1)) = current {
                rects.push(rect_span(cx0, cx1, top, height));
            }
        }
        rects.into_iter()
    }

    fn glyph_range(&self, line: usize) -> Range<usize> {
        self.lines[line].glyphs()
    }

    fn line_at_y(&self, y: f32) -> usize {
        // Line tops increase, so the last line starting at or above `y`
        // holds it; `build` guarantees at least one line.
        self.lines
            .partition_point(|line| line.top <= y)
            .saturating_sub(1)
    }

    fn line_for_byte(&self, byte: usize) -> usize {
        // Line starts are strictly increasing, so the last start <= byte wins.
        self.lines
            .partition_point(|line| line.byte_start as usize <= byte)
            .saturating_sub(1)
    }

    fn visual_left_byte(&self, i: usize) -> usize {
        let g = &self.glyphs;
        if g[i].rtl() {
            g[i].byte_end as usize
        } else {
            g[i].byte_start as usize
        }
    }

    fn visual_right_byte(&self, i: usize) -> usize {
        let g = &self.glyphs;
        if g[i].rtl() {
            g[i].byte_start as usize
        } else {
            g[i].byte_end as usize
        }
    }

    fn x_for_byte(&self, line: usize, byte: usize) -> f32 {
        let g = &self.glyphs;
        let range = self.glyph_range(line);
        if range.is_empty() {
            return 0.0;
        }
        let mut ending_at: Option<usize> = None;
        let mut first_logical = range.start;
        for i in range {
            let (gs, ge) = (g[i].byte_start as usize, g[i].byte_end as usize);
            if gs <= byte && byte < ge {
                return self.x_in_cluster(i, byte);
            }
            if ge <= byte && ending_at.is_none_or(|j| g[j].byte_end < g[i].byte_end) {
                ending_at = Some(i);
            }
            if gs < g[first_logical].byte_start as usize {
                first_logical = i;
            }
        }
        match ending_at {
            Some(i) => self.trailing_x(i),
            None => self.leading_x(first_logical),
        }
    }

    fn leading_x(&self, i: usize) -> f32 {
        let (x0, x1) = self.cluster_extent(i);
        if self.glyphs[i].rtl() { x1 } else { x0 }
    }

    fn trailing_x(&self, i: usize) -> f32 {
        let (x0, x1) = self.cluster_extent(i);
        if self.glyphs[i].rtl() { x0 } else { x1 }
    }

    /// Visual `(left, right)` of glyph `i`'s whole cluster. A cluster can
    /// shape to several adjacent glyphs (a ZWJ sequence without a color font,
    /// marks the font cannot compose), and carets must use the cluster's edges
    /// rather than its first glyph's.
    fn cluster_extent(&self, i: usize) -> (f32, f32) {
        let g = &self.glyphs;
        let same = |j: usize| {
            g[j].line == g[i].line
                && g[j].byte_start == g[i].byte_start
                && g[j].byte_end == g[i].byte_end
        };
        let mut first = i;
        while first > 0 && same(first - 1) {
            first -= 1;
        }
        let (mut x0, mut x1) = (f32::INFINITY, f32::NEG_INFINITY);
        for j in (first..g.len()).take_while(|&j| same(j)) {
            x0 = x0.min(g[j].x);
            x1 = x1.max(g[j].x + g[j].advance);
        }
        (x0, x1)
    }

    /// x of `byte` inside glyph `i`'s cluster, splitting the advance evenly
    /// between graphemes (ligatures cover several).
    fn x_in_cluster(&self, i: usize, byte: usize) -> f32 {
        let g = &self.glyphs;
        let (gs, ge) = (g[i].byte_start as usize, g[i].byte_end as usize);
        let cluster = self.text().get(gs..ge).unwrap_or_default();
        let total = cluster.graphemes(true).count().max(1);
        let before = self
            .text()
            .get(gs..byte.clamp(gs, ge))
            .map_or(0, |s| s.graphemes(true).count());
        let mut frac = before as f32 / total as f32;
        if g[i].rtl() {
            frac = 1.0 - frac;
        }
        let (x0, x1) = self.cluster_extent(i);
        x0 + (x1 - x0) * frac
    }

    /// Byte at logical fraction `frac` (0 = cluster start) through glyph `i`'s
    /// cluster, rounded to the nearest grapheme boundary.
    fn cluster_byte_at(&self, i: usize, frac: f32) -> usize {
        let g = &self.glyphs;
        let (gs, ge) = (g[i].byte_start as usize, g[i].byte_end as usize);
        let cluster = self.text().get(gs..ge).unwrap_or_default();
        let total = cluster.graphemes(true).count().max(1);
        let k = (frac * total as f32).round() as usize;
        if k >= total {
            return ge;
        }
        cluster
            .grapheme_indices(true)
            .nth(k)
            .map_or(ge, |(offset, _)| gs + offset)
    }
}

fn rect_span(x0: f32, x1: f32, y: f32, height: f32) -> Rect {
    Rect {
        x: x0,
        y,
        width: x1 - x0,
        height,
    }
}

/// Refills `runs` with each line's maximal glyph ranges of one span and
/// direction.
/// How many runs [`push_row_runs`] makes of a row's glyphs.
fn run_breaks(glyphs: &[cosmic_text::LayoutGlyph]) -> usize {
    let breaks = glyphs
        .windows(2)
        .filter(|w| {
            w[0].metadata as u32 != w[1].metadata as u32
                || w[0].level.is_rtl() != w[1].level.is_rtl()
        })
        .count();
    breaks + usize::from(!glyphs.is_empty())
}

/// Splits the glyphs from `start` on, one row's, into runs at span and
/// direction changes.
fn push_row_runs(
    runs: &mut Vec<Run>,
    glyphs: &GlyphColumns,
    start: usize,
    line: u32,
    row: &cosmic_text::LayoutRun,
) {
    let mut run_start = start;
    for i in start..glyphs.len() {
        let next_breaks = i + 1 == glyphs.len()
            || glyphs[i + 1].span != glyphs[i].span
            || glyphs[i + 1].rtl() != glyphs[i].rtl();
        if next_breaks {
            runs.push(Run {
                line,
                span: glyphs[i].span,
                rtl: glyphs[i].rtl(),
                glyph_start: run_start as u32,
                glyph_end: (i + 1) as u32,
                top: row.line_top,
                height: row.line_height,
                baseline: row.line_y,
            });
            run_start = i + 1;
        }
    }
}

/// Splits on `\n`, `\r\n`, `\r`, `\n\r` like cosmic-text's
/// `Buffer::set_text`, and also on the other Unicode bidi paragraph
/// separators (U+001C..U+001E, U+0085, U+2029), always ending with a
/// paragraph that has no line ending (empty after a trailing newline).
///
/// cosmic-text asserts that a line is one bidi paragraph, so a separator
/// followed by text of the other direction would panic if left inside.
fn split_paragraphs(text: &str, out: &mut Vec<(Range<usize>, LineEnding)>) {
    const OTHER_SEPARATORS: [char; 5] = ['\u{1c}', '\u{1d}', '\u{1e}', '\u{85}', '\u{2029}'];
    out.clear();
    let mut start = 0;
    let rest = |at: usize| text.get(at..).unwrap_or_default();
    while let Some(i) =
        rest(start).find(|c| c == '\r' || c == '\n' || OTHER_SEPARATORS.contains(&c))
    {
        let end = start + i;
        let after = rest(end);
        let (ending, len) = if after.starts_with("\r\n") {
            (LineEnding::CrLf, 2)
        } else if after.starts_with("\n\r") {
            (LineEnding::LfCr, 2)
        } else if after.starts_with('\n') {
            (LineEnding::Lf, 1)
        } else if after.starts_with('\r') {
            (LineEnding::Cr, 1)
        } else {
            // LineEnding has no variant for these; it is only metadata to
            // cosmic-text, and offsets come from `len`.
            let sep = after.chars().next().map_or(1, char::len_utf8);
            (LineEnding::Lf, sep)
        };
        out.push((start..end, ending));
        start = end + len;
    }
    out.push((start..text.len(), LineEnding::None));
}

fn family(kind: FontKind) -> Family<'static> {
    match kind {
        FontKind::Ui => Family::SansSerif,
        FontKind::Mono => Family::Monospace,
    }
}

/// The CSS weight text in `weight` asks for, the same in every family:
/// Normal is 400 (an app that wants a heavier normal asks for that
/// weight), and a numeric weight outside 1 to 1000 is clamped into it.
pub(crate) fn font_weight_value(weight: FontWeight) -> u16 {
    weight.value()
}

/// Points emoji-presentation clusters at the color emoji family, upright and
/// at its one weight, keeping the span metadata that colors them.
fn emoji_spans(attrs: &mut AttrsList, paragraph: &str, emoji: &'static str) {
    if paragraph.is_ascii() {
        return;
    }
    for (start, grapheme) in paragraph.grapheme_indices(true) {
        if !crate::fonts::is_emoji_presentation(grapheme) {
            continue;
        }
        let owned = AttrsOwned::new(&attrs.get_span(start));
        let emoji_attrs = owned
            .as_attrs()
            .family(Family::Name(emoji))
            .weight(cosmic_text::Weight::NORMAL)
            .style(cosmic_text::Style::Normal)
            .cache_key_flags(CacheKeyFlags::empty());
        attrs.add_span(start..start + grapheme.len(), &emoji_attrs);
    }
}

/// Points clusters that should draw as text at a face without color
/// glyphs when the text's own face lacks them and the color emoji face
/// has them (see [`crate::fonts::TextFaces`]).
fn text_spans(
    attrs: &mut AttrsList,
    paragraph: &str,
    fs: &mut FontSystem,
    faces: &mut crate::fonts::TextFaces,
    faces_of_text: crate::fonts::Faces,
) {
    for (start, grapheme) in paragraph.grapheme_indices(true) {
        let Some(c) = grapheme.chars().next().filter(|c| !c.is_ascii()) else {
            continue;
        };
        if crate::fonts::is_emoji_presentation(grapheme) {
            continue;
        }
        let Some(family) = faces.text_family(fs, c, faces_of_text) else {
            continue;
        };
        let db = fs.db();
        let owned = AttrsOwned::new(&attrs.get_span(start));
        // cosmic-text takes a named family only at a weight it has, so ask
        // for the face nearest the text's weight.
        let weight = db
            .query(&fontdb::Query {
                families: &[Family::Name(&family)],
                weight: owned.weight,
                ..fontdb::Query::default()
            })
            .and_then(|id| db.face(id))
            .map_or(owned.weight, |face| face.weight);
        let text_attrs = owned
            .as_attrs()
            .family(Family::Name(&family))
            .weight(weight);
        attrs.add_span(start..start + grapheme.len(), &text_attrs);
    }
}

/// Sets `features` to none (the font's defaults) with ligatures on;
/// otherwise the ligature features turned off.
fn set_font_features(features: &mut cosmic_text::FontFeatures, ligatures: bool) {
    features.features.clear();
    if !ligatures {
        for tag in [
            cosmic_text::FeatureTag::STANDARD_LIGATURES,
            cosmic_text::FeatureTag::CONTEXTUAL_LIGATURES,
            cosmic_text::FeatureTag::CONTEXTUAL_ALTERNATES,
        ] {
            features.disable(tag);
        }
    }
}

/// Attributes of text no span styles; `own_family` is the family of text
/// in `style` that no span re-kinds.
fn base_attrs<'a>(style: &TextStyle, own_family: Family<'a>) -> Attrs<'a> {
    let attrs = Attrs::new()
        .family(own_family)
        .weight(cosmic_text::Weight(font_weight_value(style.font_weight)))
        .cache_key_flags(style_flags(style));
    if style.letter_spacing != 0.0 {
        attrs.letter_spacing(style.letter_spacing)
    } else {
        attrs
    }
}

/// Rasterization flags the whole block's style asks for.
fn style_flags(style: &TextStyle) -> CacheKeyFlags {
    let flags = if style.thicken {
        CacheKeyFlags::THICKEN
    } else {
        CacheKeyFlags::empty()
    };
    match style.linear_correction {
        Some(luminance) => flags.linear_corrected(luminance),
        None => flags,
    }
}

/// Which generic families lack an italic face. Their italic spans are
/// slanted by the rasterizer instead (cosmic-text only does that when asked),
/// so italic stays visible with fonts such as Geist that ship no italic.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SyntheticItalic {
    ui: bool,
    mono: bool,
    /// For the style's named family, when it has one.
    named: bool,
}

impl SyntheticItalic {
    pub(crate) fn new(fs: &FontSystem) -> Self {
        let db = fs.db();
        let lacks_italic = |generic: Family| {
            let name = db.family_name(&generic);
            !db.faces().any(|face| {
                face.style != fontdb::Style::Normal
                    && face.families.iter().any(|(family, _)| family == name)
            })
        };
        Self {
            ui: lacks_italic(Family::SansSerif),
            mono: lacks_italic(Family::Monospace),
            named: false,
        }
    }

    /// With `named` set for `family`, a style's family by name.
    fn for_family(self, fs: &FontSystem, family: Option<&str>) -> Self {
        let Some(name) = family else {
            return self;
        };
        let named = !fs.db().faces().any(|face| {
            face.style != fontdb::Style::Normal
                && face.families.iter().any(|(family, _)| family == name)
        });
        Self { named, ..self }
    }

    fn needed(self, kind: FontKind) -> bool {
        match kind {
            FontKind::Ui => self.ui,
            FontKind::Mono => self.mono,
        }
    }
}

fn span_attrs<'a>(
    style: &TextStyle,
    own_family: Family<'a>,
    span: &TextSpan,
    index: usize,
    synth: SyntheticItalic,
    scale: f32,
) -> Attrs<'a> {
    let kind = span.kind.unwrap_or(style.font_kind);
    let weight = span.weight.unwrap_or(style.font_weight);
    let italic = span.style == Some(FontStyle::Italic);
    // The style's named family, unless the span picks a kind of its own.
    let named = span.kind.is_none() && matches!(own_family, Family::Name(_));
    let font_style = if italic {
        cosmic_text::Style::Italic
    } else {
        cosmic_text::Style::Normal
    };
    let synthesize = if named {
        synth.named
    } else {
        synth.needed(kind)
    };
    let flags = if italic && synthesize {
        CacheKeyFlags::FAKE_ITALIC
    } else {
        CacheKeyFlags::empty()
    } | style_flags(style);
    let mut attrs = base_attrs(style, own_family)
        .family(if named { own_family } else { family(kind) })
        .weight(cosmic_text::Weight(font_weight_value(weight)))
        .style(font_style)
        .cache_key_flags(flags)
        .metadata(index + 1);
    if let Some(size) = span.size {
        attrs = attrs.metrics(Metrics::new(size * scale, style.line_height * scale));
    }
    if let Some(ems) = span.letter_spacing {
        attrs = attrs.letter_spacing(ems);
    }
    attrs
}
#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use quark::scene::FontStyle;
    use quark::{FontKind, FontWeight};
    use unicode_segmentation::UnicodeSegmentation;

    use super::*;
    use crate::fonts::FontSettings;
    use crate::system::{TextSystem, test_system};

    const LOREM: &str = "The quick brown fox jumps over the lazy dog while the sleepy cat \
                         watches from a sunny windowsill and dreams of mice.";

    fn layout(text: &str, wrap: Option<f32>) -> TextLayout {
        let params = TextParams::new(text, TextStyle::new(14.0)).wrap_width(wrap);
        test_system().layout(&params).expect("layout")
    }

    /// Everything hit-testing, carets, and painting read from a layout, as
    /// text.
    fn dump(layout: &TextLayout) -> String {
        let buffer_runs: Vec<_> = layout
            .buffer()
            .layout_runs()
            .map(|run| (run.line_i, run.line_w, run.glyphs.to_vec()))
            .collect();
        format!(
            "{:?}\n{:?}\n{:?}\n{:?}\nbuffer_x {}\n{buffer_runs:?}",
            layout.size(),
            layout.lines().collect::<Vec<_>>(),
            layout.glyph_runs().collect::<Vec<_>>(),
            layout.glyphs(),
            layout.buffer_x(),
        )
    }

    /// A span styling `text` up to the char boundary at or before `end`.
    fn prefix_span(text: &str, end: usize, style: FontStyle) -> Vec<TextSpan> {
        let end = (0..=end.min(text.len()))
            .rev()
            .find(|&i| text.is_char_boundary(i))
            .unwrap_or(0);
        vec![TextSpan {
            range: 0..end,
            weight: Some(FontWeight::Bold),
            style: Some(style),
            kind: None,
            size: None,
            letter_spacing: None,
        }]
    }

    fn grapheme_boundaries(text: &str) -> Vec<usize> {
        let mut out: Vec<usize> = text.grapheme_indices(true).map(|(i, _)| i).collect();
        out.push(text.len());
        out
    }

    /// `PROPTEST_CASES` overrides the per-property default for heavier runs.
    fn config(default_cases: u32) -> ProptestConfig {
        let cases = std::env::var("PROPTEST_CASES")
            .ok()
            .and_then(|v| v.parse().ok())
            // Miri hides host env vars under isolation, so it gets its own
            // small default.
            .unwrap_or(if cfg!(miri) { 4 } else { default_cases });
        let mut config = ProptestConfig::with_cases(cases);
        if cfg!(miri) {
            config.failure_persistence = None;
        }
        config
    }

    /// Left-to-right pieces: ligature candidates, multibyte chars, a
    /// combining sequence, ZWJ emoji sequences with and without a ligature,
    /// spaces, and every line ending.
    const LTR_PIECES: &[&str] = &[
        "a",
        "b",
        "W",
        "fi",
        "ffi",
        "office",
        " ",
        "  ",
        "\n",
        "\r\n",
        "\r",
        "\u{e9}",
        "e\u{301}",
        "\u{3b1}",
        "\u{65e5}",
        "\u{1f600}",
        "\u{1f469}\u{200d}\u{1f4bb}",
        MULTI_GLYPH_CLUSTER,
        "\u{2029}",
    ];
    const RTL_PIECES: &[&str] = &["\u{5e9}\u{5dc}", "\u{5d5}\u{5dd} ", "\u{627}\u{644}"];

    fn text(pieces: &'static [&'static str]) -> impl Strategy<Value = String> {
        prop::collection::vec(prop::sample::select(pieces), 0..24).prop_map(|v| {
            // The layout reads "\n\r" as one line ending while grapheme
            // segmentation pairs a following "\r\n", so a line can start
            // inside a grapheme and no caret round trip is possible there.
            let mut text = v.concat();
            while text.contains("\n\r") {
                text = text.replace("\n\r", "\n");
            }
            text
        })
    }

    fn mixed_text() -> impl Strategy<Value = String> {
        let all: &'static [&'static str] = Box::leak([LTR_PIECES, RTL_PIECES].concat().into());
        text(all)
    }

    fn wrap() -> impl Strategy<Value = Option<f32>> {
        prop_oneof![Just(None), (1.0f32..300.0).prop_map(Some)]
    }

    fn assert_rects_within_bounds(
        layout: &TextLayout,
        range: Range<usize>,
    ) -> Result<(), TestCaseError> {
        let (width, height) = layout.size();
        // Empty lines get a 0.3 em marker even in a narrower layout.
        let max_x = width.max(layout.style().font_size * 0.3) + 0.01;
        for r in layout.selection_rects(range.clone()) {
            prop_assert!(
                r.x >= -0.01
                    && r.y >= -0.01
                    && r.x + r.width <= max_x
                    && r.y + r.height <= height + 0.01,
                "rect {:?} outside {}x{} for {:?}",
                r,
                width,
                height,
                range
            );
        }
        Ok(())
    }

    proptest! {
        #![proptest_config(config(48))]

        // Catches carets and hits disagreeing (a click lands one grapheme off)
        // at wraps, ligatures, combining marks, and line endings.
        #[test]
        fn layout_caret_then_hit_returns_each_grapheme_boundary(
            text in text(LTR_PIECES),
            wrap in wrap(),
        ) {
            let layout = layout(&text, wrap);
            prop_assert_eq!(layout.verify_integrity(), Ok(()));
            // Some distinct offsets share one caret: whitespace hung past a
            // wrap has no glyphs, and "\n\r" is one line ending but two
            // graphemes. Those must round-trip to the same caret; the rest
            // must round-trip to the same byte.
            let carets: Vec<(usize, Caret)> = grapheme_boundaries(&text)
                .into_iter()
                .map(|b| (b, layout.caret(b)))
                .collect();
            for &(b, caret) in &carets {
                let hit = layout.hit(caret.x, caret.y + caret.height * 0.5);
                prop_assert_eq!(layout.caret(hit), caret, "byte {} in {:?}", b, text);
                if carets.iter().filter(|(_, c)| *c == caret).count() == 1 {
                    prop_assert_eq!(hit, b, "caret {:?} in {:?}", caret, text);
                }
            }
            // Offsets past the end clamp to the end.
            prop_assert_eq!(layout.caret(text.len() + 7), layout.caret(text.len()));
        }

        // Catches hit returning an offset inside a grapheme (or past the
        // text) for bidi text and points off the layout. Carets at bidi run
        // boundaries have two visual positions, so this does not demand a
        // round trip; the left-to-right property above does.
        #[test]
        fn layout_hit_in_bidi_text_returns_grapheme_boundary(
            text in mixed_text(),
            wrap in wrap(),
            points in prop::collection::vec((-50.0f32..400.0, -50.0f32..400.0), 1..8),
        ) {
            let layout = layout(&text, wrap);
            prop_assert_eq!(layout.verify_integrity(), Ok(()));
            let boundaries = grapheme_boundaries(&text);
            for (x, y) in points {
                let b = layout.hit(x, y);
                prop_assert!(boundaries.contains(&b.get()), "hit({x}, {y}) = {b} in {:?}", text);
                let caret = layout.caret(b);
                prop_assert!(caret.x.is_finite() && caret.line < layout.line_count());
            }
        }

        // Catches selection painting outside the measured box: right-to-left
        // lines aligned against the wrap width, or a cluster wider than the
        // wrap pushed to negative x.
        // Catches state a rebuild carries over from the layout whose storage
        // it reuses: extra paragraphs, stale glyphs, runs, or line ranges.
        #[test]
        fn layout_rebuilt_in_another_layouts_storage_matches_fresh_layout(
            first in mixed_text(),
            second in mixed_text(),
            wraps in (wrap(), wrap()),
            span_ends in (0usize..40, 0usize..40),
        ) {
            let style = TextStyle::new(14.0);
            let first_spans = prefix_span(&first, span_ends.0, FontStyle::Italic);
            let second_spans = prefix_span(&second, span_ends.1, FontStyle::Normal);
            let mut system = test_system();
            let first = TextParams::new(first, style).spans(first_spans).wrap_width(wraps.0);
            let mut reused = system.layout(&first).expect("layout");
            let second = TextQuery {
                spans: &second_spans,
                ..TextQuery::new(&second, style).wrap_width(wraps.1)
            };
            reused.copy_inputs(&second);
            system.rebuild(&mut reused);
            let fresh = system.layout(&second.to_params()).expect("layout");
            prop_assert_eq!(dump(&reused), dump(&fresh));
        }

        #[test]
        fn layout_selection_rects_stay_within_layout_bounds(
            text in mixed_text(),
            wrap in wrap(),
            a in 0usize..120,
            b in 0usize..120,
        ) {
            let layout = layout(&text, wrap);
            assert_rects_within_bounds(&layout, a..b)?;
        }
    }

    #[test]
    fn layout_narrower_wrap_width_adds_lines_within_width() {
        assert_eq!(layout(LOREM, None).line_count(), 1);
        let mut previous = 1;
        for width in [400.0, 200.0, 100.0] {
            let wrapped = layout(LOREM, Some(width));
            assert!(wrapped.line_count() > previous, "width {width}");
            previous = wrapped.line_count();
            let (w, h) = wrapped.size();
            assert!(w <= width + 1.0, "width {w} exceeds wrap {width}");
            let line_height = 14.0 * DEFAULT_LINE_HEIGHT_FACTOR;
            assert!((h - line_height * wrapped.line_count() as f32).abs() < 0.5);
        }
    }

    // Regression: a built layout handed its lines' shaping back for reuse
    // and left them without one, so the buffer the renderer paints from
    // yielded no layout runs and every text drew nothing.
    #[test]
    fn built_layouts_still_yield_the_runs_painting_reads() {
        let mut system = test_system();
        for text in ["first build lends its shaping", "hello\nمرحبا بالعالم"] {
            let params = TextParams::new(text, TextStyle::new(14.0));
            let layout = system.layout(&params).expect("layout");
            let runs: Vec<(bool, usize)> = layout
                .buffer()
                .layout_runs()
                .map(|run| (run.rtl, run.glyphs.len()))
                .collect();
            assert_eq!(runs.len(), layout.line_count(), "{text:?}: {runs:?}");
            assert!(
                runs.iter().all(|&(_, glyphs)| glyphs > 0),
                "{text:?}: {runs:?}"
            );
            if text.contains('\n') {
                assert_eq!(runs.iter().map(|r| r.0).collect::<Vec<_>>(), [false, true]);
            }
        }
    }

    #[test]
    fn layout_at_2x_scale_reports_same_logical_size() {
        let params = TextParams::new(LOREM, TextStyle::new(14.0)).wrap_width(Some(200.0));
        let one = test_system().layout(&params).expect("1x");
        let two = test_system()
            .layout(&params.clone().scale_factor(2.0))
            .expect("2x");
        assert_eq!(one.line_count(), two.line_count());
        assert!((one.size().1 - two.size().1).abs() < 0.5);
        assert!((one.size().0 - two.size().0).abs() < 4.0);
    }

    #[test]
    fn layout_line_ranges_are_absolute_and_exclude_line_endings() {
        let cases: &[(&str, &[Range<usize>])] = &[
            ("hello\nworld\r\n\nend", &[0..5, 6..11, 13..13, 14..17]),
            ("abc\n", &[0..3, 4..4]),
            ("a\rb\n\rc", &[0..1, 2..3, 5..6]),
        ];
        for (text, expected) in cases {
            let layout = layout(text, None);
            let ranges: Vec<_> = layout.lines().map(|l| l.byte_range).collect();
            assert_eq!(ranges, *expected, "{text:?}");
        }
    }

    #[test]
    fn layout_glyph_offsets_are_absolute_across_paragraphs() {
        let text = "hello\nworld";
        let layout = layout(text, None);
        let g = layout.glyphs();
        let w = (0..g.len())
            .find(|&i| text.get(g[i].byte_start as usize..g[i].byte_end as usize) == Some("w"))
            .expect("w glyph");
        assert_eq!((g[w].byte_start, g[w].line), (6, 1));
    }

    #[test]
    fn hit_outside_layout_clamps_to_nearest_line_edge() {
        let layout = layout("one\ntwo", None);
        let cases = [
            ((-50.0, -50.0), 0),
            ((1000.0, -50.0), 3),
            ((-50.0, 1000.0), 4),
            ((1000.0, 1000.0), 7),
        ];
        for ((x, y), expected) in cases {
            assert_eq!(layout.hit(x, y), expected, "hit({x}, {y})");
        }
    }

    #[test]
    fn caret_inside_combining_sequence_snaps_to_its_start() {
        let layout = layout("e\u{301}x", None);
        assert_eq!(layout.caret(2), layout.caret(0));
    }

    #[test]
    fn selection_rects_across_three_lines_start_and_end_at_carets() {
        let layout = layout(LOREM, Some(120.0));
        let line0 = layout.line(0).expect("line 0");
        let line2 = layout.line(2).expect("line 2");
        let a = line0.byte_range.start + 2;
        let b = line2.byte_range.start + 3;
        let rects: Vec<_> = layout.selection_rects(a..b).collect();
        assert_eq!(rects.len(), 3, "{rects:?}");
        assert!(rects.windows(2).all(|w| w[0].y < w[1].y));
        assert!((rects[0].x - layout.caret(a).x).abs() < 0.01);
        assert!((rects[2].x + rects[2].width - layout.caret(b).x).abs() < 0.01);
        assert!(rects[1].x.abs() < 0.01);
        assert_eq!(layout.selection_rects(b..a).collect::<Vec<_>>(), rects);
    }

    #[test]
    fn selection_rects_mark_empty_line_inside_range() {
        let layout = layout("a\n\nb", None);
        assert_eq!(layout.selection_rects(0..4).count(), 3);
    }

    #[test]
    fn glyph_runs_split_at_span_boundaries() {
        let spans = vec![TextSpan {
            range: 6..10,
            weight: Some(FontWeight::Bold),
            style: Some(FontStyle::Italic),
            kind: None,
            size: None,
            letter_spacing: None,
        }];
        let params = TextParams::new("plain bold plain", TextStyle::new(14.0)).spans(spans);
        let layout = test_system().layout(&params).expect("layout");
        let runs: Vec<_> = layout.glyph_runs().collect();
        assert_eq!(runs.iter().map(|r| r.span).collect::<Vec<_>>(), [0, 1, 0]);
        assert_eq!(layout.glyphs()[runs[1].glyphs.start].byte_start, 6);
    }

    fn sized_span(range: Range<usize>, size: Option<f32>, letter_spacing: Option<f32>) -> TextSpan {
        TextSpan {
            range,
            weight: None,
            style: None,
            kind: None,
            size,
            letter_spacing,
        }
    }

    // Catches a span size that is ignored, or that moves the line: the
    // span's glyphs shape at its size on the same baseline, and the line
    // keeps the style's height.
    #[test]
    fn a_sized_span_shapes_smaller_on_the_lines_baseline() {
        let style = TextStyle::new(14.0).line_height(22.0);
        let plain = test_system()
            .layout(&TextParams::new("abab", style))
            .expect("layout");
        let params = TextParams::new("abab", style).spans(vec![sized_span(2..4, Some(7.0), None)]);
        let layout = test_system().layout(&params).expect("layout");
        let g = layout.glyphs();
        assert_eq!(
            g.iter().map(|g| g.font_size).collect::<Vec<_>>(),
            [14.0, 14.0, 7.0, 7.0]
        );
        assert_eq!(g[2].phys_y, g[0].phys_y);
        assert_eq!(g[2].advance, plain.glyphs()[2].advance / 2.0);
        assert_eq!(layout.size().1, plain.size().1);
    }

    // Catches span letter spacing that is ignored or applied in the wrong
    // unit: it adds that many ems of the span's size after each of its
    // glyphs and nowhere else.
    #[test]
    fn span_letter_spacing_widens_only_its_glyphs() {
        let style = TextStyle::new(14.0);
        let plain = test_system()
            .layout(&TextParams::new("abc", style))
            .expect("layout");
        let params =
            TextParams::new("abc", style).spans(vec![sized_span(1..2, Some(10.0), Some(0.5))]);
        let layout = test_system().layout(&params).expect("layout");
        let (g, p) = (layout.glyphs(), plain.glyphs());
        assert_eq!(g[0].advance, p[0].advance);
        assert!(
            (g[1].advance - (p[1].advance * 10.0 / 14.0 + 5.0)).abs() < 0.01,
            "{}",
            g[1].advance
        );
        assert_eq!(g[2].advance, p[2].advance);
    }

    // Regression: mono text used Basic shaping, which skipped font fallback.
    #[test]
    fn layout_char_missing_from_base_font_falls_back_in_both_kinds() {
        for kind in [FontKind::Ui, FontKind::Mono] {
            let params = TextParams::new("a\u{3b1}", TextStyle::new(14.0).kind(kind));
            let layout = test_system().layout(&params).expect("layout");
            let g = layout.glyphs();
            assert_eq!(g[1].byte_start, 1);
            assert_ne!(g[1].glyph_id, 0, "{kind:?} alpha is .notdef");
            assert_ne!(
                g[1].font_id, g[0].font_id,
                "{kind:?} alpha did not fall back"
            );
        }
    }

    // Hebrew has no vendored font, so this runs on `.notdef` boxes; bidi
    // levels and caret geometry do not depend on the glyphs.
    #[test]
    fn rtl_paragraph_carets_run_right_to_left_and_round_trip() {
        let text = "\u{5e9}\u{5dc}\u{5d5}\u{5dd} \u{5e2}\u{5d5}\u{5dc}\u{5dd}";
        let layout = layout(text, None);
        assert!(layout.line(0).expect("line").rtl);
        assert!(layout.caret(0).x > layout.caret(text.len()).x);
        for b in grapheme_boundaries(text) {
            let caret = layout.caret(b);
            assert_eq!(layout.hit(caret.x, caret.y + 1.0), b, "byte {b}");
        }
    }

    #[test]
    fn mixed_direction_rtl_word_selects_as_one_rect() {
        let mixed = "ab \u{5e9}\u{5dc}\u{5d5}\u{5dd} cd";
        let layout = layout(mixed, None);
        assert!(!layout.line(0).expect("line").rtl);
        assert!(layout.caret(3).x > layout.caret(5).x);
        assert_eq!(layout.selection_rects(3..11).count(), 1);
    }

    /// A ZWJ sequence no font has a ligature for (crab, ZWJ, laptop): one
    /// cluster of three glyphs.
    const MULTI_GLYPH_CLUSTER: &str = "\u{1f980}\u{200d}\u{1f4bb}";

    // Regression: found by layout_hit_then_caret property. A ZWJ sequence
    // without a ligature is one cluster of three glyphs, and the caret after
    // it sat after the first glyph.
    #[test]
    fn caret_after_multi_glyph_cluster_sits_at_its_last_glyph() {
        let text = format!("a{MULTI_GLYPH_CLUSTER}");
        let text = text.as_str();
        let layout = layout(text, None);
        assert_eq!(
            layout.glyphs().len(),
            4,
            "fixture lost its multi-glyph cluster"
        );
        let end = layout.caret(text.len());
        assert!((end.x - layout.size().0).abs() < 0.01, "{end:?}");
        assert_eq!(layout.hit(end.x, end.y + 1.0), text.len());
    }

    // Catches a min-content width that splits words (glyph widths), keeps
    // whole phrases (space-only breaking misses CJK and hyphens), or counts
    // the spaces a line drops at a wrap.
    #[test]
    fn min_content_width_is_the_widest_unbreakable_piece() {
        let cases = [
            ("a quick brownish fox", "brownish"),
            ("wide      x", "wide"),
            ("well-known x", "known"),
            ("\u{65e5}\u{672c}\u{8a9e}", "\u{65e5}"),
            ("one\nlongest\ntwo", "longest"),
            ("keep\u{a0}together x", "keep\u{a0}together"),
        ];
        for (text, widest) in cases {
            let expected = layout(widest, None).size().0;
            let min = layout(text, None).min_content_width();
            assert!(
                (min - expected).abs() < 0.5,
                "{text:?}: {min} vs {widest:?} {expected}"
            );
        }
    }

    // Regression: found by layout_caret_then_hit property. Glyph wrapping
    // put each glyph of one cluster on its own line, giving three lines with
    // the same start byte.
    #[test]
    fn layout_cluster_wider_than_wrap_width_stays_on_one_line() {
        let text = MULTI_GLYPH_CLUSTER;
        let layout = layout(text, Some(1.0));
        assert_eq!(layout.line_count(), 1);
        assert!(layout.caret(text.len()).x > layout.caret(0).x);
    }

    // Regression: an RTL line overflowing the wrap width was fitted by
    // relaying out at its width, which let the narrower letters share lines
    // and never settled. The wrap must hold, shifted inside the box.
    #[test]
    fn rtl_cluster_wider_than_wrap_keeps_wrap_and_stays_in_bounds() {
        let text = "\u{1f600}\u{5e9}\u{5dc}\u{5d5}";
        let layout = layout(text, Some(1.0));
        assert_eq!(layout.line_count(), 4);
        let (width, _) = layout.size();
        for b in grapheme_boundaries(text) {
            let x = layout.caret(b).x;
            assert!((-0.01..=width + 0.01).contains(&x), "caret {x} at {b}");
        }
    }

    // Regression: found by layout_selection_rects property; an end offset
    // inside a multibyte char panicked while slicing.
    #[test]
    fn selection_rects_offset_inside_char_snaps_to_char_start() {
        let layout = layout("a\u{e9}b", None);
        let inside: Vec<_> = layout.selection_rects(0..2).collect();
        assert_eq!(inside, layout.selection_rects(0..1).collect::<Vec<_>>());
    }

    #[test]
    fn hit_on_empty_line_starting_inside_grapheme_returns_boundary() {
        // Lines start at 0, 2, 3; graphemes are "\n" and "\r\n".
        let layout = layout("\n\r\n", None);
        let line = layout.line(1).expect("line 1");
        assert_eq!(line.byte_range, 2..2);
        assert_eq!(layout.hit(0.0, line.top + 1.0), 1);
    }

    // Regression: fuzz crash (fuzz/corpus/text_layout/crash_gs_rtl).
    // cosmic-text panicked on a bidi paragraph separator followed by text of
    // the other direction.
    #[test]
    fn layout_bidi_paragraph_separators_start_new_lines() {
        let cases: &[(&str, &[Range<usize>])] = &[
            ("\u{1d}\u{5d5}", &[0..0, 1..3]),
            ("a\u{2029}\u{5d5}", &[0..1, 4..6]),
            ("\u{5d5}\u{85}a", &[0..2, 4..5]),
        ];
        for (text, expected) in cases {
            let layout = layout(text, None);
            let ranges: Vec<_> = layout.lines().map(|l| l.byte_range).collect();
            assert_eq!(ranges, *expected, "{text:?}");
        }
    }

    fn env(synth: SyntheticItalic) -> ShapeEnv<'static> {
        static NO_NAMES: FamilyNames = FamilyNames::new();
        ShapeEnv {
            synth,
            emoji: None,
            ligatures: true,
            names: &NO_NAMES,
        }
    }

    // Regression: `hit` indexed line `count - 1`, guarded only by a
    // `debug_assert`, so a layout without shaped lines underflowed.
    #[test]
    fn layout_with_no_shaped_runs_still_hits_and_places_carets() {
        let params = TextParams::new("ab", TextStyle::new(14.0));
        let mut system = test_system();
        let fs = system.raster_font_system();
        let synth = SyntheticItalic::new(fs);
        let mut scratch = LayoutScratch::default();
        let layout = TextLayout::build_with(fs, &mut scratch, &params, &env(synth), |_| false)
            .expect("layout");
        assert_eq!(layout.line_count(), 1);
        assert_eq!(layout.hit(50.0, 50.0), 0);
        let caret = layout.caret(2);
        assert_eq!((caret.line, caret.x, caret.y), (0, 0.0, 0.0));
    }

    // cosmic-text keeps a line's shaping when its text and attributes are
    // unchanged, so without a reset a rebuild after a font change would
    // keep glyphs from the old fonts.
    #[test]
    fn layout_rebuilt_after_font_change_matches_fresh_layout() {
        // Its own system: changing fonts on the shared one would race other
        // tests.
        let mut system = TextSystem::vendored_only(&FontSettings::default());
        let params = TextParams::new("office", TextStyle::new(14.0));
        let mut reused = system.layout(&params).expect("layout");
        let old_font = reused.glyphs()[0].font_id;
        system.set_font_settings(&FontSettings {
            ui_family: "Inter".into(),
            ..FontSettings::default()
        });
        reused.copy_inputs(&params.query());
        system.rebuild(&mut reused);
        let fresh = system.layout(&params).expect("layout");
        assert_ne!(
            fresh.glyphs()[0].font_id,
            old_font,
            "fixture lost its font change"
        );
        assert_eq!(dump(&reused), dump(&fresh));
    }

    /// Texts that each need different shape plans: scripts, directions,
    /// UI and mono fonts, and weights.
    fn plan_mix() -> Vec<TextParams> {
        let style = TextStyle::new(14.0);
        let span = |range, weight, kind| TextSpan {
            range,
            weight,
            style: None,
            kind,
            size: None,
            letter_spacing: None,
        };
        vec![
            TextParams::new("office affine", style),
            TextParams::new("fn main() -> x != y", style.kind(FontKind::Mono)),
            TextParams::new(
                "\u{5e9}\u{5dc}\u{5d5}\u{5dd} \u{5e2}\u{5d5}\u{5dc}\u{5dd}",
                style,
            ),
            TextParams::new("\u{627}\u{644}\u{633}\u{644}\u{627}\u{645}", style),
            TextParams::new(
                "\u{1f600}\u{1f469}\u{200d}\u{1f4bb} \u{65e5}\u{672c}\u{8a9e}",
                style,
            ),
            TextParams::new("ab \u{5e9}\u{5dc}\u{5d5}\u{5dd} cd office", style).spans(vec![
                span(0..2, Some(FontWeight::Bold), Some(FontKind::Mono)),
                span(12..14, Some(FontWeight::Semibold), None),
            ]),
        ]
    }

    // The shape plan cache is keyed by font, script, direction, features,
    // and variation instance. A lookup that returns another key's plan, or
    // loses track of a plan when a hit moves it or a miss evicts, shapes
    // with the wrong plan.
    #[test]
    fn layout_with_evicting_shape_plan_cache_matches_cold_layout() {
        let mut cold = TextSystem::vendored_only(&FontSettings::default());
        let expected: Vec<String> = plan_mix()
            .iter()
            .map(|params| dump(&cold.layout(params).expect("layout")))
            .collect();
        let mut system = TextSystem::vendored_only(&FontSettings::default());
        // Smaller than the mix's working set, so lookups hit, promote, and
        // evict.
        system.set_shape_plan_capacity(3);
        let order = (0..expected.len()).chain((0..expected.len()).rev());
        for i in order.clone().chain(order.step_by(2)) {
            let layout = system.layout(&plan_mix()[i]).expect("layout");
            assert_eq!(dump(&layout), expected[i], "text {i}");
        }
    }

    /// Texts the shaping memo is checked over: ASCII words, digits and
    /// punctuation, tabs, ligature candidates, wide CJK, emoji sequences,
    /// combining marks, right-to-left runs, and words that repeat (so the
    /// memo answers within a line too).
    const MEMO_CORPUS: &[&str] = &[
        "the quick brown fox jumps over the lazy dog",
        "0123 4.5% = $6, (7) / 8:9 -- ... !? [] {} <> @#^&*_+|~`'\"",
        "tab\tseparated\tcolumns\t\t42",
        "fi fl ffi office -> => != === <= >= && || :: www 0xFF",
        "\u{65e5}\u{672c}\u{8a9e}\u{306e}\u{30c6}\u{30ad}\u{30b9}\u{30c8} \u{6f22}\u{5b57} kanji",
        "emoji \u{1f600} \u{1f469}\u{200d}\u{1f4bb} \u{1f1ef}\u{1f1f5} \u{2764}\u{fe0f} ok",
        "e\u{301} a\u{308}o\u{303} n\u{303}a\u{30a} combining \u{3b1}\u{3b2} \u{2603}",
        "\u{5e9}\u{5dc}\u{5d5}\u{5dd} world \u{645}\u{631}\u{62d}\u{628}\u{627} 123",
        // `<>` between right-to-left words shapes right to left, mirrored,
        // and between left-to-right ones it does not.
        "\u{5e9}\u{5dc} <> \u{5d5}\u{5dd} and <> x",
        "build build build build step step a a a",
        "  blanks  around   ",
    ];

    /// The spans each memo corpus text is laid out with: none, bold and
    /// italic words, another font kind, and a span that changes nothing
    /// but its index (so a memo answer must still take the glyph's own).
    fn memo_spans(text: &str) -> Vec<Vec<TextSpan>> {
        let snap = |i: usize| {
            (0..=i.min(text.len()))
                .rev()
                .find(|&i| text.is_char_boundary(i))
        };
        let span = |from: usize, to: usize, weight, style, kind| {
            let (from, to) = (snap(from).unwrap_or(0), snap(to).unwrap_or(0));
            TextSpan {
                range: from..to.max(from),
                weight,
                style,
                kind,
                size: None,
                letter_spacing: None,
            }
        };
        vec![
            Vec::new(),
            vec![
                span(0, 6, Some(FontWeight::Bold), None, None),
                span(6, 12, None, Some(FontStyle::Italic), None),
                span(12, 18, None, None, None),
                span(18, 24, None, None, Some(FontKind::Mono)),
            ],
            vec![span(
                5,
                40,
                Some(FontWeight::Semibold),
                Some(FontStyle::Italic),
                None,
            )],
        ]
    }

    /// A system shaping every run again, and one answering from the memo.
    fn memo_pair(settings: &FontSettings) -> (TextSystem, TextSystem) {
        let mut full = TextSystem::vendored_only(settings);
        full.raster_font_system().set_shape_run_memo(false);
        (full, TextSystem::vendored_only(settings))
    }

    // cosmic-text answers short runs it has shaped from a memo. Its key must
    // hold everything that picks the glyphs (text, direction, the start's
    // fonts and features), and each answer must take what shaping copies
    // from each glyph's own attributes; otherwise a layout differs from
    // one shaped run by run. Each text is laid out twice, so the second
    // reads every run it can from the memo.
    #[test]
    fn memoized_shaping_lays_out_like_shaping_every_run() {
        let fira = crate::fonts::FIRA_CODE_FAMILY.to_owned();
        let settings = [
            FontSettings::default(),
            FontSettings {
                ligatures: false,
                ..FontSettings::default()
            },
            FontSettings {
                mono_family: fira.clone(),
                ..FontSettings::default()
            },
            FontSettings {
                mono_family: fira,
                ligatures: false,
                ..FontSettings::default()
            },
        ];
        let styles = [
            (TextStyle::new(14.0), 1.0),
            (TextStyle::new(13.0).kind(FontKind::Mono), 1.0),
            (TextStyle::new(13.0).kind(FontKind::Mono), 1.5),
        ];
        for settings in &settings {
            let (mut full, mut memo) = memo_pair(settings);
            for pass in 0..2 {
                for text in MEMO_CORPUS {
                    for spans in memo_spans(text) {
                        for (style, scale) in styles {
                            let params = TextParams::new(*text, style)
                                .spans(spans.clone())
                                .scale_factor(scale);
                            let expected = dump(&full.layout(&params).expect("layout"));
                            let actual = dump(&memo.layout(&params).expect("layout"));
                            assert_eq!(actual, expected, "pass {pass} {text:?} {spans:?}");
                        }
                    }
                }
            }
        }
    }

    /// A glyph's cache key and integer position, and the top and height
    /// its row is culled by.
    type Placed = (CacheKey, i32, i32, f32, f32);

    /// Where the renderer's buffer path draws each glyph of `layout` with
    /// its origin at `origin` (physical pixels): glyphon's text area
    /// placement, which truncates a glyph's own offset from the origin and
    /// adds its row's baseline rounded on its own.
    fn buffer_placement(layout: &TextLayout, origin: (f32, f32)) -> Vec<Placed> {
        let (left, top) = (origin.0 + layout.buffer_x(), origin.1);
        let mut out = Vec::new();
        for run in layout.buffer().layout_runs() {
            let baseline = run.line_y.round() as i32;
            for glyph in run.glyphs {
                let p = glyph.physical((left, top), 1.0);
                out.push((
                    p.cache_key,
                    p.x,
                    p.y + baseline,
                    run.line_top,
                    run.line_height,
                ));
            }
        }
        out
    }

    proptest! {
        #![proptest_config(config(48))]

        // Catches the glyph accessors placing glyphs a pixel off the buffer
        // path the renderer draws today, or culling by another row: adding
        // a fractional baseline before truncating, summing x in another
        // order (which can cross a subpixel bin), or using a wrapped
        // cluster's line instead of its own row.
        #[test]
        fn physical_glyphs_land_where_the_buffer_path_draws_them(
            text in mixed_text(),
            wrap in prop_oneof![Just(None), (1.0f32..120.0).prop_map(Some)],
            scale in prop::sample::select(&[1.0f32, 1.25, 1.5, 2.0][..]),
            line_height in 14.0f32..24.0,
            origin in (-40.0f32..40.0, -40.0f32..40.0),
        ) {
            let style = TextStyle::new(13.0).line_height(line_height);
            let params = TextParams::new(text.as_str(), style)
                .wrap_width(wrap)
                .scale_factor(scale);
            let layout = test_system().layout(&params).expect("layout");
            let expected = buffer_placement(&layout, origin);
            let by_run: Vec<Placed> = layout
                .glyph_runs()
                .flat_map(|run| {
                    let row = (run.phys_top, run.phys_height);
                    layout
                        .physical_run(&run, origin)
                        .map(move |p| (p.cache_key, p.x, p.y, row.0, row.1))
                        .collect::<Vec<_>>()
                })
                .collect();
            let by_index: Vec<_> = (0..layout.glyph_count())
                .map(|i| layout.physical_glyph(i, origin).expect("glyph"))
                .map(|p| (p.cache_key, p.x, p.y))
                .collect();
            let positions: Vec<_> = expected.iter().map(|e| (e.0, e.1, e.2)).collect();
            prop_assert_eq!(by_run, expected);
            prop_assert_eq!(by_index, positions);
        }
    }

    proptest! {
        #![proptest_config(config(24))]

        // The memo over arbitrary mixed-direction text with a bold italic
        // prefix, wrapped or not.
        #[test]
        fn memoized_shaping_of_mixed_text_matches_shaping_every_run(
            text in mixed_text(),
            end in 0usize..48,
            wrap in wrap(),
        ) {
            let (mut full, mut memo) = memo_pair(&FontSettings::default());
            let params = TextParams::new(text.as_str(), TextStyle::new(14.0))
                .spans(prefix_span(&text, end, FontStyle::Italic))
                .wrap_width(wrap);
            let expected = dump(&full.layout(&params).expect("layout"));
            for _ in 0..2 {
                prop_assert_eq!(dump(&memo.layout(&params).expect("layout")), expected.clone());
            }
        }
    }

    // Attributes quark-text never sets (letter spacing, color, metrics,
    // stretch, font features per span) reach shaped glyphs too: a memo answer must add
    // each glyph's own spacing to the bare advance and copy its own color,
    // metadata, weight, flags, and metrics, including within one run, and
    // a run with other features (Fira Code's `ffi` without ligatures) is
    // another run.
    #[test]
    fn memoized_runs_take_each_glyphs_own_attributes() {
        use cosmic_text::{
            Attrs, AttrsList, BufferLine, CacheKeyFlags, Color, Family, FontFeatures, LineEnding,
            Metrics, Shaping, Stretch, Weight,
        };
        let shaped = |memo: bool| {
            let mut system = TextSystem::vendored_only(&FontSettings::default());
            let fs = system.raster_font_system();
            fs.set_shape_run_memo(memo);
            let base = Attrs::new().family(Family::Name(crate::fonts::FIRA_CODE_FAMILY));
            let mut no_ligatures = FontFeatures::new();
            set_font_features(&mut no_ligatures, false);
            let mut attrs = AttrsList::new(&base);
            attrs.add_span(25..31, &base.clone().font_features(no_ligatures));
            attrs.add_span(3..5, &base.clone().letter_spacing(0.25));
            let marked = base
                .clone()
                .color(Color::rgb(1, 2, 3))
                .metadata(7)
                .cache_key_flags(CacheKeyFlags::FAKE_ITALIC);
            attrs.add_span(6..8, &marked);
            attrs.add_span(9..10, &base.clone().metrics(Metrics::new(20.0, 24.0)));
            attrs.add_span(12..14, &base.clone().letter_spacing(-0.1));
            attrs.add_span(15..17, &base.clone().weight(Weight::BOLD));
            attrs.add_span(0..2, &base.clone().stretch(Stretch::Condensed));
            let mut line = BufferLine::new(
                "ab ab ab ab ab ab office office",
                LineEnding::None,
                attrs,
                Shaping::Advanced,
            );
            line.shape(fs, 8);
            line.reset();
            format!("{:?}", line.shape(fs, 8))
        };
        assert_eq!(shaped(true), shaped(false));
    }

    // Mono text falls back through monospace candidates ordered by weight
    // distance, then by how many of the word's chars they lack, with the
    // default mono font first. Geist Mono lacks Greek and the snowman;
    // JetBrains Mono (400) and Fira Code (300) have Greek, and Noto Color
    // Emoji, which counts as monospace, has the snowman at its one weight.
    #[test]
    fn mono_fallback_picks_nearest_weight_then_best_coverage() {
        let cases = [
            ("a\u{3b1}", FontWeight::Normal, 1, "JetBrains Mono", 400),
            ("a\u{3b1}", FontWeight::Bold, 1, "JetBrains Mono", 400),
            ("a\u{2603}", FontWeight::Medium, 1, "Noto Color Emoji", 400),
            ("a\u{2603}", FontWeight::Bold, 1, "Noto Color Emoji", 400),
            ("x \u{3b1}b\u{3b3}", FontWeight::Bold, 4, "Geist Mono", 700),
            (
                "x \u{3b1}b\u{3b3}",
                FontWeight::Bold,
                5,
                "JetBrains Mono",
                400,
            ),
            (
                "a\u{1f600}",
                FontWeight::Semibold,
                1,
                "Noto Color Emoji",
                400,
            ),
        ];
        let mut system = test_system();
        for (text, weight, byte, family, face_weight) in cases {
            let style = TextStyle::new(13.0).kind(FontKind::Mono).weight(weight);
            let layout = system
                .layout(&TextParams::new(text, style))
                .expect("layout");
            let g = layout.glyphs();
            let i = g.iter().position(|g| g.byte_start as usize == byte);
            let i = i.unwrap_or_else(|| panic!("{text:?} has no glyph at {byte}"));
            assert_ne!(
                g[i].glyph_id, 0,
                "{text:?} {weight:?} byte {byte} is .notdef"
            );
            let face = system.font_system().db().face(g[i].font_id).expect("face");
            assert_eq!(
                (face.families[0].0.as_str(), face.weight.0),
                (family, face_weight),
                "{text:?} {weight:?} byte {byte}"
            );
        }
    }

    /// Each line's clusters as painted left to right.
    fn painted_lines(layout: &TextLayout) -> Vec<String> {
        let g = layout.glyphs();
        let text = layout.text();
        layout
            .lines()
            .map(|line| {
                let mut glyphs: Vec<usize> = line.glyph_range.collect();
                glyphs.sort_by(|&a, &b| g[a].x.total_cmp(&g[b].x));
                glyphs.dedup_by_key(|&mut i| g[i].byte_start);
                glyphs
                    .iter()
                    .map(|&i| text.get(g[i].byte_start as usize..g[i].byte_end as usize))
                    .collect::<Option<String>>()
                    .expect("clusters start and end on char boundaries")
            })
            .collect()
    }

    // Each visual line reorders its own level runs. Lines with fewer runs
    // after lines with more catch runs or levels a line inherits from the
    // one before; the digits sit at level 2 inside RTL text in both
    // directions.
    #[test]
    fn wrapped_bidi_lines_paint_their_runs_in_visual_order() {
        let cases: [(&str, f32, &[&str]); 3] = [
            (
                "ab \u{5e9}\u{5dc}\u{5d5}\u{5dd} cd \u{5d0}\u{5d1}\u{5d2} ef gh \u{5d3}\u{5d4} ij",
                120.0,
                &[
                    "ab \u{5dd}\u{5d5}\u{5dc}\u{5e9} cd \u{5d2}\u{5d1}\u{5d0} ef",
                    "gh \u{5d4}\u{5d3} ij",
                ],
            ),
            (
                "ab \u{5e9}\u{5dc} \u{5d5}\u{5dd} cd ef \u{5d0}\u{5d1} 34 gh",
                80.0,
                &[
                    "ab \u{5dd}\u{5d5} \u{5dc}\u{5e9} cd",
                    "ef 34 \u{5d1}\u{5d0} gh",
                ],
            ),
            (
                "\u{5e9}\u{5dc}\u{5d5}\u{5dd} ab cd \u{5d0}\u{5d1} 12 \u{5d2}\u{5d3} ef \u{5d4}\u{5d5}",
                90.0,
                &[
                    "ab cd \u{5dd}\u{5d5}\u{5dc}\u{5e9}",
                    "ef \u{5d3}\u{5d2} 12 \u{5d1}\u{5d0}",
                    "\u{5d5}\u{5d4}",
                ],
            ),
        ];
        for (text, wrap, expected) in cases {
            assert_eq!(
                painted_lines(&layout(text, Some(wrap))),
                expected,
                "{text:?}"
            );
        }
    }

    // With ligatures off every span and paragraph carries the features
    // that turn them off, so Fira Code's `==`, `<=`, and `&&` shape as each
    // character alone inside and after italic spans and in a later
    // paragraph.
    #[test]
    fn ligatures_off_reach_every_span_and_paragraph() {
        let mut system = TextSystem::vendored_only(&FontSettings {
            mono_family: crate::fonts::FIRA_CODE_FAMILY.to_owned(),
            ligatures: false,
            ..FontSettings::default()
        });
        let style = TextStyle::new(16.0).kind(FontKind::Mono);
        let alone = |system: &mut TextSystem, c: char| {
            let layout = system.layout(&TextParams::new(c.to_string(), style));
            layout.expect("layout").glyphs()[0].glyph_id
        };
        let text = "== <= &&\n&& == <=";
        let italic = |range| TextSpan {
            range,
            weight: None,
            style: Some(FontStyle::Italic),
            kind: None,
            size: None,
            letter_spacing: None,
        };
        let params = TextParams::new(text, style).spans(vec![italic(0..2), italic(6..11)]);
        let layout = system.layout(&params).expect("layout");
        let g = layout.glyphs();
        let shaped: Vec<(usize, u16)> = (0..g.len())
            .map(|i| (g[i].byte_start as usize, g[i].glyph_id))
            .collect();
        let expected: Vec<(usize, u16)> = text
            .char_indices()
            .filter(|&(_, c)| c != '\n')
            .map(|(i, c)| (i, alone(&mut system, c)))
            .collect();
        assert_eq!(shaped, expected);
    }

    #[test]
    fn layout_invalid_params_return_matching_error() {
        let style = TextStyle::new(12.0);
        let span = |range: Range<usize>| TextSpan {
            range,
            weight: None,
            style: None,
            kind: None,
            size: None,
            letter_spacing: None,
        };
        let cases = [
            (
                TextParams::new("x", TextStyle::new(0.0)),
                TextError::InvalidFontSize(0.0),
            ),
            (
                TextParams::new("x", style.line_height(-1.0)),
                TextError::InvalidLineHeight(-1.0),
            ),
            (
                TextParams::new("x", style).scale_factor(0.0),
                TextError::InvalidScaleFactor(0.0),
            ),
            (
                TextParams::new("x", style).wrap_width(Some(f32::NAN)),
                TextError::InvalidWrapWidth,
            ),
            (
                TextParams::new("x", style).spans(vec![span(0..5)]),
                TextError::InvalidSpan {
                    index: 0,
                    start: 0,
                    end: 5,
                },
            ),
            (
                TextParams::new("\u{e9}", style).spans(vec![span(0..1)]),
                TextError::InvalidSpan {
                    index: 0,
                    start: 0,
                    end: 1,
                },
            ),
        ];
        let mut sys = test_system();
        for (params, expected) in cases {
            assert_eq!(sys.layout(&params).err(), Some(expected));
        }
    }
}
