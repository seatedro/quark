//! Block measurement through the frame's shared text layouts.
//!
//! The parameters here mirror what `SelectableText` and `CodeBlock` build in
//! `request_layout`, so measuring a block shapes the exact layout its
//! element paints (one shaping per frame, shared through the
//! `LayoutCache`), and hit-testing maps onto the glyphs on screen. The
//! `measure_matches_painted_blocks` test fails if the two drift apart.

use std::sync::Arc;

use quark_render::scene::FontStyle;
use quark_render::{FontKind, FontWeight};
use quark_text::{LayoutCache, TextLayout, TextParams, TextSpan, TextStyle, TextSystem};

use super::{BlockContent, BlockGeometry, BlockMeasurer, TranscriptBlock};
use crate::element::StyledSpan;

/// `SelectableText`'s line height factor.
const PROSE_LINE_HEIGHT: f32 = 1.35;
/// `CodeBlock`'s line height and padding factors.
const CODE_LINE_HEIGHT: f32 = 1.4;
const CODE_PAD_X: f32 = 0.6;
const CODE_PAD_Y: f32 = 0.5;

/// Measures blocks with the frame's text system and layout cache.
pub struct TextMeasurer<'a> {
    pub text: &'a mut TextSystem,
    pub layouts: &'a mut LayoutCache,
    pub font_size: f32,
}

impl<'a> TextMeasurer<'a> {
    pub fn new(text: &'a mut TextSystem, layouts: &'a mut LayoutCache, font_size: f32) -> Self {
        Self {
            text,
            layouts,
            font_size,
        }
    }
}

/// A block's shared layout and where its text starts inside the block.
#[derive(Debug, Clone)]
pub struct TextGeometry {
    pub layout: Option<Arc<TextLayout>>,
    pub text_origin: (f32, f32),
    pub height: f32,
}

impl BlockGeometry for TextGeometry {
    fn height(&self) -> f32 {
        self.height
    }

    fn hit(&self, x: f32, y: f32) -> usize {
        self.layout.as_ref().map_or(0, |layout| {
            layout.hit(x - self.text_origin.0, y - self.text_origin.1)
        })
    }
}

impl BlockMeasurer for TextMeasurer<'_> {
    type Geometry = TextGeometry;

    fn measure(&mut self, block: &TranscriptBlock, width: f32) -> TextGeometry {
        let font_size = self.font_size;
        match &block.content {
            BlockContent::Prose(spans) => {
                let line_height = font_size * PROSE_LINE_HEIGHT;
                let style = TextStyle::new(font_size)
                    .kind(FontKind::Ui)
                    .weight(FontWeight::Normal)
                    .line_height(line_height);
                let params = span_params(spans, style, Some(width.max(1.0)));
                let layout = self.layouts.layout(self.text, &params).ok();
                let text_height = layout.as_ref().map_or(line_height, |l| l.size().1);
                TextGeometry {
                    layout,
                    text_origin: (0.0, 0.0),
                    height: text_height.max(line_height).ceil(),
                }
            }
            BlockContent::Code(lines) => {
                let line_height = font_size * CODE_LINE_HEIGHT;
                let pad_y = font_size * CODE_PAD_Y;
                let mut spans = Vec::new();
                for (i, line) in lines.iter().enumerate() {
                    if i > 0 {
                        spans.push(StyledSpan {
                            font_kind: FontKind::Mono,
                            ..StyledSpan::plain("\n")
                        });
                    }
                    spans.extend(line.iter().cloned());
                }
                let style = TextStyle::new(font_size)
                    .kind(FontKind::Mono)
                    .line_height(line_height);
                let params = span_params(&spans, style, None);
                let layout = self.layouts.layout(self.text, &params).ok();
                let rows = lines.len().max(1) as f32;
                TextGeometry {
                    layout,
                    text_origin: (font_size * CODE_PAD_X, pad_y),
                    height: (rows * line_height + pad_y * 2.0).ceil(),
                }
            }
        }
    }
}

/// The concatenated span texts with each span's font on its byte range.
fn span_params(spans: &[StyledSpan], style: TextStyle, wrap_width: Option<f32>) -> TextParams {
    let mut text = String::with_capacity(spans.iter().map(|s| s.text.len()).sum());
    let mut text_spans = Vec::with_capacity(spans.len());
    for span in spans {
        let start = text.len();
        text.push_str(&span.text);
        text_spans.push(TextSpan {
            range: start..text.len(),
            weight: Some(span.font_weight),
            style: span.italic.then_some(FontStyle::Italic),
            kind: Some(span.font_kind),
        });
    }
    TextParams::new(text, style)
        .spans(text_spans)
        .wrap_width(wrap_width)
}
