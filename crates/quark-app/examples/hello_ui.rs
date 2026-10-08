//! A small form built from quark-ui elements through the `UiApp` adapter,
//! written with `view!` (docs/guide/writing-views.md).
//! The published accessibility tree has a named dialog containing a
//! heading, a text field, and two buttons. The adapter routes typing,
//! editing keys, IME, and the clipboard to the field; Escape quits.

#[cfg(test)]
use accesskit::Role;
use quark::view;
use quark_app::quark_ui::element::{AnyElement, IntoAnyElement, div, text, text_input};
use quark_app::quark_ui::style::Styled;
use quark_app::quark_ui::text_input::{TextEditCommand, TextEditOutcome, TextField};
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
    /// The example's clock for undo coalescing; the field never reads time.
    started: std::time::Instant,
}

impl HelloUi {
    fn new() -> Self {
        Self {
            name: TextField::new(""),
            greeting: String::new(),
            started: std::time::Instant::now(),
        }
    }

    fn button(id: &str, label: &str, msg: Msg, cx: &ViewContext) -> AnyElement {
        let colors = &cx.theme.colors;
        view! {
            <div accessibility_id={id} role="button" aria-label={label} on:click={msg}
                 class="px-4 h-9 items-center justify-center rounded-[8]
                        bg-[colors.accent] hover:bg-[colors.accent_strong]">
                <text class="font-semibold" color={colors.on_accent}>{label}</text>
            </div>
        }
    }
}

impl UiApp for HelloUi {
    type Action = Msg;
    type Message = ();

    fn view(&mut self, cx: &mut ViewContext) -> AnyElement {
        let (width, height) = cx.frame.size();
        let colors = &cx.theme.colors;
        let greeting = if self.greeting.is_empty() {
            "Type a name, then press Greet.".to_owned()
        } else {
            self.greeting.clone()
        };
        view! {
            <div w={width} h={height} class="items-center justify-center bg-[colors.background]">
                <div accessibility_id="hello.dialog" role="dialog" aria-label="Hello Quark"
                     class="w-[420px] p-6 gap-4 flex-col rounded-[16] bg-[colors.surface]">
                    <div accessibility_id="hello.heading" role="heading" aria-label="Hello from Quark">
                        <text class="text-lg font-bold">"Hello from Quark"</text>
                    </div>
                    <text_input("Name", "") field={&self.name} placeholder="Your name"
                                focus_target={NAME_FIELD} focused={cx.is_focused(NAME_FIELD)}
                                class="w-full" h={52.0} />
                    <text color={colors.text}>{greeting}</text>
                    <div class="flex-row gap-2">
                        {Self::button("hello.greet", "Greet", Msg::Greet, cx)}
                        {Self::button("hello.clear", "Clear", Msg::Clear, cx)}
                    </div>
                </div>
            </div>
        }
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

    fn edit_text(&mut self, target: FocusId, command: TextEditCommand) -> TextEditOutcome {
        if target != NAME_FIELD {
            return TextEditOutcome::default();
        }
        let now_ms = self.started.elapsed().as_millis() as u64;
        self.name.apply_at(command, now_ms)
    }

    fn set_preedit(&mut self, target: FocusId, text: String, cursor: Option<(usize, usize)>) {
        if target == NAME_FIELD {
            self.name.set_preedit(text, cursor);
        }
    }

    fn set_text_value(&mut self, target: FocusId, value: String, _cx: &mut UiContext) {
        if target == NAME_FIELD {
            self.name.set_text(value);
        }
    }

    fn event(&mut self, event: &InputEvent, cx: &mut UiContext) -> bool {
        match event {
            InputEvent::KeyPress(chord) if chord.named() == Some(NamedKey::Escape) => {
                cx.window.exit();
                true
            }
            _ => false,
        }
    }
}

fn main() -> Result<(), quark_app::RunError> {
    quark_app::run_ui(
        HelloUi::new(),
        WindowOptions {
            title: "Hello Quark UI".into(),
            size: (640.0, 400.0),
            ..WindowOptions::default()
        },
    )
}

/// The form driven headlessly through `quark_app::testing`, as a user would
/// drive it: find controls by role and name, click, and type.
#[cfg(test)]
mod tests {
    use quark_app::quark_ui::test_alloc::{self, Counting};
    use quark_app::testing::{By, UiTestHarness};

    use super::*;

    #[global_allocator]
    static ALLOCATOR: Counting = Counting;

    /// Allocations of a repeated frame with no screen reader connected.
    fn repeated_frame_allocations() -> u64 {
        let mut ui = UiTestHarness::new(HelloUi::new(), (640.0, 400.0), 2.0);
        ui.set_accessibility_active(false);
        for _ in 0..3 {
            ui.frame();
        }
        let ((), allocated) = test_alloc::count(|| {
            ui.frame();
        });
        allocated
    }

    #[test]
    #[ignore = "measurement, prints a report"]
    fn report_frame_allocations() {
        let mut ui = UiTestHarness::new(HelloUi::new(), (640.0, 400.0), 2.0);
        ui.set_accessibility_active(false);
        for _ in 0..3 {
            ui.frame();
        }
        let ((), sites) = test_alloc::profile(|| {
            ui.frame();
        });
        for (site, n) in sites {
            eprintln!("  {n:4}  {site}");
        }
    }

    // The view rebuilds every frame without a cache boundary, so what its
    // builders own is allocated again: about 21 of the 27 are its strings,
    // actions, and click handlers, and the rest the text field's semantic
    // id and value. Layout, paint, hit testing, and routing reuse their
    // buffers.
    #[test]
    fn a_repeated_frame_allocates_only_what_the_view_builds() {
        let allocated = repeated_frame_allocations();
        assert!(allocated <= 30, "{allocated} allocations");
    }

    #[test]
    fn greet_shows_the_typed_name() {
        let mut ui = UiTestHarness::new(HelloUi::new(), (640.0, 400.0), 2.0);

        ui.click_node(By::role_name(Role::TextInput, "Name"));
        ui.type_text("Ada");
        ui.click_node(By::role_name(Role::Button, "Greet"));

        let greeting = ui.find(By::name("Hello, Ada!"));
        assert_eq!(greeting.role, Some(Role::Label));
        assert!(
            ui.painted_text().lines().any(|line| line == "Hello, Ada!"),
            "painted:\n{}",
            ui.painted_text()
        );
    }
}
