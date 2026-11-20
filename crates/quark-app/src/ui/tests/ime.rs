//! IME compositions: the candidate window follows the caret painted, and a
//! composition ends where focus leaves its element, so none of it lands
//! in the element focused next.

use quark_ui::FocusId;
use quark_ui::element::{AnyElement, IntoAnyElement, canvas, div, text_input};
use quark_ui::style::Styled;
use quark_ui::text_input::{TextEditCommand, TextEditOutcome, TextField};

use super::{FIELD, Fixture, Msg};
use crate::InputEvent;
use crate::testing::UiTestHarness;
use crate::ui::{UiAdapter, UiApp, UiContext, ViewContext};

/// An app with one text field that applies the adapter's edits to it.
struct FieldApp {
    field: TextField,
}

impl UiApp for FieldApp {
    type Action = Msg;
    type Message = ();

    fn view(&mut self, _cx: &mut ViewContext) -> AnyElement {
        div().into_any()
    }

    fn update(&mut self, _action: Msg, _cx: &mut UiContext) {}

    fn edit_text(&mut self, target: FocusId, command: TextEditCommand) -> TextEditOutcome {
        if target == FIELD {
            return self.field.apply(command);
        }
        TextEditOutcome::default()
    }
}

fn field_adapter(text: &str) -> UiAdapter<FieldApp> {
    let mut adapter = UiAdapter::new(
        FieldApp {
            field: TextField::new(text),
        },
        "Test",
    );
    adapter.win.focus.current = Some(FIELD);
    adapter
}

// Regression: the IME candidate window was placed during event
// handling with the previous frame's caret, so it trailed by a frame.
#[test]
fn ime_area_requested_with_a_frame_is_that_frames_caret() {
    let mut fixture = Fixture::new();
    let mut adapter = field_adapter("hello world");

    let frames = [0, 11].map(|cursor| {
        adapter
            .app
            .field
            .apply(TextEditCommand::SetTextCursor(cursor));
        let painted = fixture.paint_field(&adapter.app.field);
        let caret = painted.text_areas[0].caret.expect("focused caret");
        let (_, ime) = adapter.finish_frame(painted);
        (caret, ime.cursor_area)
    });

    assert_ne!(frames[0].0, frames[1].0, "the caret moved");
    for (caret, requested) in frames {
        assert_eq!(requested, Some(caret));
    }
}

// Regression: winit sends no Ime::Disabled when focus leaves a field
// or the window, so the field kept painting a stale composition.
#[test]
fn leaving_a_composing_field_cancels_its_preedit() {
    let cases = [
        ("focus stays", Some(FIELD), false, Some("にほ")),
        ("focus moves away", None, false, None),
        ("window loses focus", Some(FIELD), true, None),
    ];
    for (name, focus, window_blurred, expected) in cases {
        let mut fixture = Fixture::new();
        let mut adapter = field_adapter("");
        let painted = fixture.paint_field(&adapter.app.field);
        adapter.finish_frame(painted);
        adapter.cancel_stale_preedit(false);
        adapter.app.field.set_preedit("にほ", None);

        adapter.win.focus.current = focus;
        adapter.cancel_stale_preedit(window_blurred);

        let preedit = adapter.app.field.preedit().map(|p| p.text.as_str());
        assert_eq!(preedit, expected, "{name}");
    }
}

const OTHER_FIELD: FocusId = FocusId::from_key("other field");
const TERMINAL: FocusId = FocusId::from_key("terminal");

/// Two text fields, `FIELD` above `OTHER_FIELD`, then a 200x40 pane
/// `TERMINAL` that takes IME input in the app's hook, as a terminal
/// does.
struct Editors {
    fields: [TextField; 2],
    /// The text the terminal pane was sent.
    terminal: String,
}

impl Editors {
    fn new() -> Self {
        Self {
            fields: [TextField::new(""), TextField::new("")],
            terminal: String::new(),
        }
    }

    fn field(&mut self, target: FocusId) -> &mut TextField {
        &mut self.fields[usize::from(target == OTHER_FIELD)]
    }

    /// The text in `FIELD`, `OTHER_FIELD`, and `TERMINAL`.
    fn texts(&self) -> [&str; 3] {
        [self.fields[0].text(), self.fields[1].text(), &self.terminal]
    }
}

impl UiApp for Editors {
    type Action = ();
    type Message = ();

    fn view(&mut self, cx: &mut ViewContext) -> AnyElement {
        let field = |target, field: &TextField| {
            text_input("Field", "")
                .field(field)
                .focus_target(target)
                .focused(cx.is_focused(target))
                .w(200.0)
                .h(40.0)
        };
        div()
            .w(400.0)
            .h(300.0)
            .flex_col()
            .child(field(FIELD, &self.fields[0]))
            .child(field(OTHER_FIELD, &self.fields[1]))
            .child(
                div()
                    .w(200.0)
                    .h(40.0)
                    .track_focus(TERMINAL)
                    .on_click(quark_ui::element::NoopAction)
                    .child(
                        canvas(|bounds, _scene, cx| cx.register_ime_target(TERMINAL, Some(bounds)))
                            .w(200.0)
                            .h(40.0),
                    ),
            )
            .into_any()
    }

    fn update(&mut self, (): (), _cx: &mut UiContext) {}

    fn event(&mut self, event: &InputEvent, cx: &mut UiContext) -> bool {
        if cx.focus() != Some(TERMINAL) {
            return false;
        }
        match event {
            InputEvent::TextInput(text) | InputEvent::ImeCommit(text) => {
                self.terminal.push_str(text);
                true
            }
            InputEvent::ImePreedit(..) => true,
            _ => false,
        }
    }

    fn edit_text(&mut self, target: FocusId, command: TextEditCommand) -> TextEditOutcome {
        self.field(target).apply(command)
    }

    fn set_preedit(&mut self, target: FocusId, text: String, cursor: Option<(usize, usize)>) {
        self.field(target).set_preedit(&text, cursor);
    }
}

// Regression: focus moving between fields mid-composition left the
// platform IME composing, so its next preedit or commit landed in the
// field focused next.
#[test]
fn focus_leaving_a_composing_field_for_another_resets_the_ime() {
    let mut ui = UiTestHarness::new(Editors::new(), (400.0, 300.0), 1.0);
    ui.click((10.0, 20.0));
    ui.key("tab");
    ui.key("shift+tab");
    assert_eq!(ui.ime().resets, 0, "moving focus while not composing");

    ui.ime_preedit("にほ", None);
    ui.key("tab");

    assert_eq!(ui.focus(), Some(OTHER_FIELD));
    let ime = ui.ime();
    assert_eq!((ime.resets, ime.allowed), (1, true));
    assert_eq!(ui.app().fields[0].preedit(), None);
}

// Regression: input queued between a click that moved focus off the
// element composing and the frame that resets the IME, the empty
// preedit and commit closing that composition, landed in the element
// the click focused, whether a text field or a terminal taking IME
// input in the app's hook, and whether the empty preedit or a window
// blur came before the click or after it.
#[test]
fn a_commit_queued_after_focus_left_its_composition_lands_nowhere() {
    use winit::event::{ElementState, MouseButton};
    // Each element's place in `Editors::texts`, and its center.
    let slot = |target| {
        [FIELD, OTHER_FIELD, TERMINAL]
            .iter()
            .position(|t| *t == target)
    };
    let center = |target| (100.0, 20.0 + 40.0 * slot(target).unwrap() as f32);
    let moves = [
        ("field to field", FIELD, OTHER_FIELD),
        ("field to terminal", FIELD, TERMINAL),
        ("terminal to field", TERMINAL, FIELD),
    ];
    let click = |(x, y)| {
        let button = |state| InputEvent::PointerButton {
            button: MouseButton::Left,
            state,
        };
        [
            InputEvent::PointerMoved { x, y },
            button(ElementState::Pressed),
            button(ElementState::Released),
        ]
    };
    let closed = || InputEvent::ImePreedit(String::new(), None);
    let commit = || InputEvent::ImeCommit("日本".into());
    type Closing = fn(Vec<InputEvent>, InputEvent, InputEvent) -> Vec<InputEvent>;
    let closings: [(&str, Closing); 3] = [
        ("preedit closed after the click", |click, closed, commit| {
            [click, vec![closed, commit]].concat()
        }),
        (
            "preedit closed before the click",
            |click, closed, commit| [vec![closed], click, vec![commit]].concat(),
        ),
        (
            "window blurred before the click",
            |click, closed, commit| {
                let blur = vec![InputEvent::Focused(false), InputEvent::Focused(true)];
                [blur, click, vec![closed, commit]].concat()
            },
        ),
    ];
    for (closing, sequence) in closings {
        for (name, from, to) in moves {
            let name = format!("{name}, {closing}");
            let mut ui = UiTestHarness::new(Editors::new(), (400.0, 300.0), 1.0);
            ui.click(center(from));
            ui.ime_preedit("にほ", None);

            ui.send_events(sequence(click(center(to)).to_vec(), closed(), commit()));
            assert_eq!(ui.focus(), Some(to), "{name}");
            assert_eq!(ui.app().texts(), ["", "", ""], "{name}");

            // Once a frame reset the IME, its commits are the new
            // element's again.
            ui.ime_commit("本");
            let mut expected = ["", "", ""];
            expected[slot(to).unwrap()] = "本";
            assert_eq!(ui.app().texts(), expected, "{name}: after the reset");
        }
    }
}
