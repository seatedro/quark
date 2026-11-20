use std::collections::HashSet;

use proptest::prelude::*;

use super::*;
use crate::color::Color;
use crate::geometry::Rect;
use crate::reactive::SignalStore;
use crate::scene::LayerTransform;

const K: AnimKey = AnimKey(1);
const OPACITY: PropId = PropId(0);
const X: PropId = PropId(1);

fn close(a: f32, b: f32, eps: f32) -> bool {
    (a - b).abs() <= eps
}

use crate::test_support::proptest_config as config;

fn curve() -> impl Strategy<Value = Curve> {
    let unit = 0.0f32..=1.0;
    prop_oneof![
        Just(Curve::Linear),
        Just(Curve::EaseOutCubic),
        Just(Curve::EaseOutQuint),
        Just(Curve::EaseInOutCubic),
        (unit.clone(), unit.clone(), unit.clone(), unit)
            .prop_map(|(x1, y1, x2, y2)| { Curve::CubicBezier { x1, y1, x2, y2 } }),
    ]
}

/// Stiffness spans soft to very stiff (log-uniform) and damping ratios
/// span underdamped to overdamped, so every spring settles in well under
/// a minute.
fn spring() -> impl Strategy<Value = SpringParams> {
    (3.0f32..10.8, 0.5f32..5.0, 0.3f32..2.0).prop_map(|(ln_k, mass, zeta)| {
        let stiffness = ln_k.exp();
        SpringParams {
            stiffness,
            damping: 2.0 * zeta * (stiffness * mass).sqrt(),
            mass,
        }
    })
}

fn motion() -> impl Strategy<Value = Motion> {
    prop_oneof![
        spring().prop_map(Motion::Spring),
        (1u32..800, curve()).prop_map(|(duration, curve)| Motion::tween(duration, curve)),
        (0.05f32..1.0, -0.9f32..0.9).prop_map(|(d, b)| Motion::spring_duration(d, b)),
    ]
}

fn frame_gaps() -> impl Strategy<Value = Vec<u64>> {
    prop::collection::vec(1u64..=250, 1..8)
}

/// Ticks with the repeating `gaps` until the row settles, checking the
/// value and velocity stay finite. Returns the settle time.
fn tick_until_settled(
    t: &mut AnimationTable,
    gaps: &[u64],
    mut now: u64,
) -> Result<u64, TestCaseError> {
    let deadline = now + 60_000;
    for gap in gaps.iter().cycle() {
        now += gap;
        let moving = t.tick(now);
        let (x, v) = (
            t.get(K, X).unwrap_or(f32::NAN),
            t.velocity(K, X).unwrap_or(f32::NAN),
        );
        prop_assert!(x.is_finite() && v.is_finite(), "x = {x}, v = {v} at {now}");
        if !moving {
            return Ok(now);
        }
        prop_assert!(now < deadline, "still moving at {now} ms");
    }
    unreachable!("gaps is non-empty")
}

proptest! {
    #![proptest_config(config(64))]

    // Catches the bezier solver or a curve formula missing 0 or 1.
    #[test]
    fn curve_eval_maps_endpoints_to_zero_and_one(curve in curve()) {
        prop_assert!(close(curve.eval(0.0), 0.0, 1e-5), "{curve:?}");
        prop_assert!(close(curve.eval(1.0), 1.0, 1e-5), "{curve:?}");
    }

    // Catches the Newton/bisection solver landing on the wrong root,
    // which shows up as a tween stepping backwards.
    #[test]
    fn curve_eval_with_unit_controls_never_decreases(
        curve in curve(),
        a in 0.0f32..=1.0,
        b in 0.0f32..=1.0,
    ) {
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        prop_assert!(curve.eval(lo) <= curve.eval(hi) + 1e-4, "{curve:?} {lo} {hi}");
    }

    #[test]
    fn cubic_bezier_on_diagonal_is_identity(t in 0.0f32..=1.0) {
        let diagonal = Curve::CubicBezier { x1: 0.0, y1: 0.0, x2: 1.0, y2: 1.0 };
        prop_assert!(close(diagonal.eval(t), t, 1e-4));
    }

    // Catches unstable integration (NaN, blow-up) for stiff springs or
    // long frames and springs that never report rest.
    #[test]
    fn spring_random_params_and_frames_settle_exactly_on_target(
        params in spring(),
        start in -1000.0f32..1000.0,
        target in -1000.0f32..1000.0,
        gaps in frame_gaps(),
    ) {
        let mut t = AnimationTable::new();
        t.set(K, X, start, 0);
        t.animate_to(K, X, target, Motion::Spring(params), 0);
        tick_until_settled(&mut t, &gaps, 0)?;
        prop_assert_eq!(t.get(K, X), Some(target));
        prop_assert_eq!(t.velocity(K, X), Some(0.0));
    }

    // Retargeting must not reset velocity or value, which reads as a
    // visible jerk when a spring changes direction mid-flight; a tween
    // handing over to a spring keeps its speed too.
    #[test]
    fn spring_retarget_midflight_keeps_value_and_velocity(
        first_motion in motion(),
        params in spring(),
        first in -1000.0f32..1000.0,
        second in -1000.0f32..1000.0,
        gaps in frame_gaps(),
        frames in 0usize..20,
    ) {
        let mut t = AnimationTable::new();
        t.set(K, X, 0.0, 0);
        t.animate_to(K, X, first, first_motion, 0);
        let mut now = 0;
        for gap in gaps.iter().cycle().take(frames) {
            now += gap;
            t.tick(now);
        }
        let (x, v) = (t.get(K, X).unwrap_or(f32::NAN), t.velocity(K, X).unwrap_or(f32::NAN));
        t.animate_to(K, X, second, Motion::Spring(params), now);
        if t.is_animating(K, X) {
            prop_assert_eq!(t.get(K, X), Some(x));
            prop_assert_eq!(t.velocity(K, X), Some(v));
            tick_until_settled(&mut t, &gaps, now)?;
        } else {
            // Released at rest on the new target: it snaps by no more than
            // the rest threshold.
            let eps = t.rest_epsilon;
            prop_assert!(close(x, second, eps) && close(v, 0.0, eps), "{x} {v}");
        }
        prop_assert_eq!(t.get(K, X), Some(second));
    }

    // Values must be a function of the timestamp alone: a window at 30,
    // 60, or 120 Hz shows the same motion at the frames they share, even
    // with a retarget mid-flight and a caller that repeats its target
    // every frame, as style transitions do.
    #[test]
    fn trajectory_is_the_same_at_every_frame_rate(
        first_motion in motion(),
        second_motion in motion(),
        first in -1000.0f32..1000.0,
        second in -1000.0f32..1000.0,
        retarget_frame in 0u64..30,
        repeat_target in any::<bool>(),
    ) {
        let rates = [30u64, 60, 120];
        let runs = rates.map(|hz| {
            let mut t = AnimationTable::new();
            t.set(K, X, 0.0, 0);
            let mut samples = Vec::new();
            // Every 30 Hz frame time is also a 60 and 120 Hz frame time.
            let retarget_ms = (retarget_frame * 1000 + 15) / 30;
            let mut target = (first, first_motion);
            for frame in 0..=(2 * hz) {
                let now = (frame * 1000 + hz / 2) / hz;
                t.tick(now);
                if now == retarget_ms {
                    target = (second, second_motion);
                    t.animate_to(K, X, target.0, target.1, now);
                } else if frame == 0 || repeat_target {
                    t.animate_to(K, X, target.0, target.1, now);
                }
                samples.push((now, t.get(K, X), t.velocity(K, X)));
            }
            samples
        });
        for (now, value, velocity) in &runs[0] {
            for (hz, run) in rates.iter().zip(&runs).skip(1) {
                let at = run.iter().find(|s| s.0 == *now);
                if cfg!(miri) {
                    // Miri intentionally perturbs transcendental f32 ops by a
                    // few ULPs per call; compare within a tight relative bound.
                    let Some((got_now, got_v, got_vel)) = at else {
                        prop_assert!(false, "missing sample for {} Hz at {} ms", hz, now);
                        unreachable!();
                    };
                    prop_assert_eq!(got_now, now);
                    let near_opt = |a: Option<f32>, b: Option<f32>| match (a, b) {
                        (Some(a), Some(b)) => {
                            let tol = 1e-5 * a.abs().max(b.abs()).max(1.0);
                            (a - b).abs() <= tol
                        }
                        (None, None) => true,
                        _ => false,
                    };
                    prop_assert!(
                        near_opt(*got_v, *value) && near_opt(*got_vel, *velocity),
                        "{} Hz at {} ms: got ({:?}, {:?}), expected ({:?}, {:?})",
                        hz,
                        now,
                        got_v,
                        got_vel,
                        value,
                        velocity
                    );
                } else {
                    prop_assert_eq!(at, Some(&(*now, *value, *velocity)), "{} Hz at {} ms", hz, now);
                }
            }
        }
    }

    // Zero bounce is critically damped and must never overshoot; a bouncy
    // spring must swing past its target.
    #[test]
    fn duration_bounce_spring_overshoots_only_when_bouncy(
        duration in 0.1f32..1.0,
        bounce in prop_oneof![-0.95f32..=0.0, 0.3f32..0.9],
    ) {
        let mut t = AnimationTable::new();
        t.set(K, X, 0.0, 0);
        t.animate_to(K, X, 100.0, Motion::spring_duration(duration, bounce), 0);
        let (mut now, mut peak, mut last) = (0, 0.0f32, 0.0f32);
        while t.tick(now) {
            let x = t.get(K, X).unwrap_or(f32::NAN);
            if bounce <= 0.0 {
                prop_assert!(x >= last, "{x} after {last} at {now} ms");
            }
            (peak, last) = (peak.max(x), x);
            now += 8;
        }
        if bounce <= 0.0 {
            prop_assert!(peak <= 100.0, "overshot to {peak}");
        } else {
            prop_assert!(peak > 100.5, "peaked at {peak}");
        }
        prop_assert_eq!(t.get(K, X), Some(100.0));
    }

    // Each channel must land on its own row: a settled color reads back
    // exactly, and a fade in from transparent keeps the target's hue.
    #[test]
    fn animate_color_fades_in_with_the_target_hue_and_settles_on_it(
        r: u8, g: u8, b: u8,
    ) {
        let target = Color::rgba(r, g, b, 255);
        let base = PropId(10);
        let mut t = AnimationTable::new();
        t.set_color(K, base, Color::TRANSPARENT, 0);
        t.animate_color(K, base, target, Motion::tween(100, Curve::Linear), 0);
        t.tick(50);
        let mid = t.color(K, base).unwrap_or_default();
        let near = |x: u8, y: u8| x.abs_diff(y) <= 1;
        prop_assert!(near(mid.r, r) && near(mid.g, g) && near(mid.b, b), "{mid:?}");
        prop_assert!(near(mid.a, 128), "{mid:?}");
        t.tick(100);
        prop_assert_eq!(t.color(K, base), Some(target));
    }

    // Drives arbitrary mutations so the debug integrity check catches
    // index drift across swap-removals; also checks rows exist exactly
    // when the model says so and retiring never drops a moving row.
    #[test]
    fn animation_table_arbitrary_ops_keep_rows_and_index(
        ops in prop::collection::vec((0u8..10, 0u64..3, 0u16..3, -10.0f32..10.0, 0u64..120), 0..40),
    ) {
        let mut t = AnimationTable::new();
        let mut model: HashSet<(AnimKey, PropId)> = HashSet::new();
        let mut now = 0;
        for (op, key, prop, value, ms) in ops {
            let (key, prop) = (AnimKey(key), PropId(prop));
            match op {
                0 => {
                    t.set(key, prop, value, now);
                    model.insert((key, prop));
                }
                1 => {
                    let motion = Motion::Tween {
                        duration_ms: ms as u32,
                        delay_ms: (ms / 3) as u32,
                        curve: Curve::EaseOutCubic,
                    };
                    t.animate_to(key, prop, value, motion, now);
                    model.insert((key, prop));
                }
                2 => {
                    t.animate_to(key, prop, value, Motion::spring(300.0, 30.0, 1.0), now);
                    model.insert((key, prop));
                }
                3 => {
                    now += ms;
                    t.tick(now);
                }
                4 => {
                    prop_assert_eq!(t.remove(key, prop), model.remove(&(key, prop)));
                }
                5 => {
                    let before = model.len();
                    model.retain(|(k, _)| *k != key);
                    prop_assert_eq!(t.remove_key(key), before - model.len());
                }
                6 => {
                    let before = model.len();
                    model.retain(|(_, p)| *p != prop);
                    prop_assert_eq!(t.retain(|_, p| p != prop), before - model.len());
                }
                7 => {
                    let rect = Rect { x: value, y: -value, width: ms as f32, height: 4.0 };
                    t.animate_rect(key, rect, RectMotion::leading(Motion::smooth(), 1.5), now);
                    model.extend(RECT_PROPS.map(|p| (key, p)));
                }
                8 => {
                    let layer = LayerTransform { opacity: value, ..LayerTransform::IDENTITY };
                    t.animate_layer(key, layer, Motion::snappy(), now);
                    model.extend(LAYER_PROPS.map(|p| (key, p)));
                }
                _ => {
                    let moving: Vec<_> =
                        model.iter().copied().filter(|(k, p)| t.is_animating(*k, *p)).collect();
                    let removed = t.retire_settled(1e-3);
                    let before = model.len();
                    model.retain(|(k, p)| t.get(*k, *p).is_some());
                    prop_assert_eq!(removed, before - model.len());
                    for (k, p) in moving {
                        prop_assert!(t.get(k, p).is_some(), "retired moving row {k:?} {p:?}");
                    }
                }
            }
            prop_assert_eq!(t.verify_integrity(), Ok(()));
            prop_assert_eq!(t.len(), model.len());
            for (k, p) in &model {
                prop_assert!(t.get(*k, *p).is_some());
            }
        }
    }
}

#[test]
fn tween_linear_is_half_at_midpoint_and_target_at_end() {
    let mut t = AnimationTable::new();
    t.set(K, OPACITY, 0.0, 0);
    t.animate_to(K, OPACITY, 1.0, Motion::tween(100, Curve::Linear), 0);
    assert!(t.tick(50));
    assert!(close(t.get(K, OPACITY).unwrap_or(f32::NAN), 0.5, 1e-5));
    assert!(!t.tick(100));
    assert_eq!(t.get(K, OPACITY), Some(1.0));
}

#[test]
fn tween_retarget_midway_starts_from_current_value() {
    let mut t = AnimationTable::new();
    t.set(K, OPACITY, 0.0, 0);
    t.animate_to(K, OPACITY, 1.0, Motion::tween(100, Curve::Linear), 0);
    t.tick(40);
    t.animate_to(K, OPACITY, 0.0, Motion::tween(100, Curve::Linear), 40);
    t.tick(90);
    // From 0.4 toward 0.0, halfway.
    assert!(close(t.get(K, OPACITY).unwrap_or(f32::NAN), 0.2, 1e-5));
}

#[test]
fn spring_tick_after_long_stall_snaps_to_target() {
    let mut t = AnimationTable::new();
    t.set(K, X, 0.0, 0);
    t.animate_to(K, X, 10.0, Motion::spring(300.0, 30.0, 1.0), 0);
    assert!(!t.tick(600_000));
    assert_eq!(t.get(K, X), Some(10.0));
}

// Regression: found by spring_random_params_and_frames_settle_exactly_on_target.
// The spring stalled one ulp from 462.5025 with v = -3.15e-3 and stayed
// active forever.
#[test]
fn spring_stalled_by_f32_rounding_still_settles() {
    let mut t = AnimationTable::new();
    t.set(K, X, -342.61945, 0);
    t.animate_to(K, X, 462.5025, Motion::spring(12090.947, 117.11381, 0.5), 0);
    let mut now = 0;
    while t.tick(now) {
        now += 3;
        assert!(now < 10_000, "spring never settled");
    }
    assert_eq!(t.get(K, X), Some(462.5025));
}

#[test]
fn next_deadline_delayed_tween_reports_start_and_holds_value() {
    let mut t = AnimationTable::new();
    t.set(K, OPACITY, 0.0, 0);
    let motion = Motion::Tween {
        duration_ms: 100,
        delay_ms: 200,
        curve: Curve::EaseOutCubic,
    };
    t.animate_to(K, OPACITY, 1.0, motion, 0);
    assert_eq!(t.next_deadline(), Some(200));
    assert!(t.tick(100));
    // Not exact: Miri perturbs `powi` by an ulp, so eval(0) is ~1e-8.
    assert!(close(t.get(K, OPACITY).unwrap_or(f32::NAN), 0.0, 1e-6));
}

#[test]
fn next_deadline_reports_earliest_active_row_and_none_when_idle() {
    let mut t = AnimationTable::new();
    t.set(K, OPACITY, 0.0, 0);
    t.animate_to(K, OPACITY, 1.0, Motion::tween(100, Curve::Linear), 0);
    let other = AnimKey(2);
    t.set(other, X, 0.0, 50);
    t.animate_to(other, X, 1.0, Motion::tween(100, Curve::Linear), 50);
    t.tick(60);
    // The tween row was last advanced to 60, the same as the other row.
    assert_eq!(t.next_deadline(), Some(60));
    t.remove_key(other);
    assert!(!t.tick(100));
    assert_eq!(t.next_deadline(), None);
}

#[test]
fn animate_to_unknown_row_snaps_to_target() {
    let mut t = AnimationTable::new();
    t.animate_to(K, X, 3.0, Motion::tween(100, Curve::Linear), 0);
    assert_eq!(t.get(K, X), Some(3.0));
    assert!(!t.is_animating(K, X));
}

#[test]
fn retire_settled_removes_resting_rows_and_keeps_moving_ones() {
    let mut t = AnimationTable::new();
    t.set(K, X, 5.0, 0);
    t.set(K, OPACITY, 0.0, 0);
    t.animate_to(K, OPACITY, 1.0, Motion::tween(100, Curve::Linear), 0);
    assert_eq!(t.retire_settled(1e-3), 1);
    assert_eq!(t.get(K, X), None);
    assert_eq!(t.get(K, OPACITY), Some(0.0));
}

// Keys must hash the same across runs and platforms, so pin FNV-1a's
// published test vectors.
#[test]
fn anim_key_from_str_matches_fnv1a_vectors() {
    let vectors = [
        ("", 0xcbf2_9ce4_8422_2325),
        ("a", 0xaf63_dc4c_8601_ec8c),
        ("foobar", 0x8594_4171_f739_67e8),
    ];
    for (input, expected) in vectors {
        assert_eq!(AnimKey::from_str_key(input), AnimKey(expected), "{input:?}");
    }
}

// A resize or a move along one axis must leave the other edges exactly
// where they were every frame, and the edge facing the way the rect moves
// leads the one behind it.
#[test]
fn animate_rect_keeps_still_edges_exact_and_leads_with_the_front_edge() {
    let start = Rect {
        x: 0.0,
        y: 10.0,
        width: 50.0,
        height: 20.0,
    };
    let moved = Rect { x: 100.0, ..start };
    let motion = RectMotion::leading(Motion::smooth(), 2.0);
    let mut t = AnimationTable::new();
    t.set_rect(K, start, 0);
    t.animate_rect(K, moved, motion, 0);
    let mut now = 0;
    while t.tick(now) {
        let rect = t.rect(K).unwrap_or_default();
        assert_eq!((rect.y, rect.bottom()), (10.0, 30.0), "at {now} ms");
        if now > 0 {
            assert!(
                rect.width > 50.0,
                "front edge not leading at {now} ms: {rect:?}"
            );
        }
        // Repeating the target every frame keeps each edge's motion.
        t.animate_rect(K, moved, motion, now);
        now += 8;
        assert!(now < 5_000, "never settled");
    }
    assert_eq!(t.rect(K), Some(moved));
}

// A bound signal must follow the row while it moves and stop being
// written once it rests, or the frame loop would never go idle.
#[test]
fn bound_signal_follows_the_row_and_goes_quiet_once_settled() {
    let store = SignalStore::new();
    let signal = store.create(0.0f32);
    let mut t = AnimationTable::new();
    t.set(K, OPACITY, 0.0, 0);
    t.animate_to(K, OPACITY, 1.0, Motion::tween(100, Curve::Linear), 0);
    t.bind_signal(K, OPACITY, signal);
    let mut frame = |now: u64| {
        store.clear_dirty();
        t.tick(now);
        t.publish_signals(&store);
        (signal.get(&store), store.any_dirty())
    };
    let (mid, dirty) = frame(50);
    assert!(close(mid, 0.5, 1e-5) && dirty, "{mid} {dirty}");
    assert_eq!(frame(100), (1.0, true));
    assert_eq!(frame(150), (1.0, false));
}
