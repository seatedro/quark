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
/// `TextInputHitArea`. Carries the exact wrapped runs that were painted so
/// `pointer.rs` maps a click onto the same visual lines (no re-shape → no
/// divergence) and on to a byte offset into `text`. `source_key` identifies which
/// logical text this is, so a selection survives re-wrap and only highlights its
/// own block.
#[derive(Debug, Clone)]
pub struct SelectableTextRegion {
    pub bounds: Rect,
    pub text_origin: (f32, f32),
    pub text: String,
    pub runs: Vec<WrappedRun>,
    pub line_height: f32,
    pub font_size: f32,
    pub font_kind: FontKind,
    pub font_weight: FontWeight,
    pub source_key: u64,
}

/// Static text that wraps to `width` and supports mouse drag-selection + copy.
/// Selection state lives in app state (keyed by byte offsets into the source
/// string, which survive re-wrap); the element renders the highlight from a
/// resolved `selection` range and registers a `SelectableTextRegion` for input.
pub struct SelectableText {
    spans: Vec<StyledSpan>,
    width: f32,
    font_size: f32,
    /// Base font used for the region's (approximate) hit-test and for plain text.
    /// Rich spans carry their own kind/weight; this is the fallback/normal style.
    font_kind: FontKind,
    font_weight: FontWeight,
    color: Option<Color>,
    max_lines: usize,
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
        max_lines: 64,
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
    pub fn max_lines(mut self, n: usize) -> Self {
        self.max_lines = n;
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
}

impl Element for SelectableText {
    type LayoutState = (Vec<WrappedRun>, f32); // (wrapped runs, line_height)
    type PrepaintState = ();

    fn request_layout(
        &mut self,
        engine: &mut LayoutEngine,
        cx: &mut ElementContext,
    ) -> (LayoutId, Self::LayoutState) {
        let line_height = self.font_size * 1.35;
        // Wrap from a RICH shaping so wrap points respect each span's font (a mono
        // code run is wider than UI text), then map the glyph byte ranges back onto
        // the concatenated plain string.
        let runs = wrap_rich_text_to_runs(
            cx.font_system,
            &self.spans,
            self.font_size,
            self.font_kind,
            self.font_weight,
            self.width.max(1.0),
            self.max_lines,
        );
        let n = runs.len().max(1);
        let height = (n as f32 * line_height).ceil();
        let id = engine.request_layout(
            taffy::Style {
                size: taffy::Size {
                    width: taffy::Dimension::length(self.width),
                    height: taffy::Dimension::length(height),
                },
                flex_shrink: 0.0,
                ..Default::default()
            },
            &[],
        );
        (id, (runs, line_height))
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
        state: &mut (Vec<WrappedRun>, f32),
        _prepaint_state: &mut (),
        _engine: &LayoutEngine,
        scene: &mut Scene,
        cx: &mut ElementContext,
    ) {
        let (runs, line_height) = state;
        let line_height = *line_height;
        let default_color = cx
            .text_color_override()
            .or(self.color)
            .unwrap_or(cx.theme.colors.text);

        // Plain string (markers already stripped) + each span's byte range within it.
        let text: String = self.spans.iter().map(|s| s.text.as_str()).collect();
        let mut span_ranges: Vec<(usize, usize)> = Vec::with_capacity(self.spans.len());
        {
            let mut off = 0usize;
            for s in &self.spans {
                let end = off + s.text.len();
                span_ranges.push((off, end));
                off = end;
            }
        }

        let selection = self.selection.filter(|(a, b)| a < b);
        let hl = cx.theme.colors.accent.with_alpha(Alpha::SOFT);

        // Paint each visual line as a sequence of single-style pieces, positioning
        // each piece by accumulating `measure_text_width` from the line's left edge.
        // The renderer draws each piece at exactly these offsets, so the pill,
        // highlight, and text all share one coordinate system (no drift).
        for run in runs.iter() {
            let mut pen = 0.0f32;
            for (span, &(ss, se)) in self.spans.iter().zip(span_ranges.iter()) {
                let lo = ss.max(run.start);
                let hi = se.min(run.end);
                if lo >= hi || !text.is_char_boundary(lo) || !text.is_char_boundary(hi) {
                    continue;
                }
                let sub = &text[lo..hi];
                // True pen advance (includes trailing spaces, which `line_w` trims) so
                // a span ending in a space doesn't make the next piece abut its word.
                let piece_adv = measure_text_advance(
                    cx.font_system,
                    sub,
                    self.font_size,
                    span.font_kind,
                    span.font_weight,
                );
                let x = bounds.x + pen;
                let y = bounds.y + run.line_top;

                // Inline-code background pill: drawn snug around the run without
                // advancing the pen, so byte→x mapping for selection stays linear.
                if let Some(bg) = span.pill {
                    scene.rounded_rect(RoundedRectPrimitive::uniform(
                        Rect {
                            x: x - 2.0,
                            y: y + line_height * 0.1,
                            width: piece_adv + 4.0,
                            height: line_height * 0.8,
                        },
                        4.0,
                        bg,
                    ));
                }

                // Selection highlight (above the pill, behind the glyphs), measured
                // within this piece's own font so edges land on glyph edges.
                if let Some((slo, shi)) = selection {
                    let l = slo.max(lo);
                    let r = shi.min(hi);
                    if l < r && text.is_char_boundary(l) && text.is_char_boundary(r) {
                        let x0 = measure_text_advance(
                            cx.font_system,
                            &text[lo..l],
                            self.font_size,
                            span.font_kind,
                            span.font_weight,
                        );
                        let x1 = measure_text_advance(
                            cx.font_system,
                            &text[lo..r],
                            self.font_size,
                            span.font_kind,
                            span.font_weight,
                        );
                        scene.rounded_rect(RoundedRectPrimitive::uniform(
                            Rect {
                                x: x + x0,
                                y,
                                width: (x1 - x0).max(1.0),
                                height: line_height,
                            },
                            2.0,
                            hl,
                        ));
                    }
                }

                // Shapers drop a buffer's leading whitespace, so a piece that starts
                // with a space would abut the previous word. Trim the leading space
                // from the rendered text and shift the draw position by its advance —
                // the pen still moves by the full width, keeping the gap intact.
                let trimmed = sub.trim_start();
                if !trimmed.is_empty() {
                    let lead = sub.len() - trimmed.len();
                    let lead_adv = if lead > 0 {
                        measure_text_advance(
                            cx.font_system,
                            &text[lo..lo + lead],
                            self.font_size,
                            span.font_kind,
                            span.font_weight,
                        )
                    } else {
                        0.0
                    };
                    let piece_color = span.color.unwrap_or(default_color);
                    // Position is fixed by the pen; give the piece the rest of the
                    // column as width so the renderer (which clips the buffer to rect
                    // width) never shaves the last glyph — italic slant under-measures.
                    let rect = Rect {
                        x: x + lead_adv,
                        y,
                        width: (self.width - pen - lead_adv).max(piece_adv + 2.0),
                        height: line_height,
                    };
                    if span.italic {
                        // `TextPrimitive` has no style field; italic needs a rich span.
                        scene.rich_text(RichTextPrimitive {
                            rect,
                            spans: vec![RichTextSpan {
                                text: trimmed.into(),
                                color: piece_color,
                                font_weight: Some(span.font_weight),
                                font_style: Some(FontStyle::Italic),
                            }]
                            .into(),
                            default_color: piece_color,
                            font_size: self.font_size,
                            font_kind: span.font_kind,
                            font_weight: span.font_weight,
                        });
                    } else {
                        scene.text(TextPrimitive {
                            rect,
                            text: trimmed.into(),
                            color: piece_color,
                            font_size: self.font_size,
                            font_kind: span.font_kind,
                            font_weight: span.font_weight,
                        });
                    }
                }

                pen += piece_adv;
            }
        }

        if !text.is_empty()
            && !cx.accessibility_text_hidden()
            && bounds.width > 0.0
            && bounds.height > 0.0
        {
            cx.accessibility.push(
                AccessibilityNode::new(
                    format!(
                        "selectable-text:{:?}:{:.0}:{:.0}",
                        self.source_key, bounds.x, bounds.y
                    ),
                    AccessibilityRole::Label,
                    bounds,
                )
                .label(text.clone()),
            );
        }

        cx.selectable_text_runs.push(SelectableTextRegion {
            bounds,
            text_origin: (bounds.x, bounds.y),
            text,
            runs: std::mem::take(runs),
            line_height,
            font_size: self.font_size,
            font_kind: self.font_kind,
            font_weight: self.font_weight,
            source_key: self.source_key,
        });
    }
}

impl IntoAnyElement for SelectableText {
    fn into_any(self) -> AnyElement {
        element_into_any(self)
    }
}
