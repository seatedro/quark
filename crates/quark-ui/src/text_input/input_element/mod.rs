//! [`TextEditorElement`]: paints an [`Editor`] snapshot (its lines shaped
//! around the view, selection, caret, gutter) and registers it for
//! pointers, the IME, and assistive tech.

mod paint;
#[cfg(test)]
mod tests;

use std::sync::Arc;

use super::anchor::{CaretAnchor, CaretGeometry};
use super::doc_text::TextSnapshot;
use super::editor::gutter_width_in;
use super::view::{FrameScale, FrameViewport};
use super::{Editor, EditorMode, EditorWindow, SelectionRect, SyntaxSpan, TextDecoration};
use crate::FocusId;
use crate::design::Sz;
use crate::element::*;
use crate::style::{ElementStyle, Styled};
use quark_render::Scene;
use quark_render::scene::Rect;

pub struct CursorSnapshot {
    pub x: f32,
    pub y: f32,
    pub moved_at_ms: u64,
}

/// What the element takes from an [`Editor`] each frame. Shared, not
/// copied: this runs every frame.
struct EditorView {
    window: EditorWindow,
    text: TextSnapshot,
    frame_scale: FrameScale,
    frame_viewport: FrameViewport,
    caret_anchor: CaretAnchor,
    gutter_width: f32,
    line_height: f32,
    /// Whether highlighting is still catching up past the view.
    highlighting: bool,
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
    text_color: crate::theme::Color,
    mode: EditorMode,
    /// Text laid out by the element itself when there is no editor
    /// snapshot.
    text: Arc<str>,
    /// `(anchor, caret)` byte offsets into the text, for assistive tech.
    text_selection: Option<(usize, usize)>,
    syntax_spans: Arc<[SyntaxSpan]>,
    preedit_rects: Vec<SelectionRect>,
    clause_rects: Vec<SelectionRect>,
    line_tops: Arc<[(usize, f32)]>,
    editor: Option<EditorView>,
    focus_target: FocusId,
    on_scroll: ScrollActionBuilder,
    /// The caret in content coordinates even while hidden, for the anchor.
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
        text_color: crate::theme::Color::rgba(255, 255, 255, 255),
        mode: EditorMode::ProseInput,
        text: Arc::from(""),
        text_selection: None,
        syntax_spans: Arc::from([]),
        preedit_rects: Vec::new(),
        clause_rects: Vec::new(),
        line_tops: Arc::from([]),
        editor: None,
        focus_target,
        on_scroll,
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

    /// Text for the element to lay out itself, without an editor snapshot.
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

    /// Paint `editor` as of its last flush: the lines it shaped around its
    /// view, its selection, caret, and decorations.
    pub fn editor_snapshot(mut self, editor: &Editor) -> Self {
        self.is_empty = editor.is_empty();
        self.cursor = editor.caret_visible().then_some(CursorSnapshot {
            x: editor.cursor_pos.x,
            y: editor.cursor_pos.y,
            moved_at_ms: editor.cursor_moved_at_ms,
        });
        (self.preedit_rects, self.clause_rects) = editor.preedit_rects();
        self.selection_rects = editor.selection_rects();
        self.atom_rects = editor.atom_rects();
        self.decorations = editor.decoration_rects();
        self.content_height = editor.content_height();
        self.scroll_y = editor.scroll_y;
        self.mode = editor.mode();
        self.text_selection = Some((editor.anchor().get(), editor.cursor().get()));
        self.syntax_spans = editor.syntax_spans().clone();
        self.line_tops = editor.logical_line_tops().clone();
        self.caret_at = (editor.cursor_pos.x, editor.cursor_pos.y);
        self.editor = Some(EditorView {
            window: editor.window().clone(),
            text: editor.text_snapshot(),
            frame_scale: editor.frame_scale.clone(),
            frame_viewport: editor.frame_viewport.clone(),
            caret_anchor: editor.caret_anchor().clone(),
            gutter_width: editor.gutter_width(),
            line_height: editor.scroll_line_height_px(),
            highlighting: editor.is_highlighting(),
        });
        self
    }

    pub fn focus_target(mut self, target: FocusId) -> Self {
        self.focus_target = target;
        self
    }

    fn line_height(&self) -> f32 {
        self.editor
            .as_ref()
            .map_or(self.font_size * 1.35, |e| e.line_height)
    }

    /// The line-number gutter's width in a box `width` wide.
    fn gutter_width_in(&self, width: f32) -> f32 {
        let lines = self.line_tops.last().map_or(0, |(line, _)| *line);
        match &self.editor {
            Some(editor) => editor.gutter_width,
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
        if let Some(editor) = &self.editor {
            let (x, y) = self.caret_at;
            editor.caret_anchor.set(Some(CaretGeometry {
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
        let accessibility_label = if !self.label.is_empty() {
            std::mem::take(&mut self.label)
        } else if self.placeholder.is_empty() {
            format!("{:?}", self.focus_target)
        } else {
            self.placeholder.clone()
        };
        let area = self.text_area(bounds);
        scene.clip(bounds);
        self.paint_gutter(scene, cx, bounds, &area);
        self.paint_backgrounds(scene, cx, &area);
        let hits = self.paint_text(scene, cx, &area);
        let caret = self.paint_caret(scene, cx, &area);
        scene.pop_clip();
        self.publish(cx, bounds, *prepaint_state, accessibility_label);
        register_text_input_area(
            cx,
            TextInputHitArea {
                bounds,
                focus_target: self.focus_target,
                text_rect: Rect {
                    x: area.x,
                    y: area.y,
                    width: area.width,
                    height: bounds.height,
                },
                caret,
                content: hits,
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
