//! A settings form that exercises the accessibility tree: a text field
//! whose text, caret, and selection screen readers can read; a checkbox
//! and a switch from quark-components; a Save button that shows a toast
//! (a live region); and a Check button that makes an announcement nothing
//! on screen shows. Tab moves focus; Enter or Space presses the focused
//! control. Escape quits.

use accesskit::Role;
use quark::view;
use quark_app::quark_ui::accessibility::Politeness;
use quark_app::quark_ui::element::{AnyElement, IntoAnyElement, div, text, text_input};
use quark_app::quark_ui::style::Styled;
use quark_app::quark_ui::text_input::{TextEditCommand, TextEditOutcome, TextField};
use quark_app::quark_ui::{Action, FocusId};
use quark_app::winit::keyboard::NamedKey;
use quark_app::{InputEvent, UiApp, UiContext, ViewContext, WindowOptions};
use quark_components::{Toast, ToastKind, ToastStack, checkbox, toggle};

const NAME_FIELD: FocusId = FocusId::from_key("a11y.name");

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Remember,
    Notify,
    Save,
    Check,
    Dismiss(usize),
}

impl From<Msg> for Action {
    fn from(msg: Msg) -> Self {
        Action::new(msg)
    }
}

struct A11yDemo {
    name: TextField,
    remember: bool,
    notify: bool,
    toasts: Vec<Toast>,
    next_toast: u64,
    started: std::time::Instant,
}

impl A11yDemo {
    fn button(id: &str, label: &str, msg: Msg, cx: &ViewContext) -> AnyElement {
        let colors = &cx.theme.colors;
        view! {
            <div
                accessibility_id={id}
                accessibility_role={Role::Button}
                aria-label={label}
                focus_ring={FocusId::from_key(id)}
                on:click={msg}
                class="px-4 h-9 items-center justify-center rounded-[8] bg-[colors.accent]"
                hover_bg={colors.accent_strong}
            >
                <text color={colors.on_accent} class="font-semibold">{label}</text>
            </div>
        }
    }
}

impl UiApp for A11yDemo {
    type Action = Msg;
    type Message = ();

    fn view(&mut self, cx: &mut ViewContext) -> AnyElement {
        let (width, height) = cx.frame.size();
        let scale = cx.theme.metrics.ui_scale();
        let toasts = ToastStack::new(
            &self.toasts,
            cx.animations(),
            width,
            height,
            scale,
            0.0,
            0,
            &[],
            |index| Msg::Dismiss(index).into(),
        )
        .build();
        let colors = &cx.theme.colors;
        view! {
            <div w={width} h={height} class="bg-[colors.background] items-center justify-center">
                <div
                    accessibility_id="a11y.dialog"
                    accessibility_role={Role::Dialog}
                    aria-label="Settings"
                    class="w-[420] p-6 gap-4 flex-col rounded-[16] bg-[colors.surface]"
                >
                    <text_input("Name", "")
                        field={&self.name}
                        placeholder="Your name"
                        focus_target={NAME_FIELD}
                        focused={cx.is_focused(NAME_FIELD)}
                        class="w-full"
                        h={52.0}
                    />
                    <checkbox(self.remember) label="Remember me" on:toggle={Msg::Remember} />
                    <toggle(self.notify) label="Notifications" on:toggle={Msg::Notify} />
                    <div class="flex-row gap-2">
                        {Self::button("a11y.save", "Save", Msg::Save, cx)}
                        {Self::button("a11y.check", "Check", Msg::Check, cx)}
                    </div>
                </div>
                {toasts}
            </div>
        }
    }

    fn update(&mut self, msg: Msg, cx: &mut UiContext) {
        match msg {
            Msg::Remember => self.remember = !self.remember,
            Msg::Notify => self.notify = !self.notify,
            Msg::Save => {
                self.next_toast += 1;
                self.toasts.push(Toast {
                    id: self.next_toast,
                    kind: ToastKind::Info,
                    message: format!("Settings saved ({})", self.next_toast),
                    description: None,
                    created_at_ms: 0,
                    hovered: false,
                    progress: None,
                    ..Default::default()
                });
            }
            Msg::Check => cx.announce("All checks passed", Politeness::Polite),
            Msg::Dismiss(index) => {
                if index < self.toasts.len() {
                    self.toasts.remove(index);
                }
            }
        }
        cx.window.request_redraw();
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
        A11yDemo {
            name: TextField::new(""),
            remember: false,
            notify: true,
            toasts: Vec::new(),
            next_toast: 0,
            started: std::time::Instant::now(),
        },
        WindowOptions {
            title: "Quark Accessibility".into(),
            size: (640.0, 480.0),
            ..WindowOptions::default()
        },
    )
}
