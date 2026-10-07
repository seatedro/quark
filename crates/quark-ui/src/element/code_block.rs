use super::*;

// ---------------------------------------------------------------------------
// CodeBlock — fenced syntax-highlighted code (no wrapping)
// ---------------------------------------------------------------------------

/// Line height factor of code text.
const CODE_LINE_HEIGHT: f32 = 1.4;
/// Panel padding factors. Padding scales with the font so the panel reads
/// the same at any ui_scale.
const CODE_PAD_X: f32 = 0.6;
const CODE_PAD_Y: f32 = 0.5;
/// Label text size and the height of the label row above the code.
const LABEL_SIZE: f32 = 0.8;
const LABEL_ROW: f32 = 1.4;

/// A rounded panel of monospace code. Each inner `Vec<StyledSpan>` is one source
/// line; lines are never wrapped, so height is exact (`n*line_height + padding`).
/// The lines are laid out as one text joined with `\n`, which is the string
/// selection offsets index and copy reads. An optional label (the fence
/// language) sits in a row above the code and is not part of that string.
pub struct CodeBlock {
    /// The lines joined by [`join_code_lines`], shared so a caller that keeps
    /// them builds the element every frame without copying their text.
    spans: Arc<[StyledSpan]>,
    line_count: usize,
    label: Option<Arc<str>>,
    width: f32,
    font_size: f32,
    source_key: u64,
    selection: Option<(usize, usize)>,
}

/// Where a code block's text sits and how tall the block is, for a font
/// size, line count, and whether it shows a label.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CodeBlockMetrics {
    pub height: f32,
    /// Top left of the code text relative to the block's top left.
    pub text_origin: (f32, f32),
}

pub fn code_block(lines: Vec<Vec<StyledSpan>>) -> CodeBlock {
    let line_count = lines.len();
    code_block_joined(join_code_lines(&lines), line_count)
}

/// A code block from lines already joined by [`join_code_lines`].
pub fn code_block_joined(spans: impl Into<Arc<[StyledSpan]>>, line_count: usize) -> CodeBlock {
    CodeBlock {
        spans: spans.into(),
        line_count,
        label: None,
        width: 0.0,
        font_size: 0.0,
        source_key: 0,
        selection: None,
    }
}

impl CodeBlock {
    pub fn width(mut self, w: f32) -> Self {
        self.width = w;
        self
    }
    pub fn size(mut self, s: f32) -> Self {
        self.font_size = s;
        self
    }
    pub fn source(mut self, key: u64) -> Self {
        self.source_key = key;
        self
    }
    /// Byte range of the joined text to highlight; see
    /// [`SelectableText::selection`].
    pub fn selection(mut self, selection: Option<(usize, usize)>) -> Self {
        self.selection = selection;
        self
    }
    /// A label above the code, normally the fence language. Empty labels
    /// are not shown.
    pub fn label(mut self, label: Option<Arc<str>>) -> Self {
        self.label = label.filter(|l| !l.is_empty());
        self
    }

    /// The text params `request_layout` shapes for `lines`: one unwrapped
    /// monospace text with the lines joined by `\n`.
    pub fn layout_params(lines: &[Vec<StyledSpan>], font_size: f32) -> TextParams {
        Self::joined_layout_params(&join_code_lines(lines), font_size)
    }

    /// [`Self::layout_params`] for lines already joined by
    /// [`join_code_lines`].
    pub fn joined_layout_params(spans: &[StyledSpan], font_size: f32) -> TextParams {
        let style = TextStyle::new(font_size)
            .kind(FontKind::Mono)
            .line_height(font_size * CODE_LINE_HEIGHT);
        styled_params(spans, style, None)
    }

    /// Height and text origin of a block of `line_count` lines.
    pub fn metrics(font_size: f32, line_count: usize, labeled: bool) -> CodeBlockMetrics {
        let pad_y = font_size * CODE_PAD_Y;
        let label = if labeled { font_size * LABEL_ROW } else { 0.0 };
        let rows = line_count.max(1) as f32;
        CodeBlockMetrics {
            height: (rows * font_size * CODE_LINE_HEIGHT + pad_y * 2.0 + label).ceil(),
            text_origin: (font_size * CODE_PAD_X, pad_y + label),
        }
    }
}

/// The lines flattened into spans, with a plain `\n` span between lines:
/// the text a code block lays out, selects, and copies.
pub fn join_code_lines(lines: &[Vec<StyledSpan>]) -> Vec<StyledSpan> {
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
    spans
}

/// The layout and the spans it was built from (span colors index into them).
pub struct CodeBlockState {
    layout: Option<Arc<TextLayout>>,
    label: Option<Arc<TextLayout>>,
    spans: Arc<[StyledSpan]>,
}

impl Element for CodeBlock {
    type LayoutState = CodeBlockState;
    type PrepaintState = ();

    fn request_layout(
        &mut self,
        engine: &mut LayoutEngine,
        cx: &mut ElementContext,
    ) -> (LayoutId, Self::LayoutState) {
        let params = Self::joined_layout_params(&self.spans, self.font_size);
        let layout = cx.layout_text(&params);
        let spans = self.spans.clone();
        let label = self.label.as_ref().and_then(|label| {
            let size = self.font_size * LABEL_SIZE;
            cx.layout_text(&TextParams::new(
                label.to_string(),
                TextStyle::new(size)
                    .kind(FontKind::Ui)
                    .line_height(size * 1.2),
            ))
        });
        let height = Self::metrics(self.font_size, self.line_count, self.label.is_some()).height;
        let id = engine.request_layout(
            taffy::Style {
                size: taffy::Size {
                    width: taffy::Dimension::length(self.width),
                    height: taffy::Dimension::length(height),
                },
                flex_shrink: 1.0,
                ..Default::default()
            },
            &[],
        );
        (
            id,
            CodeBlockState {
                layout,
                label,
                spans,
            },
        )
    }

    fn prepaint(
        &mut self,
        _bounds: Bounds,
        _layout_state: &mut Self::LayoutState,
        _engine: &LayoutEngine,
        _cx: &mut ElementContext,
    ) {
    }

    fn paint(
        &mut self,
        bounds: Bounds,
        state: &mut CodeBlockState,
        _prepaint_state: &mut (),
        _engine: &LayoutEngine,
        scene: &mut Scene,
        cx: &mut ElementContext,
    ) {
        let default_color = cx.text_color_override().unwrap_or(cx.theme.colors.text);
        let radius = (self.font_size * 0.35).min(8.0);
        let metrics = Self::metrics(self.font_size, self.line_count, self.label.is_some());
        let origin = (
            bounds.x + metrics.text_origin.0,
            bounds.y + metrics.text_origin.1,
        );

        // Inset panel: darker than the card with a hairline border so it reads as
        // a code block, and clipped so long lines truncate at its edge rather than
        // bleeding past the card.
        scene.rounded_rect(RoundedRectPrimitive::uniform(
            bounds,
            radius,
            cx.theme.colors.background,
        ));
        scene.border(BorderPrimitive::uniform(
            bounds,
            1.0,
            radius,
            cx.theme.colors.border,
        ));
        scene.clip_rounded(bounds, [radius; 4]);

        if let Some(label) = state.label.take() {
            let (w, h) = label.size();
            let row = self.font_size * LABEL_ROW;
            scene.rich_text(RichTextPrimitive {
                rect: Rect {
                    x: origin.0,
                    y: bounds.y + self.font_size * CODE_PAD_Y + (row - h) * 0.5,
                    width: w + self.font_size,
                    height: h.max(1.0),
                },
                layout: ShapedText::new(label),
                default_color: cx.theme.colors.text_muted,
                span_colors: Arc::from([]),
            });
        }

        let Some(layout) = state.layout.take() else {
            scene.pop_clip();
            return;
        };
        let highlight = cx.theme.colors.accent.with_alpha(Alpha::SOFT);
        paint_selection(scene, &layout, self.selection, origin, highlight);
        let (text_width, text_height) = layout.size();
        scene.rich_text(RichTextPrimitive {
            rect: Rect {
                x: origin.0,
                y: origin.1,
                width: text_width.max(bounds.width) + self.font_size,
                height: text_height.max(1.0),
            },
            layout: ShapedText::new(layout.clone()),
            default_color,
            span_colors: span_colors(&state.spans, default_color, cx.theme.colors.text_accent),
        });
        scene.pop_clip();

        let text = layout.text().clone();
        if !cx.accessibility_text_hidden()
            && bounds.width > 0.0
            && bounds.height > 0.0
            && !text.is_empty()
        {
            cx.push_accessibility(
                AccessibilityNode::new(
                    format!(
                        "code-block:{:?}:{:.0}:{:.0}",
                        self.source_key, bounds.x, bounds.y
                    ),
                    AccessibilityRole::Label,
                    bounds,
                )
                .label(text.to_string()),
            );
        }

        cx.selectable_text_runs.push(SelectableTextRegion {
            bounds,
            text_origin: origin,
            text,
            layout,
            source_key: self.source_key,
        });
    }
}

impl IntoAnyElement for CodeBlock {
    fn into_any(self) -> AnyElement {
        element_into_any(self)
    }
}
