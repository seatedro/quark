use std::sync::Arc;

use super::anchor::{CaretAnchor, CaretGeometry};
use super::editor::{gutter_digits, gutter_width_in, syntax_layout_spans};
use super::text_pointer_drag;
use super::view::{FrameScale, caret_blink};
use super::{Editor, EditorMode, SelectionRect, SyntaxSpan, SyntaxTokenKind, TextDecoration};
use crate::FocusId;
use crate::accessibility::{AccessibilityAction, AccessibilityNode, AccessibleText};
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
    /// The accessible name; the placeholder when empty.
    label: String,
    focused: bool,
    cursor: Option<CursorSnapshot>,
    selection_rects: Vec<SelectionRect>,
    /// Pills behind atom labels.
    atom_rects: Vec<SelectionRect>,
    /// Formatting lines and boxes, and spelling squiggles.
    decorations: Vec<(TextDecoration, SelectionRect)>,
    content_height: f32,
    scroll_y: f32,
    font_size: f32,
    /// The snapshotted editor's line height; else 1.35 times the font size.
    line_height: Option<f32>,
    text_color: crate::theme::Color,
    mode: EditorMode,
    text: Arc<str>,
    /// `(anchor, caret)` byte offsets into `text`, for assistive tech.
    text_selection: Option<(usize, usize)>,
    syntax_spans: Arc<[SyntaxSpan]>,
    /// The editor's own layout; painted as is so caret and selection math
    /// match the glyphs. Without one the element lays `text` out itself.
    layout: Option<Arc<TextLayout>>,
    span_kinds: Arc<[SyntaxTokenKind]>,
    preedit_rects: Vec<SelectionRect>,
    clause_rects: Vec<SelectionRect>,
    line_tops: Arc<[(usize, f32)]>,
    /// The snapshotted editor's gutter, which its layout wraps beside.
    gutter_width: Option<f32>,
    focus_target: FocusId,
    on_scroll: ScrollActionBuilder,
    /// The snapshotted editor's frame scale, set during paint.
    frame_scale: Option<FrameScale>,
    /// The snapshotted editor's caret anchor, set during prepaint.
    caret_anchor: Option<CaretAnchor>,
    /// The caret in layout coordinates even while hidden, for the anchor.
    caret_at: (f32, f32),
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
        label: String::new(),
        focused: false,
        cursor: None,
        selection_rects: Vec::new(),
        atom_rects: Vec::new(),
        decorations: Vec::new(),
        content_height: 0.0,
        scroll_y: 0.0,
        font_size: 14.0,
        line_height: None,
        text_color: crate::theme::Color::rgba(255, 255, 255, 255),
        mode: EditorMode::ProseInput,
        text: Arc::from(""),
        text_selection: None,
        syntax_spans: Arc::from([]),
        layout: None,
        span_kinds: Arc::from([]),
        preedit_rects: Vec::new(),
        clause_rects: Vec::new(),
        line_tops: Arc::from([]),
        gutter_width: None,
        focus_target,
        on_scroll,
        frame_scale: None,
        caret_anchor: None,
        caret_at: (0.0, 0.0),
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

    /// The name assistive tech reads for the editor, when it should not be
    /// the placeholder (which disappears once the user types).
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
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

    pub fn syntax_spans(mut self, spans: impl Into<Arc<[SyntaxSpan]>>) -> Self {
        self.syntax_spans = spans.into();
        self
    }

    pub fn line_tops(mut self, line_tops: impl Into<Arc<[(usize, f32)]>>) -> Self {
        self.line_tops = line_tops.into();
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
        self.atom_rects = editor.atom_rects();
        self.decorations = editor.decoration_rects();
        self.content_height = editor.content_height();
        self.scroll_y = editor.scroll_y;
        self.mode = editor.mode();
        // Shared, not copied: this runs every frame.
        self.text = editor.text_arc();
        self.text_selection = Some((editor.anchor().get(), editor.cursor().get()));
        self.syntax_spans = editor.syntax_spans().clone();
        self.line_tops = editor.logical_line_tops().clone();
        self.gutter_width = Some(editor.gutter_width());
        self.frame_scale = Some(editor.frame_scale.clone());
        self.line_height = Some(editor.scroll_line_height_px());
        self.caret_anchor = Some(editor.caret_anchor().clone());
        self.caret_at = (editor.cursor_pos.x, editor.cursor_pos.y);
        self
    }

    pub fn focus_target(mut self, target: FocusId) -> Self {
        self.focus_target = target;
        self
    }
}

impl TextEditorElement {
    fn line_height(&self) -> f32 {
        self.line_height.unwrap_or(self.font_size * 1.35)
    }

    /// The line-number gutter's width in a box `width` wide.
    fn gutter_width_in(&self, width: f32) -> f32 {
        let lines = self.line_tops.last().map_or(0, |(line, _)| *line);
        match self.gutter_width {
            Some(width) => width,
            None if self.mode.is_code() => gutter_width_in(self.font_size, lines, width),
            None => 0.0,
        }
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
        cx.note_focus_target(self.focus_target);
        if let Some(anchor) = &self.caret_anchor {
            let (x, y) = self.caret_at;
            anchor.set(Some(CaretGeometry {
                caret: Rect {
                    x: bounds.x + self.gutter_width_in(bounds.width) + x,
                    y: bounds.y - self.scroll_y + y,
                    width: Sz::CURSOR_WIDTH,
                    height: self.line_height(),
                },
                field: bounds,
            }));
        }
        cx.insert_hit(
            bounds,
            HitFlags::TEXT | HitFlags::SCROLL | HitFlags::DRAG,
            CursorHint::Text,
        )
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
        let accessibility_label = if !self.label.is_empty() {
            std::mem::take(&mut self.label)
        } else if self.placeholder.is_empty() {
            format!("{:?}", self.focus_target)
        } else {
            self.placeholder.clone()
        };
        let font_size = self.font_size;
        let line_height = self.line_height();
        let lines = self.line_tops.last().map_or(0, |(line, _)| *line);
        let gutter_w = self.gutter_width_in(bounds.width);
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
            let gutter_digits = gutter_digits(lines);
            for (line_no, y) in self.line_tops.iter() {
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

        if !self.is_empty {
            let pill = theme.colors.accent.with_alpha(Alpha::DIM);
            for rect in &self.atom_rects {
                scene.rounded_rect(RoundedRectPrimitive::uniform(
                    Rect {
                        x: text_x + rect.x - 2.0,
                        y: text_y - self.scroll_y + rect.y + 1.0,
                        width: rect.w + 4.0,
                        height: rect.h - 2.0,
                    },
                    4.0,
                    pill,
                ));
            }
        }

        if !self.is_empty {
            let code_bg = theme.colors.element_background;
            for (_, rect) in self
                .decorations
                .iter()
                .filter(|(kind, _)| *kind == TextDecoration::CodeBackground)
            {
                scene.rounded_rect(RoundedRectPrimitive::uniform(
                    Rect {
                        x: text_x + rect.x - 1.0,
                        y: text_y - self.scroll_y + rect.y + 1.0,
                        width: rect.w + 2.0,
                        height: rect.h - 2.0,
                    },
                    3.0,
                    code_bg,
                ));
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
        // The painted layout, for mapping pointer points to offsets; none
        // while composing, when it holds the preedit too.
        let mut hit_layout = None;
        if let Some(frame_scale) = &self.frame_scale {
            frame_scale.set(cx.scale_factor);
        }
        if self.is_empty {
            let placeholder_color = theme.colors.placeholder;
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
            // The editor's layout is used only if it was shaped at this
            // frame's scale; otherwise paint a fresh one until it catches up.
            let scale = cx.scale_factor;
            // Paint again right away so the editor reshapes at the frame's
            // scale (recorded above) on its next flush.
            if self
                .layout
                .as_ref()
                .is_some_and(|l| l.scale_factor() != scale)
            {
                cx.request_frame_at_ms(cx.clock_ms);
            }
            let own = self.layout.take().filter(|l| l.scale_factor() == scale);
            let (layout, kinds) = match own {
                Some(layout) => (Some(layout), std::mem::take(&mut self.span_kinds)),
                None => {
                    let (spans, kinds) = syntax_layout_spans(&self.text, &self.syntax_spans);
                    let params = TextParams::new(self.text.clone(), style)
                        .spans(spans)
                        .wrap_width(Some(text_area_w.max(1.0)));
                    (cx.layout_text(&params), kinds.into())
                }
            };
            let span_colors: Vec<_> = kinds
                .iter()
                .map(|kind| syntax_color(*kind, self.text_color, theme))
                .collect();
            if self.preedit_rects.is_empty() {
                hit_layout = layout.clone();
            }
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
            let thin = theme.metrics.ui_scale().max(1.0);
            paint_decorations(
                scene,
                &self.decorations,
                (text_x, text_y - self.scroll_y),
                thin,
                self.text_color,
                theme,
            );
            // IME composition: thin underline under the preedit, thick under
            // the clause the IME is converting.
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
            let (visible, next_toggle_ms) = caret_blink(cx.clock_ms, cur.moved_at_ms);
            if visible {
                scene.rounded_rect(RoundedRectPrimitive::uniform(rect, 1.0, theme.colors.text));
            }
            cx.request_frame_at_ms(next_toggle_ms);
        }

        scene.pop_clip();

        let target = self.focus_target;
        let key = super::target_key("text-editor", target);
        let mut semantic_node = SemanticNode::new(bounds)
            .id(key.clone())
            .role(SemanticRole::TextInput);
        semantic_node.parent = cx.current_semantic_parent();
        semantic_node.actions = SemanticActions::default().text_value().scrollable();
        semantic_node.focus = Some(target);
        let node = cx.semantic.push(semantic_node);
        cx.bind_hit(*prepaint_state, node);
        cx.handlers.on_drag(node, text_pointer_drag(target, None));
        cx.handlers.on_scroll(
            node,
            ScrollTarget {
                builder: self.on_scroll.clone(),
                offset: self.scroll_y,
                max: Some((self.content_height - bounds.height).max(0.0)),
            },
        );
        if cx.accessibility_enabled() {
            cx.push_accessibility(
                AccessibilityNode::shared(key, accesskit::Role::MultilineTextInput, bounds)
                    .label(accessibility_label)
                    .value(self.text.clone())
                    .text(match self.text_selection {
                        Some((anchor, caret)) => {
                            AccessibleText::new(self.text.clone()).selection(anchor, caret)
                        }
                        None => AccessibleText::new(self.text.clone()),
                    })
                    .action(AccessibilityAction::EditorViewport {
                        focus: target,
                        scroll: self.on_scroll.clone(),
                    }),
            );
        }
        register_text_input_area(
            cx,
            TextInputHitArea {
                bounds,
                focus_target: target,
                text_rect: Rect {
                    x: text_x,
                    y: text_y,
                    width: text_area_w,
                    height: bounds.height,
                },
                caret,
                content: TextHitContent::Multiline {
                    layout: hit_layout,
                    scroll_y: self.scroll_y,
                },
                transform: quark::Transform2D::IDENTITY,
            },
        );
    }
}

impl IntoAnyElement for TextEditorElement {
    fn into_any(self) -> AnyElement {
        AnyElement::new(self)
    }
}

/// Underlines and strikethroughs in the text color (links in the accent),
/// and a wavy line in the error color under misspellings. `origin` is the
/// layout's top left in the scene.
fn paint_decorations(
    scene: &mut Scene,
    decorations: &[(TextDecoration, SelectionRect)],
    origin: (f32, f32),
    thickness: f32,
    text_color: crate::theme::Color,
    theme: &crate::theme::Theme,
) {
    for (kind, rect) in decorations {
        let (x, y) = (origin.0 + rect.x, origin.1 + rect.y);
        let line = |scene: &mut Scene, y: f32| {
            scene.rect(RectPrimitive {
                rect: Rect {
                    x,
                    y,
                    width: rect.w,
                    height: thickness,
                },
                color: text_color,
            });
        };
        match kind {
            TextDecoration::Underline => line(scene, y + rect.h * 0.82),
            TextDecoration::Strikethrough => line(scene, y + rect.h * 0.52),
            TextDecoration::CodeBackground => {}
            TextDecoration::Misspelled => {
                // A zigzag of short dashes alternating between two rows.
                let step = 2.0 * thickness;
                let base = y + rect.h - 2.0 * thickness;
                let mut at = 0.0;
                let mut up = false;
                while at < rect.w {
                    scene.rect(RectPrimitive {
                        rect: Rect {
                            x: x + at,
                            y: if up { base - thickness } else { base },
                            width: step.min(rect.w - at),
                            height: thickness,
                        },
                        color: theme.colors.status_error,
                    });
                    at += step;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Action;
    use crate::text_input::TextEditCommand::*;
    use crate::text_input::{TextOffset, caret_popup};
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

    // Regression: apps had to call Editor::set_scale_factor themselves, and
    // an editor that never did stayed shaped at 1x on HiDPI screens.
    #[test]
    fn painting_at_a_scale_reshapes_the_editor_at_it_on_the_next_flush() {
        let mut text = quark_text::TextSystem::vendored_only(&Default::default());
        let mut editor = Editor::new(EditorMode::ProseInput);
        editor.sync_size(200.0, 100.0);
        editor.set_text("hello");
        editor.flush(&mut text);
        let shaped_at = |editor: &Editor| editor.layout().map(|l| l.scale_factor());
        let before = shaped_at(&editor);

        let theme = Theme::default_dark();
        let signals = SignalStore::new();
        let mut layouts = quark_text::LayoutCache::default();
        let mut cx = ElementContext::new(&theme, 2.0, &mut text, &mut layouts, None, &signals);
        let mut root = text_editor_element(
            FocusId::from_key("e"),
            ScrollActionBuilder::new(Action::new),
        )
        .editor_snapshot(&editor)
        .w(200.0)
        .h(100.0)
        .into_any();
        render_element(&mut root, &mut Scene::default(), &mut cx, 200.0, 100.0);
        let repaint_at = cx.next_frame_ms();
        editor.flush(&mut text);

        assert_eq!((before, shaped_at(&editor)), (Some(1.0), Some(2.0)));
        assert_eq!(repaint_at, Some(0), "a repaint picks up the new layout");
    }

    // Regression: code mode wrapped the editor's layout at the full width,
    // so lines ran on under the clip past the text area beside the gutter.
    #[test]
    fn code_editor_wraps_its_text_within_the_area_beside_the_gutter() {
        let mut text = quark_text::TextSystem::vendored_only(&Default::default());
        let mut editor = Editor::new(EditorMode::CodeInput);
        editor.sync_size(200.0, 100.0);
        // Lines from about 0.6 to 1.0 of the full width: some fit the whole
        // view but not the part beside the gutter.
        let lines: Vec<String> = (14..=24).map(|n| "x".repeat(n)).collect();
        editor.set_text(&lines.join("\n"));
        editor.flush(&mut text);

        let theme = Theme::default_dark();
        let signals = SignalStore::new();
        let mut layouts = quark_text::LayoutCache::default();
        let mut cx = ElementContext::new(&theme, 1.0, &mut text, &mut layouts, None, &signals);
        let mut root = text_editor_element(
            FocusId::from_key("e"),
            ScrollActionBuilder::new(Action::new),
        )
        .editor_snapshot(&editor)
        .w(200.0)
        .h(100.0)
        .into_any();
        let mut scene = Scene::default();
        render_element(&mut root, &mut scene, &mut cx, 200.0, 100.0);

        let (area, layout) = scene
            .primitives
            .iter()
            .find_map(|p| match p {
                Primitive::RichTextRun(run) => {
                    Some((run.rect, run.layout.downcast_ref::<TextLayout>()?))
                }
                _ => None,
            })
            .expect("painted editor text");
        assert!(area.x > 0.0, "a gutter is painted");
        let widest = layout.lines().map(|l| l.width).fold(0.0, f32::max);
        assert!(
            widest <= area.width + 0.5,
            "line {widest} wider than {}",
            area.width
        );
    }

    // Catches atoms painting without their pill, or the pill drifting off
    // the label's glyphs.
    #[test]
    fn an_atom_paints_a_pill_spanning_its_label() {
        use crate::text_input::{AtomId, InlineAtom, RichText};
        let mut text = quark_text::TextSystem::vendored_only(&Default::default());
        let mut editor = Editor::new(EditorMode::ProseInput);
        editor.sync_size(300.0, 60.0);
        let rich = RichText {
            text: "see @main.rs now".into(),
            atoms: vec![InlineAtom {
                range: 4..12,
                id: AtomId::new(1, 0),
                export: "[main.rs](main.rs)".into(),
            }],
            styles: Vec::new(),
        };
        editor.set_rich_text(&rich);
        editor.flush(&mut text);

        let theme = Theme::default_dark();
        let signals = SignalStore::new();
        let mut layouts = quark_text::LayoutCache::default();
        let mut cx = ElementContext::new(&theme, 1.0, &mut text, &mut layouts, None, &signals);
        let mut root = text_editor_element(
            FocusId::from_key("e"),
            ScrollActionBuilder::new(Action::new),
        )
        .editor_snapshot(&editor)
        .w(300.0)
        .h(60.0)
        .into_any();
        let mut scene = Scene::default();
        render_element(&mut root, &mut scene, &mut cx, 300.0, 60.0);

        let layout = editor.layout().expect("layout");
        let (start, end) = (
            layout.caret(TextOffset::snap(editor.text(), 4)).x,
            layout.caret(TextOffset::snap(editor.text(), 12)).x,
        );
        let pills: Vec<_> = scene
            .primitives
            .iter()
            .filter_map(|p| match p {
                Primitive::RoundedRect(r) => Some(r.rect),
                _ => None,
            })
            .filter(|r| r.x <= start && r.x + r.width >= end && r.width < end - start + 8.0)
            .collect();
        assert_eq!(pills.len(), 1, "start {start} end {end}");
    }

    // Catches formatting that reaches the model but not the screen: an
    // underlined run must paint a line spanning exactly its glyphs, below
    // their middle.
    #[test]
    fn an_underlined_run_paints_a_line_under_its_glyphs() {
        use crate::text_input::{InlineStyle, RichText, StyleSpan};
        let mut text = quark_text::TextSystem::vendored_only(&Default::default());
        let mut editor = Editor::new(EditorMode::ProseInput);
        editor.sync_size(300.0, 60.0);
        editor.set_rich_text(&RichText {
            text: "see under now".into(),
            styles: vec![StyleSpan::new(4..9, InlineStyle::UNDERLINE)],
            ..RichText::default()
        });
        editor.flush(&mut text);

        let theme = Theme::default_dark();
        let signals = SignalStore::new();
        let mut layouts = quark_text::LayoutCache::default();
        let mut cx = ElementContext::new(&theme, 1.0, &mut text, &mut layouts, None, &signals);
        let mut root = text_editor_element(
            FocusId::from_key("e"),
            ScrollActionBuilder::new(Action::new),
        )
        .editor_snapshot(&editor)
        .w(300.0)
        .h(60.0)
        .into_any();
        let mut scene = Scene::default();
        render_element(&mut root, &mut scene, &mut cx, 300.0, 60.0);

        let layout = editor.layout().expect("layout");
        let at = |o| layout.caret(TextOffset::snap(editor.text(), o));
        let (start, end) = (at(4), at(9));
        let lines: Vec<_> = scene
            .primitives
            .iter()
            .filter_map(|p| match p {
                Primitive::Rect(r) => Some(r.rect),
                _ => None,
            })
            .filter(|r| (r.x - start.x).abs() < 0.5 && (r.x + r.width - end.x).abs() < 0.5)
            .collect();
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].y > start.y + 14.0 * 1.35 * 0.5, "{:?}", lines[0]);
    }

    const POPUP: crate::theme::Color = crate::theme::Color::rgba(1, 2, 3, 255);

    /// A 400x300 window with an editor at (20, 240) holding two lines, and
    /// an 80pt popup at its caret before or after it in the tree. Returns
    /// the painted popup and editor text rects, and the frame asked for.
    fn render_caret_popup(
        editor: &Editor,
        text: &mut quark_text::TextSystem,
        popup_first: bool,
    ) -> (Option<Rect>, Option<Rect>, Option<u64>) {
        let theme = Theme::default_dark();
        let signals = SignalStore::new();
        let mut layouts = quark_text::LayoutCache::default();
        let mut cx = ElementContext::new(&theme, 1.0, text, &mut layouts, None, &signals);
        let field = text_editor_element(
            FocusId::from_key("e"),
            ScrollActionBuilder::new(Action::new),
        )
        .editor_snapshot(editor)
        .w(300.0)
        .h(44.0)
        .into_any();
        let popup = caret_popup(
            editor.caret_anchor(),
            div().w(120.0).h(80.0).bg(POPUP),
            (400.0, 300.0),
        )
        .into_any();
        let children = if popup_first {
            [popup, field]
        } else {
            [field, popup]
        };
        let mut root = div()
            .w(400.0)
            .h(300.0)
            .pt(240.0)
            .pl(20.0)
            .children(children)
            .into_any();
        let mut scene = Scene::default();
        render_element(&mut root, &mut scene, &mut cx, 400.0, 300.0);
        let popup = scene.primitives.iter().find_map(|p| match p {
            Primitive::RoundedRect(r) if r.color == POPUP => Some(r.rect),
            Primitive::Rect(r) if r.color == POPUP => Some(r.rect),
            _ => None,
        });
        let draft = scene.primitives.iter().find_map(|p| match p {
            Primitive::RichTextRun(run) => Some(run.rect),
            _ => None,
        });
        (popup, draft, cx.next_frame_ms())
    }

    fn two_line_editor(text: &mut quark_text::TextSystem) -> Editor {
        let mut editor = Editor::new(EditorMode::ProseInput);
        editor.sync_size(300.0, 44.0);
        editor.set_text("first\nsecond @ma");
        editor.flush(text);
        editor
    }

    // Catches a completion popup that is not placed against the caret
    // painted this frame, or does not flip above a composer at the bottom.
    #[test]
    fn a_caret_popup_after_the_editor_sits_above_its_caret_in_the_same_frame() {
        let mut text = quark_text::TextSystem::vendored_only(&Default::default());
        let editor = two_line_editor(&mut text);
        let (popup, _, next_frame) = render_caret_popup(&editor, &mut text, false);

        let caret = (20.0 + editor.cursor_pos.x, 240.0 + editor.cursor_pos.y);
        let popup = popup.expect("popup painted");
        let bottom = popup.y + popup.height + 4.0;
        assert!(
            (popup.x - caret.0).abs() < 0.01 && (bottom - caret.1).abs() < 0.01,
            "popup {popup:?} caret {caret:?}"
        );
        assert_eq!(next_frame, None, "no second frame needed");
    }

    // Catches a popup earlier in the tree than its editor staying where
    // last frame's caret was.
    #[test]
    fn a_caret_popup_before_the_editor_reaches_the_caret_on_the_next_frame() {
        let mut text = quark_text::TextSystem::vendored_only(&Default::default());
        let editor = two_line_editor(&mut text);

        let (first, _, next_frame) = render_caret_popup(&editor, &mut text, true);
        let (second, _, _) = render_caret_popup(&editor, &mut text, true);

        assert_eq!(first, None, "nothing to anchor to yet");
        assert_eq!(next_frame, Some(0));
        let popup = second.expect("popup painted");
        let caret_y = 240.0 + editor.cursor_pos.y;
        assert!((popup.y + popup.height + 4.0 - caret_y).abs() < 0.01);
    }

    // Catches suggestions taking space in the composer's layout, which
    // pushed the draft down while they were open.
    #[test]
    fn opening_a_caret_popup_leaves_the_draft_in_place() {
        let mut text = quark_text::TextSystem::vendored_only(&Default::default());
        let editor = two_line_editor(&mut text);
        let (_, with_popup, _) = render_caret_popup(&editor, &mut text, false);

        let theme = Theme::default_dark();
        let signals = SignalStore::new();
        let mut layouts = quark_text::LayoutCache::default();
        let mut cx = ElementContext::new(&theme, 1.0, &mut text, &mut layouts, None, &signals);
        let field = text_editor_element(
            FocusId::from_key("e"),
            ScrollActionBuilder::new(Action::new),
        )
        .editor_snapshot(&editor)
        .w(300.0)
        .h(44.0);
        let mut root = div()
            .w(400.0)
            .h(300.0)
            .pt(240.0)
            .pl(20.0)
            .child(field)
            .into_any();
        let mut scene = Scene::default();
        render_element(&mut root, &mut scene, &mut cx, 400.0, 300.0);
        let alone = scene.primitives.iter().find_map(|p| match p {
            Primitive::RichTextRun(run) => Some(run.rect),
            _ => None,
        });

        assert!(alone.is_some());
        assert_eq!(with_popup, alone);
    }

    // Catches the element painting the caret at its own 1.35em line height
    // instead of the editor's, so it no longer spans the line it sits on.
    #[test]
    fn the_painted_caret_spans_the_editors_line_height() {
        let mut text = quark_text::TextSystem::vendored_only(&Default::default());
        let mut editor = Editor::new(EditorMode::ProseInput);
        editor.set_line_height(Some(22.0));
        editor.sync_size(300.0, 60.0);
        editor.set_text("first\nsecond");
        editor.flush(&mut text);

        let theme = Theme::default_dark();
        let signals = SignalStore::new();
        let mut layouts = quark_text::LayoutCache::default();
        let mut cx = ElementContext::new(&theme, 1.0, &mut text, &mut layouts, None, &signals);
        let mut root = text_editor_element(
            FocusId::from_key("e"),
            ScrollActionBuilder::new(Action::new),
        )
        .editor_snapshot(&editor)
        .focused(true)
        .w(300.0)
        .h(60.0)
        .into_any();
        render_element(&mut root, &mut Scene::default(), &mut cx, 300.0, 60.0);

        let caret = cx.text_input_hit_areas[0].caret.expect("caret");
        assert_eq!(
            (caret.y, caret.y + caret.height),
            (22.0 + 1.0, 44.0 + 1.0 - Sz::CURSOR_WIDTH)
        );
    }
}
