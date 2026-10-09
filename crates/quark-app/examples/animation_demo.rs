//! Declarative transitions and springs.
//!
//! The buttons tween their background on hover. "Toggle panel" slides a
//! side panel with a spring; click it again mid-slide and the panel turns
//! around with its velocity intact instead of restarting. Escape quits.

use quark::view;
use quark_app::quark_ui::Action;
use quark_app::quark_ui::animation::{Curve, Motion, Prop};
use quark_app::quark_ui::element::{AnyElement, IntoAnyElement, div, text};
use quark_app::quark_ui::style::Styled;
use quark_app::quark_ui::theme::Color;
use quark_app::winit::keyboard::NamedKey;
use quark_app::{InputEvent, UiApp, UiContext, ViewContext, WindowOptions};

const PANEL_W: f32 = 260.0;

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    TogglePanel,
}

impl From<Msg> for Action {
    fn from(msg: Msg) -> Self {
        Action::new(msg)
    }
}

struct AnimationDemo {
    panel_open: bool,
}

/// A button whose fill animates from `idle` to `hover`, with a label color
/// readable on each: `ink` and `hover_ink`.
fn button(
    key: &str,
    label: &str,
    (idle, ink): (Color, Color),
    (hover, hover_ink): (Color, Color),
    msg: Option<Msg>,
) -> AnyElement {
    view! {
        <div
            key={key}
            test_id={key}
            class="px-4 h-9 items-center justify-center rounded-[8] bg-[idle]"
            hover_bg={hover}
            hover_text_color={hover_ink}
            transition={(Prop::Background, Motion::tween(180, Curve::EaseOutCubic))}
            on:click={if let Some(msg) = msg {
                msg
            }}
        >
            <text class="font-semibold" color={ink}>{label}</text>
        </div>
    }
}

impl UiApp for AnimationDemo {
    type Action = Msg;
    type Message = ();

    fn view(&mut self, cx: &mut ViewContext) -> AnyElement {
        let (width, height) = cx.frame.size();
        let colors = &cx.theme.colors;
        let (white, black) = (Color::rgba(255, 255, 255, 255), Color::rgba(0, 0, 0, 255));
        let swatches = [
            (
                "demo.blue",
                "Blue",
                (Color::rgba(37, 99, 235, 255), white),
                (Color::rgba(250, 204, 21, 255), black),
            ),
            (
                "demo.red",
                "Red",
                (Color::rgba(220, 38, 38, 255), white),
                (Color::rgba(16, 185, 129, 255), black),
            ),
            (
                "demo.ghost",
                "Ghost",
                (Color::TRANSPARENT, colors.text),
                (Color::rgba(168, 85, 247, 255), black),
            ),
        ];
        let panel_x = if self.panel_open {
            0.0
        } else {
            -PANEL_W - 24.0
        };
        view! {
            <div w={width} h={height} class="bg-[colors.background]">
                <div
                    key="demo.panel"
                    test_id="demo.panel"
                    class="absolute top-0 left-0 w-[PANEL_W] h-[height] p-6 flex-col gap-3
                            bg-[colors.surface] overflow-clip"
                    translate={(panel_x, 0.0)}
                    // Underdamped so the slide settles with a small overshoot.
                    transition={(Prop::Transform, Motion::spring(260.0, 22.0, 1.0))}
                >
                    <text class="text-lg font-bold">"Panel"</text>
                    <text color={colors.text_muted}>"Sprung with stiffness 260, damping 22."</text>
                </div>
                <div class="w-full h-full items-center justify-center flex-col gap-4">
                    <div class="flex-row gap-2">
                        for (key, label, idle, hover) in swatches {
                            {button(key, label, idle, hover, None)}
                        }
                    </div>
                    {button(
                        "demo.toggle",
                        "Toggle panel",
                        (colors.accent, colors.on_accent),
                        (colors.accent_strong, colors.on_accent),
                        Some(Msg::TogglePanel),
                    )}
                </div>
            </div>
        }
    }

    fn update(&mut self, msg: Msg, cx: &mut UiContext) {
        match msg {
            Msg::TogglePanel => self.panel_open = !self.panel_open,
        }
        cx.window.request_redraw();
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
        AnimationDemo { panel_open: false },
        WindowOptions {
            title: "Quark animation".into(),
            size: (720.0, 420.0),
            ..WindowOptions::default()
        },
    )
}
