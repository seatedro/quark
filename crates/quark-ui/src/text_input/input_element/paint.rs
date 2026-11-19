//! The editor element's paint, one layer per function: gutter, the boxes
//! behind the text, the text, the caret, then the semantic and
//! accessibility nodes.

use std::sync::Arc;

use quark::{SemanticActions, SemanticNode, SemanticRole};
use quark_render::scene::{FontKind, Rect, RichTextPrimitive, ShapedText, TextPrimitive};
use quark_render::{RectPrimitive, RoundedRectPrimitive, Scene};
use quark_text::{TextLayout, TextParams, TextStyle};

use super::TextEditorElement;
use crate::accessibility::{AccessibilityAction, AccessibilityNode, AccessibleText};
use crate::design::{Alpha, Sz};
use crate::element::*;
use crate::text_input::editor::{gutter_digits, syntax_layout_spans};
use crate::text_input::text_pointer_drag;
use crate::text_input::view::caret_blink;
use crate::text_input::{SelectionRect, SyntaxTokenKind, TextDecoration};

/// Where the text goes: right of the gutter, scrolled up by `scroll_y`.
pub(super) struct TextArea {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub gutter: f32,
    /// Where content y 0 is painted.
    pub origin_y: f32,
    pub bounds: Bounds,
}

impl TextArea {
    /// A content-coordinate rect, painted.
    fn place(&self, rect: &SelectionRect) -> Rect {
        Rect {
            x: self.x + rect.x,
            y: self.origin_y + rect.y,
            width: rect.w,
            height: rect.h,
        }
    }
}

impl TextEditorElement {
    pub(super) fn text_area(&self, bounds: Bounds) -> TextArea {
        let gutter = self.gutter_width_in(bounds.width);
        TextArea {
            x: bounds.x + gutter,
            y: bounds.y,
            width: (bounds.width - gutter).max(0.0),
            gutter,
            origin_y: bounds.y - self.scroll_y,
            bounds,
        }
    }

    fn style(&self) -> TextStyle {
        let kind = if self.mode.is_code() {
            FontKind::Mono
        } else {
            FontKind::Ui
        };
        TextStyle::new(self.font_size)
            .kind(kind)
            .line_height(self.line_height())
    }

    /// The gutter's background, its edge, and the numbers of the lines in
    /// view.
    pub(super) fn paint_gutter(
        &self,
        scene: &mut Scene,
        cx: &mut ElementContext,
        bounds: Bounds,
        area: &TextArea,
    ) {
        if area.gutter <= 0.0 {
            return;
        }
        let theme = cx.theme;
        scene.rect(RectPrimitive {
            rect: Rect {
                x: bounds.x,
                y: bounds.y,
                width: area.gutter,
                height: bounds.height,
            },
            color: theme.colors.editor_surface,
        });
        scene.rect(RectPrimitive {
            rect: Rect {
                x: area.x - 1.0,
                y: bounds.y,
                width: 1.0,
                height: bounds.height,
            },
            color: theme.colors.border_soft,
        });
        let line_height = self.line_height();
        let lines = self.line_tops.last().map_or(0, |(line, _)| *line);
        let digits = match &self.editor {
            Some(editor) => gutter_digits(editor.text.rope().line_count()),
            None => gutter_digits(lines),
        };
        let gutter_text = theme.colors.gutter_text;
        let style = TextStyle::new(self.font_size)
            .kind(FontKind::Mono)
            .line_height(line_height);
        for (line_no, y) in self.line_tops.iter() {
            let painted_y = area.origin_y + *y;
            if painted_y + line_height < bounds.y || painted_y > bounds.bottom() {
                continue;
            }
            let number = format!("{line_no:>digits$}");
            let Some(layout) = cx.layout_text(&TextParams::new(number, style)) else {
                continue;
            };
            scene.text(TextPrimitive {
                rect: Rect {
                    x: bounds.x,
                    y: painted_y,
                    width: (area.gutter - 8.0).max(1.0),
                    height: line_height,
                },
                layout: ShapedText::new(layout),
                color: gutter_text,
            });
        }
    }

    /// Atom pills, inline code boxes, and the selection, under the text.
    pub(super) fn paint_backgrounds(
        &self,
        scene: &mut Scene,
        cx: &ElementContext,
        area: &TextArea,
    ) {
        if self.is_empty {
            return;
        }
        let theme = cx.theme;
        let pill = theme.colors.accent.with_alpha(Alpha::DIM);
        for rect in &self.atom_rects {
            let at = area.place(rect);
            let rect = Rect {
                x: at.x - 2.0,
                y: at.y + 1.0,
                width: at.width + 4.0,
                height: at.height - 2.0,
            };
            scene.rounded_rect(RoundedRectPrimitive::uniform(rect, 4.0, pill));
        }
        let code_bg = theme.colors.element_background;
        let code = self
            .decorations
            .iter()
            .filter(|(kind, _)| *kind == TextDecoration::CodeBackground);
        for (_, rect) in code {
            let at = area.place(rect);
            let rect = Rect {
                x: at.x - 1.0,
                y: at.y + 1.0,
                width: at.width + 2.0,
                height: at.height - 2.0,
            };
            scene.rounded_rect(RoundedRectPrimitive::uniform(rect, 3.0, code_bg));
        }
        if self.focused {
            let selection = theme.colors.accent.with_alpha(Alpha::SOFT);
            for rect in &self.selection_rects {
                let mut at = area.place(rect);
                at.width = at.width.min(area.width);
                scene.rounded_rect(RoundedRectPrimitive::uniform(at, 2.0, selection));
            }
        }
    }

    /// The placeholder, or the text with its decorations and the IME's
    /// underlines. Returns how pointers map onto what was painted.
    pub(super) fn paint_text(
        &mut self,
        scene: &mut Scene,
        cx: &mut ElementContext,
        area: &TextArea,
    ) -> TextHitContent {
        let style = self.style();
        let line_height = self.line_height();
        if let Some(editor) = &self.editor {
            editor.frame_scale.set(cx.scale_factor);
            editor.frame_viewport.set(area.bounds.height);
        }
        if self.is_empty {
            let params = TextParams::new(std::mem::take(&mut self.placeholder), style);
            if let Some(layout) = cx.layout_text(&params) {
                scene.text(TextPrimitive {
                    rect: Rect {
                        x: area.x,
                        y: area.y,
                        width: area.width,
                        height: line_height,
                    },
                    layout: ShapedText::new(layout),
                    color: cx.theme.colors.placeholder,
                });
            }
            return TextHitContent::Multiline {
                layout: None,
                window: None,
                scroll_y: self.scroll_y,
            };
        }
        let hits = match &self.editor {
            Some(_) => self.paint_window(scene, cx, area),
            None => self.paint_own_layout(scene, cx, area, style),
        };
        let thin = cx.theme.metrics.ui_scale().max(1.0);
        paint_decorations(
            scene,
            &self.decorations,
            area,
            thin,
            self.text_color,
            cx.theme,
        );
        // IME composition: thin underline under the preedit, thick under
        // the clause the IME is converting.
        for (rects, thickness) in [
            (&self.preedit_rects, thin),
            (&self.clause_rects, thin * 2.0),
        ] {
            for rect in rects {
                let at = area.place(rect);
                scene.rect(RectPrimitive {
                    rect: Rect {
                        y: at.y + at.height - thickness,
                        height: thickness,
                        ..at
                    },
                    color: self.text_color,
                });
            }
        }
        hits
    }

    /// The editor's shaped lines in view, each where the editor placed it.
    fn paint_window(
        &self,
        scene: &mut Scene,
        cx: &mut ElementContext,
        area: &TextArea,
    ) -> TextHitContent {
        let Some(editor) = &self.editor else {
            return TextHitContent::Multiline {
                layout: None,
                window: None,
                scroll_y: self.scroll_y,
            };
        };
        let (top, bottom) = (self.scroll_y, self.scroll_y + area.bounds.height);
        let scale = cx.scale_factor;
        // Paint again right away so the editor shapes at this frame's
        // scale, or the part of the view it has not shaped yet, on its
        // next flush (both recorded above).
        let stale_scale = editor
            .window
            .lines()
            .first()
            .is_some_and(|l| l.layout.scale_factor() != scale);
        // (Never for an editor that has not flushed, which would loop.)
        let uncovered = !editor.window.lines().is_empty() && !editor.window.covers(top, bottom);
        if stale_scale || uncovered || editor.highlighting {
            cx.request_frame_at_ms(cx.clock_ms);
        }
        for line in editor.window.lines() {
            let height = line.layout.size().1;
            if line.top + height < top || line.top > bottom {
                continue;
            }
            // A layout shaped at another scale is shaped again here until
            // the editor catches up.
            let layout = if line.layout.scale_factor() == scale {
                Some(line.layout.clone())
            } else {
                cx.layout_text_query(&line.layout.query().scale_factor(scale))
            };
            let Some(layout) = layout else {
                continue;
            };
            self.paint_layout(scene, cx, area, layout, &line.kinds, line.top);
        }
        let window = (!editor.window.is_composing()).then(|| editor.window.clone());
        TextHitContent::Multiline {
            layout: None,
            window,
            scroll_y: self.scroll_y,
        }
    }

    /// Without an editor snapshot: `text` laid out here as one layout.
    fn paint_own_layout(
        &self,
        scene: &mut Scene,
        cx: &mut ElementContext,
        area: &TextArea,
        style: TextStyle,
    ) -> TextHitContent {
        let (spans, kinds) = syntax_layout_spans(&self.text, &self.syntax_spans);
        let params = TextParams::new(self.text.clone(), style)
            .spans(spans)
            .wrap_width(Some(area.width.max(1.0)));
        let layout = cx.layout_text(&params);
        if let Some(layout) = &layout {
            self.paint_layout(scene, cx, area, layout.clone(), &kinds, 0.0);
        }
        TextHitContent::Multiline {
            layout: (self.preedit_rects.is_empty()).then_some(layout).flatten(),
            window: None,
            scroll_y: self.scroll_y,
        }
    }

    fn paint_layout(
        &self,
        scene: &mut Scene,
        cx: &ElementContext,
        area: &TextArea,
        layout: Arc<TextLayout>,
        kinds: &[SyntaxTokenKind],
        top: f32,
    ) {
        let span_colors: Vec<_> = kinds
            .iter()
            .map(|kind| syntax_color(*kind, self.text_color, cx.theme))
            .collect();
        let height = layout.size().1.max(self.line_height());
        scene.rich_text(RichTextPrimitive {
            rect: Rect {
                x: area.x,
                y: area.origin_y + top,
                width: area.width,
                height,
            },
            layout: ShapedText::new(layout),
            default_color: self.text_color,
            span_colors: span_colors.into(),
        });
    }

    /// The caret, blinking; returns its rect while focused.
    pub(super) fn paint_caret(
        &self,
        scene: &mut Scene,
        cx: &mut ElementContext,
        area: &TextArea,
    ) -> Option<Rect> {
        let cursor = self.cursor.as_ref().filter(|_| self.focused)?;
        let rect = Rect {
            x: area.x + cursor.x,
            y: area.origin_y + cursor.y + 1.0,
            width: Sz::CURSOR_WIDTH,
            height: self.line_height() - Sz::CURSOR_WIDTH,
        };
        let (visible, next_toggle_ms) = caret_blink(cx.clock_ms, cursor.moved_at_ms);
        if visible {
            scene.rounded_rect(RoundedRectPrimitive::uniform(
                rect,
                1.0,
                cx.theme.colors.text,
            ));
        }
        cx.request_frame_at_ms(next_toggle_ms);
        Some(rect)
    }

    /// The semantic node with its drag and scroll handlers, and the
    /// accessibility node. The whole text goes to assistive tech only when
    /// it is listening.
    pub(super) fn publish(
        &self,
        cx: &mut ElementContext,
        bounds: Bounds,
        hit: HitId,
        label: String,
    ) {
        let target = self.focus_target;
        let key = crate::text_input::target_key("text-editor", target);
        let mut semantic_node = SemanticNode::new(bounds)
            .id(key.clone())
            .role(SemanticRole::TextInput);
        semantic_node.parent = cx.current_semantic_parent();
        semantic_node.actions = SemanticActions::default().text_value().scrollable();
        semantic_node.focus = Some(target);
        let node = cx.semantic.push(semantic_node);
        cx.bind_hit(hit, node);
        cx.handlers.on_drag(node, text_pointer_drag(target, None));
        cx.handlers.on_scroll(
            node,
            ScrollTarget {
                builder: self.on_scroll.clone(),
                offset: self.scroll_y,
                max: Some((self.content_height - bounds.height).max(0.0)),
            },
        );
        if !cx.accessibility_enabled() {
            return;
        }
        let value = match &self.editor {
            Some(editor) => editor.text.flat(),
            None => self.text.clone(),
        };
        let text = match self.text_selection {
            Some((anchor, caret)) => AccessibleText::new(value.clone()).selection(anchor, caret),
            None => AccessibleText::new(value.clone()),
        };
        cx.push_accessibility(
            AccessibilityNode::shared(key, accesskit::Role::MultilineTextInput, bounds)
                .label(label)
                .value(value)
                .text(text)
                .action(AccessibilityAction::EditorViewport {
                    focus: target,
                    scroll: self.on_scroll.clone(),
                }),
        );
    }
}

/// Underlines and strikethroughs in the text color (links in the accent),
/// and a wavy line in the error color under misspellings.
fn paint_decorations(
    scene: &mut Scene,
    decorations: &[(TextDecoration, SelectionRect)],
    area: &TextArea,
    thickness: f32,
    text_color: crate::theme::Color,
    theme: &crate::theme::Theme,
) {
    for (kind, rect) in decorations {
        let at = area.place(rect);
        let line = |scene: &mut Scene, y: f32| {
            scene.rect(RectPrimitive {
                rect: Rect {
                    x: at.x,
                    y,
                    width: at.width,
                    height: thickness,
                },
                color: text_color,
            });
        };
        match kind {
            TextDecoration::Underline => line(scene, at.y + at.height * 0.82),
            TextDecoration::Strikethrough => line(scene, at.y + at.height * 0.52),
            TextDecoration::CodeBackground => {}
            TextDecoration::Misspelled => {
                // A zigzag of short dashes alternating between two rows.
                let step = 2.0 * thickness;
                let base = at.y + at.height - 2.0 * thickness;
                let mut x = 0.0;
                let mut up = false;
                while x < at.width {
                    scene.rect(RectPrimitive {
                        rect: Rect {
                            x: at.x + x,
                            y: if up { base - thickness } else { base },
                            width: step.min(at.width - x),
                            height: thickness,
                        },
                        color: theme.colors.status_error,
                    });
                    x += step;
                    up = !up;
                }
            }
        }
    }
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
        Link => theme.colors.text_accent,
        Variable | Normal => default_color,
    }
}
