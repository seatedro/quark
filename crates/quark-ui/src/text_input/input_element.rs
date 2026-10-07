use std::sync::Arc;

use super::{Editor, EditorMode, SelectionRect, SyntaxSpan, SyntaxTokenKind};
use crate::FocusId;
use crate::accessibility::{AccessibilityAction, AccessibilityNode};
use crate::design::{Alpha, Sz};
use crate::element::*;
use crate::style::{ElementStyle, Styled};
use quark::{SemanticActions, SemanticNode, SemanticRole};
use quark_render::scene::{
    FontKind, FontStyle, FontWeight, Rect, RichTextPrimitive, ShapedText, TextPrimitive,
};
use quark_render::{RectPrimitive, RoundedRectPrimitive, Scene};
use quark_text::{TextParams, TextSpan, TextStyle};

pub struct CursorSnapshot {
    pub x: f32,
    pub y: f32,
    pub moved_at_ms: u64,
}

pub struct TextEditorElement {
    is_empty: bool,
    placeholder: String,
    focused: bool,
    cursor: Option<CursorSnapshot>,
    selection_rects: Vec<SelectionRect>,
    content_height: f32,
    scroll_y: f32,
    font_size: f32,
    text_color: crate::theme::Color,
    mode: EditorMode,
    text: Arc<str>,
    syntax_spans: Vec<SyntaxSpan>,
    line_tops: Vec<(usize, f32)>,
    focus_target: FocusId,
    on_scroll: ScrollActionBuilder,
    base_style: ElementStyle,
}

/// Multiline editor surface for an [`Editor`]. `focus_target` identifies it
/// for clicks and accessibility focus; `on_scroll` turns wheel input over it
/// into app actions.
pub fn text_editor_element(
    focus_target: FocusId,
    on_scroll: ScrollActionBuilder,
) -> TextEditorElement {
    TextEditorElement {
        is_empty: true,
        placeholder: String::new(),
        focused: false,
        cursor: None,
        selection_rects: Vec::new(),
        content_height: 0.0,
        scroll_y: 0.0,
        font_size: 14.0,
        text_color: crate::theme::Color::rgba(255, 255, 255, 255),
        mode: EditorMode::ProseInput,
        text: Arc::from(""),
        syntax_spans: Vec::new(),
        line_tops: Vec::new(),
        focus_target,
        on_scroll,
        base_style: ElementStyle::default(),
    }
}

impl TextEditorElement {
    pub fn placeholder(mut self, p: impl Into<String>) -> Self {
        self.placeholder = p.into();
        self
    }

    pub fn focused(mut self, f: bool) -> Self {
        self.focused = f;
        self
    }

    pub fn is_empty(mut self, empty: bool) -> Self {
        self.is_empty = empty;
        self
    }

    pub fn cursor(mut self, snap: CursorSnapshot) -> Self {
        self.cursor = Some(snap);
        self
    }

    pub fn selection(mut self, rects: Vec<SelectionRect>) -> Self {
        self.selection_rects = rects;
        self
    }

    pub fn content_height(mut self, h: f32) -> Self {
        self.content_height = h;
        self
    }

    pub fn scroll_y(mut self, offset: f32) -> Self {
        self.scroll_y = offset;
        self
    }

    pub fn font_size(mut self, size: f32) -> Self {
        self.font_size = size;
        self
    }

    pub fn text_color(mut self, color: crate::theme::Color) -> Self {
        self.text_color = color;
        self
    }

    pub fn mode(mut self, mode: EditorMode) -> Self {
        self.mode = mode;
        self
    }

    pub fn text(mut self, text: Arc<str>) -> Self {
        self.text = text;
        self
    }

    pub fn syntax_spans(mut self, spans: &[SyntaxSpan]) -> Self {
        self.syntax_spans.clear();
        self.syntax_spans.extend_from_slice(spans);
        self
    }

    pub fn line_tops(mut self, line_tops: Vec<(usize, f32)>) -> Self {
        self.line_tops = line_tops;
        self
    }

    pub fn editor_snapshot(mut self, editor: &Editor) -> Self {
        self.is_empty = editor.is_empty();
        self.cursor = Some(CursorSnapshot {
            x: editor.cursor_pos.x,
            y: editor.cursor_pos.y,
            moved_at_ms: editor.cursor_moved_at_ms,
        });
        self.selection_rects = editor.selection_rects();
        self.content_height = editor.content_height();
        self.scroll_y = editor.scroll_y;
        self.mode = editor.mode();
        self.text = editor.text_arc();
        self.syntax_spans = editor.syntax_spans().to_vec();
        self.line_tops = editor.logical_line_tops();
        self
    }

    pub fn focus_target(mut self, target: FocusId) -> Self {
        self.focus_target = target;
        self
    }
}

impl Styled for TextEditorElement {
    fn element_style_mut(&mut self) -> &mut ElementStyle {
        &mut self.base_style
    }
}

impl Element for TextEditorElement {
    type LayoutState = ();
    type PrepaintState = HitId;

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
    ) -> HitId {
        cx.insert_hit(bounds, HitFlags::TEXT | HitFlags::SCROLL, CursorHint::Text)
    }

    fn paint(
        &mut self,
        bounds: Bounds,
        _layout_state: &mut (),
        prepaint_state: &mut HitId,
        _engine: &LayoutEngine,
        scene: &mut Scene,
        cx: &mut ElementContext,
    ) {
        let theme = cx.theme;
        let accessibility_label = if self.placeholder.is_empty() {
            format!("{:?}", self.focus_target)
        } else {
            self.placeholder.clone()
        };
        let font_size = self.font_size;
        let line_height = font_size * 1.35;
        let gutter_w = if self.mode.is_code() {
            code_gutter_width(
                font_size,
                self.line_tops.last().map(|(line, _)| *line).unwrap_or(0),
            )
            .min((bounds.width * 0.35).max(0.0))
        } else {
            0.0
        };
        let text_area_w = (bounds.width - gutter_w).max(0.0);
        let text_x = bounds.x + gutter_w;
        let text_y = bounds.y;
        let font_kind = if self.mode.is_code() {
            FontKind::Mono
        } else {
            FontKind::Ui
        };

        scene.clip(bounds.into());

        if gutter_w > 0.0 {
            scene.rect(RectPrimitive {
                rect: Rect {
                    x: bounds.x,
                    y: bounds.y,
                    width: gutter_w,
                    height: bounds.height,
                },
                color: theme.colors.editor_surface,
            });
            scene.rect(RectPrimitive {
                rect: Rect {
                    x: text_x - 1.0,
                    y: bounds.y,
                    width: 1.0,
                    height: bounds.height,
                },
                color: theme.colors.border_soft,
            });
            let gutter_digits =
                gutter_digits(self.line_tops.last().map(|(line, _)| *line).unwrap_or(0));
            for (line_no, y) in &self.line_tops {
                let painted_y = text_y - self.scroll_y + *y;
                if painted_y + line_height < bounds.y || painted_y > bounds.bottom() {
                    continue;
                }
                let style = TextStyle::new(font_size)
                    .kind(FontKind::Mono)
                    .line_height(line_height);
                let number = format!("{line_no:>gutter_digits$}");
                let Some(layout) = cx.layout_text(&TextParams::new(number, style)) else {
                    continue;
                };
                scene.text(TextPrimitive {
                    rect: Rect {
                        x: bounds.x,
                        y: painted_y,
                        width: (gutter_w - 8.0).max(1.0),
                        height: line_height,
                    },
                    layout: ShapedText::new(layout),
                    color: theme.colors.gutter_text,
                });
            }
        }

        if self.focused && !self.is_empty {
            let sel_color = theme.colors.accent.with_alpha(Alpha::SOFT);
            for rect in &self.selection_rects {
                scene.rounded_rect(RoundedRectPrimitive::uniform(
                    Rect {
                        x: text_x + rect.x,
                        y: text_y - self.scroll_y + rect.y,
                        width: rect.w.min(text_area_w),
                        height: rect.h,
                    },
                    2.0,
                    sel_color,
                ));
            }
        }

        let style = TextStyle::new(font_size)
            .kind(font_kind)
            .line_height(line_height);
        if self.is_empty {
            let placeholder_color = theme.colors.text_muted.with_alpha(Alpha::PLACEHOLDER);
            let params = TextParams::new(std::mem::take(&mut self.placeholder), style);
            if let Some(layout) = cx.layout_text(&params) {
                scene.text(TextPrimitive {
                    rect: Rect {
                        x: text_x,
                        y: text_y,
                        width: text_area_w,
                        height: line_height,
                    },
                    layout: ShapedText::new(layout),
                    color: placeholder_color,
                });
            }
        } else {
            let content_h = self.content_height.max(line_height);
            let (spans, span_colors) = build_editor_spans(
                self.text.as_ref(),
                &self.syntax_spans,
                self.text_color,
                theme,
            );
            // Wraps at the same width the editor's own buffer wraps at, so its
            // caret and selection rects line up with the painted lines.
            let params = TextParams::new(self.text.clone(), style)
                .spans(spans)
                .wrap_width(Some(text_area_w.max(1.0)));
            if let Some(layout) = cx.layout_text(&params) {
                scene.rich_text(RichTextPrimitive {
                    rect: Rect {
                        x: text_x,
                        y: text_y - self.scroll_y,
                        width: text_area_w,
                        height: content_h,
                    },
                    layout: ShapedText::new(layout),
                    default_color: self.text_color,
                    span_colors: span_colors.into(),
                });
            }
        }

        if self.focused {
            if let Some(ref cur) = self.cursor {
                let elapsed = cx.clock_ms.saturating_sub(cur.moved_at_ms);
                let visible = elapsed < 530 || (elapsed / 530) % 2 == 0;
                if visible {
                    scene.rounded_rect(RoundedRectPrimitive::uniform(
                        Rect {
                            x: text_x + cur.x,
                            y: text_y - self.scroll_y + cur.y + 1.0,
                            width: Sz::CURSOR_WIDTH,
                            height: line_height - Sz::CURSOR_WIDTH,
                        },
                        1.0,
                        theme.colors.text,
                    ));
                }
            }
        }

        scene.pop_clip();

        let target = self.focus_target;
        let mut semantic_node = SemanticNode::new(bounds)
            .id(format!("text-editor:{target:?}"))
            .role(SemanticRole::TextInput);
        semantic_node.parent = cx.current_semantic_parent();
        semantic_node.actions = SemanticActions::default().text_value().scrollable();
        semantic_node.focus = Some(target);
        let node = cx.semantic.push(semantic_node);
        cx.bind_hit(*prepaint_state, node);
        cx.handlers.on_scroll(
            node,
            ScrollTarget {
                builder: self.on_scroll.clone(),
                offset: self.scroll_y,
                max: Some((self.content_height - bounds.height).max(0.0)),
            },
        );
        cx.push_accessibility(
            AccessibilityNode::new(
                format!("text-editor:{target:?}"),
                accesskit::Role::MultilineTextInput,
                bounds.into(),
            )
            .label(accessibility_label)
            .action(AccessibilityAction::Focus(target)),
        );
        cx.text_input_hit_areas.push(TextInputHitArea {
            bounds: bounds.into(),
            text_x,
            text_y,
            text_width: text_area_w,
            text_height: bounds.height,
            value: String::new(),
            font_size,
            focus_target: target,
            multiline: true,
            layout: None,
        });
    }
}

impl IntoAnyElement for TextEditorElement {
    fn into_any(self) -> AnyElement {
        AnyElement::new(self)
    }
}

fn code_gutter_width(font_size: f32, max_line: usize) -> f32 {
    let digits = gutter_digits(max_line);
    let char_w = (font_size * 0.62).max(1.0);
    (digits as f32 * char_w + 18.0).ceil()
}

fn gutter_digits(max_line: usize) -> usize {
    max_line.max(1).ilog10() as usize + 1
}

/// Layout spans and their colors for the syntax-highlighted runs; text outside
/// every run uses the base style and `default_color`.
fn build_editor_spans(
    text: &str,
    syntax_spans: &[SyntaxSpan],
    default_color: crate::theme::Color,
    theme: &crate::theme::Theme,
) -> (Vec<TextSpan>, Vec<crate::theme::Color>) {
    let mut spans = Vec::with_capacity(syntax_spans.len());
    let mut colors = Vec::with_capacity(syntax_spans.len());
    for span in syntax_spans {
        let raw_start = span.offset as usize;
        let raw_end = raw_start
            .saturating_add(span.length as usize)
            .min(text.len());
        let Some((start, end)) = valid_text_range(text, raw_start, raw_end) else {
            continue;
        };
        let (color, weight, style) = syntax_style(span.kind, default_color, theme);
        spans.push(TextSpan {
            range: start..end,
            weight,
            style,
            kind: None,
        });
        colors.push(color);
    }
    (spans, colors)
}

fn valid_text_range(text: &str, start: usize, end: usize) -> Option<(usize, usize)> {
    if start >= end || end > text.len() {
        return None;
    }
    if text.is_char_boundary(start) && text.is_char_boundary(end) {
        Some((start, end))
    } else {
        None
    }
}

fn syntax_style(
    syntax_kind: SyntaxTokenKind,
    default_color: crate::theme::Color,
    theme: &crate::theme::Theme,
) -> (crate::theme::Color, Option<FontWeight>, Option<FontStyle>) {
    use SyntaxTokenKind::*;
    let color = match syntax_kind {
        Keyword | Builtin => theme.colors.syntax_keyword,
        String => theme.colors.syntax_string,
        Comment | Label | Preprocessor => theme.colors.syntax_comment,
        Function => theme.colors.syntax_function,
        Number | Constant => theme.colors.syntax_number,
        Type | Namespace | Tag => theme.colors.syntax_type,
        Attribute | Property => theme.colors.syntax_property,
        Operator | Punctuation => theme.colors.syntax_operator,
        Variable | Normal => default_color,
    };
    let (font_weight, font_style) = match syntax_kind {
        Comment => (None, Some(FontStyle::Italic)),
        Keyword | Builtin => (Some(FontWeight::Semibold), None),
        Type | Function | Constant | Attribute | Tag | Property | Namespace | Label
        | Preprocessor => (Some(FontWeight::Medium), None),
        Normal | String | Number | Operator | Punctuation | Variable => (None, None),
    };
    (color, font_weight, font_style)
}
