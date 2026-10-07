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

use crate::identity::{UiKey, stable_hash};
use crate::selection::{FULL_INTEGRITY_CHECKS, count_integrity_steps};

/// Stable animation identity. Usually derived from a [`UiKey`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AnimKey(pub u64);

impl AnimKey {
    /// [`stable_hash`] of `key`; stable across runs and platforms.
    pub const fn from_str_key(key: &str) -> Self {
        Self(stable_hash(key))
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
    /// `1 - (1 - t)^5`: a fast start that lands softly. Toasts use it.
    EaseOutQuint,
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
            Curve::EaseOutQuint => 1.0 - (1.0 - t).powi(5),
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
        self.debug_check_row(row);
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
        self.animate_row_to(key, prop, target, motion, now_ms);
        if let Some(row) = self.row(key, prop) {
            self.debug_check_row(row);
        }
    }

    fn animate_row_to(
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
        debug_assert_eq!(self.verify_integrity(), Ok(()));
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
        debug_assert_eq!(self.verify_integrity(), Ok(()));
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
        debug_assert_eq!(self.verify_integrity(), Ok(()));
        removed
    }

    /// Keeps only the rows for which `keep(key, prop)` is true. Returns how
    /// many rows were removed.
    pub fn retain(&mut self, mut keep: impl FnMut(AnimKey, PropId) -> bool) -> usize {
        let mut removed = 0;
        let mut row = 0;
        while row < self.keys.len() {
            if keep(self.keys[row], self.props[row]) {
                row += 1;
            } else {
                self.swap_remove_row(row);
                removed += 1;
            }
        }
        debug_assert_eq!(self.verify_integrity(), Ok(()));
        removed
    }

    pub fn remove(&mut self, key: AnimKey, prop: PropId) -> bool {
        match self.row(key, prop) {
            Some(row) => {
                self.swap_remove_row(row);
                // The last row moved into `row`.
                if row < self.keys.len() {
                    self.debug_check_row(row);
                }
                true
            }
            None => false,
        }
    }

    /// Checks one row: column and index lengths, the index entry of its
    /// `(key, prop)`, and that it rests on its target when inactive. O(1);
    /// single-row mutations call it through `debug_assert!`.
    pub fn verify_row(&self, row: usize) -> Result<(), IntegrityError> {
        count_integrity_steps(1);
        self.verify_lengths()?;
        self.verify_row_entries(row)
    }

    fn debug_check_row(&self, row: usize) {
        if FULL_INTEGRITY_CHECKS {
            debug_assert_eq!(self.verify_integrity(), Ok(()));
        } else {
            debug_assert_eq!(self.verify_row(row), Ok(()));
        }
    }

    /// Checks that every column has one entry per row, `index` maps each
    /// row's `(key, prop)` to that row, and inactive rows rest exactly on
    /// their target. Whole-table mutations call this through
    /// `debug_assert!`, so release builds skip it. O(rows).
    pub fn verify_integrity(&self) -> Result<(), IntegrityError> {
        count_integrity_steps(self.keys.len());
        self.verify_lengths()?;
        for row in 0..self.keys.len() {
            self.verify_row_entries(row)?;
        }
        Ok(())
    }

    fn verify_lengths(&self) -> Result<(), IntegrityError> {
        let rows = self.keys.len();
        let columns = [
            ("props", self.props.len()),
            ("values", self.values.len()),
            ("velocities", self.velocities.len()),
            ("targets", self.targets.len()),
            ("kinds", self.kinds.len()),
            ("updated_ms", self.updated_ms.len()),
            ("active", self.active.len()),
        ];
        for (column, len) in columns {
            if len != rows {
                return Err(IntegrityError::ColumnLength { column, len, rows });
            }
        }
        if self.index.len() != rows {
            return Err(IntegrityError::IndexLength {
                rows,
                index: self.index.len(),
            });
        }
        Ok(())
    }

    fn verify_row_entries(&self, row: usize) -> Result<(), IntegrityError> {
        let found = self.index.get(&(self.keys[row], self.props[row])).copied();
        if found != Some(row as u32) {
            return Err(IntegrityError::IndexRow { row, index: found });
        }
        // Every path that clears `active` also snaps to the target, so an
        // inactive row away from it would never be ticked there.
        let (value, target) = (self.values[row], self.targets[row]);
        let on_target = value == target || (value.is_nan() && target.is_nan());
        if !self.active[row] && !(on_target && self.velocities[row] == 0.0) {
            return Err(IntegrityError::InactiveOffTarget { row });
        }
        Ok(())
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

/// A broken [`AnimationTable`] invariant, reported by
/// [`AnimationTable::verify_integrity`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntegrityError {
    ColumnLength {
        column: &'static str,
        len: usize,
        rows: usize,
    },
    IndexLength {
        rows: usize,
        index: usize,
    },
    /// `index` maps row `row`'s `(key, prop)` to `index` instead.
    IndexRow {
        row: usize,
        index: Option<u32>,
    },
    InactiveOffTarget {
        row: usize,
    },
}

impl std::fmt::Display for IntegrityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ColumnLength { column, len, rows } => {
                write!(f, "column {column} has {len} entries for {rows} rows")
            }
            Self::IndexLength { rows, index } => {
                write!(f, "{rows} rows but {index} index entries")
            }
            Self::IndexRow { row, index } => write!(f, "row {row} is indexed at {index:?}"),
            Self::InactiveOffTarget { row } => {
                write!(f, "inactive row {row} is not at rest on its target")
            }
        }
    }
}

impl std::error::Error for IntegrityError {}

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
        let next_v = v + accel * h;
        let next_x = x + next_v * h;
        // f32 rounding can park x a few ulps from the target with a velocity
        // that balances the spring force; nothing changes after that, so the
        // spring would never reach rest and would tick forever.
        if next_x == x && next_v == v {
            return (target, 0.0, true);
        }
        v = next_v;
        x = next_x;
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
    use std::collections::HashSet;

    use proptest::prelude::*;

    use super::*;

    const K: AnimKey = AnimKey(1);
    const OPACITY: PropId = PropId(0);
    const X: PropId = PropId(1);

    fn close(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() <= eps
    }

    /// `PROPTEST_CASES` overrides the per-property default for heavier runs.
    fn config(default_cases: u32) -> ProptestConfig {
        let cases = std::env::var("PROPTEST_CASES")
            .ok()
            .and_then(|v| v.parse().ok())
            // Miri hides host env vars under isolation, so it gets its own
            // small default.
            .unwrap_or(if cfg!(miri) { 4 } else { default_cases });
        let mut config = ProptestConfig::with_cases(cases);
        if cfg!(miri) {
            // Miri's isolation forbids the regression file lookups.
            config.failure_persistence = None;
        }
        config
    }

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

        // Catches retargeting resetting velocity or value, which reads as a
        // visible jerk when a spring changes direction mid-flight.
        #[test]
        fn spring_retarget_midflight_keeps_value_and_velocity(
            params in spring(),
            first in -1000.0f32..1000.0,
            second in -1000.0f32..1000.0,
            gaps in frame_gaps(),
            frames in 0usize..20,
        ) {
            let mut t = AnimationTable::new();
            t.set(K, X, 0.0, 0);
            t.animate_to(K, X, first, Motion::Spring(params), 0);
            let mut now = 0;
            for gap in gaps.iter().cycle().take(frames) {
                now += gap;
                t.tick(now);
            }
            let (x, v) = (t.get(K, X).unwrap_or(f32::NAN), t.velocity(K, X).unwrap_or(f32::NAN));
            t.animate_to(K, X, second, Motion::Spring(params), now);
            // A retarget onto the current resting point may snap by up to
            // the rest threshold.
            let eps = t.rest_epsilon;
            prop_assert!(close(t.get(K, X).unwrap_or(f32::NAN), x, eps));
            prop_assert!(close(t.velocity(K, X).unwrap_or(f32::NAN), v, eps));
            if t.is_animating(K, X) {
                tick_until_settled(&mut t, &gaps, now)?;
            }
            prop_assert_eq!(t.get(K, X), Some(second));
        }

        // Drives arbitrary mutations so the debug integrity check catches
        // index drift across swap-removals; also checks rows exist exactly
        // when the model says so and retiring never drops a moving row.
        #[test]
        fn animation_table_arbitrary_ops_keep_rows_and_index(
            ops in prop::collection::vec((0u8..8, 0u64..3, 0u16..3, -10.0f32..10.0, 0u64..120), 0..40),
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
}
