use super::*;

// ---------------------------------------------------------------------------
// TextElement — text with intrinsic sizing
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TextAlign {
    #[default]
    Left,
    Center,
    Right,
}

pub struct TextElement {
    content: String,
    font_size: f32,
    line_height_factor: f32,
    color: Option<Color>,
    font_kind: FontKind,
    font_weight: FontWeight,
    align: TextAlign,
    truncate: bool,
    wrap_width: Option<f32>,
}

pub fn text(content: impl Into<String>) -> TextElement {
    TextElement {
        content: content.into(),
        font_size: 0.0,
        line_height_factor: 1.5,
        color: None,
        font_kind: FontKind::Ui,
        font_weight: FontWeight::Normal,
        align: TextAlign::Left,
        truncate: false,
        wrap_width: None,
    }
}

impl TextElement {
    pub fn size(mut self, size: f32) -> Self {
        self.font_size = size;
        self
    }

    pub fn text_sm(mut self) -> Self {
        self.font_size = -1.0; // sentinel: use ui_small_font_size
        self
    }

    pub fn text_xs(mut self) -> Self {
        self.font_size = -2.0; // sentinel: use caption size
        self
    }

    pub fn text_lg(mut self) -> Self {
        self.font_size = -3.0; // sentinel: use heading_font_size
        self
    }

    pub fn color(mut self, color: Color) -> Self {
        self.color = Some(color);
        self
    }

    pub fn mono(mut self) -> Self {
        self.font_kind = FontKind::Mono;
        self
    }

    pub fn line_height(mut self, factor: f32) -> Self {
        self.line_height_factor = factor;
        self
    }

    pub fn bold(mut self) -> Self {
        self.font_weight = FontWeight::Bold;
        self
    }

    pub fn semibold(mut self) -> Self {
        self.font_weight = FontWeight::Semibold;
        self
    }

    pub fn medium(mut self) -> Self {
        self.font_weight = FontWeight::Medium;
        self
    }

    pub fn text_center(mut self) -> Self {
        self.align = TextAlign::Center;
        self
    }

    pub fn text_right(mut self) -> Self {
        self.align = TextAlign::Right;
        self
    }

    pub fn truncate(mut self) -> Self {
        self.truncate = true;
        self
    }

    /// Wraps lines at `width` pixels; the element is `width` wide and as tall
    /// as the wrapped text. Alignment and truncation apply to unwrapped text
    /// only.
    pub fn wrap_width(mut self, width: f32) -> Self {
        self.wrap_width = Some(width.max(1.0));
        self
    }

    fn query<'s>(&self, content: &'s str, font_size: f32) -> TextQuery<'s> {
        let style = TextStyle::new(font_size)
            .kind(self.font_kind)
            .weight(self.font_weight)
            .line_height(font_size * self.line_height_factor);
        TextQuery::new(content, style).wrap_width(self.wrap_width)
    }

    fn resolve_font_size(&self, theme: &Theme) -> f32 {
        match self.font_size.to_bits() {
            _ if self.font_size > 0.0 => self.font_size,
            _ if self.font_size == -1.0 => theme.metrics.ui_small_font_size,
            _ if self.font_size == -2.0 => theme.metrics.ui_small_font_size - 1.0,
            _ if self.font_size == -3.0 => theme.metrics.heading_font_size,
            _ => theme.metrics.ui_font_size, // 0 or anything else
        }
    }
}

/// Resolved font size, the shaped content (shared with paint), and its
/// natural width.
pub struct TextLayoutState {
    font_size: f32,
    layout: Option<Arc<TextLayout>>,
    natural_width: f32,
}

impl Element for TextElement {
    type LayoutState = TextLayoutState;
    type PrepaintState = ();

    fn request_layout(
        &mut self,
        engine: &mut LayoutEngine,
        cx: &mut ElementContext,
    ) -> (LayoutId, Self::LayoutState) {
        let font_size = self.resolve_font_size(cx.theme);
        let line_height = font_size * self.line_height_factor;
        let layout = cx.layout_text_query(&self.query(&self.content, font_size));
        let (layout_width, layout_height) = layout.as_ref().map_or((0.0, 0.0), |l| l.size());
        let text_width = layout_width.ceil();
        let (width, height) = match self.wrap_width {
            Some(wrap) => (wrap, layout_height.max(line_height).ceil()),
            None => (text_width, line_height),
        };

        // Only allow shrinking when `.truncate()` is set; otherwise the text
        // holds its natural width so it isn't crushed next to flex_shrink:0 siblings
        // like SvgIcon.
        let shrink = if self.truncate { 1.0 } else { 0.0 };

        let id = engine.request_layout(
            taffy::Style {
                size: taffy::Size {
                    width: taffy::Dimension::length(width),
                    height: taffy::Dimension::length(height),
                },
                flex_shrink: shrink,
                ..Default::default()
            },
            &[],
        );
        let state = TextLayoutState {
            font_size,
            layout,
            natural_width: text_width,
        };
        (id, state)
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
        state: &mut TextLayoutState,
        _prepaint_state: &mut (),
        _engine: &LayoutEngine,
        scene: &mut Scene,
        cx: &mut ElementContext,
    ) {
        let font_size = state.font_size;
        let natural_width = state.natural_width;
        let color = cx
            .text_color_override()
            .or(self.color)
            .unwrap_or(cx.theme.colors.text);

        let mut content = std::mem::take(&mut self.content);
        let mut layout = state.layout.take();
        let mut text_width = natural_width;

        let wraps = self.wrap_width.is_some();
        if self.truncate && !wraps && bounds.width > 0.0 && natural_width > bounds.width {
            let (truncated, truncated_width) = truncate_text_to_fit(
                cx,
                &content,
                font_size,
                self.font_kind,
                self.font_weight,
                natural_width,
                bounds.width,
            );
            layout = cx.layout_text_query(&self.query(&truncated, font_size));
            content = truncated;
            text_width = truncated_width;
        }

        let x_offset = match self.align {
            _ if wraps => 0.0,
            TextAlign::Left => 0.0,
            TextAlign::Center => ((bounds.width - text_width) * 0.5).max(0.0),
            TextAlign::Right => (bounds.width - text_width).max(0.0),
        };

        if let Some(layout) = layout {
            scene.text(TextPrimitive {
                rect: Rect {
                    x: bounds.x + x_offset,
                    ..bounds
                },
                layout: ShapedText::new(layout),
                color,
            });
        }

        if !content.is_empty()
            && cx.accessibility_enabled()
            && !cx.accessibility_text_hidden()
            && bounds.width > 0.0
            && bounds.height > 0.0
        {
            let text_bounds = Rect {
                x: bounds.x + x_offset,
                y: bounds.y,
                width: text_width.max(1.0).min(bounds.width.max(1.0)),
                height: bounds.height,
            };
            cx.push_accessibility(
                AccessibilityNode::new(
                    format!(
                        "text:{:?}:{:.0}:{:.0}:{:.0}:{:.0}",
                        content,
                        text_bounds.x,
                        text_bounds.y,
                        text_bounds.width,
                        text_bounds.height
                    ),
                    AccessibilityRole::Label,
                    text_bounds,
                )
                .label(content.clone()),
            );
        }

        // Debug wireframe: red rect around text + log measurement vs bounds
        if cx.debug_wireframe {
            let measured =
                cx.measure_text_width(&content, font_size, self.font_kind, self.font_weight);
            let crushed = bounds.width > 0.0 && bounds.width < measured * 0.9;
            let wire_color = if crushed {
                Color::rgba(255, 40, 40, 200) // red = text is crushed
            } else {
                Color::rgba(40, 200, 40, 120) // green = text fits
            };
            scene.border(BorderPrimitive {
                rect: bounds,
                widths: [1.0; 4],
                corner_radii: [0.0; 4],
                color: wire_color,
            });
            // Log short strings (likely button labels) to stderr
            if content.len() < 30 {
                eprintln!(
                    "[wireframe] text={:20} measured={:6.1} bounds_w={:6.1} scale={:.2} {}",
                    format!("{:?}", content),
                    measured,
                    bounds.width,
                    cx.scale_factor,
                    if crushed { "CRUSHED" } else { "ok" },
                );
            }
        }
    }
}

impl IntoAnyElement for TextElement {
    fn into_any(self) -> AnyElement {
        element_into_any(self)
    }
}

/// Allow `"string literal"` as a child element directly.
impl IntoAnyElement for &str {
    fn into_any(self) -> AnyElement {
        element_into_any(text(self))
    }
}

/// Text as a child: lets `view!` pass text to a component's `children`
/// prop through `Into`.
impl From<&str> for AnyElement {
    fn from(text: &str) -> Self {
        text.into_any()
    }
}

impl From<String> for AnyElement {
    fn from(text: String) -> Self {
        text.into_any()
    }
}

impl IntoAnyElement for String {
    fn into_any(self) -> AnyElement {
        element_into_any(text(self))
    }
}
