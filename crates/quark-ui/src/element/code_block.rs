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
/// Height of a toolbar row, tall enough for icon buttons at a code font
/// of 12 points to meet a 28-point target.
const TOOLBAR_ROW: f32 = 2.4;

/// The row above a code block's lines.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum CodeHeader {
    /// No row.
    #[default]
    None,
    /// A short row the block paints its label in.
    Label,
    /// A taller row for a label and actions that the caller places over
    /// it (see [`CodeBlock::toolbar`]); the block paints neither, so they
    /// stay put while the lines scroll sideways.
    Toolbar,
}

impl CodeHeader {
    /// Height of the row at `font_size`.
    pub fn height(self, font_size: f32) -> f32 {
        match self {
            Self::None => 0.0,
            Self::Label => font_size * LABEL_ROW,
            Self::Toolbar => font_size * TOOLBAR_ROW,
        }
    }

    /// Label size of the row at `font_size`.
    pub fn label_size(font_size: f32) -> f32 {
        font_size * LABEL_SIZE
    }
}

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
    toolbar: bool,
    wrap: bool,
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
        toolbar: false,
        wrap: false,
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

    /// Reserves a [`CodeHeader::Toolbar`] row above the code for the
    /// caller's label and actions, painting no label itself.
    pub fn toolbar(mut self, toolbar: bool) -> Self {
        self.toolbar = toolbar;
        self
    }

    /// Wraps lines at the block's width instead of letting them run past
    /// it. Selection offsets and copy are unchanged.
    pub fn wrap(mut self, wrap: bool) -> Self {
        self.wrap = wrap;
        self
    }

    fn header(&self) -> CodeHeader {
        match (self.toolbar, self.label.is_some()) {
            (true, _) => CodeHeader::Toolbar,
            (false, true) => CodeHeader::Label,
            (false, false) => CodeHeader::None,
        }
    }

    /// Width lines wrap at in a block `width` wide, when wrapping.
    pub fn wrap_width(width: f32, font_size: f32) -> f32 {
        (width - font_size * CODE_PAD_X * 2.0).max(1.0)
    }

    /// The text params `request_layout` shapes for `lines`: one unwrapped
    /// monospace text with the lines joined by `\n`.
    pub fn layout_params(lines: &[Vec<StyledSpan>], font_size: f32) -> TextParams {
        Self::joined_layout_params(&join_code_lines(lines), font_size)
    }

    /// [`Self::layout_params`] for lines already joined by
    /// [`join_code_lines`].
    pub fn joined_layout_params(spans: &[StyledSpan], font_size: f32) -> TextParams {
        styled_params(spans, Self::style(font_size), None)
    }

    /// [`Self::joined_layout_params`] for a block `width` wide that wraps
    /// its lines.
    pub fn wrapped_layout_params(spans: &[StyledSpan], font_size: f32, width: f32) -> TextParams {
        let wrap = Self::wrap_width(width, font_size);
        styled_params(spans, Self::style(font_size), Some(wrap))
    }

    fn style(font_size: f32) -> TextStyle {
        TextStyle::new(font_size)
            .kind(FontKind::Mono)
            .line_height(font_size * CODE_LINE_HEIGHT)
    }

    /// Height and text origin of a block of `line_count` lines.
    pub fn metrics(font_size: f32, line_count: usize, labeled: bool) -> CodeBlockMetrics {
        let header = if labeled {
            CodeHeader::Label
        } else {
            CodeHeader::None
        };
        Self::header_metrics(font_size, line_count, header)
    }

    /// Height and text origin of a block of `rows` visual lines under
    /// `header`.
    pub fn header_metrics(font_size: f32, rows: usize, header: CodeHeader) -> CodeBlockMetrics {
        let pad_y = font_size * CODE_PAD_Y;
        let header = header.height(font_size);
        let rows = rows.max(1) as f32;
        CodeBlockMetrics {
            height: (rows * font_size * CODE_LINE_HEIGHT + pad_y * 2.0 + header).ceil(),
            text_origin: (font_size * CODE_PAD_X, pad_y + header),
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
    metrics: CodeBlockMetrics,
}

impl Element for CodeBlock {
    type LayoutState = CodeBlockState;
    type PrepaintState = ();

    fn request_layout(
        &mut self,
        engine: &mut LayoutEngine,
        cx: &mut ElementContext,
    ) -> (LayoutId, Self::LayoutState) {
        let style = Self::style(self.font_size);
        let wrap = self
            .wrap
            .then(|| Self::wrap_width(self.width, self.font_size));
        let layout = with_styled_query(&self.spans, style, wrap, |q| cx.layout_text_query(q));
        let spans = self.spans.clone();
        let header = self.header();
        let label = self.label.as_ref().filter(|_| header == CodeHeader::Label);
        let label = label.and_then(|label| {
            let size = self.font_size * LABEL_SIZE;
            cx.layout_text_query(&TextQuery::new(
                label,
                TextStyle::new(size)
                    .kind(FontKind::Ui)
                    .line_height(size * 1.2),
            ))
        });
        let rows = match (&layout, self.wrap) {
            (Some(layout), true) => layout.line_count(),
            _ => self.line_count,
        };
        let metrics = Self::header_metrics(self.font_size, rows, header);
        let id = engine.request_layout(
            taffy::Style {
                size: taffy::Size {
                    width: taffy::Dimension::length(self.width),
                    height: taffy::Dimension::length(metrics.height),
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
                metrics,
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
        let metrics = state.metrics;
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

        let text = layout.source().clone();
        if cx.accessibility_enabled()
            && !cx.accessibility_text_hidden()
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

        register_selectable(
            cx,
            SelectableTextRegion {
                bounds,
                text_origin: origin,
                text,
                layout,
                source_key: self.source_key,
                transform: Transform2D::IDENTITY,
            },
        );
    }
}

impl IntoAnyElement for CodeBlock {
    fn into_any(self) -> AnyElement {
        element_into_any(self)
    }
}
