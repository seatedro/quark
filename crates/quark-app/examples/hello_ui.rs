//! A small form built from quark-ui elements through the `UiApp` adapter.
//! The published accessibility tree has a named dialog containing a
//! heading, a text field, and two buttons. Escape quits.

use accesskit::Role;
use quark_app::quark_ui::element::{AnyElement, IntoAnyElement, div, text, text_input};
use quark_app::quark_ui::style::Styled;
use quark_app::quark_ui::text_input::TextField;
use quark_app::quark_ui::{Action, FocusId};
use quark_app::winit::keyboard::NamedKey;
use quark_app::{InputEvent, UiApp, UiContext, ViewContext, WindowOptions};

const NAME_FIELD: FocusId = FocusId::from_key("hello.name");

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Greet,
    Clear,
}

impl From<Msg> for Action {
    fn from(msg: Msg) -> Self {
        Action::new(msg)
    }
}

struct HelloUi {
    name: TextField,
    greeting: String,
}

impl HelloUi {
    fn button(id: &str, label: &str, msg: Msg, cx: &ViewContext) -> AnyElement {
        let colors = &cx.theme.colors;
        div()
            .accessibility_id(id)
            .accessibility_role(Role::Button)
            .accessibility_label(label)
            .on_click(msg)
            .px(16.0)
            .h(36.0)
            .items_center()
            .justify_center()
            .rounded(8.0)
            .bg(colors.accent)
            .hover_bg(colors.accent_strong)
            .child(text(label).color(colors.text_strong).semibold())
            .into_any()
    }
}

impl UiApp for HelloUi {
    type Action = Msg;

    fn view(&mut self, cx: &mut ViewContext) -> AnyElement {
        let (width, height) = cx.frame.size();
        let colors = &cx.theme.colors;
        let greeting = if self.greeting.is_empty() {
            "Type a name, then press Greet.".to_owned()
        } else {
            self.greeting.clone()
        };
        div()
            .w(width)
            .h(height)
            .bg(colors.background)
            .items_center()
            .justify_center()
            .child(
                div()
                    .accessibility_id("hello.dialog")
                    .accessibility_role(Role::Dialog)
                    .accessibility_label("Hello Quark")
                    .w(420.0)
                    .p(24.0)
                    .gap(16.0)
                    .flex_col()
                    .rounded(16.0)
                    .bg(colors.surface)
                    .child(
                        div()
                            .accessibility_id("hello.heading")
                            .accessibility_role(Role::Heading)
                            .accessibility_label("Hello from Quark")
                            .child(text("Hello from Quark").text_lg().bold()),
                    )
                    .child(
                        text_input("Name", self.name.text())
                            .placeholder("Your name")
                            .focus_target(NAME_FIELD)
                            .focused(cx.is_focused(NAME_FIELD))
                            .cursor(self.name.cursor())
                            .anchor(self.name.anchor())
                            .w_full()
                            .h(52.0),
                    )
                    .child(text(greeting).color(colors.text))
                    .child(
                        div()
                            .flex_row()
                            .gap(8.0)
                            .child(Self::button("hello.greet", "Greet", Msg::Greet, cx))
                            .child(Self::button("hello.clear", "Clear", Msg::Clear, cx)),
                    ),
            )
            .into_any()
    }

    fn update(&mut self, msg: Msg, _cx: &mut UiContext) {
        match msg {
            Msg::Greet if self.name.text().is_empty() => self.greeting = "Hello, stranger!".into(),
            Msg::Greet => self.greeting = format!("Hello, {}!", self.name.text()),
            Msg::Clear => {
                self.name.set_text("");
                self.greeting.clear();
            }
        }
    }

    fn set_text_value(&mut self, target: FocusId, value: String, _cx: &mut UiContext) {
        if target == NAME_FIELD {
            self.name.set_text(value);
        }
    }

    fn event(&mut self, event: &InputEvent, cx: &mut UiContext) -> bool {
        let editing = cx.focus() == Some(NAME_FIELD);
        match event {
            InputEvent::KeyPress(chord) if chord.named() == Some(NamedKey::Escape) => {
                cx.window.exit();
                true
            }
            InputEvent::TextInput(text) if editing => {
                self.name.insert_text(text);
                cx.window.request_redraw();
                true
            }
            InputEvent::KeyPress(chord)
                if editing && chord.named() == Some(NamedKey::Backspace) =>
            {
                self.name.backspace();
                cx.window.request_redraw();
                true
            }
            _ => false,
        }
    }
}

fn main() -> Result<(), quark_app::RunError> {
    quark_app::run_ui(
        HelloUi {
            name: TextField::new(""),
            greeting: String::new(),
        },
        WindowOptions {
            title: "Hello Quark UI".into(),
            size: (640.0, 400.0),
            ..WindowOptions::default()
        },
    )
}
