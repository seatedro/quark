//! Disclosures expand through their measured height and unmount only after
//! collapsing; reduced motion makes them, and skeletons, still.

use quark::Rect;
use quark::reactive::SignalStore;
use quark_components::{DisclosurePhase, DisclosureState, skeleton_lines};
use quark_render::{EffectType, Primitive, Scene};
use quark_ui::animation::AnimationTable;
use quark_ui::element::{AnyElement, ElementContext, IntoAnyElement, div, render_element};
use quark_ui::style::Styled;
use quark_ui::theme::Theme;

const CONTENT_H: f32 = 120.0;

struct Painted {
    scene: Scene,
    frame: quark::SemanticFrame,
}

fn paint(theme: &Theme, root: AnyElement) -> Painted {
    let mut text = quark_text::TextSystem::vendored_only(&Default::default());
    let mut layouts = quark_text::LayoutCache::default();
    let store = SignalStore::new();
    let mut cx = ElementContext::new(theme, 1.0, &mut text, &mut layouts, None, &store);
    let mut root = root;
    let mut scene = Scene::default();
    render_element(&mut root, &mut scene, &mut cx, 400.0, 400.0);
    Painted {
        scene,
        frame: std::mem::take(&mut cx.semantic),
    }
}

fn bounds(painted: &Painted, test_id: &str) -> Option<Rect> {
    painted
        .frame
        .nodes()
        .iter()
        .find(|n| n.test_id.as_ref().is_some_and(|t| t.as_str() == test_id))
        .map(|n| n.bounds)
}

/// One frame at `now_ms`: tick the table and the disclosure, then paint
/// the region over a marker row. Returns the marker's top (the height the
/// region takes) and whether the content is painted.
fn frame(
    state: &mut DisclosureState,
    table: &mut AnimationTable,
    theme: &Theme,
    now_ms: u64,
) -> (f32, bool) {
    table.tick(now_ms);
    state.tick(table, now_ms, theme.reduced_motion);
    let content = div().test_id("content").w_full().h(CONTENT_H);
    let mut column = div().flex_col().w_full();
    if let Some(region) = state.region(table, content) {
        column = column.child(region);
    }
    column = column.child(div().test_id("below").w_full().h(10.0));
    let painted = paint(theme, column.into_any());
    let below = bounds(&painted, "below").expect("marker");
    (below.y, bounds(&painted, "content").is_some())
}

// Catches a disclosure that jumps open, never settles, or keeps clipping
// (and drawing frames) once open.
#[test]
fn disclosure_expands_through_its_measured_height_then_rests() {
    let theme = Theme::default_light();
    let mut table = AnimationTable::new();
    let mut state = DisclosureState::new("tool", false);
    assert_eq!(frame(&mut state, &mut table, &theme, 0), (0.0, false));

    state.set_open(true);
    assert_eq!(frame(&mut state, &mut table, &theme, 10).0, 0.0);
    assert_eq!(state.phase(&table), DisclosurePhase::Opening);
    let (midway, _) = frame(&mut state, &mut table, &theme, 90);
    assert!(midway > 0.0 && midway < CONTENT_H, "{midway}");

    let rest = frame(&mut state, &mut table, &theme, 200);
    assert_eq!(rest, (CONTENT_H, true));
    assert_eq!(state.phase(&table), DisclosurePhase::Open);
    assert_eq!(table.next_deadline(), None, "no frames once open");
}

// Catches content unmounted as soon as it closes, which cuts the collapse,
// or left mounted after it.
#[test]
fn collapsing_content_stays_mounted_until_the_motion_ends() {
    let theme = Theme::default_light();
    let mut table = AnimationTable::new();
    let mut state = DisclosureState::new("tool", true);
    assert_eq!(frame(&mut state, &mut table, &theme, 0), (CONTENT_H, true));

    state.set_open(false);
    assert_eq!(frame(&mut state, &mut table, &theme, 10), (CONTENT_H, true));
    let (midway, mounted) = frame(&mut state, &mut table, &theme, 90);
    assert!(mounted && midway > 0.0 && midway < CONTENT_H, "{midway}");
    assert_eq!(state.phase(&table), DisclosurePhase::Closing);

    assert_eq!(frame(&mut state, &mut table, &theme, 200), (0.0, false));
    assert_eq!(state.phase(&table), DisclosurePhase::Closed);
}

// Catches reduced motion still animating a disclosure: it opens and
// closes in one frame and schedules nothing.
#[test]
fn reduced_motion_discloses_at_once_without_frames() {
    let theme = Theme {
        reduced_motion: true,
        ..Theme::default_light()
    };
    let mut table = AnimationTable::new();
    let mut state = DisclosureState::new("tool", false);
    state.set_open(true);
    assert_eq!(frame(&mut state, &mut table, &theme, 0), (CONTENT_H, true));
    assert_eq!(table.next_deadline(), None);
    state.set_open(false);
    assert_eq!(frame(&mut state, &mut table, &theme, 16), (0.0, false));
    assert_eq!(table.next_deadline(), None);
}

// Catches a skeleton that keeps shimmering under reduced motion; the
// shimmer is time-driven and needs frames.
#[test]
fn skeletons_shimmer_unless_motion_is_reduced() {
    for reduced_motion in [false, true] {
        let theme = Theme {
            reduced_motion,
            ..Theme::default_light()
        };
        let painted = paint(&theme, skeleton_lines(3, &theme));
        let shimmers = painted
            .scene
            .primitives
            .iter()
            .filter(
                |p| matches!(p, Primitive::EffectQuad(e) if e.effect_type == EffectType::Shimmer),
            )
            .count();
        let fills = painted
            .scene
            .primitives
            .iter()
            .filter(|p| matches!(p, Primitive::RoundedRect(_)))
            .count();
        let expected = if reduced_motion { (0, 3) } else { (3, 0) };
        assert_eq!(
            (shimmers, fills),
            expected,
            "reduced_motion {reduced_motion}"
        );
    }
}
