use std::sync::Arc;

use super::editor::syntax_layout_spans;
use super::{Editor, EditorMode, SelectionRect, SyntaxSpan, SyntaxTokenKind};
use crate::FocusId;
use crate::accessibility::{AccessibilityAction, AccessibilityNode};
use crate::design::{Alpha, Sz};
use crate::element::*;
use crate::style::{ElementStyle, Styled};
use quark::{SemanticActions, SemanticNode, SemanticRole};
use quark_render::scene::{FontKind, Rect, RichTextPrimitive, ShapedText, TextPrimitive};
use quark_render::{RectPrimitive, RoundedRectPrimitive, Scene};
use quark_text::{TextLayout, TextParams, TextStyle};

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
    /// The editor's own layout; painted as is so caret and selection math
    /// match the glyphs. Without one the element lays `text` out itself.
    layout: Option<Arc<TextLayout>>,
    span_kinds: Vec<SyntaxTokenKind>,
    preedit_rects: Vec<SelectionRect>,
    clause_rects: Vec<SelectionRect>,
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
        layout: None,
        span_kinds: Vec::new(),
        preedit_rects: Vec::new(),
        clause_rects: Vec::new(),
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
        self.cursor = editor.caret_visible().then_some(CursorSnapshot {
            x: editor.cursor_pos.x,
            y: editor.cursor_pos.y,
            moved_at_ms: editor.cursor_moved_at_ms,
        });
        self.layout = editor.paint_layout();
        self.span_kinds = editor.paint_span_kinds();
        (self.preedit_rects, self.clause_rects) = editor.preedit_rects();
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

        scene.clip(bounds);

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
            let (layout, kinds) = match self.layout.take() {
                Some(layout) => (Some(layout), std::mem::take(&mut self.span_kinds)),
                None => {
                    let (spans, kinds) = syntax_layout_spans(&self.text, &self.syntax_spans);
                    let params = TextParams::new(self.text.clone(), style)
                        .spans(spans)
                        .wrap_width(Some(text_area_w.max(1.0)));
                    (cx.layout_text(&params), kinds)
                }
            };
            let span_colors: Vec<_> = kinds
                .iter()
                .map(|kind| syntax_color(*kind, self.text_color, theme))
                .collect();
            if let Some(layout) = layout {
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
            // IME composition: thin underline under the preedit, thick under
            // the clause the IME is converting.
            let thin = theme.metrics.ui_scale().max(1.0);
            for (rects, thickness) in [
                (&self.preedit_rects, thin),
                (&self.clause_rects, thin * 2.0),
            ] {
                for rect in rects {
                    scene.rect(RectPrimitive {
                        rect: Rect {
                            x: text_x + rect.x,
                            y: text_y - self.scroll_y + rect.y + rect.h - thickness,
                            width: rect.w,
                            height: thickness,
                        },
                        color: self.text_color,
                    });
                }
            }
        }

        let caret = self
            .cursor
            .as_ref()
            .filter(|_| self.focused)
            .map(|cur| Rect {
                x: text_x + cur.x,
                y: text_y - self.scroll_y + cur.y + 1.0,
                width: Sz::CURSOR_WIDTH,
                height: line_height - Sz::CURSOR_WIDTH,
            });
        if let (Some(rect), Some(cur)) = (caret, &self.cursor) {
            let elapsed = cx.clock_ms.saturating_sub(cur.moved_at_ms);
            if elapsed < 530 || (elapsed / 530).is_multiple_of(2) {
                scene.rounded_rect(RoundedRectPrimitive::uniform(rect, 1.0, theme.colors.text));
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
                bounds,
            )
            .label(accessibility_label)
            .action(AccessibilityAction::Focus(target)),
        );
        cx.text_input_hit_areas.push(TextInputHitArea {
            bounds,
            text_x,
            text_y,
            text_width: text_area_w,
            text_height: bounds.height,
            value: String::new(),
            font_size,
            focus_target: target,
            multiline: true,
            layout: None,
            scroll_x: 0.0,
            caret,
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

fn syntax_color(
    syntax_kind: SyntaxTokenKind,
    default_color: crate::theme::Color,
    theme: &crate::theme::Theme,
) -> crate::theme::Color {
    use SyntaxTokenKind::*;
    match syntax_kind {
        Keyword | Builtin => theme.colors.syntax_keyword,
        String => theme.colors.syntax_string,
        Comment | Label | Preprocessor => theme.colors.syntax_comment,
        Function => theme.colors.syntax_function,
        Number | Constant => theme.colors.syntax_number,
        Type | Namespace | Tag => theme.colors.syntax_type,
        Attribute | Property => theme.colors.syntax_property,
        Operator | Punctuation => theme.colors.syntax_operator,
        Variable | Normal => default_color,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Action;
    use crate::text_input::TextEditCommand::*;
    use crate::theme::Theme;
    use quark::reactive::SignalStore;
    use quark_render::Primitive;

    #[test]
    fn caret_and_selection_sit_on_the_painted_glyphs_after_bold_tokens() {
        let mut text = quark_text::TextSystem::vendored_only(&Default::default());
        let mut editor = Editor::new(EditorMode::CodeInput);
        // Keywords are painted semibold, which used to shift the caret.
        editor.set_syntax_highlighter(Arc::new(|text: &str| {
            text.match_indices("fn")
                .map(|(i, _)| SyntaxSpan {
                    offset: i as u32,
                    length: 2,
                    kind: SyntaxTokenKind::Keyword,
                })
                .collect()
        }));
        editor.sync_size(400.0, 200.0);
        editor.set_text("fn main() { fn inner() {} }\nlet s = \"日本\";");
        editor.apply(SetTextCursor(5));
        editor.apply(ExtendTextSelection(editor.byte_len() - 2));
        editor.flush(&mut text);

        let theme = Theme::default_dark();
        let signals = SignalStore::new();
        let mut layouts = quark_text::LayoutCache::default();
        let mut cx = ElementContext::new(&theme, 1.0, &mut text, &mut layouts, None, &signals);
        let target = FocusId::from_key("test.editor");
        let mut root = text_editor_element(target, ScrollActionBuilder::new(Action::new))
            .editor_snapshot(&editor)
            .focused(true)
            .w(400.0)
            .h(200.0)
            .into_any();
        let mut scene = Scene::default();
        render_element(&mut root, &mut scene, &mut cx, 400.0, 200.0);

        let painted = scene
            .primitives
            .iter()
            .find_map(|p| match p {
                Primitive::RichTextRun(run) => run.layout.downcast_ref::<TextLayout>(),
                _ => None,
            })
            .expect("painted editor text");
        let caret = painted.caret(editor.cursor());
        assert_eq!(
            (caret.x, caret.y),
            (editor.cursor_pos.x, editor.cursor_pos.y)
        );
        let painted_rects: Vec<_> = painted
            .selection_rects(editor.anchor()..editor.cursor())
            .map(|r| (r.x, r.y, r.width, r.height))
            .collect();
        let editor_rects: Vec<_> = editor
            .selection_rects()
            .iter()
            .map(|r| (r.x, r.y, r.w, r.h))
            .collect();
        assert_eq!(editor_rects, painted_rects);
    }
}
