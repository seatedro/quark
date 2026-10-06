//! Animation table keyed by stable UI identity and property, not node ids.
//!
//! Each row animates one `f32` for one `(AnimKey, PropId)` pair. Vector
//! values are animated as several props by convention (for example
//! `PropId(OFFSET_X)` and `PropId(OFFSET_Y)`).
//!
//! Rows are stored as parallel columns; row `i` is `keys[i]`, `props[i]`,
//! `values[i]`, and so on. `index` maps `(key, prop)` to its row and is kept
//! in sync across swap-removals.
//!
//! A row only knows a current value once it exists. Use [`AnimationTable::set`]
//! to establish a start value before [`AnimationTable::animate_to`] when
//! animating something in; `animate_to` on an unknown row snaps to the target.

use std::collections::HashMap;

use crate::identity::UiKey;

/// Stable animation identity. Usually derived from a [`UiKey`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AnimKey(pub u64);

impl AnimKey {
    /// FNV-1a over the key's bytes; stable across runs and platforms.
    pub fn from_str_key(key: &str) -> Self {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for byte in key.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        Self(hash)
    }
}

impl From<&UiKey> for AnimKey {
    fn from(key: &UiKey) -> Self {
        Self::from_str_key(key.as_str())
    }
}

/// Which property of a keyed element is animated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PropId(pub u16);

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Curve {
    Linear,
    EaseOutCubic,
    EaseInOutCubic,
    /// CSS-style `cubic-bezier(x1, y1, x2, y2)`. `x1` and `x2` are clamped to
    /// `[0, 1]` so the curve is a function of time.
    CubicBezier {
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
    },
}

impl Curve {
    /// Eased progress for linear progress `t` in `[0, 1]`.
    pub fn eval(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Curve::Linear => t,
            Curve::EaseOutCubic => 1.0 - (1.0 - t).powi(3),
            Curve::EaseInOutCubic => {
                if t < 0.5 {
                    4.0 * t * t * t
                } else {
                    1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
                }
            }
            Curve::CubicBezier { x1, y1, x2, y2 } => {
                cubic_bezier(x1.clamp(0.0, 1.0), y1, x2.clamp(0.0, 1.0), y2, t)
            }
        }
    }
}

fn bezier_axis(p1: f32, p2: f32, s: f32) -> f32 {
    // B(s) with P0 = 0 and P3 = 1.
    let inv = 1.0 - s;
    3.0 * inv * inv * s * p1 + 3.0 * inv * s * s * p2 + s * s * s
}

fn bezier_axis_slope(p1: f32, p2: f32, s: f32) -> f32 {
    let inv = 1.0 - s;
    3.0 * inv * inv * p1 + 6.0 * inv * s * (p2 - p1) + 3.0 * s * s * (1.0 - p2)
}

fn cubic_bezier(x1: f32, y1: f32, x2: f32, y2: f32, x: f32) -> f32 {
    // Solve x(s) = x: Newton first, bisection if the slope is too flat.
    let mut s = x;
    for _ in 0..8 {
        let err = bezier_axis(x1, x2, s) - x;
        if err.abs() < 1e-6 {
            return bezier_axis(y1, y2, s);
        }
        let slope = bezier_axis_slope(x1, x2, s);
        if slope.abs() < 1e-6 {
            break;
        }
        s = (s - err / slope).clamp(0.0, 1.0);
    }
    let (mut lo, mut hi) = (0.0_f32, 1.0_f32);
    s = x;
    for _ in 0..32 {
        let v = bezier_axis(x1, x2, s);
        if (v - x).abs() < 1e-6 {
            break;
        }
        if v < x {
            lo = s;
        } else {
            hi = s;
        }
        s = (lo + hi) * 0.5;
    }
    bezier_axis(y1, y2, s)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpringParams {
    pub stiffness: f32,
    pub damping: f32,
    pub mass: f32,
}

impl Default for SpringParams {
    fn default() -> Self {
        Self {
            stiffness: 300.0,
            damping: 30.0,
            mass: 1.0,
        }
    }
}

/// How a row moves toward its target. Passed to
/// [`AnimationTable::animate_to`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Motion {
    Tween {
        duration_ms: u32,
        /// Delay before the tween starts moving; reported by
        /// [`AnimationTable::next_deadline`] so the runner can sleep.
        delay_ms: u32,
        curve: Curve,
    },
    Spring(SpringParams),
}

impl Motion {
    pub fn tween(duration_ms: u32, curve: Curve) -> Self {
        Motion::Tween {
            duration_ms,
            delay_ms: 0,
            curve,
        }
    }

    pub fn spring(stiffness: f32, damping: f32, mass: f32) -> Self {
        Motion::Spring(SpringParams {
            stiffness,
            damping,
            mass,
        })
    }
}

/// Per-row driver state stored in the `kinds` column.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AnimKind {
    Tween {
        from: f32,
        start_ms: u64,
        duration_ms: u32,
        curve: Curve,
    },
    Spring(SpringParams),
}

/// Rest threshold used by [`AnimationTable::tick`] to decide when a spring
/// has settled; the row then snaps to its target.
pub const DEFAULT_REST_EPSILON: f32 = 1e-3;

const SPRING_MAX_STEP_S: f32 = 1.0 / 240.0;
/// Past this many substeps in one tick (a long stall), the spring snaps to
/// its target instead of integrating.
const SPRING_MAX_SUBSTEPS: u32 = 20_000;

#[derive(Debug, Clone)]
pub struct AnimationTable {
    keys: Vec<AnimKey>,
    props: Vec<PropId>,
    values: Vec<f32>,
    velocities: Vec<f32>,
    targets: Vec<f32>,
    kinds: Vec<AnimKind>,
    /// Time the row was last advanced to.
    updated_ms: Vec<u64>,
    active: Vec<bool>,
    index: HashMap<(AnimKey, PropId), u32>,
    pub rest_epsilon: f32,
}

impl Default for AnimationTable {
    fn default() -> Self {
        Self {
            keys: Vec::new(),
            props: Vec::new(),
            values: Vec::new(),
            velocities: Vec::new(),
            targets: Vec::new(),
            kinds: Vec::new(),
            updated_ms: Vec::new(),
            active: Vec::new(),
            index: HashMap::new(),
            rest_epsilon: DEFAULT_REST_EPSILON,
        }
    }
}

impl AnimationTable {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.keys.len()
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    fn row(&self, key: AnimKey, prop: PropId) -> Option<usize> {
        self.index.get(&(key, prop)).map(|r| *r as usize)
    }

    fn push_row(&mut self, key: AnimKey, prop: PropId, value: f32, now_ms: u64) -> usize {
        let row = self.keys.len();
        self.keys.push(key);
        self.props.push(prop);
        self.values.push(value);
        self.velocities.push(0.0);
        self.targets.push(value);
        self.kinds.push(AnimKind::Spring(SpringParams::default()));
        self.updated_ms.push(now_ms);
        self.active.push(false);
        self.index.insert((key, prop), row as u32);
        row
    }

    fn swap_remove_row(&mut self, row: usize) {
        self.index.remove(&(self.keys[row], self.props[row]));
        self.keys.swap_remove(row);
        self.props.swap_remove(row);
        self.values.swap_remove(row);
        self.velocities.swap_remove(row);
        self.targets.swap_remove(row);
        self.kinds.swap_remove(row);
        self.updated_ms.swap_remove(row);
        self.active.swap_remove(row);
        if row < self.keys.len() {
            self.index
                .insert((self.keys[row], self.props[row]), row as u32);
        }
    }

    /// Snaps `(key, prop)` to `value` with no motion, creating the row if
    /// needed. `now_ms` becomes the row's clock.
    pub fn set(&mut self, key: AnimKey, prop: PropId, value: f32, now_ms: u64) {
        let row = match self.row(key, prop) {
            Some(row) => row,
            None => self.push_row(key, prop, value, now_ms),
        };
        self.values[row] = value;
        self.targets[row] = value;
        self.velocities[row] = 0.0;
        self.updated_ms[row] = now_ms;
        self.active[row] = false;
    }

    /// Starts or retargets an animation toward `target`.
    ///
    /// The row is first advanced to `now_ms`. Retargeting a spring keeps its
    /// velocity; a tween always starts from the current value. A tween
    /// retargeted to the same target keeps its original timing. Unknown rows
    /// snap to `target` (call [`set`](Self::set) first to animate in).
    pub fn animate_to(
        &mut self,
        key: AnimKey,
        prop: PropId,
        target: f32,
        motion: Motion,
        now_ms: u64,
    ) {
        let Some(row) = self.row(key, prop) else {
            self.set(key, prop, target, now_ms);
            return;
        };
        self.advance_row(row, now_ms);
        let same_target = (self.targets[row] - target).abs() <= f32::EPSILON;
        match motion {
            Motion::Spring(params) => {
                self.kinds[row] = AnimKind::Spring(params);
                self.targets[row] = target;
                self.active[row] = !self.spring_at_rest(row);
                if !self.active[row] {
                    self.values[row] = target;
                    self.velocities[row] = 0.0;
                }
            }
            Motion::Tween {
                duration_ms,
                delay_ms,
                curve,
            } => {
                let running_tween = matches!(self.kinds[row], AnimKind::Tween { .. });
                if same_target && running_tween && self.active[row] {
                    return;
                }
                if same_target && (self.values[row] - target).abs() <= f32::EPSILON {
                    self.values[row] = target;
                    self.velocities[row] = 0.0;
                    self.active[row] = false;
                    return;
                }
                self.kinds[row] = AnimKind::Tween {
                    from: self.values[row],
                    start_ms: now_ms.saturating_add(u64::from(delay_ms)),
                    duration_ms,
                    curve,
                };
                self.targets[row] = target;
                self.active[row] = true;
            }
        }
    }

    /// Current value as of the last tick, or `None` if no row exists.
    pub fn get(&self, key: AnimKey, prop: PropId) -> Option<f32> {
        self.row(key, prop).map(|row| self.values[row])
    }

    pub fn target(&self, key: AnimKey, prop: PropId) -> Option<f32> {
        self.row(key, prop).map(|row| self.targets[row])
    }

    pub fn velocity(&self, key: AnimKey, prop: PropId) -> Option<f32> {
        self.row(key, prop).map(|row| self.velocities[row])
    }

    pub fn is_animating(&self, key: AnimKey, prop: PropId) -> bool {
        self.row(key, prop).is_some_and(|row| self.active[row])
    }

    /// Advances every active row to `now_ms`. Returns whether any row is
    /// still active afterwards.
    pub fn tick(&mut self, now_ms: u64) -> bool {
        let mut any = false;
        for row in 0..self.keys.len() {
            if self.active[row] {
                self.advance_row(row, now_ms);
                any |= self.active[row];
            }
        }
        any
    }

    pub fn has_active(&self) -> bool {
        self.active.iter().any(|a| *a)
    }

    /// Earliest time the table needs another tick, or `None` when idle.
    ///
    /// A moving row reports the time it was last advanced to (a frame is due
    /// now); a delayed tween reports its start time.
    pub fn next_deadline(&self) -> Option<u64> {
        let mut best: Option<u64> = None;
        for row in 0..self.keys.len() {
            if !self.active[row] {
                continue;
            }
            let due = match self.kinds[row] {
                AnimKind::Tween { start_ms, .. } => start_ms.max(self.updated_ms[row]),
                AnimKind::Spring(_) => self.updated_ms[row],
            };
            best = Some(best.map_or(due, |b| b.min(due)));
        }
        best
    }

    /// Removes rows that are inactive and within `epsilon` of their target
    /// with velocity within `epsilon`. Returns how many were removed.
    /// Callers fall back to the target (style) value once a row is gone.
    pub fn retire_settled(&mut self, epsilon: f32) -> usize {
        let mut removed = 0;
        let mut row = 0;
        while row < self.keys.len() {
            let settled = !self.active[row]
                && (self.values[row] - self.targets[row]).abs() <= epsilon
                && self.velocities[row].abs() <= epsilon;
            if settled {
                self.swap_remove_row(row);
                removed += 1;
            } else {
                row += 1;
            }
        }
        removed
    }

    /// Removes every prop animated for `key`. Returns how many rows went.
    pub fn remove_key(&mut self, key: AnimKey) -> usize {
        let mut removed = 0;
        let mut row = 0;
        while row < self.keys.len() {
            if self.keys[row] == key {
                self.swap_remove_row(row);
                removed += 1;
            } else {
                row += 1;
            }
        }
        removed
    }

    pub fn remove(&mut self, key: AnimKey, prop: PropId) -> bool {
        match self.row(key, prop) {
            Some(row) => {
                self.swap_remove_row(row);
                true
            }
            None => false,
        }
    }

    fn spring_at_rest(&self, row: usize) -> bool {
        (self.values[row] - self.targets[row]).abs() <= self.rest_epsilon
            && self.velocities[row].abs() <= self.rest_epsilon
    }

    fn advance_row(&mut self, row: usize, now_ms: u64) {
        let last = self.updated_ms[row];
        if now_ms <= last {
            return;
        }
        self.updated_ms[row] = now_ms;
        if !self.active[row] {
            return;
        }
        match self.kinds[row] {
            AnimKind::Tween {
                from,
                start_ms,
                duration_ms,
                curve,
            } => {
                let before = self.values[row];
                let target = self.targets[row];
                let elapsed = now_ms.saturating_sub(start_ms);
                if elapsed >= u64::from(duration_ms) {
                    self.values[row] = target;
                    self.velocities[row] = 0.0;
                    self.active[row] = false;
                    return;
                }
                let t = elapsed as f32 / duration_ms as f32;
                let value = from + (target - from) * curve.eval(t);
                self.values[row] = value;
                // Finite-difference velocity (units/s) so a later spring
                // retarget inherits the tween's motion.
                let dt_s = (now_ms - last) as f32 / 1000.0;
                self.velocities[row] = (value - before) / dt_s;
            }
            AnimKind::Spring(params) => {
                let dt_s = (now_ms - last) as f32 / 1000.0;
                let (x, v, settled) = step_spring(
                    self.values[row],
                    self.velocities[row],
                    self.targets[row],
                    params,
                    dt_s,
                    self.rest_epsilon,
                );
                self.values[row] = x;
                self.velocities[row] = v;
                self.active[row] = !settled;
            }
        }
    }
}

/// Semi-implicit Euler with uniform substeps. The step is bounded by
/// `1/240 s` and by the spring's natural frequency and damping rate, which
/// keeps the integrator stable for any `dt` and stiff springs.
fn step_spring(
    mut x: f32,
    mut v: f32,
    target: f32,
    p: SpringParams,
    dt_s: f32,
    rest: f32,
) -> (f32, f32, bool) {
    let mass = p.mass.max(1e-4);
    let k = p.stiffness.max(0.0);
    let c = p.damping.max(0.0);
    let omega = (k / mass).sqrt();
    let mut max_step = SPRING_MAX_STEP_S;
    if omega > 0.0 {
        max_step = max_step.min(0.25 / omega);
    }
    if c > 0.0 {
        max_step = max_step.min(0.5 * mass / c);
    }
    let steps_f = (dt_s / max_step).ceil();
    if !steps_f.is_finite() || steps_f > SPRING_MAX_SUBSTEPS as f32 {
        return (target, 0.0, true);
    }
    let steps = (steps_f as u32).max(1);
    let h = dt_s / steps as f32;
    for _ in 0..steps {
        let accel = (-k * (x - target) - c * v) / mass;
        v += accel * h;
        x += v * h;
        if (x - target).abs() <= rest && v.abs() <= rest {
            return (target, 0.0, true);
        }
    }
    if !x.is_finite() || !v.is_finite() {
        return (target, 0.0, true);
    }
    (x, v, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    const K: AnimKey = AnimKey(1);
    const OPACITY: PropId = PropId(0);
    const X: PropId = PropId(1);

    fn close(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() <= eps
    }

    #[test]
    fn curves_hit_endpoints() {
        let curves = [
            Curve::Linear,
            Curve::EaseOutCubic,
            Curve::EaseInOutCubic,
            Curve::CubicBezier {
                x1: 0.25,
                y1: 0.1,
                x2: 0.25,
                y2: 1.0,
            },
        ];
        for curve in curves {
            assert!(close(curve.eval(0.0), 0.0, 1e-5), "{curve:?}");
            assert!(close(curve.eval(1.0), 1.0, 1e-5), "{curve:?}");
        }
        let linear_bezier = Curve::CubicBezier {
            x1: 0.0,
            y1: 0.0,
            x2: 1.0,
            y2: 1.0,
        };
        assert!(close(linear_bezier.eval(0.3), 0.3, 1e-4));
        assert!(close(Curve::EaseInOutCubic.eval(0.5), 0.5, 1e-6));
        assert!(Curve::EaseOutCubic.eval(0.5) > 0.5);
    }

    #[test]
    fn tween_runs_from_start_to_target() {
        let mut t = AnimationTable::new();
        t.set(K, OPACITY, 0.0, 0);
        t.animate_to(K, OPACITY, 1.0, Motion::tween(100, Curve::Linear), 0);
        assert_eq!(t.get(K, OPACITY), Some(0.0));
        assert!(t.tick(50));
        assert!(close(t.get(K, OPACITY).unwrap_or(f32::NAN), 0.5, 1e-5));
        assert!(!t.tick(100));
        assert_eq!(t.get(K, OPACITY), Some(1.0));
        assert_eq!(t.next_deadline(), None);
    }

    #[test]
    fn tween_retarget_starts_from_current_value() {
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
    fn spring_settles_on_target() {
        let mut t = AnimationTable::new();
        t.set(K, X, 0.0, 0);
        t.animate_to(K, X, 100.0, Motion::spring(170.0, 26.0, 1.0), 0);
        let mut now = 0;
        while t.tick(now) && now < 10_000 {
            now += 16;
        }
        assert!(now < 10_000, "spring never settled");
        assert_eq!(t.get(K, X), Some(100.0));
        assert_eq!(t.velocity(K, X), Some(0.0));
        assert_eq!(t.retire_settled(1e-3), 1);
        assert_eq!(t.get(K, X), None);
    }

    #[test]
    fn spring_retarget_keeps_velocity() {
        let mut t = AnimationTable::new();
        t.set(K, X, 0.0, 0);
        let motion = Motion::spring(200.0, 20.0, 1.0);
        t.animate_to(K, X, 100.0, motion, 0);
        t.tick(50);
        let v = t.velocity(K, X).unwrap_or(0.0);
        assert!(v > 0.0);
        t.animate_to(K, X, -100.0, motion, 50);
        assert_eq!(t.velocity(K, X), Some(v));
        assert_eq!(t.target(K, X), Some(-100.0));
        // Momentum carries it further positive before it turns around.
        let before = t.get(K, X).unwrap_or(f32::NAN);
        t.tick(58);
        assert!(t.get(K, X).unwrap_or(f32::NAN) > before);
    }

    #[test]
    fn spring_is_stable_with_large_dt_and_stiffness() {
        let mut t = AnimationTable::new();
        t.set(K, X, 0.0, 0);
        t.animate_to(K, X, 1.0, Motion::spring(50_000.0, 5.0, 1.0), 0);
        t.tick(500);
        let x = t.get(K, X).unwrap_or(f32::NAN);
        assert!(x.is_finite() && x.abs() < 3.0, "x = {x}");

        // A multi-minute stall snaps rather than integrating forever.
        t.animate_to(K, X, 10.0, Motion::spring(300.0, 30.0, 1.0), 500);
        assert!(!t.tick(500 + 600_000));
        assert_eq!(t.get(K, X), Some(10.0));
    }

    #[test]
    fn deadline_and_active_reporting() {
        let mut t = AnimationTable::new();
        assert_eq!(t.next_deadline(), None);
        assert!(!t.tick(0));

        t.set(K, OPACITY, 0.0, 0);
        t.animate_to(
            K,
            OPACITY,
            1.0,
            Motion::Tween {
                duration_ms: 100,
                delay_ms: 200,
                curve: Curve::EaseOutCubic,
            },
            0,
        );
        assert!(t.has_active());
        assert_eq!(t.next_deadline(), Some(200));
        assert!(t.tick(100));
        assert_eq!(t.get(K, OPACITY), Some(0.0));

        let other = AnimKey(2);
        t.set(other, X, 0.0, 100);
        t.animate_to(other, X, 5.0, Motion::spring(300.0, 30.0, 1.0), 100);
        assert_eq!(t.next_deadline(), Some(100));

        assert_eq!(t.remove_key(other), 1);
        assert_eq!(t.next_deadline(), Some(200));
        assert!(!t.tick(300));
        assert_eq!(t.next_deadline(), None);
    }

    #[test]
    fn unknown_row_snaps_and_swap_remove_keeps_index() {
        let mut t = AnimationTable::new();
        t.animate_to(K, X, 3.0, Motion::tween(100, Curve::Linear), 0);
        assert_eq!(t.get(K, X), Some(3.0));
        assert!(!t.is_animating(K, X));

        for i in 0..5u16 {
            t.set(AnimKey(9), PropId(10 + i), f32::from(i), 0);
        }
        assert_eq!(t.remove_key(K), 1);
        for i in 0..5u16 {
            assert_eq!(t.get(AnimKey(9), PropId(10 + i)), Some(f32::from(i)));
        }
    }

    #[test]
    fn ui_key_hash_is_stable() {
        let a = AnimKey::from(&UiKey::from("row.42"));
        let b = AnimKey::from_str_key("row.42");
        assert_eq!(a, b);
        assert_ne!(a, AnimKey::from_str_key("row.43"));
    }
}
