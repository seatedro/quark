use super::*;
use quark_render::scene::{
    AlphaMask, FadeEdge, StyledDecoration, TextBackdrop, TextDecorationStyle, TextFill,
    TextRendering,
};
use quark_text::fonts::FontFamily;

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

/// What one-line text does when its box is narrower than the text. The
/// whole text stays the element's accessible label either way.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TextOverflow {
    /// Cut at the box's edge.
    Clip,
    /// Shortened at a grapheme boundary to end in an ellipsis.
    Ellipsis,
    /// Faded out over this many points at the box's trailing edge,
    /// showing whatever lies behind (a hovered row, a material).
    Fade(f32),
}

/// How a [`TextElement`] breaks into lines.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub enum WrapMode {
    /// Wrap to the width layout gives the element: as wide as the text up
    /// to the width offered, and as tall as the lines it wraps into. In a
    /// flex row it shrinks down to its widest word.
    #[default]
    Auto,
    /// One line at the text's natural width, which never shrinks.
    NoWrap,
    /// Wrap at this many points. The element is this wide whatever its
    /// container offers, so it can overflow a narrower one.
    Explicit(f32),
}

/// Height of one line box of text. Resolved to logical points before
/// shaping, so measurement, paint, selection, and hit testing all use the
/// same line boxes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LineHeight {
    /// A multiple of the font size.
    Relative(f32),
    /// Logical points, whatever the font size.
    Points(f32),
}

impl LineHeight {
    /// Line height of [`text`]: 1.5 times the font size.
    pub const TEXT: Self = Self::Relative(1.5);
    /// Line height of selectable text, rich text, and documents: 1.35 times
    /// the font size.
    pub const PARAGRAPH: Self = Self::Relative(1.35);

    /// Whether the value is finite and positive. Builders ignore other
    /// values and keep the line height they had.
    pub fn is_valid(self) -> bool {
        let value = match self {
            Self::Relative(factor) => factor,
            Self::Points(points) => points,
        };
        value.is_finite() && value > 0.0
    }

    /// `self` when valid, else `fallback`.
    pub fn valid_or(self, fallback: Self) -> Self {
        if self.is_valid() { self } else { fallback }
    }

    /// The line box height in logical points for text of `font_size`.
    pub fn resolve(self, font_size: f32) -> f32 {
        match self {
            Self::Relative(factor) => font_size * factor,
            Self::Points(points) => points,
        }
    }

    /// The line height of text `factor` times as large: a fixed height
    /// grows with it, a relative one already does. Headings scale the body
    /// line height this way.
    pub fn scaled(self, factor: f32) -> Self {
        match self {
            Self::Relative(relative) => Self::Relative(relative),
            Self::Points(points) => Self::Points(points * factor),
        }
    }
}

impl Default for LineHeight {
    fn default() -> Self {
        Self::PARAGRAPH
    }
}

pub struct TextElement {
    content: String,
    font_size: f32,
    line_height: LineHeight,
    color: Option<Color>,
    font_kind: FontKind,
    font_weight: FontWeight,
    /// A family in place of `font_kind`'s generic one.
    family: Option<FontFamily>,
    /// Extra advance after every glyph, in ems.
    letter_spacing: f32,
    underline: bool,
    /// How the underline looks; `None` is solid in the text color.
    underline_style: Option<TextDecorationStyle>,
    strikethrough: bool,
    paint: TextPaint,
    align: TextAlign,
    /// One line that shrinks with its box, overflowing this way.
    overflow: Option<TextOverflow>,
    wrap: WrapMode,
}

pub fn text(content: impl Into<String>) -> TextElement {
    TextElement {
        content: content.into(),
        font_size: 0.0,
        line_height: LineHeight::TEXT,
        color: None,
        font_kind: FontKind::Ui,
        font_weight: FontWeight::Normal,
        family: None,
        letter_spacing: 0.0,
        underline: false,
        underline_style: None,
        strikethrough: false,
        paint: TextPaint::default(),
        align: TextAlign::Left,
        overflow: None,
        wrap: WrapMode::Auto,
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

    /// Line height as a multiple of the font size (1.5 by default).
    pub fn line_height(self, factor: f32) -> Self {
        self.line_height_of(LineHeight::Relative(factor))
    }

    /// Line height in logical points, whatever the font size.
    pub fn line_height_points(self, points: f32) -> Self {
        self.line_height_of(LineHeight::Points(points))
    }

    /// Line height as either kind; invalid values are ignored.
    pub fn line_height_of(mut self, line_height: LineHeight) -> Self {
        if line_height.is_valid() {
            self.line_height = line_height;
        }
        self
    }

    pub fn weight(mut self, weight: FontWeight) -> Self {
        self.font_weight = weight;
        self
    }

    /// Draws in `family`: the system UI or monospace font, or a named one
    /// from [`TextSystem::family_id`]. Wins over [`Self::mono`]; a family
    /// the system lacks draws in the generic one.
    pub fn font_family(mut self, family: FontFamily) -> Self {
        self.family = Some(family);
        self
    }

    /// Extra advance after every glyph, in ems (zero by default). Applies
    /// to measurement, wrapping, and paint alike. Nonfinite values are
    /// ignored.
    pub fn letter_spacing(mut self, ems: f32) -> Self {
        if ems.is_finite() {
            self.letter_spacing = ems;
        }
        self
    }

    /// A solid line under the text, in its color.
    pub fn underline(mut self) -> Self {
        self.underline = true;
        self
    }

    /// A line under the text in `style`: dotted, dashed, or solid, with
    /// its own thickness, offset, and color.
    pub fn underline_style(mut self, style: TextDecorationStyle) -> Self {
        self.underline = true;
        self.underline_style = Some(style);
        self
    }

    /// Fills the glyphs with `fill` (a gradient or a shimmer) in place of
    /// the text color. A shimmer sweeps with the frame clock and stands
    /// still under reduced motion. Never reshapes the text.
    pub fn fill(mut self, fill: TextFill) -> Self {
        self.paint.fill = Some(fill);
        self
    }

    /// How glyph coverage blends; see [`TextRendering`]. The renderer's
    /// default applies without it.
    pub fn text_rendering(mut self, rendering: TextRendering) -> Self {
        self.paint.rendering = Some(rendering);
        self
    }

    /// The opaque color behind the text, which perceptual coverage needs;
    /// see [`TextBackdrop`].
    pub fn text_backdrop(mut self, backdrop: TextBackdrop) -> Self {
        self.paint.backdrop = backdrop;
        self
    }

    /// A solid line through the text, in its color.
    pub fn strikethrough(mut self) -> Self {
        self.strikethrough = true;
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

    /// One line that shrinks with an ellipsis when its box is narrower
    /// than the text. Truncated text does not wrap automatically.
    pub fn truncate(self) -> Self {
        self.overflow(TextOverflow::Ellipsis)
    }

    /// One line that shrinks with its box and overflows it as `overflow`
    /// says. Like [`Self::truncate`], it does not wrap automatically.
    pub fn overflow(mut self, overflow: TextOverflow) -> Self {
        self.overflow = Some(overflow);
        self
    }

    /// Wraps lines at `width` pixels; the element is `width` wide and as tall
    /// as the wrapped text. Alignment and truncation apply to unwrapped text
    /// only.
    pub fn wrap_width(mut self, width: f32) -> Self {
        self.wrap = WrapMode::Explicit(width.max(1.0));
        self
    }

    /// Keep the text on one line at its natural width instead of wrapping
    /// it to its box.
    pub fn no_wrap(mut self) -> Self {
        self.wrap = WrapMode::NoWrap;
        self
    }

    pub fn wrap(mut self, mode: WrapMode) -> Self {
        self.wrap = match mode {
            WrapMode::Explicit(width) => WrapMode::Explicit(width.max(1.0)),
            mode => mode,
        };
        self
    }

    /// The wrap mode in effect: truncation keeps text on one line.
    fn wrap_mode(&self) -> WrapMode {
        match self.wrap {
            WrapMode::Auto if self.overflow.is_some() => WrapMode::NoWrap,
            mode => mode,
        }
    }

    fn query<'s>(&self, content: &'s str, font_size: f32, wrap: Option<f32>) -> TextQuery<'s> {
        let mut style = TextStyle::new(font_size)
            .kind(self.font_kind)
            .weight(self.font_weight)
            .letter_spacing(self.letter_spacing)
            .line_height(self.line_height.resolve(font_size));
        if let Some(family) = self.family {
            style = style.font_family(family);
        }
        TextQuery::new(content, style).wrap_width(wrap)
    }

    /// The decorations to paint over `len` bytes of text in `color`. A
    /// plain [`Self::underline`] takes the text color.
    fn decorations(&self, len: usize, color: Color) -> Vec<StyledDecoration> {
        let underline = self.underline.then(|| StyledDecoration {
            range: 0..len,
            kind: TextDecorationKind::Underline,
            style: self
                .underline_style
                .unwrap_or_else(|| TextDecorationStyle::solid(color)),
        });
        let strike = self.strikethrough.then(|| StyledDecoration {
            range: 0..len,
            kind: TextDecorationKind::Strikethrough,
            style: TextDecorationStyle::solid(color),
        });
        underline.into_iter().chain(strike).collect()
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

/// The element's node, resolved font size, the shaped content (shared
/// with paint; unwrapped unless the wrap width is explicit), and its
/// natural width.
pub struct TextLayoutState {
    id: LayoutId,
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
        let line_height = self.line_height.resolve(font_size);
        let wrap = self.wrap_mode();
        let explicit = match wrap {
            WrapMode::Explicit(width) => Some(width),
            WrapMode::Auto | WrapMode::NoWrap => None,
        };
        let layout = cx.layout_text_query(&self.query(&self.content, font_size, explicit));
        let (layout_width, layout_height) = layout.as_ref().map_or((0.0, 0.0), |l| l.size());
        let text_width = layout_width.ceil();

        let id = match (wrap, &layout) {
            // Sized by layout: shrinks in a flex row, wrapping as it does.
            (WrapMode::Auto, Some(unwrapped)) => engine.request_text_layout(
                &taffy::Style::default(),
                TextMeasure::new(unwrapped.clone(), line_height),
            ),
            _ => {
                let (width, height) = match explicit {
                    Some(wrap) => (wrap, layout_height.max(line_height).ceil()),
                    None => (text_width, line_height),
                };
                // Unwrapped text shrinks only when `.truncate()` is set;
                // otherwise it holds its natural width so it isn't crushed
                // next to flex_shrink:0 siblings like SvgIcon.
                let shrink = if self.overflow.is_some() { 1.0 } else { 0.0 };
                engine.request_layout(
                    taffy::Style {
                        size: taffy::Size {
                            width: taffy::Dimension::length(width),
                            height: taffy::Dimension::length(height),
                        },
                        flex_shrink: shrink,
                        ..Default::default()
                    },
                    &[],
                )
            }
        };
        let state = TextLayoutState {
            id,
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
        engine: &LayoutEngine,
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

        let wrap = self.wrap_mode();
        let mut wrapped = false;
        let overflows =
            wrap == WrapMode::NoWrap && bounds.width > 0.0 && natural_width > bounds.width;
        // Clipped or faded: the painted line keeps the whole text.
        let cut = match self.overflow {
            Some(TextOverflow::Clip) if overflows => Some(None),
            Some(TextOverflow::Fade(length)) if overflows => Some(Some(length)),
            _ => None,
        };
        match wrap {
            // Shaped where measurement wrapped at the width layout
            // resolved, which the last measure query may not have been (it
            // can be an intrinsic-size probe).
            WrapMode::Auto => {
                if let Some(width) = engine.auto_wrap_width(state.id)
                    && let Some(unwrapped) = &layout
                {
                    layout = cx.layout_text_query(&unwrapped.query().wrap_width(Some(width)));
                    text_width = layout.as_ref().map_or(0.0, |l| l.size().0.ceil());
                    wrapped = true;
                }
            }
            WrapMode::NoWrap if self.overflow == Some(TextOverflow::Ellipsis) && overflows => {
                let style = self.query("", font_size, None).style;
                let (truncated, truncated_width) =
                    truncate_text_to_fit_styled(cx, &content, style, natural_width, bounds.width);
                layout = cx.layout_text_query(&self.query(&truncated, font_size, None));
                content = truncated;
                text_width = truncated_width;
            }
            WrapMode::NoWrap | WrapMode::Explicit(_) => {}
        }

        let x_offset = match self.align {
            _ if matches!(wrap, WrapMode::Explicit(_)) => 0.0,
            TextAlign::Left => 0.0,
            TextAlign::Center => ((bounds.width - text_width) * 0.5).max(0.0),
            TextAlign::Right => (bounds.width - text_width).max(0.0),
        };

        if let Some(length) = cut {
            if let Some(length) = length {
                scene.push_mask(
                    bounds,
                    AlphaMask::fade_edge(bounds, FadeEdge::Right, length),
                );
            }
            scene.clip(bounds);
            text_width = bounds.width;
        }
        if let Some(layout) = layout {
            let rect = Rect {
                x: bounds.x + x_offset,
                ..bounds
            };
            if self.paint.is_plain() {
                scene.text(TextPrimitive {
                    rect,
                    layout: ShapedText::new(layout.clone()),
                    color,
                });
            } else {
                let fill = self.paint.fill_now(color, cx);
                scene.styled_text(self.paint.primitive(
                    rect,
                    ShapedText::new(layout.clone()),
                    fill,
                ));
            }
            if self.underline || self.strikethrough {
                let decorations = self.decorations(layout.text().len(), color);
                paint_decorations(scene, &layout, (rect.x, rect.y), &decorations);
            }
        }
        if let Some(length) = cut {
            scene.pop_clip();
            if length.is_some() {
                scene.pop_isolate();
            }
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
            // The key is written into a reused buffer and the label shared
            // straight from the content: two allocations per text node
            // instead of a growing format string and a cloned label.
            thread_local! {
                static KEY: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
            }
            let key: std::sync::Arc<str> = KEY.with(|key| {
                use std::fmt::Write;
                let mut key = key.borrow_mut();
                key.clear();
                let _ = write!(
                    key,
                    "text:{:?}:{:.0}:{:.0}:{:.0}:{:.0}",
                    content, text_bounds.x, text_bounds.y, text_bounds.width, text_bounds.height
                );
                std::sync::Arc::from(key.as_str())
            });
            cx.push_accessibility(
                AccessibilityNode::shared(key, AccessibilityRole::Label, text_bounds)
                    .label(std::sync::Arc::<str>::from(content.as_str())),
            );
        }

        // Debug wireframe: red rect around text + log measurement vs bounds
        if cx.debug_wireframe {
            let measured =
                cx.measure_text_width(&content, font_size, self.font_kind, self.font_weight);
            let crushed = !wrapped && bounds.width > 0.0 && bounds.width < measured * 0.9;
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

#[cfg(test)]
mod tests {
    use super::*;

    const SENTENCE: &str = "the quick brown fox jumps over the lazy dog";
    /// 14pt text at the default 1.5 line height.
    const LINE: f32 = 21.0;
    /// 14pt selectable text.
    const SELECTABLE_LINE: f32 = 14.0 * 1.35;
    const SWATCH: Color = Color::rgba(10, 20, 30, 255);
    /// Text color whose solid quads are decorations.
    const INK: Color = Color::rgba(200, 100, 50, 255);

    /// A window's text state and element cache, kept across frames.
    struct Window {
        scale: f32,
        text: TextSystem,
        layouts: LayoutCache,
        signals: SignalStore,
        theme: Theme,
        cache: ElementCache,
        /// The frame clock elements read.
        clock_ms: u64,
    }

    /// What a frame painted: each text's (plain or rich) rect and lines,
    /// and where the
    /// `SWATCH` box landed.
    #[derive(Debug, PartialEq)]
    struct Frame {
        texts: Vec<(Rect, Vec<String>)>,
        swatch: Option<Rect>,
        /// Solid quads in the `INK` color: text decorations.
        decorations: Vec<Rect>,
        /// Family of each text's first glyph, in paint order.
        families: Vec<String>,
        /// Fill of each styled text, in paint order.
        fills: Vec<TextFill>,
        /// Masked groups and clips, in paint order.
        masks: Vec<(Rect, AlphaMask)>,
        clips: Vec<Rect>,
    }

    impl Frame {
        fn text(&self) -> &(Rect, Vec<String>) {
            assert_eq!(self.texts.len(), 1, "{self:?}");
            &self.texts[0]
        }
    }

    impl Window {
        fn new() -> Self {
            Self::at_scale(1.0)
        }

        fn at_scale(scale: f32) -> Self {
            Self {
                scale,
                text: TextSystem::vendored_only(&Default::default()),
                layouts: LayoutCache::default(),
                signals: SignalStore::new(),
                theme: Theme::default_dark(),
                cache: ElementCache::new(),
                clock_ms: 0,
            }
        }

        fn paint(&mut self, root: impl IntoAnyElement) -> Frame {
            self.layouts.begin_frame();
            let fonts = self.text.font_snapshot();
            let mut cx = ElementContext::new(
                &self.theme,
                self.scale,
                &mut self.text,
                &mut self.layouts,
                None,
                &self.signals,
            )
            .with_accessibility(false)
            .with_element_cache(&mut self.cache);
            cx.clock_ms = self.clock_ms;
            let mut scene = Scene::default();
            render_element(&mut root.into_any(), &mut scene, &mut cx, 400.0, 300.0);
            let mut frame = Frame {
                texts: Vec::new(),
                swatch: None,
                decorations: Vec::new(),
                families: Vec::new(),
                fills: Vec::new(),
                masks: Vec::new(),
                clips: Vec::new(),
            };
            for primitive in &scene.primitives {
                let (rect, shaped) = match primitive {
                    quark_render::Primitive::Rect(r) if r.color == INK => {
                        frame.decorations.push(r.rect);
                        continue;
                    }
                    quark_render::Primitive::TextRun(run) => (run.rect, &run.layout),
                    quark_render::Primitive::RichTextRun(run) => (run.rect, &run.layout),
                    quark_render::Primitive::IsolateStart(group) => {
                        frame
                            .masks
                            .extend(group.mask.map(|mask| (group.bounds, mask)));
                        continue;
                    }
                    quark_render::Primitive::ClipStart(clip) => {
                        frame.clips.push(clip.rect);
                        continue;
                    }
                    quark_render::Primitive::StyledText(run) => {
                        frame.fills.push(run.fill);
                        (run.rect, &run.layout)
                    }
                    quark_render::Primitive::RoundedRect(r) if r.color == SWATCH => {
                        frame.swatch = Some(r.rect);
                        continue;
                    }
                    _ => continue,
                };
                let layout = shaped.downcast_ref::<TextLayout>().expect("layout");
                let lines = layout
                    .lines()
                    .map(|line| layout.text()[line.byte_range].to_owned())
                    .collect();
                frame.texts.push((rect, lines));
                let family = layout
                    .glyph(0)
                    .and_then(|g| fonts.database().face(g.font_id))
                    .and_then(|face| face.families.first())
                    .map_or_else(String::new, |(name, _)| name.clone());
                frame.families.push(family);
            }
            frame
        }

        /// Width of `s` on one line, at the window's scale.
        fn width_of(&mut self, s: &str) -> f32 {
            let params =
                TextParams::new(s, TextStyle::new(14.0).line_height(LINE)).scale_factor(self.scale);
            self.layouts
                .layout(&mut self.text, &params)
                .expect("layout")
                .size()
                .0
        }
    }

    fn swatch() -> Div {
        div().w(10.0).h(10.0).bg(SWATCH)
    }

    fn sentence() -> TextElement {
        text(SENTENCE).size(14.0)
    }

    /// `content` above the swatch in a column `width` wide.
    fn column(width: f32, content: impl IntoAnyElement) -> Div {
        div().w(width).flex_col().child(content).child(swatch())
    }

    /// Lines that are not a whole number of words of `SENTENCE`, or that
    /// do not tile it.
    fn assert_word_lines(lines: &[String]) {
        assert_eq!(lines.concat(), SENTENCE, "{lines:?}");
        for line in lines {
            assert!(!line.starts_with(' '), "{lines:?}");
        }
    }

    // Catches text that ignores the width layout leaves it (the old fixed
    // natural width), measured and painted heights that disagree (the
    // swatch would overlap or float below the lines), and text that wraps
    // when it has room.
    #[test]
    fn text_wraps_to_the_width_layout_resolves() {
        type Build = fn() -> Div;
        let cases: &[(&str, Build, f32)] = &[
            (
                "row beside a fixed sibling",
                || {
                    column(
                        200.0,
                        div()
                            .flex_row()
                            .child(div().w(50.0).h(5.0).flex_shrink_0())
                            .child(sentence()),
                    )
                },
                150.0,
            ),
            (
                "row beside a wider sibling",
                || {
                    column(
                        200.0,
                        div()
                            .flex_row()
                            .child(div().w(100.0).h(5.0).flex_shrink_0())
                            .child(sentence()),
                    )
                },
                100.0,
            ),
            (
                "padded column",
                || column(160.0, div().flex_col().p(10.0).child(sentence())),
                140.0,
            ),
        ];
        for (name, build, width) in cases {
            let mut window = Window::new();
            let frame = window.paint(build());
            let (rect, lines) = frame.text();
            assert_eq!(rect.width, *width, "{name}");
            assert!(lines.len() > 1, "{name}: {lines:?}");
            assert_word_lines(lines);
            for line in lines {
                assert!(
                    window.width_of(line.trim_end()) <= *width,
                    "{name}: {line:?}"
                );
            }
            let swatch = frame.swatch.expect("swatch");
            let padding = if name.contains("padded") { 10.0 } else { 0.0 };
            assert_eq!(
                swatch.y,
                rect.y + lines.len() as f32 * LINE + padding,
                "{name}"
            );
        }
    }

    // Catches max-content probes that wrap: a row with room keeps the text
    // on one line at its natural width.
    #[test]
    fn text_with_room_keeps_one_line_at_its_natural_width() {
        let mut window = Window::new();
        let natural = window.width_of(SENTENCE).ceil();
        let frame = window.paint(div().w(400.0).flex_row().items_start().child(sentence()));
        let (rect, lines) = frame.text();
        assert_eq!((rect.width, lines.len()), (natural, 1));
    }

    // Catches the node keeping a size measured for another width: the same
    // tree in a narrower container lays out as a fresh window would.
    #[test]
    fn a_narrower_container_rewraps_its_text() {
        let mut window = Window::new();
        window.paint(column(300.0, sentence()));
        let narrow = window.paint(column(120.0, sentence()));
        assert_eq!(narrow, Window::new().paint(column(120.0, sentence())));
        assert!(narrow.text().1.len() > 2, "{narrow:?}");
    }

    // Catches the opt-outs following automatic wrapping: no-wrap and
    // truncated text stay on one line, and an explicit width holds even
    // past a narrower container.
    #[test]
    fn wrap_modes_override_the_container_width() {
        let mut window = Window::new();
        let natural = window.width_of(SENTENCE).ceil();
        let no_wrap = window.paint(column(100.0, sentence().no_wrap()));
        assert_eq!(
            (no_wrap.text().0.width, no_wrap.text().1.len()),
            (natural, 1)
        );

        let explicit = window.paint(column(100.0, sentence().wrap_width(150.0)));
        let (rect, lines) = explicit.text();
        assert_eq!(rect.width, 150.0);
        assert_word_lines(lines);
        assert_eq!(
            explicit.swatch.expect("swatch").y,
            lines.len() as f32 * LINE
        );

        let row = div().w(100.0).flex_row().child(sentence().truncate());
        let truncated = window.paint(row);
        let (rect, lines) = truncated.text();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].ends_with('\u{2026}'), "{lines:?}");
        assert!(window.width_of(&lines[0]) <= rect.width);
    }

    // Catches a min-content width below the widest word: in a flex row too
    // narrow for it, an unbroken word keeps its width on one line rather
    // than splitting between glyphs.
    #[test]
    fn an_unbroken_word_holds_its_width_in_a_narrow_row() {
        let word = "supercalifragilisticexpialidocious";
        let mut window = Window::new();
        let natural = window.width_of(word).ceil();
        let frame = window.paint(
            div()
                .w(60.0)
                .flex_row()
                .child(div().w(20.0).h(5.0))
                .child(text(word).size(14.0)),
        );
        let (rect, lines) = frame.text();
        assert_eq!((rect.width, lines.len()), (natural, 1));
    }

    // Catches paint rewrapping at a width other than the one measured: at
    // a fractional width taffy rounds the box after measuring it, and text
    // shaped at the rounded width painted one line in a box measured for
    // two. Plain and selectable text break the same lines, each fitting
    // the box.
    #[test]
    fn fractional_widths_paint_the_measured_lines() {
        let strings = ["hello world", "one two three", "alpha beta gamma", SENTENCE];
        for scale in [1.0, 1.5] {
            let mut window = Window::at_scale(scale);
            for s in strings {
                let natural = window.width_of(s);
                // Just under the rounded-up natural width, where the box
                // rounds to fit the text unwrapped but measurement wraps;
                // and a fractional width part way through.
                for width in [
                    natural.ceil() - 0.2,
                    natural.ceil() - 0.6,
                    natural * 0.6 + 0.3,
                ] {
                    let case = format!("{s:?} at {width} scale {scale}");
                    let plain = window.paint(column(width, text(s).size(14.0)));
                    let rich = window.paint(column(width, selectable_text(s).size(14.0)));
                    for (frame, line) in [(&plain, LINE), (&rich, SELECTABLE_LINE)] {
                        let (rect, lines) = frame.text();
                        assert_eq!(lines.concat(), s, "{case}");
                        assert_eq!(
                            rect.height,
                            (lines.len() as f32 * line).ceil(),
                            "{case}: {lines:?}"
                        );
                        for painted in lines {
                            assert!(
                                window.width_of(painted.trim_end()) <= rect.width,
                                "{case}: {painted:?}"
                            );
                        }
                    }
                    assert_eq!(plain.text().1, rich.text().1, "{case}");
                }
            }
        }
    }

    // Catches measurement and paint disagreeing at a zero-width box:
    // forced narrower than any word, text breaks between glyphs, and the
    // height layout gave it holds every painted line. Inside a cache
    // boundary the text is a block child, whose final measure query
    // offers the width without saying it is known.
    #[test]
    fn a_zero_width_box_holds_its_painted_lines() {
        type Build = fn() -> Div;
        let cases: &[(&str, Build)] = &[
            ("flex column", || column(0.0, text("ab cd").size(14.0))),
            ("cache boundary", || {
                let boundary = cached("text", 1, || text("ab cd").size(14.0)).w(0.0);
                div()
                    .flex_col()
                    .items_start()
                    .child(boundary)
                    .child(swatch())
            }),
        ];
        for (name, build) in cases {
            let frame = Window::new().paint(build());
            let (rect, lines) = frame.text();
            assert_eq!(lines.concat(), "ab cd", "{name}");
            assert!(lines.len() > 2, "{name}: {lines:?}");
            assert_eq!(
                frame.swatch.expect("swatch").y,
                rect.y + lines.len() as f32 * LINE,
                "{name}"
            );
        }
    }

    // Catches a point line height that paint and measurement disagree on:
    // wrapped 14-point lines at 22 points stack 22 apart, and the box
    // below starts after the last one.
    #[test]
    fn point_line_height_spaces_wrapped_lines() {
        let frame = Window::new().paint(column(120.0, sentence().line_height_points(22.0)));
        let (rect, lines) = frame.text();
        assert!(lines.len() > 1, "{lines:?}");
        assert_eq!(
            frame.swatch.expect("swatch").y,
            rect.y + lines.len() as f32 * 22.0
        );
    }

    // Catches tracking that paint applies but measurement ignores: 0.1 em
    // at 14 points adds 1.4 points after each of ten glyphs, and a
    // sentence that fits its column untracked wraps once tracked.
    #[test]
    fn letter_spacing_widens_and_rewraps_text() {
        let mut window = Window::new();
        let word = "abcdefghij";
        let plain = window.width_of(word);
        let tracked = window.paint(text(word).size(14.0).letter_spacing(0.1).no_wrap());
        assert_eq!(tracked.text().0.width, (plain + 14.0).ceil());

        let natural = window.width_of(SENTENCE).ceil();
        let untracked = window.paint(column(natural, sentence()));
        let wrapped = window.paint(column(natural, sentence().letter_spacing(0.1)));
        assert_eq!(untracked.text().1.len(), 1);
        assert!(wrapped.text().1.len() > 1, "{wrapped:?}");
        assert_word_lines(&wrapped.text().1);
    }

    // Catches an underline drawn once across the box instead of per line:
    // wrapped underlined text gets one quad under each painted line, below
    // its baseline and inside its line box.
    #[test]
    fn an_underline_follows_each_wrapped_line() {
        let frame = Window::new().paint(column(120.0, sentence().color(INK).underline()));
        let (rect, lines) = frame.text();
        assert!(lines.len() > 1, "{lines:?}");
        assert_eq!(frame.decorations.len(), lines.len(), "{frame:?}");
        for (i, quad) in frame.decorations.iter().enumerate() {
            let top = rect.y + i as f32 * LINE;
            // The baseline of 14-point text sits past the middle of its
            // 21-point line box.
            assert!(
                quad.y > top + LINE / 2.0 && quad.bottom() <= top + LINE,
                "line {i}: {quad:?}"
            );
            assert!(quad.width > 0.0 && quad.x >= rect.x, "line {i}: {quad:?}");
        }
    }

    // Catches a family that stops at the builder: a named family reaches
    // the painted glyphs of plain and selectable text alike.
    #[test]
    fn a_named_family_reaches_the_painted_glyphs() {
        let mut window = Window::new();
        let named = FontFamily::Named(window.text.family_id("JetBrains Mono"));
        let frame = window.paint(
            div()
                .flex_col()
                .child(text("Typography").size(14.0))
                .child(text("Typography").size(14.0).font_family(named))
                .child(selectable_text("Typography").size(14.0).font_family(named)),
        );
        assert_eq!(frame.families[1..], ["JetBrains Mono", "JetBrains Mono"]);
        assert_ne!(frame.families[0], "JetBrains Mono");
    }

    // Catches a shimmer frozen at its authored phase, or animating under
    // reduced motion: the painted phase follows the frame clock, and
    // reduced motion paints the static base color.
    #[test]
    fn a_shimmer_follows_the_clock_and_rests_under_reduced_motion() {
        use quark_render::scene::ShimmerSpec;
        let spec = ShimmerSpec::new(SWATCH, INK).duration_ms(1000);
        let shimmer = || text("Thinking").size(14.0).fill(TextFill::Shimmer(spec));
        let mut window = Window::new();
        window.clock_ms = 2250;
        let moving = window.paint(shimmer());
        window.theme.reduced_motion = true;
        let still = window.paint(shimmer());
        assert_eq!(
            (moving.fills, still.fills),
            (
                vec![TextFill::Shimmer(spec.phase(0.25))],
                vec![TextFill::Solid(SWATCH)]
            )
        );
    }

    // Catches tracked text truncated by its untracked width: the ellipsis
    // line, tracked as painted, still fits the box.
    #[test]
    fn truncation_measures_with_letter_spacing() {
        let mut window = Window::new();
        let row = div()
            .w(150.0)
            .flex_row()
            .child(sentence().letter_spacing(0.2).truncate());
        let frame = window.paint(row);
        let (rect, lines) = frame.text();
        assert!(lines[0].ends_with('\u{2026}'), "{lines:?}");
        let params = TextParams::new(
            lines[0].as_str(),
            TextStyle::new(14.0).line_height(LINE).letter_spacing(0.2),
        );
        let tracked = window
            .layouts
            .layout(&mut window.text, &params)
            .expect("layout");
        assert!(
            tracked.size().0 <= rect.width,
            "{} > {}",
            tracked.size().0,
            rect.width
        );
    }

    // Catches a fade that shortens the text or leaks past the box: the
    // whole sentence stays on its line, clipped to the box, under a mask
    // opaque at the start and clear at the trailing edge, half way 12
    // points into a 24-point fade.
    #[test]
    fn a_fading_overflow_keeps_the_text_and_fades_its_trailing_edge() {
        let frame = Window::new().paint(
            div()
                .w(100.0)
                .flex_row()
                .child(sentence().overflow(TextOverflow::Fade(24.0))),
        );
        let (rect, lines) = frame.text();
        let [(bounds, mask)] = frame.masks[..] else {
            panic!("{frame:?}");
        };
        let y = rect.y + 5.0;
        assert_eq!(
            (lines.as_slice(), bounds, frame.clips.as_slice()),
            (&[SENTENCE.to_owned()][..], *rect, &[*rect][..])
        );
        assert_eq!(
            [rect.x, rect.right() - 12.0, rect.right()].map(|x| mask.alpha_at([x, y])),
            [1.0, 0.5, 0.0]
        );
    }
}
