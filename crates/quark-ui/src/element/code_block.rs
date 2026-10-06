use super::*;

// ---------------------------------------------------------------------------
// CodeBlock — fenced syntax-highlighted code (no wrapping)
// ---------------------------------------------------------------------------

/// A rounded panel of monospace code. Each inner `Vec<StyledSpan>` is one source
/// line; lines are never wrapped, so height is exact (`n*line_height + padding`).
pub struct CodeBlock {
    lines: Vec<Vec<StyledSpan>>,
    width: f32,
    font_size: f32,
    source_key: u64,
}

pub fn code_block(lines: Vec<Vec<StyledSpan>>) -> CodeBlock {
    CodeBlock {
        lines,
        width: 0.0,
        font_size: 0.0,
        source_key: 0,
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
}

impl Element for CodeBlock {
    type LayoutState = f32; // line_height
    type PrepaintState = ();

    fn request_layout(
        &mut self,
        engine: &mut LayoutEngine,
        _cx: &mut ElementContext,
    ) -> (LayoutId, Self::LayoutState) {
        let line_height = self.line_height();
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
        (id, line_height)
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
        state: &mut f32,
        _prepaint_state: &mut (),
        _engine: &LayoutEngine,
        scene: &mut Scene,
        cx: &mut ElementContext,
    ) {
        let line_height = *state;
        let default_color = cx.text_color_override().unwrap_or(cx.theme.colors.text);
        let radius = (self.font_size * 0.35).min(8.0);
        let pad_x = self.pad_x();
        let pad_y = self.pad_y();

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

        let text_left = bounds.x + pad_x;
        let text_width = (bounds.width - pad_x * 2.0).max(1.0);
        for (i, line) in self.lines.iter().enumerate() {
            let y = bounds.y + pad_y + i as f32 * line_height;
            let line_text: String = line.iter().map(|s| s.text.as_str()).collect();
            let mut off = 0usize;
            let mut pen = 0.0f32;
            for span in line {
                let lo = off;
                let hi = off + span.text.len();
                off = hi;
                let sub = &line_text[lo..hi];
                if sub.is_empty() {
                    continue;
                }
                let piece_adv = measure_text_advance(
                    cx.font_system,
                    sub,
                    self.font_size,
                    span.font_kind,
                    span.font_weight,
                );
                let x = text_left + pen;

                // Trim leading whitespace (shapers drop a buffer's leading space)
                // and re-add it as a positional offset; see SelectableText::paint.
                let trimmed = sub.trim_start();
                if !trimmed.is_empty() {
                    let lead = sub.len() - trimmed.len();
                    let lead_adv = if lead > 0 {
                        measure_text_advance(
                            cx.font_system,
                            &sub[..lead],
                            self.font_size,
                            span.font_kind,
                            span.font_weight,
                        )
                    } else {
                        0.0
                    };
                    let piece_color = span.color.unwrap_or(default_color);
                    let remaining = (text_width - pen - lead_adv).max(0.0);
                    let rect = Rect {
                        x: x + lead_adv,
                        y,
                        width: remaining,
                        height: line_height,
                    };
                    if span.italic {
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
        scene.pop_clip();

        if !cx.accessibility_text_hidden() && bounds.width > 0.0 && bounds.height > 0.0 {
            let body: String = self
                .lines
                .iter()
                .map(|line| line.iter().map(|s| s.text.as_str()).collect::<String>())
                .collect::<Vec<_>>()
                .join("\n");
            if !body.is_empty() {
                cx.push_accessibility(
                    AccessibilityNode::new(
                        format!(
                            "code-block:{:?}:{:.0}:{:.0}",
                            self.source_key, bounds.x, bounds.y
                        ),
                        AccessibilityRole::Label,
                        bounds,
                    )
                    .label(body),
                );
            }
        }
    }
}

impl IntoAnyElement for CodeBlock {
    fn into_any(self) -> AnyElement {
        element_into_any(self)
    }
}
