// Byte slicing of strings lives in `quark_text::offset`.
#![deny(clippy::string_slice, clippy::indexing_slicing)]

use super::*;
use quark_text::{TextOffset, ToTextOffset};

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
    pub underline: bool,
    pub strikethrough: bool,
    /// Link target. Adjacent spans with the same URL form one link: one
    /// click target, underlined together on hover. Without an explicit
    /// `color`, link text paints in the theme's accent text color.
    pub link: Option<Arc<str>>,
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
            underline: false,
            strikethrough: false,
            link: None,
        }
    }

    pub fn bold(mut self) -> Self {
        self.font_weight = FontWeight::Bold;
        self
    }

    pub fn italic(mut self) -> Self {
        self.italic = true;
        self
    }

    /// Monospaced, for inline code. Set a background with [`Self::pill`].
    pub fn code(mut self) -> Self {
        self.font_kind = FontKind::Mono;
        self
    }

    pub fn underline(mut self) -> Self {
        self.underline = true;
        self
    }

    pub fn strikethrough(mut self) -> Self {
        self.strikethrough = true;
        self
    }

    pub fn color(mut self, color: Color) -> Self {
        self.color = Some(color);
        self
    }

    pub fn pill(mut self, background: Color) -> Self {
        self.pill = Some(background);
        self
    }

    pub fn link(mut self, url: impl Into<Arc<str>>) -> Self {
        self.link = Some(url.into());
        self
    }
}

/// Emitted when a link inside selectable text is clicked and the element
/// has no [`SelectableText::on_link`] mapping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkClicked {
    pub url: Arc<str>,
}

impl From<LinkClicked> for Action {
    fn from(value: LinkClicked) -> Self {
        Action::new(value)
    }
}

/// Maps a clicked link's URL to the app action it emits.
type LinkFn = dyn Fn(&Arc<str>) -> Action;

#[derive(Clone)]
pub struct LinkHandler(Rc<LinkFn>);

impl LinkHandler {
    pub fn new(f: impl Fn(&Arc<str>) -> Action + 'static) -> Self {
        Self(Rc::new(f))
    }

    fn action(&self, url: &Arc<str>) -> Action {
        (self.0)(url)
    }
}

impl Default for LinkHandler {
    fn default() -> Self {
        Self::new(|url| LinkClicked { url: url.clone() }.into())
    }
}

/// One link of a text block: the byte range it covers, its URL, and the hit
/// entries registered for each line segment it occupies.
pub struct LinkHits {
    range: std::ops::Range<usize>,
    url: Arc<str>,
    rects: Vec<Rect>,
    hits: Vec<HitId>,
}

/// Groups adjacent spans that share a URL into links (byte range + URL).
pub(super) fn link_ranges(
    spans: &[StyledSpan],
    layout: &TextLayout,
) -> Vec<(std::ops::Range<usize>, Arc<str>)> {
    let mut out: Vec<(std::ops::Range<usize>, Arc<str>)> = Vec::new();
    for (span, text_span) in spans.iter().zip(layout.spans().iter()) {
        let Some(url) = &span.link else {
            continue;
        };
        let range = text_span.range.clone();
        match out.last_mut() {
            Some((last, last_url)) if last.end == range.start && last_url == url => {
                last.end = range.end;
            }
            _ => out.push((range, url.clone())),
        }
    }
    out
}

/// Registers one pointer hit per line segment of every link.
pub(super) fn register_link_hits(
    spans: &[StyledSpan],
    layout: &TextLayout,
    origin: (f32, f32),
    cx: &mut ElementContext,
) -> Vec<LinkHits> {
    link_ranges(spans, layout)
        .into_iter()
        .map(|(range, url)| {
            let rects: Vec<Rect> = layout
                .selection_rects(range.clone())
                .map(|r| r.offset(origin.0, origin.1))
                .collect();
            let hits = rects
                .iter()
                .map(|r| cx.insert_hit(*r, HitFlags::CLICK | HitFlags::HOVER, CursorHint::Pointer))
                .collect();
            LinkHits {
                range,
                url,
                rects,
                hits,
            }
        })
        .collect()
}

/// Decorations for spans that ask for them plus hovered links.
pub(super) fn span_decorations(
    spans: &[StyledSpan],
    layout: &TextLayout,
    colors: &[Color],
    links: &[LinkHits],
    cx: &ElementContext,
) -> Vec<TextDecoration> {
    let mut out = Vec::new();
    for ((span, text_span), color) in spans.iter().zip(layout.spans().iter()).zip(colors) {
        let range = text_span.range.clone();
        if span.underline {
            out.push(TextDecoration {
                range: range.clone(),
                kind: TextDecorationKind::Underline,
                color: *color,
            });
        }
        if span.strikethrough {
            out.push(TextDecoration {
                range,
                kind: TextDecorationKind::Strikethrough,
                color: *color,
            });
        }
    }
    for link in links {
        if !link.hits.iter().any(|hit| cx.is_hovered(*hit)) {
            continue;
        }
        let color = layout
            .spans()
            .iter()
            .position(|s| s.range.start == link.range.start)
            .and_then(|i| colors.get(i).copied())
            .unwrap_or(cx.theme.colors.text_accent);
        out.push(TextDecoration {
            range: link.range.clone(),
            kind: TextDecorationKind::Underline,
            color,
        });
    }
    out
}

/// Binds each link's hits to its own clickable semantic node and click
/// handler, and exposes it to assistive tech as a link.
pub(super) fn register_link_input(
    links: &[LinkHits],
    text: &str,
    handler: &LinkHandler,
    source_key: u64,
    cx: &mut ElementContext,
) {
    for link in links {
        let Some(first) = link.rects.first() else {
            continue;
        };
        let (x0, y0, x1, y1) = link.rects.iter().fold(
            (first.x, first.y, first.right(), first.bottom()),
            |(x0, y0, x1, y1), r| {
                (
                    x0.min(r.x),
                    y0.min(r.y),
                    x1.max(r.right()),
                    y1.max(r.bottom()),
                )
            },
        );
        let bounds = Rect {
            x: x0,
            y: y0,
            width: x1 - x0,
            height: y1 - y0,
        };
        let label = text.get(link.range.clone()).unwrap_or_default().to_owned();
        let action = handler.action(&link.url);
        let mut node = SemanticNode::new(bounds).label(label.clone());
        node.parent = cx.current_semantic_parent();
        node.actions = SemanticActions::default().clickable().hit_test();
        let index = cx.semantic.push(node);
        for hit in &link.hits {
            cx.bind_hit(*hit, index);
        }
        cx.handlers
            .on_click(index, ClickHandler::from_action(action.clone()));
        if cx.accessibility_enabled() && !cx.accessibility_text_hidden() {
            cx.push_accessibility(
                AccessibilityNode::new(
                    format!("link:{source_key}:{}:{}", link.range.start, link.url),
                    AccessibilityRole::Link,
                    bounds,
                )
                .label(label)
                .value(link.url.to_string())
                .action(AccessibilityAction::Click(action)),
            );
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
    /// The block's box in layout coordinates (before `transform`).
    pub bounds: Rect,
    /// The layout's top left in layout coordinates.
    pub text_origin: (f32, f32),
    pub text: Arc<str>,
    pub layout: Arc<TextLayout>,
    pub source_key: u64,
    /// Layout coordinates to window coordinates: the transforms of the
    /// block's ancestors. Always invertible; a block under a flattening
    /// transform registers no region.
    pub transform: Transform2D,
}

impl SelectableTextRegion {
    /// A window point in layout coordinates.
    pub fn to_layout(&self, x: f32, y: f32) -> (f32, f32) {
        match self.transform.invert() {
            Some(inverse) => inverse.apply(x, y),
            None => (x, y),
        }
    }

    /// Grapheme boundary nearest to a window point.
    pub fn hit(&self, x: f32, y: f32) -> TextOffset {
        let (x, y) = self.to_layout(x, y);
        self.layout
            .hit(x - self.text_origin.0, y - self.text_origin.1)
    }
}

/// Register `region` for pointer selection under the current transform. A
/// transform that flattens the block (a zero scale) leaves it unselectable,
/// as it is unclickable.
pub(super) fn register_selectable(cx: &mut ElementContext, mut region: SelectableTextRegion) {
    region.transform = cx.current_transform();
    if region.transform.invert().is_some() {
        cx.selectable_text_runs.push(region);
    }
}

/// Static text that wraps to its box (or an explicit width) and supports
/// mouse drag-selection + copy. Selection state lives in app state (keyed by byte offsets into the source
/// string, which survive re-wrap); the element renders the highlight from a
/// resolved `selection` range and registers a `SelectableTextRegion` for input.
pub struct SelectableText {
    /// Shared so a caller that keeps its spans (a document block) builds
    /// the element every frame without copying their text.
    spans: Arc<[StyledSpan]>,
    wrap: WrapMode,
    font_size: f32,
    /// Base font for text outside any span's overrides.
    font_kind: FontKind,
    font_weight: FontWeight,
    color: Option<Color>,
    max_lines: Option<usize>,
    source_key: u64,
    selection: Option<(usize, usize)>,
    on_link: LinkHandler,
}

pub fn selectable_text(text: impl Into<String>) -> SelectableText {
    selectable_rich_text(vec![StyledSpan::plain(text)])
}

/// Selectable text whose runs carry inline styles (code/bold/italic/link). The
/// concatenation of the span texts is the plain body; selection/copy/a11y all
/// operate on that string, so styling never changes what gets copied.
pub fn selectable_rich_text(spans: impl Into<Arc<[StyledSpan]>>) -> SelectableText {
    SelectableText {
        spans: spans.into(),
        wrap: WrapMode::Auto,
        font_size: 0.0,
        font_kind: FontKind::Ui,
        font_weight: FontWeight::Normal,
        color: None,
        max_lines: None,
        source_key: 0,
        selection: None,
        on_link: LinkHandler::default(),
    }
}

impl SelectableText {
    /// Wraps lines at `w` points; the element is `w` wide whatever its
    /// container offers. Without it, the text wraps to the width layout
    /// gives it, like [`text`].
    pub fn width(mut self, w: f32) -> Self {
        self.wrap = WrapMode::Explicit(w);
        self
    }

    /// Keep each line whole at its natural width instead of wrapping it to
    /// its box.
    pub fn no_wrap(mut self) -> Self {
        self.wrap = WrapMode::NoWrap;
        self
    }

    pub fn wrap(mut self, mode: WrapMode) -> Self {
        self.wrap = mode;
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
    /// Resolved (normalized) byte range to highlight, or `None` when this
    /// block is not the selected one. The bytes are snapped onto grapheme
    /// boundaries of the laid-out text when painted. Highlight is painted
    /// behind the text, so passing a selection never alters layout
    /// (measure == render).
    pub fn selection(mut self, selection: Option<(usize, usize)>) -> Self {
        self.selection = selection;
        self
    }

    /// Action a link click emits, given its URL. Defaults to [`LinkClicked`].
    pub fn on_link(mut self, f: impl Fn(&Arc<str>) -> Action + 'static) -> Self {
        self.on_link = LinkHandler::new(f);
        self
    }

    pub fn link_handler(mut self, handler: LinkHandler) -> Self {
        self.on_link = handler;
        self
    }

    /// Line height of selectable text at `font_size`.
    pub fn line_height_for(font_size: f32) -> f32 {
        font_size * 1.35
    }

    /// The text params `request_layout` shapes: `spans` over a base font
    /// of `kind` and `weight`, wrapped to `width`. Measuring with these
    /// through the frame's `LayoutCache` yields the layout the element
    /// paints.
    pub fn layout_params(
        spans: &[StyledSpan],
        font_size: f32,
        kind: FontKind,
        weight: FontWeight,
        width: f32,
    ) -> TextParams {
        Self::params(spans, font_size, kind, weight, Some(width))
    }

    fn params(
        spans: &[StyledSpan],
        font_size: f32,
        kind: FontKind,
        weight: FontWeight,
        width: Option<f32>,
    ) -> TextParams {
        let style = TextStyle::new(font_size)
            .kind(kind)
            .weight(weight)
            .line_height(Self::line_height_for(font_size));
        styled_params(spans, style, width.map(|w| w.max(1.0)))
    }

    /// Height the element lays out at for `layout` (from
    /// [`Self::layout_params`]), showing at most `max_lines`.
    pub fn measured_height(
        layout: Option<&TextLayout>,
        font_size: f32,
        max_lines: Option<usize>,
    ) -> f32 {
        let line_height = Self::line_height_for(font_size);
        match layout {
            Some(layout) => text_height(layout, max_lines, line_height),
            None => line_height.ceil(),
        }
    }
}

/// One layout for the concatenated span texts, each span's font applied to
/// its byte range. Span `i` of the layout is `spans[i]`, which is how paint
/// maps glyphs back to span colors.
pub(crate) fn styled_params(
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

/// Paints a normalized `selection` (start before end; anything else paints
/// nothing), snapped onto `layout`'s text.
pub(super) fn paint_selection<O: ToTextOffset>(
    scene: &mut Scene,
    layout: &TextLayout,
    selection: Option<(O, O)>,
    origin: (f32, f32),
    color: Color,
) {
    let text = layout.text();
    let Some((lo, hi)) = selection
        .map(|(a, b)| (a.to_offset(text), b.to_offset(text)))
        .filter(|(a, b)| a < b)
    else {
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

pub(super) fn span_colors(
    spans: &[StyledSpan],
    default_color: Color,
    link_color: Color,
) -> Arc<[Color]> {
    spans
        .iter()
        .map(|span| match (span.color, &span.link) {
            (Some(color), _) => color,
            (None, Some(_)) => link_color,
            (None, None) => default_color,
        })
        .collect()
}

impl Element for SelectableText {
    type LayoutState = Option<Arc<TextLayout>>;
    type PrepaintState = Vec<LinkHits>;

    fn request_layout(
        &mut self,
        engine: &mut LayoutEngine,
        cx: &mut ElementContext,
    ) -> (LayoutId, Self::LayoutState) {
        let explicit = match self.wrap {
            WrapMode::Explicit(width) => Some(width),
            WrapMode::Auto | WrapMode::NoWrap => None,
        };
        let params = Self::params(
            &self.spans,
            self.font_size,
            self.font_kind,
            self.font_weight,
            explicit,
        );
        // Unwrapped unless the width is explicit; automatic wrapping
        // reshapes at the resolved width in prepaint.
        let layout = cx.layout_text(&params);
        let id = match (self.wrap, &layout) {
            (WrapMode::Auto, Some(unwrapped)) => engine.request_text_layout(
                &taffy::Style::default(),
                TextMeasure::new(unwrapped.clone(), Self::line_height_for(self.font_size))
                    .max_lines(self.max_lines),
            ),
            _ => {
                let width =
                    explicit.unwrap_or_else(|| layout.as_ref().map_or(0.0, |l| l.size().0.ceil()));
                let height =
                    Self::measured_height(layout.as_deref(), self.font_size, self.max_lines);
                engine.request_layout(
                    taffy::Style {
                        size: taffy::Size {
                            width: taffy::Dimension::length(width),
                            height: taffy::Dimension::length(height),
                        },
                        flex_shrink: 0.0,
                        ..Default::default()
                    },
                    &[],
                )
            }
        };
        (id, layout)
    }

    fn prepaint(
        &mut self,
        bounds: Bounds,
        layout_state: &mut Self::LayoutState,
        _engine: &LayoutEngine,
        cx: &mut ElementContext,
    ) -> Vec<LinkHits> {
        // Shaped at the width layout resolved, which the last measure query
        // may not have been (it can be an intrinsic-size probe). Link hits,
        // `max_lines` clipping, paint, and the selectable region all use
        // this layout.
        let wrapped = match layout_state {
            Some(unwrapped) if self.wrap == WrapMode::Auto => {
                auto_wrap_width(bounds.width, unwrapped.size().0.ceil())
                    .and_then(|w| cx.layout_text_query(&unwrapped.query().wrap_width(Some(w))))
            }
            _ => None,
        };
        if wrapped.is_some() {
            *layout_state = wrapped;
        }
        match layout_state {
            Some(layout) => register_link_hits(&self.spans, layout, (bounds.x, bounds.y), cx),
            None => Vec::new(),
        }
    }

    fn paint(
        &mut self,
        bounds: Bounds,
        state: &mut Option<Arc<TextLayout>>,
        links: &mut Vec<LinkHits>,
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
        let colors = span_colors(&self.spans, default_color, cx.theme.colors.text_accent);
        let decorations = span_decorations(&self.spans, &layout, &colors, links, cx);
        scene.rich_text(RichTextPrimitive {
            rect: Rect {
                width: bounds.width + self.font_size * 0.5,
                ..bounds
            },
            layout: ShapedText::new(layout.clone()),
            default_color,
            span_colors: colors,
        });
        push_text_decorations(scene, &layout, origin, &decorations);

        if clipped {
            scene.pop_clip();
        }

        let text = layout.text().clone();
        if !text.is_empty()
            && cx.accessibility_enabled()
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
                .label(text.to_string())
                .read_only(true)
                .text(match self.selection {
                    Some((start, end)) => AccessibleText::new(text.clone()).selection(start, end),
                    None => AccessibleText::new(text.clone()),
                }),
            );
        }

        register_link_input(links, &text, &self.on_link, self.source_key, cx);

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

impl IntoAnyElement for SelectableText {
    fn into_any(self) -> AnyElement {
        element_into_any(self)
    }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing, clippy::string_slice)]
mod tests {
    use super::*;

    const SENTENCE: &str = "the quick brown fox jumps over the lazy dog";
    /// 14pt selectable text.
    const LINE: f32 = 14.0 * 1.35;
    const SWATCH: Color = Color::rgba(10, 20, 30, 255);

    /// A window's text state and element cache, kept across frames, and
    /// what the last frame registered for input.
    struct Window {
        text: TextSystem,
        layouts: LayoutCache,
        signals: SignalStore,
        theme: Theme,
        cache: ElementCache,
        regions: Vec<SelectableTextRegion>,
        router: InputRouter,
    }

    /// What a frame painted: each selectable region's box and lines, the
    /// text runs, selection highlights, clips, and where the `SWATCH` box
    /// landed.
    #[derive(Debug, PartialEq)]
    struct Frame {
        regions: Vec<(Rect, Vec<String>)>,
        texts: Vec<Rect>,
        highlights: Vec<Rect>,
        clips: Vec<Rect>,
        swatch: Option<Rect>,
    }

    impl Frame {
        fn region(&self) -> &(Rect, Vec<String>) {
            assert_eq!(self.regions.len(), 1, "{self:?}");
            &self.regions[0]
        }
    }

    fn lines(layout: &TextLayout) -> Vec<String> {
        layout
            .lines()
            .map(|line| layout.text()[line.byte_range].to_owned())
            .collect()
    }

    impl Window {
        fn new() -> Self {
            Self {
                text: TextSystem::vendored_only(&Default::default()),
                layouts: LayoutCache::default(),
                signals: SignalStore::new(),
                theme: Theme::default_dark(),
                cache: ElementCache::new(),
                regions: Vec::new(),
                router: InputRouter::default(),
            }
        }

        fn paint(&mut self, root: impl IntoAnyElement) -> Frame {
            self.layouts.begin_frame();
            let highlight = self.theme.colors.accent.with_alpha(Alpha::SOFT);
            let mut cx = ElementContext::new(
                &self.theme,
                1.0,
                &mut self.text,
                &mut self.layouts,
                None,
                &self.signals,
            )
            .with_accessibility(false)
            .with_element_cache(&mut self.cache);
            cx.semantic = SemanticFrame::new(400.0, 300.0);
            let mut scene = Scene::default();
            render_element(&mut root.into_any(), &mut scene, &mut cx, 400.0, 300.0);
            self.regions = std::mem::take(&mut cx.selectable_text_runs);
            self.router.set_frame(cx.take_input_frame());

            let mut frame = Frame {
                regions: self
                    .regions
                    .iter()
                    .map(|r| (r.bounds, lines(&r.layout)))
                    .collect(),
                texts: Vec::new(),
                highlights: Vec::new(),
                clips: Vec::new(),
                swatch: None,
            };
            for primitive in &scene.primitives {
                match primitive {
                    quark_render::Primitive::RichTextRun(run) => frame.texts.push(run.rect),
                    quark_render::Primitive::RoundedRect(r) if r.color == SWATCH => {
                        frame.swatch = Some(r.rect);
                    }
                    quark_render::Primitive::RoundedRect(r) if r.color == highlight => {
                        frame.highlights.push(r.rect);
                    }
                    quark_render::Primitive::ClipStart(clip) => frame.clips.push(clip.rect),
                    _ => {}
                }
            }
            frame
        }

        /// Width of `s` on one line.
        fn width_of(&mut self, s: &str) -> f32 {
            let params = TextParams::new(s, TextStyle::new(14.0).line_height(LINE));
            self.layouts
                .layout(&mut self.text, &params)
                .expect("layout")
                .size()
                .0
        }

        /// The URLs a click at `(x, y)` opens.
        fn click(&mut self, x: f32, y: f32) -> Vec<Arc<str>> {
            self.router
                .pointer_down(x, y, &mut None)
                .actions
                .iter()
                .filter_map(|a| a.downcast_ref::<LinkClicked>())
                .map(|link| link.url.clone())
                .collect()
        }
    }

    fn swatch() -> Div {
        div().w(10.0).h(10.0).bg(SWATCH)
    }

    fn sentence() -> SelectableText {
        selectable_text(SENTENCE).size(14.0)
    }

    /// `content` above the swatch in a column `width` wide.
    fn column(width: f32, content: impl IntoAnyElement) -> Div {
        div().w(width).flex_col().child(content).child(swatch())
    }

    /// Lines that are a whole number of words of `SENTENCE`, each no wider
    /// than `width`.
    fn assert_wrapped(window: &mut Window, lines: &[String], width: f32) {
        assert!(lines.len() > 1, "{lines:?}");
        assert_eq!(lines.concat(), SENTENCE, "{lines:?}");
        for line in lines {
            assert!(!line.starts_with(' '), "{lines:?}");
            assert!(window.width_of(line.trim_end()) <= width, "{line:?}");
        }
    }

    // Catches selectable text that ignores the width a flex row leaves it,
    // a measured height that disagrees with the painted lines (the swatch
    // would overlap or float below them), and a selection highlight or hit
    // mapped through a layout other than the painted one.
    #[test]
    fn selectable_text_wraps_to_a_constrained_flex_width() {
        let mut window = Window::new();
        let lazy = SENTENCE.find("lazy").expect("word");
        let row = div()
            .flex_row()
            .child(div().w(50.0).h(5.0).flex_shrink_0())
            .child(sentence().selection(Some((lazy, lazy + 4))));
        let frame = window.paint(column(200.0, row));

        let (bounds, lines) = frame.region().clone();
        assert_eq!(bounds.width, 150.0);
        assert_wrapped(&mut window, &lines, 150.0);
        assert_eq!(
            frame.swatch.expect("swatch").y,
            bounds.y + (lines.len() as f32 * LINE).ceil()
        );

        let [highlight] = frame.highlights[..] else {
            panic!("{frame:?}");
        };
        assert!(highlight.y > bounds.y, "not on a wrapped line: {frame:?}");
        assert!(highlight.right() <= bounds.right(), "{frame:?}");
        let region = &window.regions[0];
        let mid = highlight.y + highlight.height / 2.0;
        assert_eq!(region.hit(highlight.x + 0.5, mid), lazy);
        assert_eq!(region.hit(highlight.right() - 0.5, mid), lazy + 4);
    }

    // Catches link hits registered on the unwrapped layout: a link that
    // wraps onto a second line opens from every line it is painted on.
    #[test]
    fn a_wrapped_link_opens_from_each_painted_line() {
        let mut window = Window::new();
        let spans = vec![
            StyledSpan::plain("see "),
            StyledSpan::plain("the quick brown fox jumps").link("https://quark.dev"),
            StyledSpan::plain(" over the lazy dog"),
        ];
        window.paint(column(120.0, selectable_rich_text(spans).size(14.0)));

        let region = window.regions[0].clone();
        let start = "see ".len();
        let link = start..start + "the quick brown fox jumps".len();
        let rects: Vec<Rect> = region.layout.selection_rects(link).collect();
        assert!(rects.len() > 1, "link fits one line: {rects:?}");
        for rect in rects {
            let x = region.text_origin.0 + rect.x + rect.width / 2.0;
            let y = region.text_origin.1 + rect.y + rect.height / 2.0;
            assert!(x < region.bounds.right(), "{rect:?}");
            assert_eq!(
                window.click(x, y),
                [Arc::from("https://quark.dev")],
                "{rect:?}"
            );
        }
    }

    // Catches `max_lines` measured or clipped against the unwrapped
    // layout: automatically wrapped text is as tall as its first lines and
    // clips the rest at its box.
    #[test]
    fn max_lines_clips_automatically_wrapped_text() {
        let mut window = Window::new();
        let frame = window.paint(column(120.0, sentence().max_lines(2)));

        let (bounds, lines) = frame.region().clone();
        assert!(lines.len() > 2, "{lines:?}");
        assert_eq!(bounds.height, (2.0 * LINE).ceil());
        assert_eq!(frame.swatch.expect("swatch").y, bounds.bottom());
        assert_eq!(frame.clips, [bounds]);
    }

    // Catches the opt-outs following automatic wrapping: an explicit
    // width holds past a narrower container, and no-wrap keeps one line
    // at the natural width.
    #[test]
    fn wrap_modes_override_the_container_width() {
        let mut window = Window::new();
        let frame = window.paint(column(100.0, sentence().width(150.0)));
        let (bounds, lines) = frame.region().clone();
        assert_eq!(bounds.width, 150.0);
        assert_wrapped(&mut window, &lines, 150.0);
        assert_eq!(
            frame.swatch.expect("swatch").y,
            (lines.len() as f32 * LINE).ceil()
        );

        let natural = window.width_of(SENTENCE).ceil();
        let frame = window.paint(column(100.0, sentence().no_wrap()));
        let (bounds, lines) = frame.region();
        assert_eq!((bounds.width, lines.len()), (natural, 1));
    }

    // Catches a cache boundary replaying lines, link hits, or a selectable
    // region recorded at another width: after the container narrows, the
    // frame matches one a fresh window paints.
    #[test]
    fn cached_text_rewraps_like_a_fresh_build_after_a_width_change() {
        let block = || {
            cached("selectable", 1, || {
                let lazy = SENTENCE.find("lazy").expect("word");
                sentence().selection(Some((lazy, lazy + 4))).into_any()
            })
        };
        let mut window = Window::new();
        window.paint(column(300.0, block()));
        let narrow = window.paint(column(120.0, block()));
        assert_eq!(narrow, Window::new().paint(column(120.0, block())));
        assert!(narrow.region().1.len() > 2, "{narrow:?}");
    }
}
