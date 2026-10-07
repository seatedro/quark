//! Declarative transitions and springs.
//!
//! The buttons tween their background on hover. "Toggle panel" slides a
//! side panel with a spring; click it again mid-slide and the panel turns
//! around with its velocity intact instead of restarting. Escape quits.

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

fn button(key: &str, label: &str, idle: Color, hover: Color, msg: Option<Msg>) -> AnyElement {
    let button = div()
        .key(key)
        .test_id(key)
        .px(16.0)
        .h(36.0)
        .items_center()
        .justify_center()
        .rounded(8.0)
        .bg(idle)
        .hover_bg(hover)
        .transition(Prop::Background, Motion::tween(180, Curve::EaseOutCubic))
        .child(text(label).semibold());
    match msg {
        Some(msg) => button.on_click(msg).into_any(),
        None => button.into_any(),
    }
}

impl UiApp for AnimationDemo {
    type Action = Msg;
    type Message = ();

    fn view(&mut self, cx: &mut ViewContext) -> AnyElement {
        let (width, height) = cx.frame.size();
        let colors = &cx.theme.colors;
        let swatches = [
            (
                "demo.blue",
                "Blue",
                Color::rgba(37, 99, 235, 255),
                Color::rgba(250, 204, 21, 255),
            ),
            (
                "demo.red",
                "Red",
                Color::rgba(220, 38, 38, 255),
                Color::rgba(16, 185, 129, 255),
            ),
            (
                "demo.ghost",
                "Ghost",
                Color::TRANSPARENT,
                Color::rgba(168, 85, 247, 255),
            ),
        ];
        let panel_x = if self.panel_open {
            0.0
        } else {
            -PANEL_W - 24.0
        };
        div()
            .w(width)
            .h(height)
            .bg(colors.background)
            .child(
                div()
                    .key("demo.panel")
                    .test_id("demo.panel")
                    .absolute()
                    .top(0.0)
                    .left(0.0)
                    .w(PANEL_W)
                    .h(height)
                    .p(24.0)
                    .flex_col()
                    .gap(12.0)
                    .bg(colors.surface)
                    .clip()
                    .translate(panel_x, 0.0)
                    // Underdamped so the slide settles with a small overshoot.
                    .transition(Prop::Transform, Motion::spring(260.0, 22.0, 1.0))
                    .child(text("Panel").text_lg().bold())
                    .child(text("Sprung with stiffness 260, damping 22.").color(colors.text_muted)),
            )
            .child(
                div()
                    .w_full()
                    .h_full()
                    .items_center()
                    .justify_center()
                    .flex_col()
                    .gap(16.0)
                    .child(
                        div().flex_row().gap(8.0).children(swatches.into_iter().map(
                            |(key, label, idle, hover)| button(key, label, idle, hover, None),
                        )),
                    )
                    .child(button(
                        "demo.toggle",
                        "Toggle panel",
                        colors.accent,
                        colors.accent_strong,
                        Some(Msg::TogglePanel),
                    )),
            )
            .into_any()
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
