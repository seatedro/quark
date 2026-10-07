//! Block measurement through the frame's shared text layouts.
//!
//! Text params and heights come from the element hooks
//! ([`SelectableText::layout_params`], [`CodeBlock::layout_params`],
//! [`CodeBlock::metrics`]), so measuring a block shapes the exact layout its
//! element paints (one shaping per frame, shared through the
//! `LayoutCache`), and hit-testing maps onto the glyphs on screen.

use std::sync::Arc;

use quark_render::FontKind;
use quark_text::{LayoutCache, TextLayout, TextParams, TextSystem};

use super::{BlockContent, BlockGeometry, BlockMeasurer, RULE_HEIGHT, TranscriptBlock};
use crate::element::{CodeBlock, SelectableText};

/// Measures blocks with the frame's text system and layout cache.
/// `font_size` is in logical points; `scale_factor` must be the one the
/// frame's `ElementContext` shapes at, or the painted layout misses the
/// cache and is shaped a second time.
pub struct TextMeasurer<'a> {
    pub text: &'a mut TextSystem,
    pub layouts: &'a mut LayoutCache,
    pub font_size: f32,
    pub scale_factor: f32,
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
        }
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
}

impl BlockGeometry for TextGeometry {
    fn height(&self) -> f32 {
        self.height
    }

    fn hit(&self, x: f32, y: f32) -> usize {
        let (ox, oy) = self.text_origin;
        match &self.layout {
            Some(layout) => layout.hit(x - ox, y - oy).get(),
            None if x - ox >= self.width * 0.5 => self.text_len,
            None => 0,
        }
    }
}

impl BlockMeasurer for TextMeasurer<'_> {
    type Geometry = TextGeometry;

    fn settings_key(&self) -> u64 {
        (u64::from(self.font_size.to_bits()) << 32) | u64::from(self.scale_factor.to_bits())
    }

    fn measure(&mut self, block: &TranscriptBlock, width: f32) -> TextGeometry {
        let style = &block.style;
        let font_size = self.font_size * style.scale;
        let inset = style.inset(self.font_size);
        let width = (width - inset).max(1.0);
        let text_len = block.text().len();
        match &block.content {
            BlockContent::Prose(spans) => {
                let params = SelectableText::layout_params(
                    spans,
                    font_size,
                    FontKind::Ui,
                    style.weight,
                    width,
                );
                let layout = self.layout(params);
                TextGeometry {
                    height: SelectableText::measured_height(layout.as_deref(), font_size, None),
                    layout,
                    text_origin: (inset, 0.0),
                    width,
                    text_len,
                }
            }
            BlockContent::Code {
                spans,
                line_count,
                label,
            } => {
                let layout = self.layout(CodeBlock::joined_layout_params(spans, font_size));
                let metrics = CodeBlock::metrics(font_size, *line_count, label.is_some());
                TextGeometry {
                    layout,
                    text_origin: (inset + metrics.text_origin.0, metrics.text_origin.1),
                    height: metrics.height,
                    width,
                    text_len,
                }
            }
            BlockContent::Rule => TextGeometry {
                layout: None,
                text_origin: (inset, 0.0),
                height: (font_size * RULE_HEIGHT).ceil(),
                width,
                text_len,
            },
        }
    }
}
