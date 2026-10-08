//! Pointer selection in text fields: click to place the caret, drag to
//! select, double and triple click for word and line, Shift to extend, and
//! autoscroll while the pointer is held past a field's edge.
//!
//! [`crate::element::TextInput`] and [`super::TextEditorElement`] register a
//! drag on their node that emits [`TextPointerEvent`]s as actions. The host
//! feeds them to a [`TextPointer`], which maps them to
//! [`TextEditCommand`]s using the latest frame's [`TextInputHitArea`]s; the
//! app applies those to the field's model.

use quark::ClickEvent;

use super::TextEditCommand;
use super::view::ClickCounter;
use crate::element::{CursorHint, DragHandler, DragReleaseResult, DragStart, TextInputHitArea};
use crate::{Action, FocusId};

/// How often a pointer held past a field's edge extends the selection.
/// Each step scrolls by about the pointer's distance past the edge.
pub const AUTOSCROLL_STEP_MS: u64 = 50;

/// A pointer gesture on the text field `target`, in window points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TextPointerEvent {
    Press { target: FocusId, x: f32, y: f32 },
    Drag { target: FocusId, x: f32, y: f32 },
    Release { target: FocusId },
}

/// The drag a text field registers: it reports the press at press time,
/// with the field's own click action first when it has one.
pub(crate) fn text_pointer_drag(target: FocusId, on_click: Option<Action>) -> DragStart {
    DragStart::new(move |press| {
        Box::new(TextDrag {
            target,
            press,
            on_click: on_click.clone(),
        })
    })
}

struct TextDrag {
    target: FocusId,
    press: ClickEvent,
    on_click: Option<Action>,
}

impl DragHandler for TextDrag {
    fn on_press(&mut self) -> Vec<Action> {
        let press = TextPointerEvent::Press {
            target: self.target,
            x: self.press.x,
            y: self.press.y,
        };
        self.on_click
            .take()
            .into_iter()
            .chain([Action::new(press)])
            .collect()
    }

    fn on_move(&mut self, x: f32, y: f32) -> Vec<Action> {
        let target = self.target;
        vec![Action::new(TextPointerEvent::Drag { target, x, y })]
    }

    fn on_release(&mut self) -> DragReleaseResult {
        let target = self.target;
        DragReleaseResult {
            actions: vec![Action::new(TextPointerEvent::Release { target })],
        }
    }

    /// The selection made so far stays; the release only stops autoscroll.
    fn on_cancel(&mut self) -> Vec<Action> {
        self.on_release().actions
    }

    fn cursor(&self) -> CursorHint {
        CursorHint::Text
    }
}

#[derive(Debug, Clone, Copy)]
struct Held {
    target: FocusId,
    x: f32,
    y: f32,
    next_step_ms: u64,
}

/// Pointer selection state across frames: the multi-click counter and the
/// drag in progress.
#[derive(Debug, Default)]
pub struct TextPointer {
    clicks: ClickCounter,
    held: Option<Held>,
}

impl TextPointer {
    /// The command for `event`, mapped through `areas` (the frame the event
    /// was routed through). `extend` is Shift: a single press extends the
    /// selection instead of moving the caret.
    pub fn event(
        &mut self,
        event: TextPointerEvent,
        areas: &[TextInputHitArea],
        now_ms: u64,
        extend: bool,
    ) -> Option<(FocusId, TextEditCommand)> {
        let (target, x, y) = match event {
            TextPointerEvent::Press { target, x, y } | TextPointerEvent::Drag { target, x, y } => {
                (target, x, y)
            }
            TextPointerEvent::Release { .. } => {
                self.held = None;
                return None;
            }
        };
        self.held = Some(Held {
            target,
            x,
            y,
            next_step_ms: now_ms + AUTOSCROLL_STEP_MS,
        });
        let offset = area(areas, target)?.offset_at_point(x, y)?.get();
        let command = match event {
            TextPointerEvent::Press { .. } => match self.clicks.press(x, y, now_ms) {
                1 if extend => TextEditCommand::ExtendTextSelection(offset),
                1 => TextEditCommand::SetTextCursor(offset),
                2 => TextEditCommand::SelectWordAt(offset),
                _ => TextEditCommand::SelectLineAt(offset),
            },
            _ => TextEditCommand::ExtendTextSelection(offset),
        };
        Some((target, command))
    }

    /// While the pointer is held past a field's edge, extend the selection
    /// to it through `areas` (the last painted frame, scrolled by the
    /// previous step) once every [`AUTOSCROLL_STEP_MS`]. The pointer does
    /// not need to move.
    pub fn autoscroll(
        &mut self,
        areas: &[TextInputHitArea],
        now_ms: u64,
    ) -> Option<(FocusId, TextEditCommand)> {
        let held = self.held.as_mut()?;
        let area = area(areas, held.target)?;
        if now_ms < held.next_step_ms || !area.is_past_edge(held.x, held.y) {
            return None;
        }
        held.next_step_ms = now_ms + AUTOSCROLL_STEP_MS;
        let offset = area.offset_at_point(held.x, held.y)?.get();
        Some((held.target, TextEditCommand::ExtendTextSelection(offset)))
    }

    /// When [`Self::autoscroll`] next has work, if the pointer is held past
    /// an edge of a field in `areas`.
    pub fn next_autoscroll_ms(&self, areas: &[TextInputHitArea]) -> Option<u64> {
        let held = self.held?;
        area(areas, held.target)?
            .is_past_edge(held.x, held.y)
            .then_some(held.next_step_ms)
    }
}

fn area(areas: &[TextInputHitArea], target: FocusId) -> Option<&TextInputHitArea> {
    areas.iter().find(|area| area.focus_target == target)
}

#[cfg(test)]
mod tests {
    use quark::SemanticFrame;
    use quark::reactive::SignalStore;
    use quark_render::Scene;
    use quark_text::{TextOffset, offset};

    use std::f32::consts::FRAC_PI_2;

    use super::*;
    use crate::element::{
        Delivery, ElementContext, InputRouter, IntoAnyElement, ScrollActionBuilder, div,
        render_element, text_input,
    };
    use crate::style::Styled;
    use crate::text_input::{Editor, EditorMode, TextField, text_editor_element};
    use crate::theme::Theme;

    const FIELD: FocusId = FocusId::from_key("test.field");
    const LONG: &str = "the quick brown fox jumps over the lazy dog and keeps running";

    /// The text model behind the field under test.
    enum Model {
        Field(Box<TextField>),
        Editor(Box<Editor>),
    }

    impl Model {
        fn apply(&mut self, command: TextEditCommand) {
            match self {
                Model::Field(field) => drop(field.apply(command)),
                Model::Editor(editor) => drop(editor.apply(command)),
            }
        }

        fn selection(&self) -> (TextOffset, TextOffset) {
            match self {
                Model::Field(field) => (field.anchor(), field.cursor()),
                Model::Editor(editor) => (editor.anchor(), editor.cursor()),
            }
        }

        /// How far the field has scrolled along the axis it scrolls on.
        fn scroll(&self) -> f32 {
            match self {
                Model::Field(field) => field.scroll().get(),
                Model::Editor(editor) => editor.scroll_y,
            }
        }
    }

    /// A focused 160x52 field (or 160x60 editor) at the window origin, with
    /// pointer input routed through each painted frame the way the adapter
    /// routes it.
    struct Harness {
        model: Model,
        /// Scale and rotation of a div around the field, about its center.
        wrap: Option<(f32, f32)>,
        text: quark_text::TextSystem,
        layouts: quark_text::LayoutCache,
        router: InputRouter,
        areas: Vec<TextInputHitArea>,
        pointer: TextPointer,
        now_ms: u64,
    }

    impl Harness {
        fn new(model: Model) -> Self {
            Self::wrapped(model, None)
        }

        fn wrapped(model: Model, wrap: Option<(f32, f32)>) -> Self {
            let mut harness = Self {
                model,
                wrap,
                text: quark_text::TextSystem::vendored_only(&Default::default()),
                layouts: quark_text::LayoutCache::default(),
                router: InputRouter::default(),
                areas: Vec::new(),
                pointer: TextPointer::default(),
                now_ms: 1_000,
            };
            harness.paint();
            harness
        }

        fn field(text: &str) -> Self {
            let mut field = TextField::new(text);
            field.apply(TextEditCommand::SetTextCursor(0));
            Self::new(Model::Field(Box::new(field)))
        }

        /// A field in a div scaled by `scale` and turned by `rotate`.
        fn transformed_field(text: &str, scale: f32, rotate: f32) -> Self {
            let mut field = TextField::new(text);
            field.apply(TextEditCommand::SetTextCursor(0));
            Self::wrapped(Model::Field(Box::new(field)), Some((scale, rotate)))
        }

        fn editor(text: &str) -> Self {
            let mut editor = Editor::new(EditorMode::ProseInput);
            editor.set_text(text);
            editor.apply(TextEditCommand::SetTextCursor(0));
            Self::new(Model::Editor(Box::new(editor)))
        }

        fn paint(&mut self) {
            let theme = Theme::default_dark();
            let signals = SignalStore::new();
            let (w, h) = (160.0, 60.0);
            let mut root = match &mut self.model {
                Model::Field(field) => text_input("Name", "")
                    .field(field)
                    .focused(true)
                    .focus_target(FIELD)
                    .w(w)
                    .h(52.0)
                    .into_any(),
                Model::Editor(editor) => {
                    editor.sync_size(w, h);
                    editor.flush(&mut self.text);
                    text_editor_element(FIELD, ScrollActionBuilder::new(Action::new))
                        .editor_snapshot(editor)
                        .focused(true)
                        .w(w)
                        .h(h)
                        .into_any()
                }
            };
            if let Some((scale, rotate)) = self.wrap {
                root = div()
                    .w(w)
                    .h(h)
                    .scale(scale)
                    .rotate(rotate)
                    .child(root)
                    .into_any();
            }
            let mut cx = ElementContext::new(
                &theme,
                1.0,
                &mut self.text,
                &mut self.layouts,
                None,
                &signals,
            )
            .with_focus(Some(FIELD));
            cx.semantic = SemanticFrame::new(w, h);
            render_element(&mut root, &mut Scene::default(), &mut cx, w, h);
            self.areas = std::mem::take(&mut cx.text_input_hit_areas);
            self.router.set_frame(cx.take_input_frame());
        }

        fn area(&self) -> &TextInputHitArea {
            &self.areas[0]
        }

        /// Window x of byte `offset` in the painted single-line text.
        fn x_of(&self, offset: usize) -> f32 {
            let area = self.area();
            let layout = area.layout().expect("value layout");
            area.origin().0 + layout.caret(offset).x
        }

        /// Window point at byte `offset` of the painted single-line text,
        /// halfway down its text area, through the wrapping div's scale and
        /// rotation (about its center, 80,30).
        fn point_at(&self, offset: usize) -> (f32, f32) {
            let area = self.area();
            let y = area.text_rect.y + area.text_rect.height / 2.0;
            let (scale, rotate) = self.wrap.unwrap_or((1.0, 0.0));
            quark::Transform2D::rotate(rotate)
                .then(quark::Transform2D::scale(scale, scale))
                .around(80.0, 30.0)
                .apply(self.x_of(offset), y)
        }

        fn deliver(&mut self, delivery: Delivery) {
            for action in delivery.actions {
                let Some(event) = action.downcast_ref::<TextPointerEvent>() else {
                    continue;
                };
                if let Some((target, command)) =
                    self.pointer.event(*event, &self.areas, self.now_ms, false)
                {
                    assert_eq!(target, FIELD);
                    self.model.apply(command);
                }
            }
        }

        fn press(&mut self, x: f32, y: f32) {
            let delivery = self.router.pointer_down(x, y, &mut Some(FIELD));
            self.deliver(delivery);
        }

        fn move_to(&mut self, x: f32, y: f32) {
            let delivery = self.router.pointer_move(x, y);
            self.deliver(delivery);
        }

        fn release(&mut self) {
            let delivery = self.router.pointer_up();
            self.deliver(delivery);
        }

        /// Let `ms` pass, then run a frame: the autoscroll step, then paint.
        fn frame_after(&mut self, ms: u64) {
            self.now_ms += ms;
            if let Some((_, command)) = self.pointer.autoscroll(&self.areas, self.now_ms) {
                self.model.apply(command);
            }
            self.paint();
        }

        fn selected(&self, text: &str) -> String {
            let (a, b) = self.model.selection();
            offset::slice(text, a..b).to_owned()
        }
    }

    // Regression: the adapter never turned routed presses on a text field
    // into caret placement, so double and triple clicks did nothing.
    #[test]
    fn consecutive_clicks_on_a_routed_text_input_select_caret_word_then_line() {
        let text = "hello brave world";
        let mut harness = Harness::field(text);
        let (x, y) = (harness.x_of(8), harness.area().text_rect.y + 5.0);

        let selected: Vec<String> = (0..3)
            .map(|_| {
                harness.press(x, y);
                harness.release();
                harness.now_ms += 100;
                harness.selected(text)
            })
            .collect();

        assert_eq!(selected, ["", "brave", text]);
    }

    // Regression: a selection drag held past the edge scrolled only when
    // the pointer moved, so the hidden text could not be reached.
    #[test]
    fn holding_past_the_edge_keeps_scrolling_until_release() {
        let long_lines: String = (0..30).map(|i| format!("line {i}\n")).collect();
        let cases = [
            ("field", Harness::field(LONG), (1.0, 0.0)),
            ("editor", Harness::editor(&long_lines), (0.0, 1.0)),
        ];
        for (name, mut harness, (dx, dy)) in cases {
            let area = harness.area().clone();
            let r = area.text_rect;
            let (x, y) = (r.x + 4.0, r.y + 8.0);
            harness.press(x, y);
            // One move past the far edge (right, or below), then hold still.
            let (right, bottom) = (r.right(), r.bottom());
            harness.move_to(
                if dx > 0.0 { right + 15.0 } else { x },
                if dy > 0.0 { bottom + 15.0 } else { y },
            );
            harness.frame_after(0);

            let mut scrolls = vec![harness.model.scroll()];
            for _ in 0..4 {
                harness.frame_after(AUTOSCROLL_STEP_MS);
                scrolls.push(harness.model.scroll());
            }
            harness.release();
            harness.frame_after(AUTOSCROLL_STEP_MS);
            let after_release = harness.model.scroll();

            assert!(
                scrolls.windows(2).all(|w| w[1] > w[0]),
                "{name}: scroll offsets {scrolls:?}"
            );
            assert_eq!(after_release, *scrolls.last().unwrap(), "{name}");
        }
    }

    // Catches pointer selection mapped through untransformed coordinates:
    // a drag across "brave" on a scaled or turned field selects "brave".
    #[test]
    fn drag_selection_follows_a_transformed_field() {
        let text = "hello brave world";
        for (name, scale, rotate) in [("scaled", 2.0, 0.0), ("turned", 1.0, FRAC_PI_2)] {
            let mut harness = Harness::transformed_field(text, scale, rotate);
            let (from, to) = (harness.point_at(6), harness.point_at(11));
            harness.press(from.0, from.1);
            harness.move_to(to.0, to.1);
            harness.release();
            assert_eq!(harness.selected(text), "brave", "{name}");
        }
    }

    // Catches an IME candidate rect left in layout coordinates: after a
    // press on a scaled field, the caret is where the press was.
    #[test]
    fn ime_caret_of_a_scaled_field_is_where_the_press_put_it() {
        let mut harness = Harness::transformed_field("hello brave world", 2.0, 0.0);
        let (x, y) = harness.point_at(6);
        harness.press(x, y);
        harness.release();
        harness.paint();

        let caret = harness.area().caret.expect("focused field reports a caret");
        assert!(
            caret.contains(x, y),
            "caret {caret:?} not at the press {x},{y}"
        );
    }

    // Catches a field under a zero scale that still reports a caret for
    // the IME or maps pointers onto its text.
    #[test]
    fn flattened_field_shows_no_caret_and_takes_no_points() {
        let harness = Harness::transformed_field("hello", 0.0, 0.0);
        let area = harness.area();
        assert_eq!(area.caret, None);
        let r = area.text_rect;
        assert_eq!(area.offset_at_point(r.x + 2.0, r.y + 2.0), None);
    }

    // Catches presses mapped through unscrolled or unwrapped coordinates:
    // a press lands on the line painted under it.
    #[test]
    fn press_in_a_scrolled_or_wrapped_editor_lands_on_the_painted_line() {
        let cases = [
            ("scrolled", "line0\nline1\nline2\nline3", 1.0),
            (
                "wrapped",
                "asdf asf asdf asdf fasd fasd fasdf sdaf asdf sadf",
                0.0,
            ),
        ];
        for (name, text, scroll_lines) in cases {
            let mut harness = Harness::editor(text);
            if let Model::Editor(editor) = &mut harness.model {
                editor.scroll(editor.scroll_line_height_px() * scroll_lines);
            }
            harness.paint();
            let area = harness.area().clone();
            let line = area.layout().and_then(|l| l.line(1)).expect("second line");
            let (_, top) = area.origin();
            harness.press(area.text_rect.x + 1.0, top + line.top + line.height * 0.5);
            let start = TextOffset::snap(text, line.byte_range.start);
            assert_eq!(harness.model.selection(), (start, start), "{name}");
        }
    }
}
