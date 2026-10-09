// Raw byte slicing belongs in `quark_text::offset`; offsets here are
// `TextOffset`s snapped onto the value being painted.
#![deny(clippy::string_slice)]
#![cfg_attr(not(test), deny(clippy::indexing_slicing))]

use super::*;
use crate::text_input::{
    HorizontalScroll, Preedit, TextField, caret_blink, compose, reveal_offset, text_pointer_drag,
};
use quark_render::RectPrimitive;
use quark_text::TextOffset;
use quark_text::offset;

/// Space between a labeled field's label line and its value line.
const LABEL_GAP: f32 = 2.0;
/// Caret height as a multiple of the value font size.
const CARET_EM: f32 = 1.2;

/// The natural height of a labeled field: padding, label line, gap, value
/// line, padding. Matches the metrics `paint` uses.
fn labeled_height(metrics: &crate::theme::ThemeMetrics) -> f32 {
    let scale = metrics.ui_scale();
    let label_lh = (metrics.ui_small_font_size - 1.0) * 1.4;
    let value_lh = metrics.ui_font_size * 1.5;
    let top_pad = (Sz::INPUT_TOP_PAD * scale).round();
    top_pad * 2.0 + label_lh + LABEL_GAP + value_lh
}

const BULLET: &str = "\u{2022}";

// ---------------------------------------------------------------------------
// TextInput — text field with cursor, selection, and editing support
// ---------------------------------------------------------------------------

pub struct TextInput {
    label: String,
    value: Arc<str>,
    placeholder: String,
    focused: bool,
    on_click: Option<Action>,
    base_style: ElementStyle,
    cursor: TextOffset,
    anchor: TextOffset,
    cursor_moved_at_ms: u64,
    focus_target: Option<FocusId>,
    bare: bool,
    masked: bool,
    search: bool,
    preedit: Option<Preedit>,
    scroll: Option<HorizontalScroll>,
}

pub fn text_input(label: impl Into<String>, value: impl Into<String>) -> TextInput {
    TextInput {
        label: label.into(),
        value: Arc::from(value.into()),
        placeholder: String::new(),
        focused: false,
        on_click: None,
        base_style: ElementStyle::default(),
        cursor: TextOffset::ZERO,
        anchor: TextOffset::ZERO,
        cursor_moved_at_ms: 0,
        focus_target: None,
        bare: false,
        masked: false,
        search: false,
        preedit: None,
        scroll: None,
    }
}

impl TextInput {
    pub fn placeholder(mut self, p: impl Into<String>) -> Self {
        self.placeholder = p.into();
        self
    }

    pub fn focused(mut self, f: bool) -> Self {
        self.focused = f;
        self
    }

    pub fn on_click(mut self, action: impl Into<Action>) -> Self {
        self.on_click = Some(action.into());
        self
    }

    /// The caret. It is snapped onto the value when painted, so an offset
    /// from an older value is safe.
    pub fn cursor(mut self, offset: TextOffset) -> Self {
        self.cursor = offset;
        self
    }

    /// The selection anchor, snapped like [`Self::cursor`].
    pub fn anchor(mut self, offset: TextOffset) -> Self {
        self.anchor = offset;
        self
    }

    pub fn cursor_moved_at(mut self, ms: u64) -> Self {
        self.cursor_moved_at_ms = ms;
        self
    }

    pub fn focus_target(mut self, target: FocusId) -> Self {
        self.focus_target = Some(target);
        self
    }

    pub fn bare(mut self) -> Self {
        self.bare = true;
        self
    }

    pub fn masked(mut self, masked: bool) -> Self {
        self.masked = masked;
        self
    }

    /// IME composition to show at the caret (ignored when masked).
    pub fn preedit(mut self, preedit: Option<Preedit>) -> Self {
        self.preedit = preedit;
        self
    }

    /// Scroll offset shared with the model; the element updates it each
    /// frame so the caret stays visible. Without one, the text scrolls
    /// just enough to show the caret.
    pub fn scroll(mut self, scroll: HorizontalScroll) -> Self {
        self.scroll = Some(scroll);
        self
    }

    /// Value, caret, selection, composition, and scroll from `field`.
    pub fn field(self, field: &TextField) -> Self {
        let mut this = self
            .cursor(field.cursor())
            .anchor(field.anchor())
            .preedit(field.preedit().cloned())
            .scroll(field.scroll().clone());
        this.value = Arc::from(field.text());
        this
    }

    /// Expose the field to assistive tech as a search box.
    pub fn search(mut self, search: bool) -> Self {
        self.search = search;
        self
    }
}

impl Styled for TextInput {
    fn element_style_mut(&mut self) -> &mut ElementStyle {
        &mut self.base_style
    }
}

/// A painted text field, recorded each frame so pointer input and the IME
/// can be mapped through what is on screen.
#[derive(Debug, Clone)]
pub struct TextInputHitArea {
    /// The field's box in layout coordinates (before `transform`).
    pub bounds: Rect,
    pub focus_target: FocusId,
    /// The text area in layout coordinates. Pointers past its edges along
    /// the scroll axis autoscroll.
    pub text_rect: Rect,
    /// Bounds of the caret in window coordinates while focused, for placing
    /// the IME candidate window. `None` under a transform that flattens the
    /// field, where no caret shows.
    pub caret: Option<Rect>,
    pub content: TextHitContent,
    /// Layout coordinates to window coordinates: the transforms of the
    /// field's ancestors.
    pub transform: Transform2D,
}

/// Register `area` under the current transform, its `caret` given in
/// layout coordinates. A transform that flattens the field (a zero scale)
/// keeps the area, so keys still edit a focused field, but drops its caret
/// and takes no pointer input.
pub(crate) fn register_text_input_area(cx: &mut ElementContext, mut area: TextInputHitArea) {
    cx.register_ime_target(area.focus_target, area.caret);
    area.transform = cx.current_transform();
    area.caret = area
        .caret
        .filter(|_| area.transform.invert().is_some())
        .map(|caret| window_rect(area.transform, caret));
    cx.text_input_hit_areas.push(area);
}

/// How a [`TextInputHitArea`] maps points to offsets of its text.
#[derive(Debug, Clone)]
pub enum TextHitContent {
    /// A single-line field. `layout` is the painted value (bullets when
    /// masked) with its origin `scroll_x` left of the text area; only `x`
    /// matters for hits.
    SingleLine {
        layout: Option<Arc<TextLayout>>,
        scroll_x: f32,
        /// The real value behind the bullets of a masked field.
        masked: Option<Arc<str>>,
    },
    /// An editor. `layout` is its wrapped text with its origin `scroll_y`
    /// above the text area.
    Multiline {
        layout: Option<Arc<TextLayout>>,
        scroll_y: f32,
    },
}

impl TextInputHitArea {
    /// The painted layout pointer hits go through: `None` for placeholders
    /// and compositions.
    pub fn layout(&self) -> Option<&Arc<TextLayout>> {
        match &self.content {
            TextHitContent::SingleLine { layout, .. }
            | TextHitContent::Multiline { layout, .. } => layout.as_ref(),
        }
    }

    /// A window point in layout coordinates; `None` under a flattening
    /// transform.
    pub fn to_layout(&self, x: f32, y: f32) -> Option<(f32, f32)> {
        if self.transform.is_identity() {
            return Some((x, y));
        }
        Some(self.transform.invert()?.apply(x, y))
    }

    /// Position of the layout origin, in layout coordinates.
    pub fn origin(&self) -> (f32, f32) {
        let (x, y) = (self.text_rect.x, self.text_rect.y);
        match self.content {
            TextHitContent::SingleLine { scroll_x, .. } => (x - scroll_x, y),
            TextHitContent::Multiline { scroll_y, .. } => (x, y - scroll_y),
        }
    }

    /// Offset into the field's text under a window point. A point past an
    /// edge maps to text scrolled out of view there, so a drag selection
    /// past the edge extends into it and the next frame scrolls it in.
    /// `None` when there is no layout, or the field is flattened.
    pub fn offset_at_point(&self, x: f32, y: f32) -> Option<TextOffset> {
        let layout = self.layout()?;
        let (x, y) = self.to_layout(x, y)?;
        let (ox, oy) = self.origin();
        let y = match self.content {
            TextHitContent::SingleLine { .. } => self.text_rect.height * 0.5,
            TextHitContent::Multiline { .. } => y - oy,
        };
        let hit = layout.hit(x - ox, y);
        match &self.content {
            // One bullet is painted per grapheme of the value.
            TextHitContent::SingleLine {
                masked: Some(value),
                ..
            } => Some(offset::nth_grapheme(
                value,
                offset::graphemes_before(layout.text(), hit),
            )),
            _ => Some(hit),
        }
    }

    /// Whether a point is past the edge the field scrolls along: left or
    /// right for single-line fields, above or below for editors.
    pub(crate) fn is_past_edge(&self, x: f32, y: f32) -> bool {
        let Some((x, y)) = self.to_layout(x, y) else {
            return false;
        };
        let r = &self.text_rect;
        match self.content {
            TextHitContent::SingleLine { .. } => x < r.x || x > r.x + r.width,
            TextHitContent::Multiline { .. } => y < r.y || y > r.y + r.height,
        }
    }
}

impl Element for TextInput {
    type LayoutState = ();
    type PrepaintState = Option<HitId>;

    fn request_layout(
        &mut self,
        engine: &mut LayoutEngine,
        cx: &mut ElementContext,
    ) -> (LayoutId, ()) {
        let mut layout = self.base_style.layout.clone();
        // A labeled field is at least tall enough for its label, value line,
        // and padding, so apps need not guess a height.
        if !self.bare
            && layout.size.height == taffy::Dimension::auto()
            && layout.min_size.height == taffy::Dimension::auto()
        {
            layout.min_size.height = taffy::Dimension::length(labeled_height(&cx.theme.metrics));
        }
        let id = engine.request_layout(layout, &[]);
        (id, ())
    }

    fn prepaint(
        &mut self,
        bounds: Bounds,
        _layout_state: &mut (),
        _engine: &LayoutEngine,
        cx: &mut ElementContext,
    ) -> Option<HitId> {
        if let Some(target) = self.focus_target {
            cx.note_focus_target(target);
        }
        let mut flags = HitFlags::NONE;
        if self.focus_target.is_some() {
            flags |= HitFlags::TEXT;
        }
        if self.on_click.is_some() {
            flags |= HitFlags::CLICK;
        }
        if self.focus_target.is_some() {
            flags |= HitFlags::DRAG;
        }
        (!flags.is_empty()).then(|| cx.insert_hit(bounds, flags, CursorHint::Text))
    }

    fn paint(
        &mut self,
        bounds: Bounds,
        _layout_state: &mut (),
        prepaint_state: &mut Option<HitId>,
        _engine: &LayoutEngine,
        scene: &mut Scene,
        cx: &mut ElementContext,
    ) {
        let theme = cx.theme;
        let accessibility_label = if !self.label.is_empty() {
            self.label.clone()
        } else if !self.placeholder.is_empty() {
            self.placeholder.clone()
        } else if let Some(target) = self.focus_target {
            format!("{target:?}")
        } else {
            String::new()
        };
        let radius = theme.metrics.control_radius;
        let value_size = if self.bare {
            theme.metrics.ui_small_font_size
        } else {
            theme.metrics.ui_font_size
        };
        let value_lh = value_size * 1.5;

        let (text_x, text_y, text_area_w);

        if self.bare {
            let pad = 0.0;
            text_x = bounds.x + pad;
            text_y = bounds.y + ((bounds.height - value_lh) * 0.5).max(0.0);
            text_area_w = (bounds.width - pad * 2.0).max(0.0);
        } else {
            let fill = if self.focused {
                theme.colors.surface
            } else {
                theme.colors.element_background
            };
            scene.rounded_rect(RoundedRectPrimitive::uniform(bounds, radius, fill));
            scene.border(BorderPrimitive::uniform(
                bounds,
                1.0,
                radius,
                theme.colors.border,
            ));
            // Focus shows as the ring every other control draws, which the
            // high-contrast themes rely on; a recolored 1pt border was much
            // fainter than the rings around it.
            if self.focused {
                paint_focus_ring(scene, cx, bounds, [radius; 4], 0.0);
            }

            let scale = theme.metrics.ui_scale();
            let label_size = theme.metrics.ui_small_font_size - 1.0;
            let label_lh = label_size * 1.4;
            let pad = (Sz::INPUT_SIDE_PAD * scale).round();

            // Center the label and value lines in whatever height the field
            // was given; a field shorter than its content would otherwise
            // push the value line into the bottom border.
            let stack_h = label_lh + LABEL_GAP + value_lh;
            let stack_top = bounds.y + ((bounds.height - stack_h) * 0.5).max(0.0);

            let label_style = TextStyle::new(label_size)
                .weight(FontWeight::Medium)
                .line_height(label_lh);
            if let Some(layout) = cx.layout_text_query(&TextQuery::new(&self.label, label_style)) {
                scene.text(TextPrimitive {
                    rect: Rect {
                        x: bounds.x + pad,
                        y: stack_top,
                        width: bounds.width - pad * 2.0,
                        height: label_lh,
                    },
                    layout: ShapedText::new(layout),
                    color: theme.colors.text_muted,
                });
            }

            text_x = bounds.x + pad;
            text_y = stack_top + label_lh + LABEL_GAP;
            text_area_w = bounds.width - pad * 2.0;
        }

        // While composing, the preedit replaces the selection in what is
        // painted; the committed value is untouched until the IME commits.
        let composition = self
            .preedit
            .as_ref()
            .filter(|_| self.focused && !self.masked)
            .map(|preedit| compose(&self.value, self.anchor..self.cursor, preedit));
        let is_placeholder = self.value.is_empty() && composition.is_none();
        let masked_display = self.masked.then(|| {
            BULLET.repeat(offset::graphemes_before(
                &self.value,
                TextOffset::end(&self.value),
            ))
        });
        // Offset into the painted text for an offset into the value. Both
        // builders' offsets may be stale, so they are snapped here.
        let to_display = |at: TextOffset| -> TextOffset {
            match &masked_display {
                Some(bullets) => {
                    offset::nth_grapheme(bullets, offset::graphemes_before(&self.value, at))
                }
                None => at.within(&self.value),
            }
        };

        let display: Arc<str> = if let Some(composition) = &composition {
            Arc::from(composition.text.as_str())
        } else if is_placeholder {
            Arc::from(std::mem::take(&mut self.placeholder))
        } else if let Some(bullets) = &masked_display {
            Arc::from(bullets.as_str())
        } else {
            self.value.clone()
        };
        let text_color = if is_placeholder {
            theme.colors.placeholder
        } else {
            theme.colors.text
        };
        let value_style = TextStyle::new(value_size).line_height(value_lh);
        // One layout for the displayed text: painted, and used for the caret
        // and selection so they sit on the painted glyph edges.
        let layout = cx.layout_text(&TextParams::new(display, value_style));
        let value_layout = layout.as_ref().filter(|_| !is_placeholder);
        let caret_x = |at: TextOffset| value_layout.map_or(0.0, |l| l.caret(at).x);

        // Caret in layout coordinates; `None` when the IME hides it.
        let caret_byte = match &composition {
            Some(composition) => composition.caret,
            None => Some(to_display(self.cursor)),
        };
        let content_w = value_layout.map_or(0.0, |l| l.size().0);
        let previous = self.scroll.as_ref().map_or(0.0, HorizontalScroll::get);
        let scroll_x = match caret_byte.filter(|_| self.focused) {
            Some(byte) => reveal_offset(
                previous,
                caret_x(byte),
                Sz::CURSOR_WIDTH,
                content_w,
                text_area_w,
            ),
            None => previous.clamp(0.0, (content_w - text_area_w).max(0.0)),
        };
        if let Some(scroll) = &self.scroll {
            scroll.set(scroll_x);
        }
        let origin_x = text_x - scroll_x;
        let text_rect = Rect {
            x: text_x,
            y: text_y,
            width: text_area_w.max(0.0),
            height: value_lh,
        };
        // Keep scrolled text and the caret inside the text area.
        scene.clip(Rect {
            x: text_x - Sz::CURSOR_WIDTH,
            width: text_rect.width + Sz::CURSOR_WIDTH * 2.0,
            ..text_rect
        });

        // Selection highlight (render before text so it appears behind)
        if self.focused && !is_placeholder && composition.is_none() {
            let (sel_start, sel_end) = (to_display(self.anchor), to_display(self.cursor));
            if sel_start != sel_end {
                let (x0, x1) = (caret_x(sel_start), caret_x(sel_end));
                let (x_start, x_end) = (x0.min(x1), x0.max(x1));
                scene.rounded_rect(RoundedRectPrimitive::uniform(
                    Rect {
                        x: origin_x + x_start,
                        y: text_y,
                        width: x_end - x_start,
                        height: value_lh,
                    },
                    2.0,
                    theme.colors.accent.with_alpha(Alpha::SOFT),
                ));
            }
        }

        // Value text
        if let Some(layout) = &layout {
            scene.text(TextPrimitive {
                rect: Rect {
                    x: origin_x,
                    width: content_w.max(text_area_w),
                    ..text_rect
                },
                layout: ShapedText::new(layout.clone()),
                color: text_color,
            });
        }

        // Composition underline: thin under the preedit, thick under the
        // clause the IME is converting.
        if let Some(composition) = &composition {
            let thin = theme.metrics.ui_scale().max(1.0);
            let ranges = [
                Some((composition.preedit.clone(), thin)),
                composition
                    .clause
                    .clone()
                    .map(|clause| (clause, thin * 2.0)),
            ];
            for (range, thickness) in ranges.into_iter().flatten() {
                let (x0, x1) = (caret_x(range.start), caret_x(range.end));
                scene.rect(RectPrimitive {
                    rect: Rect {
                        x: origin_x + x0.min(x1),
                        y: text_y + value_lh - thickness,
                        width: (x1 - x0).abs(),
                        height: thickness,
                    },
                    color: text_color,
                });
            }
        }

        // Cursor caret
        // As tall as the text, not the line box, centered on the line where
        // the glyphs are painted.
        let caret_h = (value_size * CARET_EM).min(value_lh);
        let caret = caret_byte.filter(|_| self.focused).map(|byte| Rect {
            x: origin_x + caret_x(byte),
            y: text_y + (value_lh - caret_h) * 0.5,
            width: Sz::CURSOR_WIDTH,
            height: caret_h,
        });
        if let Some(rect) = caret {
            let (visible, next_toggle_ms) = caret_blink(cx.clock_ms, self.cursor_moved_at_ms);
            if visible {
                scene.rounded_rect(RoundedRectPrimitive::uniform(rect, 1.0, theme.colors.text));
            }
            cx.request_frame_at_ms(next_toggle_ms);
        }
        scene.pop_clip();

        let on_click = self.on_click.take();
        let mut semantic_index = None;

        // Register hit area for click-to-position (stored in cx for app.rs to use)
        if let Some(target) = self.focus_target {
            let role = if self.masked {
                AccessibilityRole::PasswordInput
            } else if self.search {
                AccessibilityRole::SearchInput
            } else {
                AccessibilityRole::TextInput
            };
            // Caret and selection in the text assistive tech reads (the
            // bullets of a masked field), from the committed value.
            let (text_anchor, text_focus) = (to_display(self.anchor), to_display(self.cursor));
            let accessible_value = masked_display.unwrap_or_else(|| self.value.to_string());
            let semantic_id = crate::text_input::target_key("text-input", target);
            // Built only for a listening screen reader; the semantic node
            // then takes the strings.
            let accessibility = cx.accessibility_enabled().then(|| {
                AccessibilityNode::shared(semantic_id.clone(), role, bounds)
                    .label(accessibility_label.clone())
                    .value(accessible_value.clone())
                    .text(
                        AccessibleText::new(accessible_value.as_str())
                            .selection(text_anchor.get(), text_focus.get()),
                    )
                    .action(AccessibilityAction::TextValue(target))
            });
            let mut style_state = StyleState::empty();
            if self.focused {
                style_state.insert(StyleState::FOCUS_VISIBLE);
            }
            let mut semantic_node = SemanticNode::new(bounds)
                .id(semantic_id)
                .role(SemanticRole::TextInput)
                .label(accessibility_label)
                .value(accessible_value);
            semantic_node.parent = cx.current_semantic_parent();
            semantic_node.actions = SemanticActions::default().text_value().hit_test();
            if on_click.is_some() {
                semantic_node.actions = semantic_node.actions.clickable();
            }
            semantic_node.focus = Some(target);
            semantic_node.state = SemanticNodeState {
                style_state,
                ..SemanticNodeState::default()
            };
            let index = cx.semantic.push(semantic_node);
            semantic_index = Some(index);
            if let Some(node) = accessibility {
                cx.push_accessibility_for_semantic(node, index);
            }
            register_text_input_area(
                cx,
                TextInputHitArea {
                    bounds,
                    focus_target: target,
                    text_rect,
                    caret,
                    content: TextHitContent::SingleLine {
                        layout: value_layout.filter(|_| composition.is_none()).cloned(),
                        scroll_x,
                        masked: self.masked.then(|| self.value.clone()),
                    },
                    transform: Transform2D::IDENTITY,
                },
            );
        } else if on_click.is_some() {
            let mut semantic_node = SemanticNode::new(bounds)
                .role(SemanticRole::TextInput)
                .label(accessibility_label);
            semantic_node.parent = cx.current_semantic_parent();
            semantic_node.actions = SemanticActions::default().clickable();
            semantic_index = Some(cx.semantic.push(semantic_node));
        }

        if let Some(node) = semantic_index {
            if let Some(hit) = *prepaint_state {
                cx.bind_hit(hit, node);
            }
            // A field with a focus target owns the pointer for caret
            // placement and selection; its drag reports the click action.
            match (self.focus_target, on_click) {
                (Some(target), on_click) => {
                    cx.handlers
                        .on_drag(node, text_pointer_drag(target, on_click));
                }
                (None, Some(action)) => {
                    cx.handlers
                        .on_click(node, ClickHandler::from_action(action));
                }
                (None, None) => {}
            }
        }
    }
}

impl IntoAnyElement for TextInput {
    fn into_any(self) -> AnyElement {
        element_into_any(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text_input::TextEditCommand::*;

    const FIELD: FocusId = FocusId::from_key("test.field");

    struct Painter {
        text: quark_text::TextSystem,
        layouts: quark_text::LayoutCache,
        theme: Theme,
        signals: SignalStore,
    }

    impl Painter {
        fn new() -> Self {
            Self {
                text: quark_text::TextSystem::vendored_only(&Default::default()),
                layouts: quark_text::LayoutCache::default(),
                theme: Theme::default_dark(),
                signals: SignalStore::new(),
            }
        }

        /// Paint `field` focused in a 160px input; return its hit area.
        fn paint(&mut self, field: &TextField) -> TextInputHitArea {
            self.paint_input(text_input("Name", "").field(field))
        }

        fn paint_input(&mut self, input: TextInput) -> TextInputHitArea {
            let mut cx = ElementContext::new(
                &self.theme,
                1.0,
                &mut self.text,
                &mut self.layouts,
                None,
                &self.signals,
            )
            .with_focus(Some(FIELD));
            let mut root = input
                .focused(true)
                .focus_target(FIELD)
                .w(160.0)
                .h(52.0)
                .into_any();
            render_element(&mut root, &mut Scene::default(), &mut cx, 160.0, 52.0);
            cx.text_input_hit_areas.pop().expect("text input hit area")
        }
    }

    // Regression: in a 52px labeled field (hello_ui's size) the label and
    // value lines plus top padding filled the field, and the caret spanned
    // the whole 1.5x line box, so it ran into the bottom border.
    #[test]
    fn caret_in_a_labeled_field_clears_the_border() {
        let mut painter = Painter::new();
        let mut field = TextField::new("");
        field.apply(InsertText("Quark".into()));
        let area = painter.paint(&field);
        let caret = area.caret.expect("focused field reports a caret");
        let bottom = 52.0 - 1.0; // inside the 1px border
        assert!(
            caret.y >= 1.0 && caret.y + caret.height <= bottom - 2.0,
            "caret {}..{} reaches the border (field 0..52)",
            caret.y,
            caret.y + caret.height
        );
        let font = painter.theme.metrics.ui_font_size;
        assert!(
            caret.height <= font * 1.25,
            "caret {} taller than its text",
            caret.height
        );
    }

    fn assert_caret_visible(area: &TextInputHitArea) {
        let caret = area.caret.expect("focused field reports a caret");
        let (left, right) = (area.text_rect.x, area.text_rect.right());
        assert!(
            caret.x >= left - 0.5 && caret.x + caret.width <= right + 0.5,
            "caret {}..{} outside {left}..{right}",
            caret.x,
            caret.x + caret.width
        );
    }

    #[test]
    fn caret_stays_visible_while_typing_past_the_right_edge() {
        let mut painter = Painter::new();
        let mut field = TextField::new("");
        for ch in "the quick brown fox jumps over the lazy dog".chars() {
            field.apply(InsertText(ch.to_string()));
            assert_caret_visible(&painter.paint(&field));
        }
        assert!(field.scroll().get() > 0.0, "long text scrolled");
        field.apply(CursorHome);
        let area = painter.paint(&field);
        assert_eq!(area.origin().0, area.text_rect.x, "scrolled home");
        assert_caret_visible(&area);
    }

    #[test]
    fn dragging_past_the_right_edge_selects_hidden_text_and_scrolls_to_it() {
        let mut painter = Painter::new();
        let mut field = TextField::new("the quick brown fox jumps over the lazy dog");
        field.apply(CursorHome);
        let area = painter.paint(&field);
        let layout = area.layout().cloned().expect("value layout");
        let past = area
            .offset_at_point(area.text_rect.right() + 12.0, area.text_rect.y)
            .expect("offset");
        assert!(
            layout.caret(past).x > area.text_rect.width,
            "offset was hidden"
        );

        field.apply(ExtendTextSelection(past.get()));
        let area = painter.paint(&field);
        assert_eq!(field.selection_range(), Some(TextOffset::ZERO..past));
        assert!(area.origin().0 < area.text_rect.x, "scrolled right");
        assert_caret_visible(&area);
    }

    // Regression: a masked field counted chars up to its raw cursor by
    // slicing the value, so a cursor left over from another value (here
    // inside the "é") panicked.
    #[test]
    fn masked_field_with_a_stale_cursor_puts_the_caret_on_a_grapheme() {
        let mut painter = Painter::new();
        let stale = TextOffset::snap("abc", 1);
        let caret = |painter: &mut Painter, cursor| {
            let input = text_input("Pin", "\u{e9}a").masked(true).cursor(cursor);
            painter.paint_input(input).caret.expect("caret")
        };
        assert_eq!(
            caret(&mut painter, stale),
            caret(&mut painter, TextOffset::ZERO)
        );
    }
}
