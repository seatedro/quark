//! Block measurement through the frame's shared text layouts.
//!
//! Text params and heights come from the element hooks
//! ([`SelectableText::layout_params`], [`CodeBlock::layout_params`],
//! [`CodeBlock::metrics`]), so measuring a block shapes the exact layout its
//! element paints (one shaping per frame, shared through the
//! `LayoutCache`), and hit-testing maps onto the glyphs on screen.

use std::sync::Arc;

use quark_render::FontWeight;
use quark_render::scene::Rect;
use quark_text::{LayoutCache, TextLayout, TextParams, TextSystem};

use super::{
    Block, BlockContent, BlockGeometry, BlockMeasurer, IMAGE_PLACEHOLDER_HEIGHT, ImageState,
    MeasureKey, MeasureSpec, RULE_HEIGHT, TableGeometry, TableMetrics,
};
use crate::element::{
    CodeBlock, CodeHeader, LineHeight, ParagraphStyle, SelectableText, StyledSpan,
};

/// Measures blocks with the frame's text system and layout cache.
/// `font_size` is in logical points; `scale_factor` must be the one the
/// frame's `ElementContext` shapes at, or the painted layout misses the
/// cache and is shaped a second time.
pub struct TextMeasurer<'a> {
    pub text: &'a mut TextSystem,
    pub layouts: &'a mut LayoutCache,
    pub font_size: f32,
    pub scale_factor: f32,
    /// Body line height; the document sets it from its
    /// [`DocumentStyle::line_height`](super::DocumentStyle::line_height)
    /// before measuring.
    pub line_height: LineHeight,
}

impl<'a> TextMeasurer<'a> {
    pub fn new(
        text: &'a mut TextSystem,
        layouts: &'a mut LayoutCache,
        font_size: f32,
        scale_factor: f32,
    ) -> Self {
        Self {
            text,
            layouts,
            font_size,
            scale_factor,
            line_height: LineHeight::PARAGRAPH,
        }
    }

    /// The paragraph style of body text `scale` times the body size.
    fn paragraph(&self, scale: f32, weight: FontWeight) -> ParagraphStyle {
        let line_height = self.line_height.valid_or(LineHeight::PARAGRAPH);
        ParagraphStyle::new(self.font_size * scale)
            .weight(weight)
            .line_height(line_height.scaled(scale))
    }

    fn layout(&mut self, params: TextParams) -> Option<Arc<TextLayout>> {
        let params = params.scale_factor(self.scale_factor);
        self.layouts.layout(self.text, &params).ok()
    }
}

/// A block's shared layout and where its text starts inside the block.
#[derive(Debug, Clone)]
pub struct TextGeometry {
    pub layout: Option<Arc<TextLayout>>,
    pub text_origin: (f32, f32),
    pub height: f32,
    /// Content width and text length, for hit-testing blocks without a
    /// layout (rules): the left half maps to the start, the right to the
    /// end.
    width: f32,
    text_len: usize,
    /// Unwrapped content width from the inset (code), padding included.
    natural_width: Option<f32>,
    /// The grid of a table block.
    table: Option<Arc<TableGeometry>>,
}

impl BlockGeometry for TextGeometry {
    fn height(&self) -> f32 {
        self.height
    }

    fn hit(&self, x: f32, y: f32) -> usize {
        let (ox, oy) = self.text_origin;
        if let Some(table) = &self.table {
            return table.hit(x - ox, y - oy);
        }
        match &self.layout {
            Some(layout) => layout.hit(x - ox, y - oy).get(),
            None if x - ox >= self.width * 0.5 => self.text_len,
            None => 0,
        }
    }

    fn range_rects(&self, range: std::ops::Range<usize>, out: &mut Vec<Rect>) {
        let (ox, oy) = self.text_origin;
        if let Some(table) = &self.table {
            let first = out.len();
            table.range_rects(range, out);
            for rect in &mut out[first..] {
                *rect = rect.offset(ox, oy);
            }
            return;
        }
        match &self.layout {
            Some(layout) => out.extend(layout.selection_rects(range).map(|r| r.offset(ox, oy))),
            // A block without text (a rule or an image) highlights whole.
            None if range.start < range.end => out.push(Rect {
                x: ox,
                y: oy,
                width: self.width,
                height: self.height - oy,
            }),
            None => {}
        }
    }

    fn natural_width(&self) -> Option<f32> {
        self.natural_width
    }

    fn table_metrics(&self) -> Option<&TableMetrics> {
        self.table.as_ref().map(|table| &table.metrics)
    }
}

impl BlockMeasurer for TextMeasurer<'_> {
    type Geometry = TextGeometry;

    fn settings_key(&self) -> MeasureKey {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        (self.font_size.to_bits(), self.scale_factor.to_bits()).hash(&mut hasher);
        match self.line_height {
            LineHeight::Relative(factor) => (0u8, factor.to_bits()).hash(&mut hasher),
            LineHeight::Points(points) => (1u8, points.to_bits()).hash(&mut hasher),
        }
        MeasureKey {
            settings: hasher.finish(),
            fonts: Some(self.text.font_epoch()),
        }
    }

    fn apply_style(&mut self, style: &super::DocumentStyle) {
        self.line_height = style.line_height;
    }

    fn background_spec(&self) -> Option<MeasureSpec> {
        Some(MeasureSpec {
            fonts: self.text.recipe(),
            font_size: self.font_size,
            scale_factor: self.scale_factor,
        })
    }

    fn measure(&mut self, block: &Block, width: f32) -> TextGeometry {
        let style = &block.style;
        let font_size = self.font_size * style.scale;
        let inset = style.inset(self.font_size);
        let width = (width - inset).max(1.0);
        let text_len = block.text().len();
        match &block.content {
            BlockContent::Prose(spans) => {
                let paragraph = self.paragraph(style.scale, style.weight);
                let params = SelectableText::paragraph_params(spans, &paragraph, width);
                let layout = self.layout(params);
                TextGeometry {
                    height: SelectableText::paragraph_height(layout.as_deref(), &paragraph, None),
                    layout,
                    text_origin: (inset, 0.0),
                    width,
                    text_len,
                    natural_width: None,
                    table: None,
                }
            }
            BlockContent::Code {
                spans,
                line_count,
                label,
                toolbar,
                wrap,
            } => {
                let params = if *wrap {
                    CodeBlock::wrapped_layout_params(spans, font_size, width)
                } else {
                    CodeBlock::joined_layout_params(spans, font_size)
                };
                let layout = self.layout(params);
                let header = match (toolbar, label.is_some()) {
                    (true, _) => CodeHeader::Toolbar,
                    (false, true) => CodeHeader::Label,
                    (false, false) => CodeHeader::None,
                };
                let rows = match (&layout, wrap) {
                    (Some(layout), true) => layout.line_count(),
                    _ => *line_count,
                };
                let metrics = CodeBlock::header_metrics(font_size, rows, header);
                // Wrapped lines fit the column, so nothing scrolls sideways.
                let natural_width = layout
                    .as_ref()
                    .filter(|_| !wrap)
                    .map(|l| (l.size().0 + metrics.text_origin.0 * 2.0).ceil());
                TextGeometry {
                    natural_width,
                    table: None,
                    layout,
                    text_origin: (inset + metrics.text_origin.0, metrics.text_origin.1),
                    height: metrics.height,
                    width,
                    text_len,
                }
            }
            BlockContent::Image {
                state: ImageState::Failed,
                ..
            } => {
                let spans = failed_image_spans(block.text());
                let paragraph = self.paragraph(style.scale, style.weight);
                let params = SelectableText::paragraph_params(&spans, &paragraph, width);
                let layout = self.layout(params);
                TextGeometry {
                    height: SelectableText::paragraph_height(layout.as_deref(), &paragraph, None),
                    layout,
                    text_origin: (inset, 0.0),
                    width,
                    text_len,
                    natural_width: None,
                    table: None,
                }
            }
            BlockContent::Image { state, .. } => {
                let height = match state.size() {
                    Some(size) => image_extent(size, width).1,
                    None => (self.font_size * IMAGE_PLACEHOLDER_HEIGHT).ceil(),
                };
                TextGeometry {
                    layout: None,
                    text_origin: (inset, 0.0),
                    height,
                    width,
                    text_len,
                    natural_width: None,
                    table: None,
                }
            }
            BlockContent::Table(cells) => {
                let paragraph = self.paragraph(style.scale, FontWeight::Normal);
                let table = TableGeometry::new(cells, &paragraph, |params| self.layout(params));
                TextGeometry {
                    layout: None,
                    text_origin: (inset, 0.0),
                    height: table.metrics.height(cells.rows),
                    width,
                    text_len,
                    natural_width: Some(table.metrics.width()),
                    table: Some(Arc::new(table)),
                }
            }
            BlockContent::Rule => TextGeometry {
                layout: None,
                text_origin: (inset, 0.0),
                height: (font_size * RULE_HEIGHT).ceil(),
                width,
                text_len,
                natural_width: None,
                table: None,
            },
        }
    }
}

/// The size an image of intrinsic `(width, height)` pixels shows at in a
/// column `max_width` wide: its own size, scaled down to fit the width.
/// One image pixel is one logical point.
pub(super) fn image_extent((width, height): (u32, u32), max_width: f32) -> (f32, f32) {
    let (w, h) = (width.max(1) as f32, height as f32);
    let shown = w.min(max_width.max(1.0));
    (shown, (h * shown / w).ceil())
}

/// The alt text an image that failed to load shows, italic as inline
/// image alt text is.
pub(super) fn failed_image_spans(alt: &str) -> Arc<[StyledSpan]> {
    Arc::from([StyledSpan {
        italic: true,
        ..StyledSpan::plain(alt)
    }])
}
