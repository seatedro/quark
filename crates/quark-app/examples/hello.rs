//! Opens a window with a card; click the card to change its color, press
//! Escape to quit.

use quark::Color;
use quark::Rect;
use quark::scene::{
    FontKind, FontWeight, RoundedRectPrimitive, Scene, ShadowPrimitive, TextPrimitive,
};
use quark_app::winit::event::{ElementState, MouseButton};
use quark_app::winit::keyboard::NamedKey;
use quark_app::{App, EventContext, FrameContext, InputEvent, WindowOptions};

const COLORS: [Color; 3] = [
    Color::rgba(88, 101, 242, 255),
    Color::rgba(235, 69, 158, 255),
    Color::rgba(87, 242, 135, 255),
];

#[derive(Default)]
struct Hello {
    color: usize,
    card: Rect,
}

impl App for Hello {
    fn frame(&mut self, cx: &mut FrameContext) -> Scene {
        let (width, height) = cx.size();
        let scale = cx.scale_factor();
        self.card = Rect {
            x: 0.0,
            y: 0.0,
            width,
            height: height,
        }
        .center(320.0 * scale, 160.0 * scale);

        let mut scene = Scene::default();
        scene.rect(quark::scene::RectPrimitive {
            rect: Rect {
                x: 0.0,
                y: 0.0,
                width,
                height,
            },
            color: Color::rgba(24, 24, 27, 255),
        });
        scene.shadow(ShadowPrimitive {
            rect: self.card,
            blur_radius: 24.0 * scale,
            corner_radius: 16.0 * scale,
            offset: [0.0, 8.0 * scale],
            color: Color::rgba(0, 0, 0, 160),
        });
        scene.rounded_rect(RoundedRectPrimitive::uniform(
            self.card,
            16.0 * scale,
            COLORS[self.color],
        ));
        scene.text(TextPrimitive {
            rect: self.card.inset(24.0 * scale),
            text: "Hello from Quark. Click me, Esc quits.".into(),
            color: Color::rgba(255, 255, 255, 255),
            font_size: 20.0 * scale,
            font_kind: FontKind::Ui,
            font_weight: FontWeight::Semibold,
        });
        scene
    }

    fn event(&mut self, event: InputEvent, cx: &mut EventContext) {
        match event {
            InputEvent::KeyPress(chord) if chord.named() == Some(NamedKey::Escape) => cx.exit(),
            InputEvent::PointerButton {
                button: MouseButton::Left,
                state: ElementState::Pressed,
            } => {
                if let Some((x, y)) = cx.pointer_position()
                    && self.card.contains(x, y)
                {
                    self.color = (self.color + 1) % COLORS.len();
                    cx.request_redraw();
                }
            }
            _ => {}
        }
    }
}

fn main() -> Result<(), quark_app::RunError> {
    quark_app::run(
        Hello::default(),
        WindowOptions {
            title: "Hello Quark".into(),
            size: (640.0, 400.0),
            ..WindowOptions::default()
        },
    )
}
