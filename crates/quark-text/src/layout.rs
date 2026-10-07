use std::ops::Range;
use std::sync::Arc;

use cosmic_text::{
    Attrs, AttrsList, Buffer, BufferLine, CacheKey, CacheKeyFlags, Family, FontSystem, LineEnding,
    Metrics, PhysicalGlyph, Shaping, Wrap, fontdb,
};
use quark::scene::FontStyle;
use quark::{FontKind, FontWeight, Rect};
use unicode_segmentation::{GraphemeCursor, UnicodeSegmentation};

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

impl TextParams {
    pub fn new(text: impl Into<Arc<str>>, style: TextStyle) -> Self {
        Self {
            text: text.into(),
            spans: Arc::from(Vec::new()),
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
    pub(crate) fn build(fs: &mut FontSystem, params: &TextParams) -> Result<Self, TextError> {
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
        // Scanning the font database costs a pass over every face; skip it
        // for the common case of text without italic spans.
        let synth = if params
            .spans
            .iter()
            .any(|s| s.style == Some(FontStyle::Italic))
        {
            SyntheticItalic::new(fs)
        } else {
            SyntheticItalic {
                ui: false,
                mono: false,
            }
        };
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
                BufferLine::new(&text[range.clone()], *ending, attrs, Shaping::Advanced)
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

        for run in buffer.layout_runs() {
            let para_start = paragraphs[run.line_i].0.start;
            let line_index = lines.top.len() as u32;
            let glyph_start = glyphs.len() as u32;
            let mut min_start = usize::MAX;
            for g in run.glyphs {
                let start = para_start + g.start;
                min_start = min_start.min(start);
                glyphs.x.push(g.x * inv);
                glyphs.advance.push(g.w * inv);
                glyphs.line.push(line_index);
                glyphs.byte_start.push(start as u32);
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
            let first_of_paragraph = line_paragraph.last() != Some(&run.line_i);
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
            width = width.max(run.line_w * inv);
            height = height.max((run.line_top + run.line_height) * inv);
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
        Ok(Self {
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
        })
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

    /// Byte offset (grapheme boundary) nearest to the logical point. Points
    /// outside the text clamp to the nearest line and line edge.
    pub fn hit(&self, x: f32, y: f32) -> usize {
        let line = self.line_at_y(y);
        let g = &self.glyphs;
        let range = self.glyph_range(line);
        if range.is_empty() {
            return self.lines.byte_start[line] as usize;
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
            let w = g.advance[i];
            let mut frac = if w > 0.0 { (x - g.x[i]) / w } else { 0.0 };
            if g.rtl(i) {
                frac = 1.0 - frac;
            }
            self.cluster_byte_at(i, frac.clamp(0.0, 1.0))
        };
        let start = self.lines.byte_start[line] as usize;
        let end = self.lines.byte_end[line] as usize;
        self.snap_grapheme(byte.clamp(start, end))
    }

    /// Caret position for a byte offset (clamped, snapped down to a grapheme
    /// boundary). At a soft wrap the caret goes to the start of the next line.
    pub fn caret(&self, byte: usize) -> Caret {
        let byte = self.snap_grapheme(byte.min(self.text.len()));
        let line = self.line_for_byte(byte);
        Caret {
            x: self.x_for_byte(line, byte),
            y: self.lines.top[line],
            height: self.lines.height[line],
            line,
        }
    }

    /// Highlight rectangles (logical pixels) covering `range`, one or more per
    /// line; RTL/mixed runs may produce several per line.
    pub fn selection_rects(&self, range: Range<usize>) -> impl Iterator<Item = Rect> + use<> {
        let len = self.text.len();
        let a = range.start.min(range.end).min(len);
        let b = range.end.max(range.start).min(len);
        let mut rects = Vec::new();
        if a == b {
            return rects.into_iter();
        }
        let g = &self.glyphs;
        for line in 0..self.line_count() {
            let start = self.lines.byte_start[line] as usize;
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
        let l = &self.lines;
        let count = self.line_count();
        let i = (0..count).position(|i| y < l.top[i] + l.height[i]);
        i.unwrap_or(count - 1)
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
        let g = &self.glyphs;
        if g.rtl(i) {
            g.x[i] + g.advance[i]
        } else {
            g.x[i]
        }
    }

    fn trailing_x(&self, i: usize) -> f32 {
        let g = &self.glyphs;
        if g.rtl(i) {
            g.x[i]
        } else {
            g.x[i] + g.advance[i]
        }
    }

    /// x of `byte` inside glyph `i`'s cluster, splitting the advance evenly
    /// between graphemes (ligatures cover several).
    fn x_in_cluster(&self, i: usize, byte: usize) -> f32 {
        let g = &self.glyphs;
        let (gs, ge) = (g.byte_start[i] as usize, g.byte_end[i] as usize);
        let cluster = &self.text[gs..ge];
        let total = cluster.graphemes(true).count().max(1);
        let before = self.text[gs..byte.clamp(gs, ge)].graphemes(true).count();
        let mut frac = before as f32 / total as f32;
        if g.rtl(i) {
            frac = 1.0 - frac;
        }
        g.x[i] + g.advance[i] * frac
    }

    /// Byte at logical fraction `frac` (0 = cluster start) through glyph `i`'s
    /// cluster, rounded to the nearest grapheme boundary.
    fn cluster_byte_at(&self, i: usize, frac: f32) -> usize {
        let g = &self.glyphs;
        let (gs, ge) = (g.byte_start[i] as usize, g.byte_end[i] as usize);
        let cluster = &self.text[gs..ge];
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

    fn snap_grapheme(&self, mut byte: usize) -> usize {
        let text = self.text.as_ref();
        while !text.is_char_boundary(byte) {
            byte -= 1;
        }
        let mut cursor = GraphemeCursor::new(byte, text.len(), true);
        match cursor.is_boundary(text, 0) {
            Ok(true) => byte,
            _ => cursor.prev_boundary(text, 0).ok().flatten().unwrap_or(0),
        }
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

/// Splits on `\n`, `\r\n`, `\r`, `\n\r` exactly like cosmic-text's
/// `Buffer::set_text`, always ending with a paragraph that has no line ending
/// (empty after a trailing newline).
fn split_paragraphs(text: &str) -> Vec<(Range<usize>, LineEnding)> {
    let mut out = Vec::new();
    let mut start = 0;
    while let Some(i) = text[start..].find(['\r', '\n']) {
        let end = start + i;
        let after = &text[end..];
        let ending = if after.starts_with("\r\n") {
            LineEnding::CrLf
        } else if after.starts_with("\n\r") {
            LineEnding::LfCr
        } else if after.starts_with('\n') {
            LineEnding::Lf
        } else {
            LineEnding::Cr
        };
        out.push((start..end, ending));
        start = end + ending.as_str().len();
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
struct SyntheticItalic {
    ui: bool,
    mono: bool,
}

impl SyntheticItalic {
    fn new(fs: &FontSystem) -> Self {
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
