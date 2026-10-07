use super::*;

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
    /// origin at `(text_x, text_y)`. `None` for placeholders and multiline
    /// editors.
    pub layout: Option<Arc<TextLayout>>,
}

impl Element for TextInput {
    type LayoutState = ();
    type PrepaintState = Option<HitboxId>;

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
    ) -> Option<HitboxId> {
        if self.on_click.is_some() || self.focus_target.is_some() {
            Some(cx.insert_hitbox(bounds, HitboxBehavior::Normal))
        } else {
            None
        }
    }

    fn paint(
        &mut self,
        bounds: Bounds,
        _layout_state: &mut (),
        _prepaint_state: &mut Option<HitboxId>,
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

        let is_placeholder = self.value.is_empty();
        let bullet_str = if self.masked {
            let chars = self.value.chars().count();
            "\u{2022}".repeat(chars)
        } else {
            String::new()
        };
        let byte_to_measure_byte = |offset: usize| -> usize {
            if !self.masked {
                return offset.min(self.value.len());
            }
            let char_index = self.value[..offset.min(self.value.len())].chars().count();
            char_index * "\u{2022}".len()
        };

        let display = if is_placeholder {
            std::mem::take(&mut self.placeholder)
        } else if self.masked {
            bullet_str
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

        // Selection highlight (render before text so it appears behind)
        if self.focused && !is_placeholder {
            let sel_start = self.cursor.min(self.anchor);
            let sel_end = self.cursor.max(self.anchor);
            if sel_start != sel_end && sel_end <= self.value.len() {
                let caret_x = |byte| value_layout.map_or(0.0, |l| l.caret(byte).x);
                let x_start = caret_x(byte_to_measure_byte(sel_start));
                let x_end = caret_x(byte_to_measure_byte(sel_end));
                scene.rounded_rect(RoundedRectPrimitive::uniform(
                    Rect {
                        x: text_x + x_start,
                        y: text_y,
                        width: (x_end - x_start).min(text_area_w),
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
                    x: text_x,
                    y: text_y,
                    width: text_area_w,
                    height: value_lh,
                },
                layout: ShapedText::new(layout.clone()),
                color: text_color,
            });
        }

        // Cursor caret
        if self.focused && !is_placeholder || (self.focused && self.value.is_empty()) {
            let elapsed = cx.clock_ms.saturating_sub(self.cursor_moved_at_ms);
            let cursor_visible = elapsed < 530 || (elapsed / 530) % 2 == 0;
            if cursor_visible {
                let cursor_x =
                    value_layout.map_or(0.0, |l| l.caret(byte_to_measure_byte(self.cursor)).x);
                scene.rounded_rect(RoundedRectPrimitive::uniform(
                    Rect {
                        x: text_x + cursor_x,
                        y: text_y + 1.0,
                        width: Sz::CURSOR_WIDTH,
                        height: value_lh - Sz::CURSOR_WIDTH,
                    },
                    1.0,
                    theme.colors.text,
                ));
            }
        }

        if let Some(action) = self.on_click.take() {
            cx.hits.push(HitRegion::new(
                bounds,
                CursorHint::Text,
                ClickHandler::from_action(action),
            ));
        }

        // Register hit area for click-to-position (stored in cx for app.rs to use)
        if let Some(target) = self.focus_target {
            let value = std::mem::take(&mut self.value);
            let role = if self.masked {
                AccessibilityRole::PasswordInput
            } else {
                if self.search {
                    AccessibilityRole::SearchInput
                } else {
                    AccessibilityRole::TextInput
                }
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
            semantic_node.focus = Some(target);
            semantic_node.state = SemanticNodeState {
                style_state,
                ..SemanticNodeState::default()
            };
            let semantic_index = cx.semantic.push(semantic_node);
            cx.push_accessibility_for_semantic(
                AccessibilityNode::new(semantic_id, role, bounds)
                    .label(accessibility_label)
                    .value(accessible_value)
                    .action(AccessibilityAction::TextValue(target)),
                semantic_index,
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
                layout: value_layout.cloned(),
            });
        }
    }
}

impl IntoAnyElement for TextInput {
    fn into_any(self) -> AnyElement {
        element_into_any(self)
    }
}
