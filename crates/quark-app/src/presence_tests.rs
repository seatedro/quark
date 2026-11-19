//! Divs entering and leaving through the whole adapter path, audited at
//! 120 Hz with [`MotionAudit`].

use quark::Color;
use quark::scene::{Primitive, Scene};
use quark_ui::animation::Transition;
use quark_ui::element::{AnyElement, IntoAnyElement, div};
use quark_ui::style::Styled;

use crate::testing::{MotionAudit, UiTestHarness};
use crate::ui::{UiApp, UiContext, ViewContext};

const CARD: Color = Color::rgba(200, 40, 40, 255);
/// The most a [`Transition`]'s default motion moves opacity in one 120 Hz
/// frame; a snap moves it much further.
const MAX_STEP: f32 = 0.08;

struct Card {
    shown: bool,
}

impl UiApp for Card {
    type Action = ();
    type Message = ();

    fn view(&mut self, _cx: &mut ViewContext) -> AnyElement {
        let card = div()
            .key("card")
            .w(80.0)
            .h(40.0)
            .bg(CARD)
            .presence(Transition::fade().and(Transition::slide(0.0, 12.0)));
        div()
            .size_full()
            .children(self.shown.then(|| card.into_any()))
            .into_any()
    }

    fn update(&mut self, _action: (), _cx: &mut UiContext) {}
}

/// The opacity of the card's chunk in a frame, `None` when not painted.
fn card_opacity(scene: &Scene) -> Option<f32> {
    scene.primitives.iter().find_map(|p| match p {
        Primitive::Chunk(placed)
            if placed
                .chunk
                .primitives()
                .iter()
                .any(|p| matches!(p, Primitive::RoundedRect(r) if r.color == CARD)) =>
        {
            Some(placed.layer.opacity)
        }
        _ => None,
    })
}

fn shown_card() -> UiTestHarness<Card> {
    let mut ui = UiTestHarness::new(Card { shown: true }, (200.0, 100.0), 2.0);
    ui.set_accessibility_active(false);
    ui
}

#[test]
fn hidden_card_fades_out_at_120_hz_without_snapping_or_vanishing() {
    let mut ui = shown_card();
    ui.app_mut().shown = false;
    let audit = MotionAudit::until_settled(&mut ui, 2_000, card_opacity);
    assert_eq!(audit.samples()[0].1, Some(1.0));
    audit.assert_monotonic(false);
    audit.assert_no_snaps(MAX_STEP);
    audit.assert_no_vanishing(0.0, 0.01);
    assert_eq!(card_opacity(ui.frame()), None);
}

#[test]
fn card_shown_again_mid_exit_eases_back_without_a_pop() {
    let mut ui = shown_card();
    ui.app_mut().shown = false;
    let out = MotionAudit::for_ms(&mut ui, 100, card_opacity);
    ui.app_mut().shown = true;
    let back = MotionAudit::until_settled(&mut ui, 2_000, card_opacity);
    assert!(out.last().is_some_and(|o| o < 0.9), "{:?}", out.samples());
    let audit = out.then(back);
    audit.assert_no_snaps(MAX_STEP);
    audit.assert_no_vanishing(0.0, 0.01);
    assert_eq!(audit.last(), Some(1.0));
}
