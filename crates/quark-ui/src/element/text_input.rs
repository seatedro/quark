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
    pub bounds: Rect,
    pub focus_target: FocusId,
    /// The text area in window coordinates. Pointers past its edges along
    /// the scroll axis autoscroll.
    pub text_rect: Rect,
    /// Caret rect in window coordinates while focused, for placing the IME
    /// candidate window.
    pub caret: Option<Rect>,
    pub content: TextHitContent,
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

    /// Window position of the layout origin.
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
    /// `None` when there is no layout.
    pub fn offset_at_point(&self, x: f32, y: f32) -> Option<TextOffset> {
        let layout = self.layout()?;
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
        _cx: &mut ElementContext,
    ) -> (LayoutId, ()) {
        let id = engine.request_layout(self.base_style.layout.clone(), &[]);
        (id, ())
    }

    fn prepaint(
        &mut self,
        bounds: Bounds,
        _layout_state: &mut (),
        _engine: &LayoutEngine,
        cx: &mut ElementContext,
    ) -> Option<HitId> {
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
            let border = if self.focused {
                theme.colors.focus_border
            } else {
                theme.colors.border
            };

            scene.rounded_rect(RoundedRectPrimitive::uniform(bounds, radius, fill));
            scene.border(BorderPrimitive::uniform(bounds, 1.0, radius, border));

            let scale = theme.metrics.ui_scale();
            let label_size = theme.metrics.ui_small_font_size - 1.0;
            let label_lh = label_size * 1.4;
            let pad = (Sz::INPUT_SIDE_PAD * scale).round();
            let top_pad = (Sz::INPUT_TOP_PAD * scale).round();

            let label_style = TextStyle::new(label_size)
                .weight(FontWeight::Medium)
                .line_height(label_lh);
            let label_params = TextParams::new(std::mem::take(&mut self.label), label_style);
            if let Some(layout) = cx.layout_text(&label_params) {
                scene.text(TextPrimitive {
                    rect: Rect {
                        x: bounds.x + pad,
                        y: bounds.y + top_pad,
                        width: bounds.width - pad * 2.0,
                        height: label_lh,
                    },
                    layout: ShapedText::new(layout),
                    color: theme.colors.text_muted,
                });
            }

            text_x = bounds.x + pad;
            text_y = bounds.y + top_pad + label_lh + 2.0;
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
            theme.colors.text_muted.with_alpha(Alpha::PLACEHOLDER)
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
        let caret = caret_byte.filter(|_| self.focused).map(|byte| Rect {
            x: origin_x + caret_x(byte),
            y: text_y + 1.0,
            width: Sz::CURSOR_WIDTH,
            height: value_lh - Sz::CURSOR_WIDTH,
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
            let accessible_text = AccessibleText::new(accessible_value.as_str())
                .selection(text_anchor.get(), text_focus.get());
            let semantic_id = format!("text-input:{target:?}");
            let mut style_state = StyleState::empty();
            if self.focused {
                style_state.insert(StyleState::FOCUS_VISIBLE);
            }
            let mut semantic_node = SemanticNode::new(bounds)
                .id(semantic_id.clone())
                .role(SemanticRole::TextInput)
                .label(accessibility_label.clone())
                .value(accessible_value.clone());
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
            cx.push_accessibility_for_semantic(
                AccessibilityNode::new(semantic_id, role, bounds)
                    .label(accessibility_label)
                    .value(accessible_value)
                    .text(accessible_text)
                    .action(AccessibilityAction::TextValue(target)),
                index,
            );
            cx.text_input_hit_areas.push(TextInputHitArea {
                bounds,
                focus_target: target,
                text_rect,
                caret,
                content: TextHitContent::SingleLine {
                    layout: value_layout.filter(|_| composition.is_none()).cloned(),
                    scroll_x,
                    masked: self.masked.then(|| self.value.clone()),
                },
            });
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
