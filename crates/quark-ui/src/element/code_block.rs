use super::*;

// ---------------------------------------------------------------------------
// CodeBlock — fenced syntax-highlighted code (no wrapping)
// ---------------------------------------------------------------------------

/// A rounded panel of monospace code. Each inner `Vec<StyledSpan>` is one source
/// line; lines are never wrapped, so height is exact (`n*line_height + padding`).
/// The lines are laid out as one text joined with `\n`, which is the string
/// selection offsets index and copy reads.
pub struct CodeBlock {
    lines: Vec<Vec<StyledSpan>>,
    width: f32,
    font_size: f32,
    source_key: u64,
    selection: Option<(usize, usize)>,
}

pub fn code_block(lines: Vec<Vec<StyledSpan>>) -> CodeBlock {
    CodeBlock {
        lines,
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

    fn line_height(&self) -> f32 {
        self.font_size * 1.4
    }
    // Padding scales with the font so the panel reads the same at any ui_scale.
    fn pad_x(&self) -> f32 {
        self.font_size * 0.6
    }
    fn pad_y(&self) -> f32 {
        self.font_size * 0.5
    }

    /// The lines flattened into spans, with a plain `\n` span between lines.
    fn joined_spans(&self) -> Vec<StyledSpan> {
        let mut spans = Vec::new();
        for (i, line) in self.lines.iter().enumerate() {
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
}

/// The layout and the spans it was built from (span colors index into them).
pub struct CodeBlockState {
    layout: Option<Arc<TextLayout>>,
    spans: Vec<StyledSpan>,
}

impl Element for CodeBlock {
    type LayoutState = CodeBlockState;
    type PrepaintState = ();

    fn request_layout(
        &mut self,
        engine: &mut LayoutEngine,
        cx: &mut ElementContext,
    ) -> (LayoutId, Self::LayoutState) {
        let line_height = self.line_height();
        let spans = self.joined_spans();
        let style = TextStyle::new(self.font_size)
            .kind(FontKind::Mono)
            .line_height(line_height);
        let layout = cx.layout_text(&styled_params(&spans, style, None));
        let n = self.lines.len().max(1);
        let height = (n as f32 * line_height + self.pad_y() * 2.0).ceil();
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
        (id, CodeBlockState { layout, spans })
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
        let origin = (bounds.x + self.pad_x(), bounds.y + self.pad_y());

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
