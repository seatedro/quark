use std::ops::Range;
use std::sync::Arc;

use cosmic_text::{
    Attrs, AttrsList, Buffer, BufferLine, CacheKey, CacheKeyFlags, Family, FontSystem, LineEnding,
    Metrics, PhysicalGlyph, Shaping, Wrap, fontdb,
};
use quark::scene::FontStyle;
use quark::{FontKind, FontWeight, Rect};
use unicode_segmentation::UnicodeSegmentation;

use crate::offset::{TextOffset, ToTextOffset};

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

/// A broken [`TextLayout`] column invariant, reported by
/// [`TextLayout::verify_integrity`].
#[derive(Debug, Clone, thiserror::Error, PartialEq)]
pub enum IntegrityError {
    #[error("{table}.{column} has {len} entries, expected {expected}")]
    ColumnLength {
        table: &'static str,
        column: &'static str,
        len: usize,
        expected: usize,
    },
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
}

impl TextStyle {
    pub fn new(font_size: f32) -> Self {
        Self {
            font_kind: FontKind::Ui,
            font_weight: FontWeight::Normal,
            font_size,
            line_height: font_size * DEFAULT_LINE_HEIGHT_FACTOR,
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
}

/// Attribute override for a byte range of the text. Later spans win where
/// they overlap. Glyphs record the index of the span that styled them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextSpan {
    pub range: Range<usize>,
    pub weight: Option<FontWeight>,
    pub style: Option<FontStyle>,
    /// Font family override, e.g. an inline code run in UI text.
    pub kind: Option<FontKind>,
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

/// Consecutive glyphs on one line sharing a span and direction.
#[derive(Debug, Clone, PartialEq)]
pub struct GlyphRun {
    pub line: usize,
    /// 0 = base style, `i + 1` = `params.spans[i]`.
    pub span: u32,
    pub rtl: bool,
    pub glyphs: Range<usize>,
}

/// Per-glyph columns, stored line by line in cosmic-text's order (left to
/// right for LTR, logical order within RTL runs, so `x` is not sorted). Logical columns are in logical pixels; `phys_*` are physical
/// pixels relative to the layout origin, already including the baseline.
#[derive(Debug, Default, Clone)]
pub struct GlyphColumns {
    pub x: Vec<f32>,
    pub advance: Vec<f32>,
    pub line: Vec<u32>,
    pub byte_start: Vec<u32>,
    pub byte_end: Vec<u32>,
    /// Unicode bidi embedding level; odd = RTL.
    pub level: Vec<u8>,
    pub span: Vec<u32>,
    pub font_id: Vec<fontdb::ID>,
    pub glyph_id: Vec<u16>,
    pub font_size: Vec<f32>,
    pub font_weight: Vec<fontdb::Weight>,
    pub flags: Vec<CacheKeyFlags>,
    pub phys_x: Vec<f32>,
    pub phys_y: Vec<f32>,
}

impl GlyphColumns {
    pub fn len(&self) -> usize {
        self.x.len()
    }

    pub fn is_empty(&self) -> bool {
        self.x.is_empty()
    }

    fn rtl(&self, i: usize) -> bool {
        self.level[i] % 2 == 1
    }
}

#[derive(Debug, Default, Clone)]
struct LineColumns {
    byte_start: Vec<u32>,
    byte_end: Vec<u32>,
    top: Vec<f32>,
    height: Vec<f32>,
    baseline: Vec<f32>,
    width: Vec<f32>,
    glyph_start: Vec<u32>,
    glyph_end: Vec<u32>,
    rtl: Vec<bool>,
}

#[derive(Debug, Default, Clone)]
struct RunColumns {
    line: Vec<u32>,
    span: Vec<u32>,
    rtl: Vec<bool>,
    glyph_start: Vec<u32>,
    glyph_end: Vec<u32>,
}

/// Immutable result of shaping + layout, shared via `Arc` by measurement,
/// hit-testing, selection, and painting.
#[derive(Debug)]
pub struct TextLayout {
    text: Arc<str>,
    spans: Arc<[TextSpan]>,
    style: TextStyle,
    wrap_width: Option<f32>,
    scale_factor: f32,
    width: f32,
    height: f32,
    glyphs: GlyphColumns,
    lines: LineColumns,
    runs: RunColumns,
    // Kept so the renderer can hand it to glyphon's TextRenderer, which only
    // accepts Buffers. Shaped at physical size; draw it with `scale: 1.0`.
    buffer: Buffer,
}

impl TextLayout {
    pub(crate) fn build(
        fs: &mut FontSystem,
        params: &TextParams,
        synth: SyntheticItalic,
    ) -> Result<Self, TextError> {
        Self::build_with(fs, params, synth, |_| true)
    }

    /// [`Self::build`] keeping only the shaped runs `keep` accepts, so tests
    /// can reproduce a shaper that yields no lines.
    fn build_with(
        fs: &mut FontSystem,
        params: &TextParams,
        synth: SyntheticItalic,
        keep: impl Fn(&cosmic_text::LayoutRun) -> bool,
    ) -> Result<Self, TextError> {
        params.validate()?;
        let text = params.text.as_ref();
        let style = params.style;
        let scale = params.scale_factor;

        let mut buffer = Buffer::new_empty(Metrics::new(
            style.font_size * scale,
            style.line_height * scale,
        ));
        buffer.set_wrap(fs, Wrap::WordOrGlyph);
        buffer.set_size(fs, params.wrap_width.map(|w| (w * scale).max(1.0)), None);

        let paragraphs = split_paragraphs(text);
        let base = base_attrs(&style);
        buffer.lines = paragraphs
            .iter()
            .map(|(range, ending)| {
                let mut attrs = AttrsList::new(&base);
                for (i, span) in params.spans.iter().enumerate() {
                    let start = span.range.start.max(range.start);
                    let end = span.range.end.min(range.end);
                    if start < end {
                        let span_attrs = span_attrs(&style, span, i, synth);
                        attrs.add_span(start - range.start..end - range.start, &span_attrs);
                    }
                }
                let paragraph = text.get(range.clone()).unwrap_or_default();
                BufferLine::new(paragraph, *ending, attrs, Shaping::Advanced)
            })
            .collect();
        for line_i in 0..buffer.lines.len() {
            buffer.line_layout(fs, line_i);
        }

        let inv = 1.0 / scale;
        let mut glyphs = GlyphColumns::default();
        let mut lines = LineColumns::default();
        let mut line_paragraph: Vec<usize> = Vec::new();
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
                && lines
                    .byte_start
                    .last()
                    .is_some_and(|&prev| min_start == usize::MAX || min_start <= prev as usize);
            let line_index = if continuation {
                lines.top.len() as u32 - 1
            } else {
                lines.top.len() as u32
            };
            let glyph_start = glyphs.len() as u32;
            for g in run.glyphs {
                glyphs.x.push(g.x * inv);
                glyphs.advance.push(g.w * inv);
                glyphs.line.push(line_index);
                glyphs.byte_start.push((para_start + g.start) as u32);
                glyphs.byte_end.push((para_start + g.end) as u32);
                glyphs.level.push(g.level.number());
                glyphs.span.push(g.metadata as u32);
                glyphs.font_id.push(g.font_id);
                glyphs.glyph_id.push(g.glyph_id);
                glyphs.font_size.push(g.font_size);
                glyphs.font_weight.push(g.font_weight);
                glyphs.flags.push(g.cache_key_flags);
                glyphs.phys_x.push(g.x + g.font_size * g.x_offset);
                glyphs
                    .phys_y
                    .push(run.line_y + g.y - g.font_size * g.y_offset);
            }
            width = width.max(run.line_w * inv);
            height = height.max((run.line_top + run.line_height) * inv);
            if continuation {
                let last = line_index as usize;
                lines.glyph_end[last] = glyphs.len() as u32;
                lines.width[last] = lines.width[last].max(run.line_w * inv);
                continue;
            }
            let byte_start = if first_of_paragraph || min_start == usize::MAX {
                para_start
            } else {
                min_start
            };
            lines.byte_start.push(byte_start as u32);
            lines.byte_end.push(0); // fixed up below
            lines.top.push(run.line_top * inv);
            lines.height.push(run.line_height * inv);
            lines.baseline.push(run.line_y * inv);
            lines.width.push(run.line_w * inv);
            lines.glyph_start.push(glyph_start);
            lines.glyph_end.push(glyphs.len() as u32);
            lines.rtl.push(run.rtl);
            line_paragraph.push(run.line_i);
        }

        // cosmic-text yields a run for every laid-out paragraph, but `hit`,
        // `caret`, and `line_at_y` read the last line, so one line is made
        // structural here instead of trusted.
        if lines.top.is_empty() {
            lines.byte_start.push(0);
            lines.byte_end.push(0);
            lines.top.push(0.0);
            lines.height.push(style.line_height);
            lines.baseline.push(style.line_height.min(style.font_size));
            lines.width.push(0.0);
            lines.glyph_start.push(0);
            lines.glyph_end.push(glyphs.len() as u32);
            lines.rtl.push(false);
            line_paragraph.push(0);
            height = height.max(style.line_height);
        }

        for i in 0..line_paragraph.len() {
            let same_paragraph_next = line_paragraph.get(i + 1) == Some(&line_paragraph[i]);
            lines.byte_end[i] = if same_paragraph_next {
                lines.byte_start[i + 1]
            } else {
                paragraphs[line_paragraph[i]].0.end as u32
            };
        }

        let runs = build_runs(&glyphs, &lines);
        let layout = Self {
            text: params.text.clone(),
            spans: params.spans.clone(),
            style,
            wrap_width: params.wrap_width,
            scale_factor: scale,
            width,
            height,
            glyphs,
            lines,
            runs,
            buffer,
        };
        debug_assert_eq!(layout.verify_integrity(), Ok(()));
        Ok(layout)
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
        let line_count = l.top.len();
        let run_count = r.line.len();
        let columns = [
            ("glyphs", "advance", g.advance.len(), glyph_count),
            ("glyphs", "line", g.line.len(), glyph_count),
            ("glyphs", "byte_start", g.byte_start.len(), glyph_count),
            ("glyphs", "byte_end", g.byte_end.len(), glyph_count),
            ("glyphs", "level", g.level.len(), glyph_count),
            ("glyphs", "span", g.span.len(), glyph_count),
            ("glyphs", "font_id", g.font_id.len(), glyph_count),
            ("glyphs", "glyph_id", g.glyph_id.len(), glyph_count),
            ("glyphs", "font_size", g.font_size.len(), glyph_count),
            ("glyphs", "font_weight", g.font_weight.len(), glyph_count),
            ("glyphs", "flags", g.flags.len(), glyph_count),
            ("glyphs", "phys_x", g.phys_x.len(), glyph_count),
            ("glyphs", "phys_y", g.phys_y.len(), glyph_count),
            ("lines", "byte_start", l.byte_start.len(), line_count),
            ("lines", "byte_end", l.byte_end.len(), line_count),
            ("lines", "height", l.height.len(), line_count),
            ("lines", "baseline", l.baseline.len(), line_count),
            ("lines", "width", l.width.len(), line_count),
            ("lines", "glyph_start", l.glyph_start.len(), line_count),
            ("lines", "glyph_end", l.glyph_end.len(), line_count),
            ("lines", "rtl", l.rtl.len(), line_count),
            ("runs", "span", r.span.len(), run_count),
            ("runs", "rtl", r.rtl.len(), run_count),
            ("runs", "glyph_start", r.glyph_start.len(), run_count),
            ("runs", "glyph_end", r.glyph_end.len(), run_count),
        ];
        for (table, column, len, expected) in columns {
            if len != expected {
                return Err(IntegrityError::ColumnLength {
                    table,
                    column,
                    len,
                    expected,
                });
            }
        }
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

        let text = self.text.as_ref();
        let in_text = |start: usize, end: usize| {
            start <= end
                && end <= text.len()
                && text.is_char_boundary(start)
                && text.is_char_boundary(end)
        };
        let mut next_glyph = 0;
        for line in 0..line_count {
            let (gs, ge) = (l.glyph_start[line] as usize, l.glyph_end[line] as usize);
            if gs != next_glyph || ge < gs || ge > glyph_count {
                return Err(IntegrityError::LineGlyphs {
                    line,
                    start: gs,
                    end: ge,
                    expected: next_glyph,
                });
            }
            next_glyph = ge;
            let (bs, be) = (l.byte_start[line] as usize, l.byte_end[line] as usize);
            // `line_for_byte` binary-searches line starts.
            let after_previous = line == 0 || bs > l.byte_start[line - 1] as usize;
            let before_next = l.byte_start.get(line + 1).is_none_or(|&n| be <= n as usize);
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

        for glyph in 0..glyph_count {
            let (start, end) = (g.byte_start[glyph] as usize, g.byte_end[glyph] as usize);
            if !in_text(start, end) {
                return Err(IntegrityError::GlyphBytes { glyph, start, end });
            }
            let line = g.line[glyph] as usize;
            let on_line = line < line_count
                && (l.glyph_start[line] as usize..l.glyph_end[line] as usize).contains(&glyph);
            if !on_line {
                return Err(IntegrityError::GlyphLine { glyph, line });
            }
            if g.span[glyph] as usize > self.spans.len() {
                return Err(IntegrityError::GlyphSpan {
                    glyph,
                    span: g.span[glyph],
                    spans: self.spans.len(),
                });
            }
        }

        let mut next_glyph = 0;
        for run in 0..run_count {
            let (gs, ge) = (r.glyph_start[run] as usize, r.glyph_end[run] as usize);
            let line = r.line[run] as usize;
            let same_line = gs < ge
                && ge <= glyph_count
                && line < line_count
                && (gs..ge).all(|i| g.line[i] as usize == line);
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

    pub fn text(&self) -> &Arc<str> {
        &self.text
    }

    pub fn spans(&self) -> &Arc<[TextSpan]> {
        &self.spans
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
        self.lines.top.len()
    }

    pub fn line(&self, i: usize) -> Option<LineInfo> {
        (i < self.line_count()).then(|| self.line_info(i))
    }

    pub fn lines(&self) -> impl ExactSizeIterator<Item = LineInfo> + '_ {
        (0..self.line_count()).map(|i| self.line_info(i))
    }

    fn line_info(&self, i: usize) -> LineInfo {
        let l = &self.lines;
        LineInfo {
            byte_range: l.byte_start[i] as usize..l.byte_end[i] as usize,
            top: l.top[i],
            height: l.height[i],
            baseline: l.baseline[i],
            width: l.width[i],
            glyph_range: l.glyph_start[i] as usize..l.glyph_end[i] as usize,
            rtl: l.rtl[i],
        }
    }

    pub fn glyphs(&self) -> &GlyphColumns {
        &self.glyphs
    }

    pub fn glyph_runs(&self) -> impl ExactSizeIterator<Item = GlyphRun> + '_ {
        let r = &self.runs;
        (0..r.line.len()).map(|i| GlyphRun {
            line: r.line[i] as usize,
            span: r.span[i],
            rtl: r.rtl[i],
            glyphs: r.glyph_start[i] as usize..r.glyph_end[i] as usize,
        })
    }

    /// Rasterization key and integer pixel position for glyph `i`, with the
    /// layout origin at `origin` in physical pixels. Equivalent to
    /// cosmic-text's `LayoutGlyph::physical`.
    pub fn physical_glyph(&self, i: usize, origin: (f32, f32)) -> Option<PhysicalGlyph> {
        let g = &self.glyphs;
        if i >= g.len() {
            return None;
        }
        let (cache_key, x, y) = CacheKey::new(
            g.font_id[i],
            g.glyph_id[i],
            g.font_size[i],
            (g.phys_x[i] + origin.0, (g.phys_y[i] + origin.1).trunc()),
            g.font_weight[i],
            g.flags[i],
        );
        Some(PhysicalGlyph { cache_key, x, y })
    }

    /// The shaped cosmic-text buffer (physical pixels), for glyphon.
    pub fn buffer(&self) -> &Buffer {
        &self.buffer
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
            return TextOffset::snap(&self.text, self.lines.byte_start[line] as usize);
        }
        // Glyph storage order is not visual (RTL runs are stored logically),
        // so scan for the containing glyph and the visual extremes.
        let (mut left, mut right, mut inside) = (range.start, range.start, None);
        let mut nearest = (f32::INFINITY, range.start);
        for i in range {
            let (x0, x1) = (g.x[i], g.x[i] + g.advance[i]);
            if x0 < g.x[left] {
                left = i;
            }
            if x1 > g.x[right] + g.advance[right] {
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
        let byte = if x < g.x[left] {
            self.visual_left_byte(left)
        } else if x >= g.x[right] + g.advance[right] {
            self.visual_right_byte(right)
        } else {
            // Falls back to the nearest glyph when x is in a gap between glyphs.
            let i = inside.unwrap_or(nearest.1);
            let (x0, x1) = self.cluster_extent(i);
            let w = x1 - x0;
            let mut frac = if w > 0.0 { (x - x0) / w } else { 0.0 };
            if g.rtl(i) {
                frac = 1.0 - frac;
            }
            self.cluster_byte_at(i, frac.clamp(0.0, 1.0))
        };
        let start = self.lines.byte_start[line] as usize;
        let end = self.lines.byte_end[line] as usize;
        TextOffset::snap(&self.text, byte.clamp(start, end))
    }

    /// Caret position for an offset (a raw index is clamped and snapped down
    /// to a grapheme boundary). At a soft wrap the caret goes to the start
    /// of the next line.
    pub fn caret(&self, offset: impl ToTextOffset) -> Caret {
        let byte = offset.to_offset(&self.text).get();
        let line = self.line_for_byte(byte);
        Caret {
            x: self.x_for_byte(line, byte),
            y: self.lines.top[line],
            height: self.lines.height[line],
            line,
        }
    }

    /// Highlight rectangles (logical pixels) covering `range` (either
    /// order), one or more per line; RTL/mixed runs may produce several per
    /// line. Visits only the lines the range touches.
    pub fn selection_rects(&self, range: Range<impl ToTextOffset>) -> std::vec::IntoIter<Rect> {
        let len = self.text.len();
        // Snap like `caret` so arbitrary offsets never slice inside a char
        // and rect edges line up with carets.
        let range = crate::offset::ordered(&self.text, range);
        let (a, b) = (range.start.get(), range.end.get());
        let mut rects = Vec::new();
        if a == b {
            return rects.into_iter();
        }
        let g = &self.glyphs;
        for line in self.line_for_byte(a)..self.line_count() {
            let start = self.lines.byte_start[line] as usize;
            if start >= b {
                break;
            }
            // Include the line ending so an empty line inside the selection shows.
            let end_with_break = self
                .lines
                .byte_start
                .get(line + 1)
                .map_or(len, |&s| s as usize);
            if start >= b || end_with_break <= a {
                continue;
            }
            let top = self.lines.top[line];
            let height = self.lines.height[line];
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
                let (gs, ge) = (g.byte_start[i] as usize, g.byte_end[i] as usize);
                let (os, oe) = (gs.max(a), ge.min(b));
                if os >= oe {
                    continue;
                }
                if os == gs && oe == ge {
                    spans.push((g.x[i], g.x[i] + g.advance[i]));
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
        self.lines.glyph_start[line] as usize..self.lines.glyph_end[line] as usize
    }

    fn line_at_y(&self, y: f32) -> usize {
        // Line tops increase, so the last line starting at or above `y`
        // holds it; `build` guarantees at least one line.
        self.lines
            .top
            .partition_point(|&top| top <= y)
            .saturating_sub(1)
    }

    fn line_for_byte(&self, byte: usize) -> usize {
        // Line starts are strictly increasing, so the last start <= byte wins.
        self.lines
            .byte_start
            .partition_point(|&s| s as usize <= byte)
            .saturating_sub(1)
    }

    fn visual_left_byte(&self, i: usize) -> usize {
        let g = &self.glyphs;
        if g.rtl(i) {
            g.byte_end[i] as usize
        } else {
            g.byte_start[i] as usize
        }
    }

    fn visual_right_byte(&self, i: usize) -> usize {
        let g = &self.glyphs;
        if g.rtl(i) {
            g.byte_start[i] as usize
        } else {
            g.byte_end[i] as usize
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
            let (gs, ge) = (g.byte_start[i] as usize, g.byte_end[i] as usize);
            if gs <= byte && byte < ge {
                return self.x_in_cluster(i, byte);
            }
            if ge <= byte && ending_at.is_none_or(|j| g.byte_end[j] < g.byte_end[i]) {
                ending_at = Some(i);
            }
            if gs < g.byte_start[first_logical] as usize {
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
        if self.glyphs.rtl(i) { x1 } else { x0 }
    }

    fn trailing_x(&self, i: usize) -> f32 {
        let (x0, x1) = self.cluster_extent(i);
        if self.glyphs.rtl(i) { x0 } else { x1 }
    }

    /// Visual `(left, right)` of glyph `i`'s whole cluster. A cluster can
    /// shape to several adjacent glyphs (a ZWJ sequence without a color font,
    /// marks the font cannot compose), and carets must use the cluster's edges
    /// rather than its first glyph's.
    fn cluster_extent(&self, i: usize) -> (f32, f32) {
        let g = &self.glyphs;
        let same = |j: usize| {
            g.line[j] == g.line[i]
                && g.byte_start[j] == g.byte_start[i]
                && g.byte_end[j] == g.byte_end[i]
        };
        let mut first = i;
        while first > 0 && same(first - 1) {
            first -= 1;
        }
        let (mut x0, mut x1) = (f32::INFINITY, f32::NEG_INFINITY);
        for j in (first..g.len()).take_while(|&j| same(j)) {
            x0 = x0.min(g.x[j]);
            x1 = x1.max(g.x[j] + g.advance[j]);
        }
        (x0, x1)
    }

    /// x of `byte` inside glyph `i`'s cluster, splitting the advance evenly
    /// between graphemes (ligatures cover several).
    fn x_in_cluster(&self, i: usize, byte: usize) -> f32 {
        let g = &self.glyphs;
        let (gs, ge) = (g.byte_start[i] as usize, g.byte_end[i] as usize);
        let cluster = self.text.get(gs..ge).unwrap_or_default();
        let total = cluster.graphemes(true).count().max(1);
        let before = self
            .text
            .get(gs..byte.clamp(gs, ge))
            .map_or(0, |s| s.graphemes(true).count());
        let mut frac = before as f32 / total as f32;
        if g.rtl(i) {
            frac = 1.0 - frac;
        }
        let (x0, x1) = self.cluster_extent(i);
        x0 + (x1 - x0) * frac
    }

    /// Byte at logical fraction `frac` (0 = cluster start) through glyph `i`'s
    /// cluster, rounded to the nearest grapheme boundary.
    fn cluster_byte_at(&self, i: usize, frac: f32) -> usize {
        let g = &self.glyphs;
        let (gs, ge) = (g.byte_start[i] as usize, g.byte_end[i] as usize);
        let cluster = self.text.get(gs..ge).unwrap_or_default();
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

fn build_runs(glyphs: &GlyphColumns, lines: &LineColumns) -> RunColumns {
    let mut runs = RunColumns::default();
    for line in 0..lines.top.len() {
        let (start, end) = (
            lines.glyph_start[line] as usize,
            lines.glyph_end[line] as usize,
        );
        let mut run_start = start;
        for i in start..end {
            let next_breaks = i + 1 == end
                || glyphs.span[i + 1] != glyphs.span[i]
                || glyphs.rtl(i + 1) != glyphs.rtl(i);
            if next_breaks {
                runs.line.push(line as u32);
                runs.span.push(glyphs.span[i]);
                runs.rtl.push(glyphs.rtl(i));
                runs.glyph_start.push(run_start as u32);
                runs.glyph_end.push((i + 1) as u32);
                run_start = i + 1;
            }
        }
    }
    runs
}

/// Splits on `\n`, `\r\n`, `\r`, `\n\r` like cosmic-text's
/// `Buffer::set_text`, and also on the other Unicode bidi paragraph
/// separators (U+001C..U+001E, U+0085, U+2029), always ending with a
/// paragraph that has no line ending (empty after a trailing newline).
///
/// cosmic-text asserts that a line is one bidi paragraph, so a separator
/// followed by text of the other direction would panic if left inside.
fn split_paragraphs(text: &str) -> Vec<(Range<usize>, LineEnding)> {
    const OTHER_SEPARATORS: [char; 5] = ['\u{1c}', '\u{1d}', '\u{1e}', '\u{85}', '\u{2029}'];
    let mut out = Vec::new();
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
    out
}

fn family(kind: FontKind) -> Family<'static> {
    match kind {
        FontKind::Ui => Family::SansSerif,
        FontKind::Mono => Family::Monospace,
    }
}

pub(crate) fn weight_value(kind: FontKind, weight: FontWeight) -> u16 {
    match (kind, weight) {
        (FontKind::Ui, FontWeight::Normal) => 450,
        (_, FontWeight::Normal) => 400,
        (_, FontWeight::Medium) => 500,
        (_, FontWeight::Semibold) => 600,
        (_, FontWeight::Bold) => 700,
    }
}

fn base_attrs(style: &TextStyle) -> Attrs<'static> {
    Attrs::new()
        .family(family(style.font_kind))
        .weight(cosmic_text::Weight(weight_value(
            style.font_kind,
            style.font_weight,
        )))
}

/// Which generic families lack an italic face. Their italic spans are
/// slanted by the rasterizer instead (cosmic-text only does that when asked),
/// so italic stays visible with fonts such as Geist that ship no italic.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SyntheticItalic {
    ui: bool,
    mono: bool,
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
        }
    }

    fn needed(self, kind: FontKind) -> bool {
        match kind {
            FontKind::Ui => self.ui,
            FontKind::Mono => self.mono,
        }
    }
}

fn span_attrs(
    style: &TextStyle,
    span: &TextSpan,
    index: usize,
    synth: SyntheticItalic,
) -> Attrs<'static> {
    let kind = span.kind.unwrap_or(style.font_kind);
    let weight = span.weight.unwrap_or(style.font_weight);
    let italic = span.style == Some(FontStyle::Italic);
    let font_style = if italic {
        cosmic_text::Style::Italic
    } else {
        cosmic_text::Style::Normal
    };
    let flags = if italic && synth.needed(kind) {
        CacheKeyFlags::FAKE_ITALIC
    } else {
        CacheKeyFlags::empty()
    };
    base_attrs(style)
        .family(family(kind))
        .weight(cosmic_text::Weight(weight_value(kind, weight)))
        .style(font_style)
        .cache_key_flags(flags)
        .metadata(index + 1)
}
#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use quark::scene::FontStyle;
    use quark::{FontKind, FontWeight};
    use unicode_segmentation::UnicodeSegmentation;

    use super::*;
    use crate::system::test_system;

    const LOREM: &str = "The quick brown fox jumps over the lazy dog while the sleepy cat \
                         watches from a sunny windowsill and dreams of mice.";

    fn layout(text: &str, wrap: Option<f32>) -> TextLayout {
        let params = TextParams::new(text, TextStyle::new(14.0)).wrap_width(wrap);
        test_system().layout(&params).expect("layout")
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
    /// combining sequence, a ZWJ emoji sequence (all `.notdef` past Latin in
    /// the vendored fonts), spaces, and every line ending.
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
        // Lines can align within the wrap width, which can exceed the
        // measured width, and empty lines get a 0.3 em marker even in a
        // narrower layout.
        let box_width = layout.wrap_width().map_or(width, |w| w.max(width));
        let max_x = box_width.max(layout.style().font_size * 0.3) + 0.01;
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

        // Left-to-right only: cosmic-text lets an RTL word wider than the
        // wrap width overflow to negative x. Widths start above the widest
        // single cluster for the same reason.
        #[test]
        fn layout_selection_rects_stay_within_layout_bounds(
            text in text(LTR_PIECES),
            wrap in prop_oneof![Just(None), (40.0f32..300.0).prop_map(Some)],
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
            .find(|&i| text.get(g.byte_start[i] as usize..g.byte_end[i] as usize) == Some("w"))
            .expect("w glyph");
        assert_eq!((g.byte_start[w], g.line[w]), (6, 1));
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
        }];
        let params = TextParams::new("plain bold plain", TextStyle::new(14.0)).spans(spans);
        let layout = test_system().layout(&params).expect("layout");
        let runs: Vec<_> = layout.glyph_runs().collect();
        assert_eq!(runs.iter().map(|r| r.span).collect::<Vec<_>>(), [0, 1, 0]);
        assert_eq!(layout.glyphs().byte_start[runs[1].glyphs.start], 6);
    }

    // Regression: mono text used Basic shaping, which skipped font fallback.
    #[test]
    fn layout_char_missing_from_base_font_falls_back_in_both_kinds() {
        for kind in [FontKind::Ui, FontKind::Mono] {
            let params = TextParams::new("a\u{3b1}", TextStyle::new(14.0).kind(kind));
            let layout = test_system().layout(&params).expect("layout");
            let g = layout.glyphs();
            assert_eq!(g.byte_start[1], 1);
            assert_ne!(g.glyph_id[1], 0, "{kind:?} alpha is .notdef");
            assert_ne!(
                g.font_id[1], g.font_id[0],
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

    // Regression: found by layout_hit_then_caret property. Without a color
    // emoji font a ZWJ sequence is one cluster of three glyphs, and the caret
    // after it sat after the first glyph.
    #[test]
    fn caret_after_multi_glyph_cluster_sits_at_its_last_glyph() {
        let text = "a\u{1f469}\u{200d}\u{1f4bb}";
        let layout = layout(text, None);
        let end = layout.caret(text.len());
        assert!((end.x - layout.size().0).abs() < 0.01, "{end:?}");
        assert_eq!(layout.hit(end.x, end.y + 1.0), text.len());
    }

    // Regression: found by layout_caret_then_hit property. Glyph wrapping
    // put each glyph of one cluster on its own line, giving three lines with
    // the same start byte.
    #[test]
    fn layout_cluster_wider_than_wrap_width_stays_on_one_line() {
        let text = "\u{1f469}\u{200d}\u{1f4bb}";
        let layout = layout(text, Some(1.0));
        assert_eq!(layout.line_count(), 1);
        assert!(layout.caret(text.len()).x > layout.caret(0).x);
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

    // Regression: `hit` indexed line `count - 1`, guarded only by a
    // `debug_assert`, so a layout without shaped lines underflowed.
    #[test]
    fn layout_with_no_shaped_runs_still_hits_and_places_carets() {
        let params = TextParams::new("ab", TextStyle::new(14.0));
        let mut system = test_system();
        let fs = system.font_system_mut();
        let synth = SyntheticItalic::new(fs);
        let layout = TextLayout::build_with(fs, &params, synth, |_| false).expect("layout");
        assert_eq!(layout.line_count(), 1);
        assert_eq!(layout.hit(50.0, 50.0), 0);
        let caret = layout.caret(2);
        assert_eq!((caret.line, caret.x, caret.y), (0, 0.0, 0.0));
    }

    #[test]
    fn layout_invalid_params_return_matching_error() {
        let style = TextStyle::new(12.0);
        let span = |range: Range<usize>| TextSpan {
            range,
            weight: None,
            style: None,
            kind: None,
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
