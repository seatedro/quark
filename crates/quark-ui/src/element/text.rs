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
    /// Extra advance after every glyph, in ems.
    letter_spacing: f32,
    underline: bool,
    strikethrough: bool,
    align: TextAlign,
    truncate: bool,
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
        letter_spacing: 0.0,
        underline: false,
        strikethrough: false,
        align: TextAlign::Left,
        truncate: false,
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
    pub fn truncate(mut self) -> Self {
        self.truncate = true;
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
            WrapMode::Auto if self.truncate => WrapMode::NoWrap,
            mode => mode,
        }
    }

    fn query<'s>(&self, content: &'s str, font_size: f32, wrap: Option<f32>) -> TextQuery<'s> {
        let style = TextStyle::new(font_size)
            .kind(self.font_kind)
            .weight(self.font_weight)
            .letter_spacing(self.letter_spacing)
            .line_height(self.line_height.resolve(font_size));
        TextQuery::new(content, style).wrap_width(wrap)
    }

    /// The decorations to paint over `len` bytes of text in `color`.
    fn decorations(&self, len: usize, color: Color) -> impl Iterator<Item = TextDecoration> {
        [
            (self.underline, TextDecorationKind::Underline),
            (self.strikethrough, TextDecorationKind::Strikethrough),
        ]
        .into_iter()
        .filter(|(on, _)| *on)
        .map(move |(_, kind)| TextDecoration {
            range: 0..len,
            kind,
            color,
        })
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
                let shrink = if self.truncate { 1.0 } else { 0.0 };
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
            WrapMode::NoWrap
                if self.truncate && bounds.width > 0.0 && natural_width > bounds.width =>
            {
                let (truncated, truncated_width) = truncate_text_to_fit(
                    cx,
                    &content,
                    font_size,
                    self.font_kind,
                    self.font_weight,
                    natural_width,
                    bounds.width,
                );
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

        if let Some(layout) = layout {
            let decorated = self.underline || self.strikethrough;
            scene.text(TextPrimitive {
                rect: Rect {
                    x: bounds.x + x_offset,
                    ..bounds
                },
                layout: ShapedText::new(layout.clone()),
                color,
            });
            if decorated {
                let decorations: Vec<TextDecoration> =
                    self.decorations(layout.text().len(), color).collect();
                push_text_decorations(
                    scene,
                    &layout,
                    (bounds.x + x_offset, bounds.y),
                    &decorations,
                );
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

    /// A window's text state and element cache, kept across frames.
    struct Window {
        scale: f32,
        text: TextSystem,
        layouts: LayoutCache,
        signals: SignalStore,
        theme: Theme,
        cache: ElementCache,
    }

    /// What a frame painted: each text's (plain or rich) rect and lines,
    /// and where the
    /// `SWATCH` box landed.
    #[derive(Debug, PartialEq)]
    struct Frame {
        texts: Vec<(Rect, Vec<String>)>,
        swatch: Option<Rect>,
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
            }
        }

        fn paint(&mut self, root: impl IntoAnyElement) -> Frame {
            self.layouts.begin_frame();
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
            let mut scene = Scene::default();
            render_element(&mut root.into_any(), &mut scene, &mut cx, 400.0, 300.0);
            let mut frame = Frame {
                texts: Vec::new(),
                swatch: None,
            };
            for primitive in &scene.primitives {
                let (rect, shaped) = match primitive {
                    quark_render::Primitive::TextRun(run) => (run.rect, &run.layout),
                    quark_render::Primitive::RichTextRun(run) => (run.rect, &run.layout),
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
}
