use super::*;

// ---------------------------------------------------------------------------
// SelectableText — multi-line, mouse-selectable static text
// ---------------------------------------------------------------------------

/// One run of comment text with a single display style. The concatenation of
/// every span's `text` is the plain string selection and copy operate on, so the
/// markup markers (backticks, asterisks, link URLs) must already be stripped.
#[derive(Debug, Clone)]
pub struct StyledSpan {
    pub text: String,
    pub font_kind: FontKind,
    pub font_weight: FontWeight,
    pub italic: bool,
    /// `None` paints in the block's default color.
    pub color: Option<Color>,
    /// `Some(bg)` paints a rounded background pill behind the run (inline code).
    pub pill: Option<Color>,
}

impl StyledSpan {
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            font_kind: FontKind::Ui,
            font_weight: FontWeight::Normal,
            italic: false,
            color: None,
            pill: None,
        }
    }
}

/// Per-frame record of a painted selectable-text block, mirroring
/// `TextInputHitArea`. Carries the layout that was painted, so pointer
/// hit-testing maps a click onto exactly the glyphs on screen (bold, italic,
/// and code runs included) and on to a byte offset into `text`. `source_key`
/// identifies which logical text this is, so a selection survives re-wrap and
/// only highlights its own block.
#[derive(Debug, Clone)]
pub struct SelectableTextRegion {
    pub bounds: Rect,
    pub text_origin: (f32, f32),
    pub text: Arc<str>,
    pub layout: Arc<TextLayout>,
    pub source_key: u64,
}

impl SelectableTextRegion {
    /// Byte offset (grapheme boundary) nearest to a scene-space point.
    pub fn hit(&self, x: f32, y: f32) -> usize {
        self.layout
            .hit(x - self.text_origin.0, y - self.text_origin.1)
    }
}

/// Static text that wraps to `width` and supports mouse drag-selection + copy.
/// Selection state lives in app state (keyed by byte offsets into the source
/// string, which survive re-wrap); the element renders the highlight from a
/// resolved `selection` range and registers a `SelectableTextRegion` for input.
pub struct SelectableText {
    spans: Vec<StyledSpan>,
    width: f32,
    font_size: f32,
    /// Base font for text outside any span's overrides.
    font_kind: FontKind,
    font_weight: FontWeight,
    color: Option<Color>,
    max_lines: Option<usize>,
    source_key: u64,
    selection: Option<(usize, usize)>,
}

pub fn selectable_text(text: impl Into<String>) -> SelectableText {
    selectable_rich_text(vec![StyledSpan::plain(text)])
}

/// Selectable text whose runs carry inline styles (code/bold/italic/link). The
/// concatenation of the span texts is the plain body; selection/copy/a11y all
/// operate on that string, so styling never changes what gets copied.
pub fn selectable_rich_text(spans: Vec<StyledSpan>) -> SelectableText {
    SelectableText {
        spans,
        width: 0.0,
        font_size: 0.0,
        font_kind: FontKind::Ui,
        font_weight: FontWeight::Normal,
        color: None,
        max_lines: None,
        source_key: 0,
        selection: None,
    }
}

impl SelectableText {
    pub fn width(mut self, w: f32) -> Self {
        self.width = w;
        self
    }
    pub fn size(mut self, s: f32) -> Self {
        self.font_size = s;
        self
    }
    pub fn color(mut self, c: Color) -> Self {
        self.color = Some(c);
        self
    }
    pub fn weight(mut self, w: FontWeight) -> Self {
        self.font_weight = w;
        self
    }
    /// Shows at most `n` lines; the rest is laid out but clipped. Unlimited by
    /// default.
    pub fn max_lines(mut self, n: usize) -> Self {
        self.max_lines = Some(n);
        self
    }
    pub fn source(mut self, key: u64) -> Self {
        self.source_key = key;
        self
    }
    /// Resolved (normalized) byte range to highlight, or `None` when this block
    /// is not the selected one. Highlight is painted behind the text, so passing
    /// a selection never alters layout (measure == render).
    pub fn selection(mut self, selection: Option<(usize, usize)>) -> Self {
        self.selection = selection;
        self
    }

    fn line_height(&self) -> f32 {
        self.font_size * 1.35
    }
}

/// One layout for the concatenated span texts, each span's font applied to
/// its byte range. Span `i` of the layout is `spans[i]`, which is how paint
/// maps glyphs back to span colors.
pub(super) fn styled_params(
    spans: &[StyledSpan],
    style: TextStyle,
    wrap_width: Option<f32>,
) -> TextParams {
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

/// Paints inline-code pills behind each span that has one, snug around the
/// span's glyphs on every line it covers.
pub(super) fn paint_pills(
    scene: &mut Scene,
    layout: &TextLayout,
    spans: &[StyledSpan],
    origin: (f32, f32),
) {
    for (span, text_span) in spans.iter().zip(layout.spans().iter()) {
        let Some(bg) = span.pill else {
            continue;
        };
        for r in layout.selection_rects(text_span.range.clone()) {
            scene.rounded_rect(RoundedRectPrimitive::uniform(
                Rect {
                    x: origin.0 + r.x - 2.0,
                    y: origin.1 + r.y + r.height * 0.1,
                    width: r.width + 4.0,
                    height: r.height * 0.8,
                },
                4.0,
                bg,
            ));
        }
    }
}

pub(super) fn paint_selection(
    scene: &mut Scene,
    layout: &TextLayout,
    selection: Option<(usize, usize)>,
    origin: (f32, f32),
    color: Color,
) {
    let Some((lo, hi)) = selection.filter(|(a, b)| a < b) else {
        return;
    };
    for r in layout.selection_rects(lo..hi) {
        scene.rounded_rect(RoundedRectPrimitive::uniform(
            Rect {
                x: origin.0 + r.x,
                y: origin.1 + r.y,
                width: r.width.max(1.0),
                height: r.height,
            },
            2.0,
            color,
        ));
    }
}

pub(super) fn span_colors(spans: &[StyledSpan], default_color: Color) -> Arc<[Color]> {
    spans
        .iter()
        .map(|span| span.color.unwrap_or(default_color))
        .collect()
}

impl Element for SelectableText {
    type LayoutState = Option<Arc<TextLayout>>;
    type PrepaintState = ();

    fn request_layout(
        &mut self,
        engine: &mut LayoutEngine,
        cx: &mut ElementContext,
    ) -> (LayoutId, Self::LayoutState) {
        let line_height = self.line_height();
        let style = TextStyle::new(self.font_size)
            .kind(self.font_kind)
            .weight(self.font_weight)
            .line_height(line_height);
        let params = styled_params(&self.spans, style, Some(self.width.max(1.0)));
        let layout = cx.layout_text(&params);
        let height = match &layout {
            Some(layout) => match self.max_lines.and_then(|n| layout.line(n)) {
                Some(first_hidden) => first_hidden.top,
                None => layout.size().1,
            },
            None => line_height,
        };
        let id = engine.request_layout(
            taffy::Style {
                size: taffy::Size {
                    width: taffy::Dimension::length(self.width),
                    height: taffy::Dimension::length(height.max(line_height).ceil()),
                },
                flex_shrink: 0.0,
                ..Default::default()
            },
            &[],
        );
        (id, layout)
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
        state: &mut Option<Arc<TextLayout>>,
        _prepaint_state: &mut (),
        _engine: &LayoutEngine,
        scene: &mut Scene,
        cx: &mut ElementContext,
    ) {
        let Some(layout) = state.take() else {
            return;
        };
        let default_color = cx
            .text_color_override()
            .or(self.color)
            .unwrap_or(cx.theme.colors.text);
        let origin = (bounds.x, bounds.y);
        let clipped = self.max_lines.is_some_and(|n| n < layout.line_count());
        if clipped {
            scene.clip(bounds);
        }

        paint_pills(scene, &layout, &self.spans, origin);
        let highlight = cx.theme.colors.accent.with_alpha(Alpha::SOFT);
        paint_selection(scene, &layout, self.selection, origin, highlight);

        // Italic glyphs ink past their advance; widen the text rect (which
        // the renderer clips to) so the last glyph of a line is not shaved.
        scene.rich_text(RichTextPrimitive {
            rect: Rect {
                width: bounds.width + self.font_size * 0.5,
                ..bounds
            },
            layout: ShapedText::new(layout.clone()),
            default_color,
            span_colors: span_colors(&self.spans, default_color),
        });

        if clipped {
            scene.pop_clip();
        }

        let text = layout.text().clone();
        if !text.is_empty()
            && !cx.accessibility_text_hidden()
            && bounds.width > 0.0
            && bounds.height > 0.0
        {
            cx.push_accessibility(
                AccessibilityNode::new(
                    format!(
                        "selectable-text:{:?}:{:.0}:{:.0}",
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

impl IntoAnyElement for SelectableText {
    fn into_any(self) -> AnyElement {
        element_into_any(self)
    }
}
