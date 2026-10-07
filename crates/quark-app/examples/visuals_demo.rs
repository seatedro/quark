//! Visual primitives: a card that fades as one group, a tile that turns
//! (and is still clicked where it is drawn), a frosted panel with uneven
//! corners, path charts drawn on the GPU, and an animated image that plays
//! on the frame clock. Click the card and the tile; Escape quits.

use std::f32::consts::{FRAC_PI_4, TAU};

use accesskit::Role;
use quark::{FillRule, LineCap, LineJoin, Path, StrokeStyle};
use quark_app::quark_ui::Action;
use quark_app::quark_ui::animation::{Curve, Motion, Prop};
use quark_app::quark_ui::element::{
    AnimatedImage, AnyElement, ImageFrames, IntoAnyElement, animated_image, div, path_canvas,
    sparkline_path, text,
};
use quark_app::quark_ui::style::Styled;
use quark_app::quark_ui::theme::Color;
use quark_app::winit::keyboard::NamedKey;
use quark_app::{InputEvent, UiApp, UiContext, ViewContext, WindowOptions};

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    ToggleFade,
    Turn,
}

impl From<Msg> for Action {
    fn from(msg: Msg) -> Self {
        Action::new(msg)
    }
}

struct Visuals {
    faded: bool,
    turns: u32,
    spinner: AnimatedImage,
    series: Vec<f32>,
}

impl Visuals {
    fn new() -> Self {
        Self {
            faded: false,
            turns: 0,
            spinner: AnimatedImage::from_frames(spinner_frames()),
            series: (0..48)
                .map(|i| {
                    let x = i as f32 * 0.35;
                    x.sin() * 3.0 + (x * 2.7).cos() + i as f32 * 0.08
                })
                .collect(),
        }
    }
}

/// Eight 32x32 frames of a bar sweeping across, 80 ms each. Apps decode
/// GIF, WebP, or APNG bytes with `AnimatedImage::decode` (the `images`
/// feature) instead.
fn spinner_frames() -> ImageFrames {
    const SIDE: u32 = 32;
    let frames = (0..8)
        .map(|frame| {
            let mut rgba = vec![0u8; (SIDE * SIDE * 4) as usize];
            for y in 0..SIDE {
                for x in 0..SIDE {
                    let lit = x / 4 == frame;
                    let at = ((y * SIDE + x) * 4) as usize;
                    let shade = if lit {
                        [120, 200, 255, 255]
                    } else {
                        [30, 40, 60, 255]
                    };
                    rgba[at..at + 4].copy_from_slice(&shade);
                }
            }
            (rgba, 80)
        })
        .collect();
    ImageFrames::from_rgba(SIDE, SIDE, frames, 0x5eed)
}

fn button(id: &str, label: &str, msg: Msg) -> quark_app::quark_ui::element::Div {
    div()
        .accessibility_id(id)
        .accessibility_role(Role::Button)
        .accessibility_label(label)
        .on_click(msg)
}

/// A five-pointed star; even-odd leaves its pentagon center empty.
fn star(cx: f32, cy: f32, r: f32) -> Path {
    let mut builder = Path::builder();
    for i in 0..5 {
        let angle = -TAU / 4.0 + i as f32 * TAU * 2.0 / 5.0;
        let (x, y) = (cx + r * angle.cos(), cy + r * angle.sin());
        if i == 0 {
            builder.move_to(x, y);
        } else {
            builder.line_to(x, y);
        }
    }
    builder.close();
    builder.build()
}

impl UiApp for Visuals {
    type Action = Msg;
    type Message = ();

    fn view(&mut self, cx: &mut ViewContext) -> AnyElement {
        let (width, height) = cx.frame.size();
        let colors = cx.theme.colors;
        let card = button("visuals.fade", "Fade", Msg::ToggleFade)
            .key("visuals.fade")
            .w(180.0)
            .h(120.0)
            .p(16.0)
            .gap(8.0)
            .flex_col()
            .rounded(12.0)
            .bg(colors.surface)
            .border(colors.border)
            .opacity(if self.faded { 0.35 } else { 1.0 })
            .transition(Prop::Opacity, Motion::tween(220, Curve::EaseOutCubic))
            .child(text("Group opacity").semibold())
            .child(
                div()
                    .test_id("visuals.swatch")
                    .w(60.0)
                    .h(24.0)
                    .rounded(6.0)
                    .bg(colors.accent),
            )
            .child(text("click to fade").color(colors.text_muted));

        let tile = button("visuals.turn", "Turn", Msg::Turn)
            .key("visuals.turn")
            .w(90.0)
            .h(90.0)
            .items_center()
            .justify_center()
            .rounded(10.0)
            .bg(colors.accent)
            .rotate(self.turns as f32 * FRAC_PI_4)
            .transition(Prop::Transform, Motion::spring(260.0, 22.0, 1.0))
            .child(text("Turn").color(colors.text_strong).semibold());

        let stripes = (0..6).map(|i| {
            let shade = if i % 2 == 0 {
                colors.accent
            } else {
                colors.surface
            };
            div().w(30.0).h_full().bg(shade).into_any()
        });
        let frosted = div()
            .relative()
            .w(180.0)
            .h(120.0)
            .flex_row()
            .children(stripes)
            .child(
                div()
                    .absolute()
                    .top(20.0)
                    .left(20.0)
                    .w(140.0)
                    .h(80.0)
                    .rounded_corners([28.0, 4.0, 28.0, 4.0])
                    .blur(10.0)
                    .items_center()
                    .justify_center()
                    .child(text("Frosted").color(colors.text_strong)),
            );

        let series = self.series.clone();
        let (accent, strong) = (colors.accent, colors.text_strong);
        let chart = path_canvas(move |painter| {
            let (w, h) = painter.size();
            let line = sparkline_path(&series, w, h - 4.0);
            let mut area = Path::builder();
            for (i, verb) in line.verbs().iter().enumerate() {
                if let quark::PathVerb::MoveTo(p) | quark::PathVerb::LineTo(p) = *verb {
                    if i == 0 {
                        area.move_to(p[0], p[1]);
                    } else {
                        area.line_to(p[0], p[1]);
                    }
                }
            }
            area.line_to(w, h).line_to(0.0, h).close();
            painter.fill(area.build(), Color { a: 70, ..accent });
            painter.stroke(
                line,
                accent,
                StrokeStyle::new(2.0)
                    .join(LineJoin::Round)
                    .cap(LineCap::Round),
            );
        })
        .w(260.0)
        .h(110.0)
        .clip();

        let shapes = path_canvas(move |painter| {
            painter.fill_with_rule(star(55.0, 55.0, 48.0), accent, FillRule::EvenOdd);
            let mut ring = Path::builder();
            ring.arc(150.0, 55.0, 36.0, 0.0, TAU * 0.8);
            painter.stroke(
                ring.build(),
                strong,
                StrokeStyle::new(8.0).cap(LineCap::Round),
            );
        })
        .w(200.0)
        .h(110.0);

        let row = |children: Vec<AnyElement>| {
            div().flex_row().gap(24.0).items_center().children(children)
        };
        div()
            .w(width)
            .h(height)
            .p(24.0)
            .gap(24.0)
            .flex_col()
            .bg(colors.background)
            .child(row(vec![
                card.into_any(),
                tile.into_any(),
                frosted.into_any(),
            ]))
            .child(row(vec![
                chart.into_any(),
                shapes.into_any(),
                animated_image(&self.spinner).size(48.0, 48.0).into_any(),
            ]))
            .into_any()
    }

    fn update(&mut self, msg: Msg, _cx: &mut UiContext) {
        match msg {
            Msg::ToggleFade => self.faded = !self.faded,
            Msg::Turn => self.turns += 1,
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
        Visuals::new(),
        WindowOptions {
            title: "Quark visuals".into(),
            size: (760.0, 360.0),
            ..WindowOptions::default()
        },
    )
}

#[cfg(test)]
mod tests {
    use quark_app::testing::{By, UiTestHarness};

    use super::*;

    fn harness() -> UiTestHarness<Visuals> {
        UiTestHarness::new(Visuals::new(), (760.0, 360.0), 2.0)
    }

    /// The tile's center and half side, in points.
    fn tile_center(ui: &mut UiTestHarness<Visuals>) -> (f32, f32) {
        let bounds = ui.find(By::role_name(Role::Button, "Turn")).bounds;
        (
            bounds.x + bounds.width / 2.0,
            bounds.y + bounds.height / 2.0,
        )
    }

    // Catches a click routed by the tile's unturned rect: once it has
    // turned 45 degrees, a click past its old side on the diagonal's tip
    // turns it again, and its old corner is dead.
    #[test]
    fn turned_tile_takes_clicks_where_it_is_drawn() {
        let mut ui = harness();
        let (cx, cy) = tile_center(&mut ui);
        ui.click((cx, cy));
        ui.advance(2_000);
        assert_eq!(ui.app().turns, 1);

        // Half side 45; the turned tip reaches 63 along the axis.
        ui.click((cx + 58.0, cy));
        assert_eq!(ui.app().turns, 2, "tip missed");
        ui.advance(2_000);
        // Two turns is 90 degrees: the corners are back. Turn once more.
        ui.click((cx, cy));
        ui.advance(2_000);
        ui.click((cx - 43.0, cy - 43.0));
        assert_eq!(ui.app().turns, 3, "old corner took the click");
    }

    // The faded card at 2x: through scene_to_physical and an offscreen
    // layer, its child swatch and the card behind it fade together.
    #[test]
    fn faded_card_dims_its_child_at_scale_two() {
        let mut ui = harness();
        let Some(before) = pixels(&mut ui) else {
            return;
        };
        let swatch = ui.find(By::test_id("visuals.swatch")).bounds;
        let (x, y) = (
            ((swatch.x + swatch.width / 2.0) * 2.0) as u32,
            ((swatch.y + swatch.height / 2.0) * 2.0) as u32,
        );
        let solid = before.pixel(x, y);
        ui.click_node(By::role_name(Role::Button, "Fade"));
        ui.advance(1_000);
        let after = pixels(&mut ui).expect("adapter found before");
        let faded = after.pixel(x, y);
        let background = after.pixel(2, 2);
        for channel in 0..3 {
            let (lo, hi) = (
                solid[channel].min(background[channel]),
                solid[channel].max(background[channel]),
            );
            assert!(
                (lo..=hi).contains(&faded[channel]),
                "{faded:?} not between {solid:?} and {background:?}"
            );
        }
        assert_ne!(faded, solid, "swatch did not fade");
    }

    fn pixels(ui: &mut UiTestHarness<Visuals>) -> Option<quark_app::testing::Pixels> {
        match ui.render_rgba() {
            Ok(pixels) => Some(pixels),
            Err(quark_render::RenderError::NoAdapter) => {
                assert!(
                    std::env::var_os("QUARK_REQUIRE_GPU").is_none(),
                    "QUARK_REQUIRE_GPU is set but no wgpu adapter is available"
                );
                None
            }
            Err(error) => panic!("render failed: {error}"),
        }
    }
}
