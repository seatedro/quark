use super::*;
use crate::text_input::{
    HorizontalScroll, Preedit, TextField, caret_blink, compose, reveal_offset, text_pointer_drag,
};
use quark_render::RectPrimitive;

// ---------------------------------------------------------------------------
// TextInput — text field with cursor, selection, and editing support
// ---------------------------------------------------------------------------

pub struct TextInput {
    label: String,
    value: String,
    placeholder: String,
    focused: bool,
    on_click: Option<Action>,
    base_style: ElementStyle,
    cursor: usize,
    anchor: usize,
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
        value: value.into(),
        placeholder: String::new(),
        focused: false,
        on_click: None,
        base_style: ElementStyle::default(),
        cursor: 0,
        anchor: 0,
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

    pub fn cursor(mut self, offset: usize) -> Self {
        self.cursor = offset;
        self
    }

    pub fn anchor(mut self, offset: usize) -> Self {
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
        this.value = field.text().to_owned();
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

/// Describes a text input hit area for click-to-position cursor placement.
#[derive(Debug, Clone)]
pub struct TextInputHitArea {
    pub bounds: Rect,
    pub text_x: f32,
    pub text_y: f32,
    pub text_width: f32,
    pub text_height: f32,
    pub value: String,
    pub font_size: f32,
    pub focus_target: FocusId,
    pub multiline: bool,
    /// Painted layout of the displayed value (bullets when masked), with its
    /// origin at `(text_x - scroll_x, text_y - scroll_y)`. `None` for
    /// placeholders and compositions.
    pub layout: Option<Arc<TextLayout>>,
    /// How far the single-line text is scrolled left.
    pub scroll_x: f32,
    /// How far an editor's text is scrolled up.
    pub scroll_y: f32,
    /// Caret rect in window coordinates while focused, for placing the IME
    /// candidate window.
    pub caret: Option<Rect>,
}

impl TextInputHitArea {
    /// Byte offset into `value` for a pointer at window x `x`. A pointer
    /// past either edge maps to text scrolled out of view there, so a drag
    /// selection past the edge extends into it and the next frame scrolls
    /// it in. `None` when there is no value layout.
    pub fn offset_at(&self, x: f32) -> Option<usize> {
        let layout = self.layout.as_ref()?;
        let byte = layout.hit(x - self.text_x + self.scroll_x, self.text_height * 0.5);
        if layout.text().as_ref() == self.value {
            return Some(byte);
        }
        // Masked: the layout shows one bullet per char of the value.
        let chars = layout.text()[..byte].chars().count();
        Some(
            self.value
                .char_indices()
                .nth(chars)
                .map_or(self.value.len(), |(i, _)| i),
        )
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
            .map(|preedit| compose(&self.value, (self.anchor, self.cursor), preedit));
        let is_placeholder = self.value.is_empty() && composition.is_none();
        let byte_to_measure_byte = |offset: usize| -> usize {
            if !self.masked {
                return offset.min(self.value.len());
            }
            let char_index = self.value[..offset.min(self.value.len())].chars().count();
            char_index * "\u{2022}".len()
        };

        let display = if let Some(composition) = &composition {
            composition.text.clone()
        } else if is_placeholder {
            std::mem::take(&mut self.placeholder)
        } else if self.masked {
            "\u{2022}".repeat(self.value.chars().count())
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
        let caret_x = |byte: usize| value_layout.map_or(0.0, |l| l.caret(byte).x);

        // Caret in layout coordinates; `None` when the IME hides it.
        let caret_byte = match &composition {
            Some(composition) => composition.caret,
            None => Some(byte_to_measure_byte(self.cursor)),
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
            let sel_start = self.cursor.min(self.anchor);
            let sel_end = self.cursor.max(self.anchor);
            if sel_start != sel_end && sel_end <= self.value.len() {
                let x_start = caret_x(byte_to_measure_byte(sel_start));
                let x_end = caret_x(byte_to_measure_byte(sel_end));
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
            let value = std::mem::take(&mut self.value);
            let role = if self.masked {
                AccessibilityRole::PasswordInput
            } else if self.search {
                AccessibilityRole::SearchInput
            } else {
                AccessibilityRole::TextInput
            };
            let accessible_value = if self.masked {
                "\u{2022}".repeat(value.chars().count())
            } else {
                value.clone()
            };
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
                    .action(AccessibilityAction::TextValue(target)),
                index,
            );
            cx.text_input_hit_areas.push(TextInputHitArea {
                bounds,
                text_x,
                text_y,
                text_width: text_area_w,
                text_height: value_lh,
                value,
                font_size: value_size,
                focus_target: target,
                multiline: false,
                layout: value_layout.filter(|_| composition.is_none()).cloned(),
                scroll_x,
                scroll_y: 0.0,
                caret,
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
            let mut cx = ElementContext::new(
                &self.theme,
                1.0,
                &mut self.text,
                &mut self.layouts,
                None,
                &self.signals,
            )
            .with_focus(Some(FIELD));
            let mut root = text_input("Name", "")
                .field(field)
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
        let (left, right) = (area.text_x, area.text_x + area.text_width);
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
        assert_eq!(area.scroll_x, 0.0);
        assert_caret_visible(&area);
    }

    #[test]
    fn dragging_past_the_right_edge_selects_hidden_text_and_scrolls_to_it() {
        let mut painter = Painter::new();
        let mut field = TextField::new("the quick brown fox jumps over the lazy dog");
        field.apply(CursorHome);
        let area = painter.paint(&field);
        let layout = area.layout.clone().expect("value layout");
        let past = area
            .offset_at(area.text_x + area.text_width + 12.0)
            .expect("offset");
        assert!(layout.caret(past).x > area.text_width, "offset was hidden");

        field.apply(ExtendTextSelection(past));
        let area = painter.paint(&field);
        assert_eq!(field.selection_range(), Some((0, past)));
        assert!(area.scroll_x > 0.0);
        assert_caret_visible(&area);
    }
}
