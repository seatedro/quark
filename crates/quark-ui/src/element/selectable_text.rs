// Byte slicing of strings lives in `quark_text::offset`.
#![deny(clippy::string_slice, clippy::indexing_slicing)]

use std::cell::RefCell;

use super::*;
use quark_render::scene::{
    LineCap, Path, PathPrimitive, StrokePattern, StrokeStyle, StyledDecoration,
    StyledTextPrimitive, TextBackdrop, TextDecorationStyle, TextFill, TextRendering,
};
use quark_text::fonts::FontFamily;
use quark_text::{TextOffset, TextSource, ToTextOffset};

/// Paint-only text options: glyph fill, coverage policy, and backdrop.
/// None of them affects shaping, so changing one reuses the layout.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct TextPaint {
    pub(crate) fill: Option<TextFill>,
    pub(crate) rendering: Option<TextRendering>,
    pub(crate) backdrop: TextBackdrop,
}

impl TextPaint {
    /// Whether the text paints as a plain text primitive.
    pub(crate) fn is_plain(&self) -> bool {
        self.fill.is_none() && self.rendering.is_none() && self.backdrop == TextBackdrop::Unknown
    }

    /// The fill to paint this frame, `color` without one. A shimmer takes
    /// its phase from the frame clock and asks for the next frame; under
    /// reduced motion it paints its static base color.
    pub(crate) fn fill_now(&self, color: Color, cx: &mut ElementContext) -> TextFill {
        match self.fill {
            None => TextFill::Solid(color),
            Some(TextFill::Shimmer(spec)) if cx.theme.reduced_motion => TextFill::Solid(spec.base),
            Some(TextFill::Shimmer(spec)) => {
                cx.request_frame_at_ms(cx.clock_ms + SHIMMER_FRAME_MS);
                TextFill::Shimmer(spec.phase(spec.phase_at(0, cx.clock_ms)))
            }
            Some(fill) => fill,
        }
    }

    /// A styled text primitive for `layout` at `rect` in this paint.
    pub(crate) fn primitive(
        &self,
        rect: Rect,
        layout: ShapedText,
        fill: TextFill,
    ) -> StyledTextPrimitive {
        StyledTextPrimitive::new(rect, layout, fill)
            .rendering(self.rendering.unwrap_or_default())
            .backdrop(self.backdrop)
    }
}

/// Frame interval a shimmering text asks for while it is painted.
const SHIMMER_FRAME_MS: u64 = 16;

/// Paints `decorations` over `layout` at `origin`, after the text. Solid
/// ones with default metrics are quads, as [`push_text_decorations`]
/// draws them; the rest are stroked paths, one per line segment, so a
/// pattern restarts at each wrapped line and runs unbroken along a range
/// on one line. Thickness defaults to 7% of the font size (at least one
/// point) and the line sits just below the baseline (underline) or
/// through the x-height (strikethrough), unless the style sets them.
pub(crate) fn paint_decorations(
    scene: &mut Scene,
    layout: &TextLayout,
    origin: (f32, f32),
    decorations: &[StyledDecoration],
) {
    let size = layout.style().font_size;
    let default_thickness = (size * 0.07).max(1.0);
    let text = layout.text();
    for decoration in decorations {
        let style = decoration.style;
        let plain = style.pattern == StrokePattern::Solid
            && style.thickness.is_none()
            && style.offset.is_none();
        if plain {
            push_text_decorations(
                scene,
                layout,
                origin,
                &[TextDecoration {
                    range: decoration.range.clone(),
                    kind: decoration.kind,
                    color: style.color,
                }],
            );
            continue;
        }
        let thickness = style
            .thickness
            .filter(|t| t.is_finite() && *t > 0.0)
            .unwrap_or(default_thickness);
        let center = style
            .offset
            .filter(|o| o.is_finite())
            .unwrap_or(match decoration.kind {
                TextDecorationKind::Underline => size * 0.12 + default_thickness * 0.5,
                TextDecorationKind::Strikethrough => -size * 0.28,
            });
        let (a, b) = (decoration.range.start, decoration.range.end.min(text.len()));
        for line in layout.lines() {
            let start = a.max(line.byte_range.start);
            let mut end = b.min(line.byte_range.end);
            // A wrapped line's trailing space is not underlined.
            if let Some(slice) = text.get(start..end) {
                end = start + slice.trim_end().len();
            }
            if start >= end {
                continue;
            }
            let y = line.baseline + center;
            for r in layout.selection_rects(start..end) {
                // Rects of a neighbouring line can come back when the range
                // touches a line break.
                if (r.y - line.top).abs() > 0.01 || r.width <= 0.0 {
                    continue;
                }
                let mut path = Path::builder();
                path.move_to(r.x, y).line_to(r.x + r.width, y);
                let mut stroke = StrokeStyle::new(thickness);
                stroke.pattern = style.pattern;
                stroke.cap = LineCap::Butt;
                scene.path(
                    PathPrimitive::new(Arc::new(path.build()), [origin.0, origin.1])
                        .stroke(style.color, stroke),
                );
            }
        }
    }
}

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
    /// Font size relative to the text's: 0.86 sets 12 point inline code in
    /// 14 point prose. `None` is the text's size.
    pub font_scale: Option<f32>,
    /// `None` paints in the block's default color.
    pub color: Option<Color>,
    /// `Some(bg)` paints a rounded background pill behind the run (inline
    /// code). The pill hugs the run's glyphs on the line's baseline, and the
    /// layout keeps room for its padding on both sides, so the words around
    /// it keep their distance.
    pub pill: Option<Color>,
    pub underline: bool,
    /// How the underline looks (dotted, dashed, thickness, offset, color);
    /// `None` draws a solid line in the span's color. Setting it underlines
    /// the span.
    pub underline_style: Option<TextDecorationStyle>,
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
            font_scale: None,
            color: None,
            pill: None,
            underline: false,
            underline_style: None,
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

    /// Underlines the span in `style`: a dotted or dashed pattern,
    /// thickness, offset, and color. Adjacent spans with the same style
    /// share one line, so a link split across spans underlines unbroken.
    pub fn underline_style(mut self, style: TextDecorationStyle) -> Self {
        self.underline = true;
        self.underline_style = Some(style);
        self
    }

    pub fn strikethrough(mut self) -> Self {
        self.strikethrough = true;
        self
    }

    /// Weight of the span's glyphs.
    pub fn weight(mut self, weight: FontWeight) -> Self {
        self.font_weight = weight;
        self
    }

    pub fn color(mut self, color: Color) -> Self {
        self.color = Some(color);
        self
    }

    /// Size relative to the text's; see [`Self::font_scale`](field@Self::font_scale).
    pub fn font_scale(mut self, scale: f32) -> Self {
        self.font_scale = Some(scale);
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

/// Emitted while a drag changes the selection of a [`RichTextState`]:
/// the state already holds the new selection, so apps only use it to clear
/// other selections or to repaint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextSelectionChanged {
    /// The [`SelectableText::source`] key of the text.
    pub source: u64,
}

impl From<TextSelectionChanged> for Action {
    fn from(value: TextSelectionChanged) -> Self {
        Action::new(value)
    }
}

/// The selection of one standalone paragraph, kept across frames by the
/// app and shared with its element through [`SelectableText::state`]:
/// pressing and dragging over the painted text selects it, and the app
/// reads the selection or copies it from here. Offsets are bytes of the
/// concatenated span texts, so a selection survives rewrapping and
/// restyling. Clones share one selection; keep one per paragraph, keyed by
/// the same identity as its [`SelectableText::source`].
#[derive(Clone, Default)]
pub struct RichTextState(Rc<RefCell<RichTextSelection>>);

#[derive(Default)]
struct RichTextSelection {
    /// Where the selection started and where it ends now.
    anchor: usize,
    focus: usize,
    /// The text last painted with this state, which offsets index.
    text: Option<TextSource>,
}

impl RichTextState {
    pub fn new() -> Self {
        Self::default()
    }

    /// The selected byte range, start before end, or `None` when nothing
    /// is selected.
    pub fn selection(&self) -> Option<(usize, usize)> {
        let s = self.0.borrow();
        let (lo, hi) = (s.anchor.min(s.focus), s.anchor.max(s.focus));
        (lo < hi).then_some((lo, hi))
    }

    /// Selects bytes `start..end` (in either order), or clears the
    /// selection.
    pub fn set_selection(&self, selection: Option<(usize, usize)>) {
        let mut s = self.0.borrow_mut();
        (s.anchor, s.focus) = selection.unwrap_or((0, 0));
    }

    /// Selects all of the text last painted with this state.
    pub fn select_all(&self) {
        let mut s = self.0.borrow_mut();
        s.anchor = 0;
        s.focus = s.text.as_ref().map_or(0, |t| t.len());
    }

    /// The selected text, as copy puts it on the clipboard: the bytes of
    /// the painted text, snapped onto character boundaries. Empty when
    /// nothing is selected or nothing was painted.
    pub fn selected_text(&self) -> String {
        let Some((lo, hi)) = self.selection() else {
            return String::new();
        };
        let s = self.0.borrow();
        let Some(text) = &s.text else {
            return String::new();
        };
        let text = text.as_str();
        let (lo, hi) = (text.floor_char_boundary(lo), text.floor_char_boundary(hi));
        text.get(lo..hi).unwrap_or_default().to_owned()
    }

    fn set_text(&self, text: &TextSource) {
        self.0.borrow_mut().text = Some(text.clone());
    }

    fn press(&self, at: usize) {
        let mut s = self.0.borrow_mut();
        (s.anchor, s.focus) = (at, at);
    }

    fn extend(&self, to: usize) {
        self.0.borrow_mut().focus = to;
    }
}

impl std::fmt::Debug for RichTextState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = self.0.borrow();
        f.debug_struct("RichTextState")
            .field("anchor", &s.anchor)
            .field("focus", &s.focus)
            .finish()
    }
}

/// A press and drag over a paragraph with a [`RichTextState`]: the press
/// collapses the selection at the grapheme boundary under the pointer and
/// each move extends it, through the layout that was painted.
struct RichTextDrag {
    state: RichTextState,
    region: SelectableTextRegion,
    press: ClickEvent,
}

impl RichTextDrag {
    fn changed(&self) -> Vec<Action> {
        vec![
            TextSelectionChanged {
                source: self.region.source_key,
            }
            .into(),
        ]
    }
}

impl DragHandler for RichTextDrag {
    fn on_press(&mut self) -> Vec<Action> {
        let at = self.region.hit(self.press.x, self.press.y).get();
        self.state.press(at);
        self.changed()
    }

    fn on_move(&mut self, x: f32, y: f32) -> Vec<Action> {
        self.state.extend(self.region.hit(x, y).get());
        self.changed()
    }

    fn on_release(&mut self) -> DragReleaseResult {
        DragReleaseResult {
            actions: Vec::new(),
        }
    }

    /// The selection made so far stays.
    fn on_cancel(&mut self) -> Vec<Action> {
        Vec::new()
    }

    fn cursor(&self) -> CursorHint {
        CursorHint::Text
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

/// Decorations for spans that ask for them plus hovered links. Adjacent
/// spans with the same decoration style share one decoration; a hovered
/// link already underlined by its spans gets no second line.
pub(super) fn span_decorations(
    spans: &[StyledSpan],
    layout: &TextLayout,
    colors: &[Color],
    links: &[LinkHits],
    cx: &ElementContext,
) -> Vec<StyledDecoration> {
    let mut out: Vec<StyledDecoration> = Vec::new();
    let mut push = |range: std::ops::Range<usize>, kind, style| match out.last_mut() {
        Some(last) if last.kind == kind && last.style == style && last.range.end == range.start => {
            last.range.end = range.end;
        }
        _ => out.push(StyledDecoration { range, kind, style }),
    };
    for ((span, text_span), color) in spans.iter().zip(layout.spans().iter()).zip(colors) {
        let range = text_span.range.clone();
        if span.underline {
            let style = span
                .underline_style
                .unwrap_or_else(|| TextDecorationStyle::solid(*color));
            push(range.clone(), TextDecorationKind::Underline, style);
        }
        if span.strikethrough {
            push(
                range,
                TextDecorationKind::Strikethrough,
                TextDecorationStyle::solid(*color),
            );
        }
    }
    for link in links {
        if !link.hits.iter().any(|hit| cx.is_hovered(*hit)) {
            continue;
        }
        let underlined = out.iter().any(|d| {
            d.kind == TextDecorationKind::Underline
                && d.range.start <= link.range.start
                && d.range.end >= link.range.end
        });
        if underlined {
            continue;
        }
        let color = layout
            .spans()
            .iter()
            .position(|s| s.range.start == link.range.start)
            .and_then(|i| colors.get(i).copied())
            .unwrap_or(cx.theme.colors.text_accent);
        out.push(StyledDecoration {
            range: link.range.clone(),
            kind: TextDecorationKind::Underline,
            style: TextDecorationStyle::solid(color),
        });
    }
    out
}

/// Binds each link's hits to its own clickable semantic node and click
/// handler, and exposes it to assistive tech as a link. Each link is a Tab
/// stop, ringed while focused, and Enter or Space opens it as a click does.
pub(super) fn register_link_input(
    links: &[LinkHits],
    text: &str,
    handler: &LinkHandler,
    source_key: u64,
    scene: &mut Scene,
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
        let key = format!("link:{source_key}:{}:{}", link.range.start, link.url);
        let focus = FocusId::from_key(&key);
        if cx.is_focused(focus) {
            for rect in &link.rects {
                paint_focus_ring(scene, cx, *rect, [2.0; 4], 0.0);
            }
        }
        let label = text.get(link.range.clone()).unwrap_or_default().to_owned();
        let action = handler.action(&link.url);
        let mut node = SemanticNode::new(bounds).label(label.clone());
        node.parent = cx.current_semantic_parent();
        node.role = Some(SemanticRole::Link);
        node.actions = SemanticActions::default().clickable().hit_test();
        node.focus = Some(focus);
        node.tab_stop = Some(TabStop::new(0));
        let index = cx.semantic.push(node);
        for hit in &link.hits {
            cx.bind_hit(*hit, index);
        }
        cx.handlers
            .on_click(index, ClickHandler::from_action(action.clone()));
        if cx.accessibility_enabled() && !cx.accessibility_text_hidden() {
            cx.push_accessibility_for_semantic(
                AccessibilityNode::new(key, AccessibilityRole::Link, bounds)
                    .label(label)
                    .value(link.url.to_string())
                    .focus(focus)
                    .action(AccessibilityAction::Click(action)),
                index,
            );
        }
    }
}

/// Per-frame record of a painted selectable-text block, mirroring
/// `TextInputHitArea`. Carries the layout that was painted, so pointer
/// hit-testing maps a click onto exactly the glyphs on screen (bold, italic,
/// and code runs included) and on to a byte offset into `text`, the
/// layout's own source: what was painted, whatever the text is now. `source_key`
/// identifies which logical text this is, so a selection survives re-wrap and
/// only highlights its own block.
#[derive(Debug, Clone)]
pub struct SelectableTextRegion {
    /// The block's box in layout coordinates (before `transform`).
    pub bounds: Rect,
    /// The layout's top left in layout coordinates.
    pub text_origin: (f32, f32),
    pub text: TextSource,
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

/// What a paragraph's line boxes and shaping depend on: its base font,
/// line height, and tracking. Spans override the font per run; everything
/// else applies to the whole paragraph. Selectable text, rich text, and
/// document blocks all lay out through one, so measuring with
/// [`SelectableText::paragraph_params`] and [`SelectableText::paragraph_height`]
/// yields exactly the layout and height the element paints.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct ParagraphStyle {
    /// Logical points.
    pub font_size: f32,
    pub line_height: LineHeight,
    /// Base font for text outside any span's overrides.
    pub font_kind: FontKind,
    pub font_weight: FontWeight,
    /// Extra advance after every glyph, in ems.
    pub letter_spacing: f32,
    /// A family in place of `font_kind`'s generic one; spans that set
    /// their own kind still use that kind's family.
    pub family: Option<FontFamily>,
}

impl ParagraphStyle {
    /// UI text of `font_size` points at [`LineHeight::PARAGRAPH`].
    pub fn new(font_size: f32) -> Self {
        Self {
            font_size,
            line_height: LineHeight::PARAGRAPH,
            font_kind: FontKind::Ui,
            font_weight: FontWeight::Normal,
            letter_spacing: 0.0,
            family: None,
        }
    }

    pub fn font_family(mut self, family: FontFamily) -> Self {
        self.family = Some(family);
        self
    }

    /// Invalid line heights are ignored.
    pub fn line_height(mut self, line_height: LineHeight) -> Self {
        if line_height.is_valid() {
            self.line_height = line_height;
        }
        self
    }

    pub fn kind(mut self, kind: FontKind) -> Self {
        self.font_kind = kind;
        self
    }

    pub fn weight(mut self, weight: FontWeight) -> Self {
        self.font_weight = weight;
        self
    }

    /// Nonfinite values are ignored.
    pub fn letter_spacing(mut self, ems: f32) -> Self {
        if ems.is_finite() {
            self.letter_spacing = ems;
        }
        self
    }

    /// Height of one line box in logical points.
    pub fn line_height_points(&self) -> f32 {
        self.line_height
            .valid_or(LineHeight::PARAGRAPH)
            .resolve(self.font_size)
    }

    /// The text style the paragraph shapes with.
    pub fn text_style(&self) -> TextStyle {
        let style = TextStyle::new(self.font_size)
            .kind(self.font_kind)
            .weight(self.font_weight)
            .letter_spacing(self.letter_spacing)
            .line_height(self.line_height_points());
        match self.family {
            Some(family) => style.font_family(family),
            None => style,
        }
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
    /// Font size zero takes the theme's UI font size.
    paragraph: ParagraphStyle,
    color: Option<Color>,
    max_lines: Option<usize>,
    source_key: u64,
    selection: Option<(usize, usize)>,
    state: Option<RichTextState>,
    paint: TextPaint,
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
        paragraph: ParagraphStyle::new(0.0),
        color: None,
        max_lines: None,
        source_key: 0,
        selection: None,
        state: None,
        paint: TextPaint::default(),
        on_link: LinkHandler::default(),
    }
}

/// A styled paragraph: one text flow of [`StyledSpan`]s that wraps as a
/// whole, with links, inline code pills, selection highlight, copy, and
/// accessible text over the concatenated span texts. The same element as
/// [`selectable_rich_text`]; build the spans once and share them across
/// frames.
pub fn rich_text(spans: impl Into<Arc<[StyledSpan]>>) -> SelectableText {
    selectable_rich_text(spans)
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
        self.paragraph.font_size = s;
        self
    }
    pub fn color(mut self, c: Color) -> Self {
        self.color = Some(c);
        self
    }
    pub fn weight(mut self, w: FontWeight) -> Self {
        self.paragraph.font_weight = w;
        self
    }
    /// Base font kind of text outside spans that set their own.
    pub fn font_kind(mut self, kind: FontKind) -> Self {
        self.paragraph.font_kind = kind;
        self
    }
    /// Draws in `family` (system UI, system monospace, or a named family)
    /// in place of the base font kind's generic one.
    pub fn font_family(mut self, family: FontFamily) -> Self {
        self.paragraph = self.paragraph.font_family(family);
        self
    }
    /// Line height of every line ([`LineHeight::PARAGRAPH`] by default);
    /// invalid values are ignored.
    pub fn line_height(mut self, line_height: LineHeight) -> Self {
        self.paragraph = self.paragraph.line_height(line_height);
        self
    }
    /// Line height in logical points, whatever the font size.
    pub fn line_height_points(self, points: f32) -> Self {
        self.line_height(LineHeight::Points(points))
    }
    /// Extra advance after every glyph, in ems (zero by default).
    pub fn letter_spacing(mut self, ems: f32) -> Self {
        self.paragraph = self.paragraph.letter_spacing(ems);
        self
    }
    /// Fills every glyph with `fill` (a gradient or a shimmer) in place of
    /// the text and span colors. A shimmer sweeps with the frame clock and
    /// stands still under reduced motion. Never reshapes the text.
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

    /// Keeps the selection in `state`: pressing and dragging over the text
    /// selects it, emitting [`TextSelectionChanged`], and the highlight
    /// paints from the state. An explicit [`Self::selection`] wins over it.
    /// Links stay clickable.
    pub fn state(mut self, state: &RichTextState) -> Self {
        self.state = Some(state.clone());
        self
    }

    /// The highlighted range: the explicit one, else the state's.
    fn effective_selection(&self) -> Option<(usize, usize)> {
        self.selection
            .or_else(|| self.state.as_ref().and_then(RichTextState::selection))
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

    /// Line height of selectable text at `font_size` with the default
    /// [`LineHeight::PARAGRAPH`].
    pub fn line_height_for(font_size: f32) -> f32 {
        LineHeight::PARAGRAPH.resolve(font_size)
    }

    /// The text params `request_layout` shapes: `spans` over a base font
    /// of `kind` and `weight` at the default line height, wrapped to
    /// `width`. See [`Self::paragraph_params`].
    pub fn layout_params(
        spans: &[StyledSpan],
        font_size: f32,
        kind: FontKind,
        weight: FontWeight,
        width: f32,
    ) -> TextParams {
        let paragraph = ParagraphStyle::new(font_size).kind(kind).weight(weight);
        Self::paragraph_params(spans, &paragraph, width)
    }

    /// The text params `request_layout` shapes for `spans` in `paragraph`,
    /// wrapped to `width`. Measuring with these through the frame's
    /// `LayoutCache` yields the layout the element paints.
    pub fn paragraph_params(
        spans: &[StyledSpan],
        paragraph: &ParagraphStyle,
        width: f32,
    ) -> TextParams {
        styled_params(spans, paragraph.text_style(), Some(width.max(1.0)))
    }

    /// Height the element lays out at for `layout` (from
    /// [`Self::layout_params`]), showing at most `max_lines`, at the
    /// default line height.
    pub fn measured_height(
        layout: Option<&TextLayout>,
        font_size: f32,
        max_lines: Option<usize>,
    ) -> f32 {
        Self::paragraph_height(layout, &ParagraphStyle::new(font_size), max_lines)
    }

    /// Height the element lays out at for `layout` (from
    /// [`Self::paragraph_params`]), showing at most `max_lines`.
    pub fn paragraph_height(
        layout: Option<&TextLayout>,
        paragraph: &ParagraphStyle,
        max_lines: Option<usize>,
    ) -> f32 {
        let line_height = paragraph.line_height_points();
        match layout {
            Some(layout) => text_height(layout, max_lines, line_height),
            None => line_height.ceil(),
        }
    }
}

/// [`styled_params`] borrowed: the text and spans are joined into storage
/// this thread keeps, and `f` gets the query over them, so laying out a
/// block the layout cache already holds allocates nothing.
pub(crate) fn with_styled_query<R>(
    spans: &[StyledSpan],
    style: TextStyle,
    wrap_width: Option<f32>,
    f: impl FnOnce(&TextQuery) -> R,
) -> R {
    thread_local! {
        static JOINED: RefCell<(String, Vec<TextSpan>)> = const {
            RefCell::new((String::new(), Vec::new()))
        };
    }
    JOINED.with(|joined| {
        let (text, text_spans) = &mut *joined.borrow_mut();
        text.clear();
        text_spans.clear();
        join_spans(spans, &style, text, text_spans);
        f(&TextQuery {
            text,
            spans: text_spans,
            ..TextQuery::new("", style).wrap_width(wrap_width)
        })
    })
}

/// The text span of `span` over `range` in a paragraph of `base`. A font
/// the span shares with the paragraph is left to the paragraph, so a
/// paragraph's named family reaches its plain spans while a code span
/// keeps the monospace family.
fn styled_span(span: &StyledSpan, range: std::ops::Range<usize>, base: &TextStyle) -> TextSpan {
    TextSpan {
        range,
        weight: (span.font_weight != base.font_weight).then_some(span.font_weight),
        style: span.italic.then_some(FontStyle::Italic),
        kind: (span.font_kind != base.font_kind).then_some(span.font_kind),
        size: span.font_scale.map(|scale| base.font_size * scale),
        letter_spacing: None,
    }
}

/// Room a pill keeps on each side of its run, in ems of its size: the
/// padding inside the pill and the gap outside it.
const PILL_PAD: f32 = 0.4;
const PILL_GAP: f32 = 0.1;
/// How far the pill reaches above and below the baseline, and its corner
/// radius, in ems of its size.
const PILL_ASCENT: f32 = 1.0;
const PILL_DESCENT: f32 = 0.38;
const PILL_RADIUS: f32 = 0.35;

/// The extra advance that makes room for pills: one entry per character
/// that gets some (a pill's last character, and the character before a
/// pill on its line), as its byte range, the span that styles it, and
/// the extra ems of that span's size. Empty without pills.
fn pill_room(spans: &[StyledSpan]) -> Vec<(std::ops::Range<usize>, &StyledSpan, f32)> {
    let mut out: Vec<(std::ops::Range<usize>, &StyledSpan, f32)> = Vec::new();
    if spans.iter().all(|s| s.pill.is_none()) {
        return out;
    }
    let scale = |span: &StyledSpan| span.font_scale.unwrap_or(1.0);
    let mut add = |range: std::ops::Range<usize>, owner, ems: f32| match out
        .iter_mut()
        .find(|(r, ..)| *r == range)
    {
        Some(entry) => entry.2 += ems,
        None => out.push((range, owner, ems)),
    };
    // The span holding the last character so far, and where it ends.
    let mut last: Option<(usize, char, &StyledSpan)> = None;
    let mut start = 0;
    for span in spans {
        let end = start + span.text.len();
        if span.pill.is_some()
            && let Some(c) = span.text.chars().next_back()
        {
            if let Some((before, prev, owner)) = last.filter(|&(_, c, _)| c != '\n') {
                let ems = (PILL_PAD + PILL_GAP) * scale(span) / scale(owner);
                add(before - prev.len_utf8()..before, owner, ems);
            }
            add(end - c.len_utf8()..end, span, PILL_PAD + PILL_GAP);
        }
        if let Some(c) = span.text.chars().next_back() {
            last = Some((end, c, span));
        }
        start = end;
    }
    out
}

/// Joins `spans` into `text` and their text spans into `out`: one per
/// span, in order (so span `i` of a layout is `spans[i]`), then one per
/// character that makes room for a pill, which [`span_colors`] colors
/// after the span that styles it.
fn join_spans(spans: &[StyledSpan], style: &TextStyle, text: &mut String, out: &mut Vec<TextSpan>) {
    for span in spans {
        let start = text.len();
        text.push_str(&span.text);
        out.push(styled_span(span, start..text.len(), style));
    }
    for (range, owner, ems) in pill_room(spans) {
        // The style's own spacing, in ems of this span's size.
        let own = style.letter_spacing / owner.font_scale.unwrap_or(1.0);
        out.push(TextSpan {
            letter_spacing: Some(own + ems),
            ..styled_span(owner, range, style)
        });
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
    join_spans(spans, &style, &mut text, &mut text_spans);
    TextParams::new(text, style)
        .spans(text_spans)
        .wrap_width(wrap_width)
}

/// Paints inline-code pills behind each span that has one, on every line
/// it covers: from the room [`pill_room`] keeps before the run to the room
/// after it, and from just above the run's glyphs to just below its
/// baseline.
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
        let size = text_span.size.unwrap_or(layout.style().font_size);
        let rects: Vec<Rect> = layout.selection_rects(text_span.range.clone()).collect();
        let last = rects.len().saturating_sub(1);
        for (i, r) in rects.into_iter().enumerate() {
            // The room after the run is on its last character only; a line
            // the run wraps from ends at the run's glyphs.
            let right = if i == last {
                r.right() - PILL_GAP * size
            } else {
                r.right() + PILL_PAD * size
            };
            let x = r.x - PILL_PAD * size;
            let baseline = layout
                .lines()
                .find(|l| r.y + r.height / 2.0 >= l.top && r.y + r.height / 2.0 < l.top + l.height)
                .map_or(r.y + r.height * 0.75, |l| l.baseline);
            scene.rounded_rect(RoundedRectPrimitive::uniform(
                Rect {
                    x: origin.0 + x,
                    y: origin.1 + baseline - PILL_ASCENT * size,
                    width: right - x,
                    height: (PILL_ASCENT + PILL_DESCENT) * size,
                },
                PILL_RADIUS * size,
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

/// The color of each text span [`join_spans`] makes: each span's own, then
/// the color of the span under each character that keeps a pill's room.
pub(super) fn span_colors(
    spans: &[StyledSpan],
    default_color: Color,
    link_color: Color,
) -> Arc<[Color]> {
    let color = |span: &StyledSpan| match (span.color, &span.link) {
        (Some(color), _) => color,
        (None, Some(_)) => link_color,
        (None, None) => default_color,
    };
    let room = pill_room(spans);
    spans
        .iter()
        .map(color)
        .chain(room.into_iter().map(|(_, owner, _)| color(owner)))
        .collect()
}

impl Element for SelectableText {
    /// The element's node and its shaped text.
    type LayoutState = (LayoutId, Option<Arc<TextLayout>>);
    /// The drag-select hit of a text with a state, and its links' hits.
    type PrepaintState = (Option<HitId>, Vec<LinkHits>);

    fn request_layout(
        &mut self,
        engine: &mut LayoutEngine,
        cx: &mut ElementContext,
    ) -> (LayoutId, Self::LayoutState) {
        let explicit = match self.wrap {
            WrapMode::Explicit(width) => Some(width),
            WrapMode::Auto | WrapMode::NoWrap => None,
        };
        if self.paragraph.font_size <= 0.0 {
            self.paragraph.font_size = cx.theme.metrics.ui_font_size;
        }
        let paragraph = self.paragraph;
        // Unwrapped unless the width is explicit; automatic wrapping
        // reshapes at the resolved width in prepaint.
        let style = paragraph.text_style();
        let layout = with_styled_query(&self.spans, style, explicit.map(|w| w.max(1.0)), |q| {
            cx.layout_text_query(q)
        });
        let id = match (self.wrap, &layout) {
            (WrapMode::Auto, Some(unwrapped)) => engine.request_text_layout(
                &taffy::Style::default(),
                TextMeasure::new(unwrapped.clone(), paragraph.line_height_points())
                    .max_lines(self.max_lines),
            ),
            _ => {
                let width =
                    explicit.unwrap_or_else(|| layout.as_ref().map_or(0.0, |l| l.size().0.ceil()));
                let height = Self::paragraph_height(layout.as_deref(), &paragraph, self.max_lines);
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
        (id, (id, layout))
    }

    fn prepaint(
        &mut self,
        bounds: Bounds,
        (id, layout_state): &mut Self::LayoutState,
        engine: &LayoutEngine,
        cx: &mut ElementContext,
    ) -> (Option<HitId>, Vec<LinkHits>) {
        // Shaped where measurement wrapped at the width layout resolved,
        // which the last measure query may not have been (it can be an
        // intrinsic-size probe). Link hits, `max_lines` clipping, paint,
        // and the selectable region all use this layout.
        let wrapped = match layout_state {
            Some(unwrapped) if self.wrap == WrapMode::Auto => engine
                .auto_wrap_width(*id)
                .and_then(|w| cx.layout_text_query(&unwrapped.query().wrap_width(Some(w)))),
            _ => None,
        };
        if wrapped.is_some() {
            *layout_state = wrapped;
        }
        // Under the links, which are inserted after it and so win a press.
        let select = (self.state.is_some() && layout_state.is_some())
            .then(|| cx.insert_hit(bounds, HitFlags::DRAG | HitFlags::HOVER, CursorHint::Text));
        let links = match layout_state {
            Some(layout) => register_link_hits(&self.spans, layout, (bounds.x, bounds.y), cx),
            None => Vec::new(),
        };
        (select, links)
    }

    fn paint(
        &mut self,
        bounds: Bounds,
        (_, state): &mut Self::LayoutState,
        (select, links): &mut (Option<HitId>, Vec<LinkHits>),
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
        let selection = self.effective_selection();
        paint_selection(scene, &layout, selection, origin, highlight);

        // Italic glyphs ink past their advance; widen the text rect (which
        // the renderer clips to) so the last glyph of a line is not shaved.
        let colors = span_colors(&self.spans, default_color, cx.theme.colors.text_accent);
        let decorations = span_decorations(&self.spans, &layout, &colors, links, cx);
        let rect = Rect {
            width: bounds.width + self.paragraph.font_size * 0.5,
            ..bounds
        };
        if self.paint.is_plain() {
            scene.rich_text(RichTextPrimitive {
                rect,
                layout: ShapedText::new(layout.clone()),
                default_color,
                span_colors: colors,
            });
        } else {
            // A fill covers every glyph; without one, spans keep their
            // colors under the chosen coverage.
            let fill = self.paint.fill_now(default_color, cx);
            let span_colors = if self.paint.fill.is_some() {
                Arc::from([])
            } else {
                colors
            };
            scene.styled_text(
                self.paint
                    .primitive(rect, ShapedText::new(layout.clone()), fill)
                    .span_colors(span_colors),
            );
        }
        paint_decorations(scene, &layout, origin, &decorations);

        if clipped {
            scene.pop_clip();
        }

        let text = layout.source().clone();
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
                .text(match selection {
                    Some((start, end)) => AccessibleText::new(text.as_str()).selection(start, end),
                    None => AccessibleText::new(text.as_str()),
                }),
            );
        }

        let region = SelectableTextRegion {
            bounds,
            text_origin: origin,
            text,
            layout,
            source_key: self.source_key,
            transform: cx.current_transform(),
        };
        if let (Some(state), Some(hit)) = (&self.state, *select) {
            state.set_text(&region.text);
            // Under a transform that flattens the text nothing can be
            // pressed, so it registers no drag.
            if region.transform.invert().is_some() {
                let mut node = SemanticNode::new(bounds);
                node.parent = cx.current_semantic_parent();
                node.actions = SemanticActions::default().draggable();
                let index = cx.semantic.push(node);
                cx.bind_hit(hit, index);
                let (state, region) = (state.clone(), region.clone());
                cx.handlers.on_drag(
                    index,
                    DragStart::new(move |press| {
                        Box::new(RichTextDrag {
                            state: state.clone(),
                            region: region.clone(),
                            press,
                        })
                    }),
                );
            }
        }

        register_link_input(
            links,
            &region.text,
            &self.on_link,
            self.source_key,
            scene,
            cx,
        );
        register_selectable(cx, region);
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
        /// The last frame's accessibility nodes.
        accessibility: AccessibilityFrame,
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
                accessibility: AccessibilityFrame::default(),
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
            .with_element_cache(&mut self.cache);
            cx.semantic = SemanticFrame::new(400.0, 300.0);
            cx.accessibility = AccessibilityFrame::new(400.0, 300.0);
            let mut scene = Scene::default();
            render_element(&mut root.into_any(), &mut scene, &mut cx, 400.0, 300.0);
            self.regions = std::mem::take(&mut cx.selectable_text_runs);
            self.accessibility = std::mem::take(&mut cx.accessibility);
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

    // Catches inline code pills that crowd the words around them (the
    // owner saw "npm testpasses"): the pill pads its run by at least a
    // third of an em on each side, and the words beside it keep at least a
    // space's width clear of it.
    #[test]
    fn a_code_pill_pads_its_run_and_keeps_clear_of_its_neighbors() {
        const PILL: Color = Color::rgba(1, 2, 3, 255);
        let spans = vec![
            StyledSpan::plain("Run "),
            StyledSpan::plain("npm test")
                .code()
                .font_scale(12.0 / 14.0)
                .pill(PILL),
            StyledSpan::plain(" now"),
        ];
        let mut window = Window::new();
        let mut scene = Scene::default();
        {
            let mut cx = ElementContext::new(
                &window.theme,
                1.0,
                &mut window.text,
                &mut window.layouts,
                None,
                &window.signals,
            );
            let mut root = div().w(400.0).child(rich_text(spans).size(14.0)).into_any();
            render_element(&mut root, &mut scene, &mut cx, 400.0, 300.0);
            window.regions = std::mem::take(&mut cx.selectable_text_runs);
        }
        let pill = scene
            .primitives
            .iter()
            .find_map(|p| match p {
                quark_render::Primitive::RoundedRect(r) if r.color == PILL => Some(r.rect),
                _ => None,
            })
            .expect("a pill");
        let space = window.width_of(" ");
        let region = &window.regions[0];
        let x = |offset: usize| region.bounds.x + region.layout.caret(offset).x;
        // "Run" ends at 3, the code runs 4..12, "now" starts at 13. The
        // code is monospaced: its glyphs end one cell past the last's start.
        let code: Vec<_> = region
            .layout
            .glyphs()
            .iter()
            .filter(|g| (4..12).contains(&(g.byte_start as usize)))
            .collect();
        let code_start = region.bounds.x + code[0].x;
        let code_end = region.bounds.x + code[code.len() - 1].x + code[0].advance;
        assert!(pill.x <= code_start - 4.0, "{pill:?} pads {code_start}");
        assert!(pill.right() >= code_end + 4.0, "{pill:?} pads {code_end}");
        assert!(pill.x >= x(3) + space, "{pill:?} after {}", x(3));
        assert!(pill.right() + space <= x(13), "{pill:?} before {}", x(13));
    }

    /// A paragraph mixing prose, bold, inline code, and a link long enough
    /// to wrap in a narrow column.
    fn mixed_spans() -> Vec<StyledSpan> {
        vec![
            StyledSpan::plain("Call "),
            StyledSpan::plain("fn main()").code(),
            StyledSpan::plain(" then read "),
            StyledSpan::plain("the bold part").bold(),
            StyledSpan::plain(" and "),
            StyledSpan::plain("follow this wrapping link").link("https://quark.dev"),
            StyledSpan::plain(" at the end."),
        ]
    }

    /// Window point at the middle of the left or right edge of the first
    /// painted rectangle of bytes `range` of the painted region.
    fn edge_of(
        region: &SelectableTextRegion,
        range: std::ops::Range<usize>,
        right: bool,
    ) -> (f32, f32) {
        let rects: Vec<Rect> = region.layout.selection_rects(range).collect();
        let r = if right {
            rects[rects.len() - 1]
        } else {
            rects[0]
        };
        let x = if right { r.right() - 0.5 } else { r.x + 0.5 };
        (
            region.text_origin.0 + x,
            region.text_origin.1 + r.y + r.height / 2.0,
        )
    }

    // Catches a line height that only part of the paragraph honors: at 22
    // points, 14-point lines are 22 apart in the painted layout, selection
    // covers each 22-point line box, and the box below starts after them.
    #[test]
    fn paragraph_lines_sit_at_the_requested_point_line_height() {
        let mut window = Window::new();
        let frame = window.paint(column(
            120.0,
            rich_text(mixed_spans()).size(14.0).line_height_points(22.0),
        ));

        let region = &window.regions[0];
        let lines: Vec<(f32, f32)> = region.layout.lines().map(|l| (l.top, l.baseline)).collect();
        assert!(lines.len() > 2, "{lines:?}");
        for (i, pair) in lines.windows(2).enumerate() {
            assert_eq!(pair[0].0, 22.0 * i as f32, "{lines:?}");
            assert!((pair[1].1 - pair[0].1 - 22.0).abs() < 0.01, "{lines:?}");
        }
        let text_len = region.text.len();
        let boxes: Vec<(f32, f32)> = region
            .layout
            .selection_rects(0..text_len)
            .map(|r| (r.y, r.height))
            .collect();
        let expected: Vec<(f32, f32)> = (0..lines.len()).map(|i| (22.0 * i as f32, 22.0)).collect();
        assert_eq!(boxes, expected);
        let (bounds, _) = frame.region();
        assert_eq!(
            frame.swatch.expect("swatch").y,
            bounds.y + 22.0 * lines.len() as f32
        );
    }

    // Catches standalone rich text that cannot be selected without a
    // document around it, or whose copy reads anything but the spans'
    // own text: a drag from inline code into the bold run selects across
    // the styled runs, highlights them, and copies the source string.
    #[test]
    fn dragging_over_rich_text_selects_and_copies_its_source_text() {
        let state = RichTextState::new();
        let paragraph = || rich_text(mixed_spans()).size(14.0).source(7).state(&state);
        let mut window = Window::new();
        window.paint(column(160.0, paragraph()));
        let text = window.regions[0].text.as_str().to_owned();
        let code = text.find("fn main").expect("code");
        let bold_end = text.find(" part").expect("bold") + " part".len();
        let from = edge_of(&window.regions[0], code..code + 2, false);
        let to = edge_of(&window.regions[0], bold_end - 4..bold_end, true);

        let press = window.router.pointer_down(from.0, from.1, &mut None);
        window.router.pointer_move(to.0, to.1);
        window.router.pointer_up();
        let frame = window.paint(column(160.0, paragraph()));

        let changed = press
            .actions
            .iter()
            .filter_map(|a| a.downcast_ref::<TextSelectionChanged>())
            .count();
        assert_eq!(
            (state.selected_text().as_str(), changed),
            ("fn main() then read the bold part", 1)
        );
        assert!(!frame.highlights.is_empty(), "{frame:?}");
    }

    // Catches links only a pointer can reach: a wrapped link is a Tab
    // stop exposed as a link, and Enter on it opens its URL.
    #[test]
    fn a_link_is_reachable_and_opened_from_the_keyboard() {
        let mut window = Window::new();
        window.paint(column(160.0, rich_text(mixed_spans()).size(14.0)));

        let focus = window.router.traverse_focus(None, false);
        let enter: Binding = "enter".parse().expect("binding");
        let opened: Vec<Arc<str>> = window
            .router
            .activate(&enter, focus)
            .actions
            .iter()
            .filter_map(|a| a.downcast_ref::<LinkClicked>())
            .map(|link| link.url.clone())
            .collect();
        let dump = crate::accessibility::dump_accessibility(&window.accessibility);
        let links: Vec<&str> = dump
            .lines()
            .filter_map(|line| {
                let fields: Vec<&str> = line.split(" | ").collect();
                (fields[1] == "Link").then(|| fields[2])
            })
            .collect();
        assert_eq!(opened, [Arc::from("https://quark.dev")]);
        assert_eq!(links, ["follow this wrapping link"]);
    }

    /// Solid quads and stroked paths in `color` the last frame painted:
    /// `(rect, stroke width, pattern)`, paths at their bounds.
    fn decoration_strokes(scene: &Scene, color: Color) -> Vec<(Rect, f32, StrokePattern)> {
        scene
            .primitives
            .iter()
            .filter_map(|p| match p {
                quark_render::Primitive::Path(path) => {
                    let stroke = path.stroke.filter(|s| s.color == color)?;
                    let b = path.path.bounds();
                    Some((
                        b.offset(path.origin[0], path.origin[1]),
                        stroke.style.width,
                        stroke.style.pattern,
                    ))
                }
                _ => None,
            })
            .collect()
    }

    const INK: Color = Color::rgba(200, 100, 50, 255);

    /// A frame's scene, painted through the window.
    fn scene_of(window: &mut Window, root: impl IntoAnyElement) -> Scene {
        window.layouts.begin_frame();
        let mut cx = ElementContext::new(
            &window.theme,
            1.0,
            &mut window.text,
            &mut window.layouts,
            None,
            &window.signals,
        );
        let mut scene = Scene::default();
        render_element(&mut root.into_any(), &mut scene, &mut cx, 400.0, 300.0);
        window.regions = std::mem::take(&mut cx.selectable_text_runs);
        scene
    }

    // Catches a dotted link underline drawn per span, across the box, or
    // continuing from one line into the next: a link split over two spans
    // wraps and gets one dotted stroke per painted line, spanning exactly
    // the link's glyphs on that line, under the baseline.
    #[test]
    fn a_dotted_link_underline_restarts_on_each_wrapped_line() {
        let dotted = TextDecorationStyle::solid(INK).pattern(StrokePattern::Dotted {
            spacing: 3.0,
            offset: 0.0,
        });
        let link = |s: &str| {
            StyledSpan::plain(s)
                .link("https://quark.dev")
                .underline_style(dotted)
        };
        let spans = vec![
            StyledSpan::plain("see "),
            link("the quick brown fox "),
            link("jumps over"),
            StyledSpan::plain(" the lazy dog"),
        ];
        let mut window = Window::new();
        let scene = scene_of(&mut window, column(120.0, rich_text(spans).size(14.0)));

        let region = &window.regions[0];
        let start = "see ".len();
        let end = start + "the quick brown fox jumps over".len();
        let (ox, oy) = region.text_origin;
        let expected: Vec<(f32, f32, f32)> = region
            .layout
            .lines()
            .filter_map(|line| {
                let text = &region.text.as_str()[line.byte_range.clone()];
                let lo = start.max(line.byte_range.start);
                let hi = end.min(line.byte_range.start + text.trim_end().len());
                let r = region
                    .layout
                    .selection_rects(lo..hi)
                    .next()
                    .filter(|_| lo < hi)?;
                Some((ox + r.x, ox + r.right(), oy + line.baseline))
            })
            .collect();
        let strokes = decoration_strokes(&scene, INK);
        assert!(expected.len() > 1, "link fits one line: {expected:?}");
        assert_eq!(strokes.len(), expected.len(), "{strokes:?}");
        for ((rect, _, pattern), (left, right, baseline)) in strokes.iter().zip(&expected) {
            assert_eq!(*pattern, dotted.pattern);
            assert!(
                (rect.x - left).abs() < 0.01 && (rect.right() - right).abs() < 0.01,
                "{rect:?}"
            );
            assert!(rect.y > *baseline, "{rect:?} above baseline {baseline}");
        }
    }

    // Catches hostile decoration metrics reaching the renderer: a NaN
    // thickness and an infinite offset fall back to the default one-point
    // line just under the baseline.
    #[test]
    fn hostile_decoration_metrics_fall_back_to_defaults() {
        let hostile = TextDecorationStyle::solid(INK)
            .pattern(StrokePattern::Dashed {
                dash: 0.0,
                gap: f32::NAN,
                offset: 0.0,
            })
            .thickness(f32::NAN)
            .offset(f32::INFINITY);
        let spans = vec![StyledSpan::plain("dashed").underline_style(hostile)];
        let mut window = Window::new();
        let scene = scene_of(&mut window, rich_text(spans).size(14.0));

        let baseline = window.regions[0]
            .layout
            .lines()
            .next()
            .expect("line")
            .baseline;
        let [(rect, width, _)] = decoration_strokes(&scene, INK)[..] else {
            panic!("one stroke");
        };
        assert_eq!(width, 1.0);
        // 0.12 em below the baseline plus half the line.
        assert!(
            (rect.y - (window.regions[0].text_origin.1 + baseline + 14.0 * 0.12 + 0.5)).abs()
                < 0.01,
            "{rect:?}"
        );
    }
}
