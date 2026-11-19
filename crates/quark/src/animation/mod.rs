//! Animation table keyed by stable UI identity and property, not node ids.
//!
//! Each row animates one `f32` for one `(AnimKey, PropId)` pair. Vector
//! values are animated as several props by convention (for example
//! `PropId(OFFSET_X)` and `PropId(OFFSET_Y)`); [`AnimationTable::animate_rect`],
//! [`AnimationTable::animate_color`], and [`AnimationTable::animate_layer`]
//! do it for rects, colors, and layers.
//!
//! Rows are stored as parallel columns; row `i` is `keys[i]`, `props[i]`,
//! `values[i]`, and so on. `index` maps `(key, prop)` to its row and is kept
//! in sync across swap-removals.
//!
//! A row's motion is a pure function of time: it keeps where the motion
//! started (its anchor) and every tick evaluates the tween or the spring at
//! the tick's time in closed form. Frames at 30, 60, or 120 Hz therefore
//! see the same values at the same timestamps, and retargeting a moving row
//! starts the new motion from exactly the value and velocity it had.
//! Each row is [`AnimPhase::Delayed`], [`AnimPhase::Running`], or
//! [`AnimPhase::Settled`] exactly on its target.
//!
//! A row only knows a current value once it exists. Use [`AnimationTable::set`]
//! to establish a start value before [`AnimationTable::animate_to`] when
//! animating something in; `animate_to` on an unknown row snaps to the target.

use std::collections::HashMap;

use crate::identity::{UiKey, stable_hash};
use crate::selection::{FULL_INTEGRITY_CHECKS, count_integrity_steps};

mod channels;
mod curve;
mod signals;
mod spring;
#[cfg(test)]
mod tests;

pub use channels::{LAYER_PROPS, RECT_PROPS, RESERVED_PROP_BASE, RectMotion, color_props};
pub use curve::Curve;
pub use signals::SignalBinding;
pub use spring::SpringParams;

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

    /// A spring by perceptual duration and bounce; see
    /// [`SpringParams::from_duration_bounce`].
    pub fn spring_duration(duration_s: f32, bounce: f32) -> Self {
        Motion::Spring(SpringParams::from_duration_bounce(duration_s, bounce))
    }

    /// Quick feedback (hover, press, toggles): short, with a trace of
    /// bounce.
    pub fn snappy() -> Self {
        Self::spring_duration(0.25, 0.15)
    }

    /// Things that move or resize: critically damped, so it never
    /// overshoots.
    pub fn smooth() -> Self {
        Self::spring_duration(0.35, 0.0)
    }

    /// Large surfaces (sheets, panels, page changes): slower and
    /// critically damped.
    pub fn calm() -> Self {
        Self::spring_duration(0.6, 0.0)
    }

    /// What reduced motion falls back to: a short fade with no travel.
    pub fn reduced_fade() -> Self {
        Self::tween(120, Curve::EaseOutCubic)
    }

    /// The same motion `factor` times faster.
    pub fn time_scaled(self, factor: f32) -> Self {
        let factor = factor.max(1e-3);
        match self {
            Motion::Tween {
                duration_ms,
                delay_ms,
                curve,
            } => Motion::Tween {
                duration_ms: (duration_ms as f32 / factor).round() as u32,
                delay_ms: (delay_ms as f32 / factor).round() as u32,
                curve,
            },
            Motion::Spring(params) => Motion::Spring(params.time_scaled(factor)),
        }
    }
}

/// Where a row is in its motion; the `phases` column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnimPhase {
    /// Waiting out a tween's delay, holding the value it starts from.
    Delayed,
    Running,
    /// At rest exactly on its target, with zero velocity.
    Settled,
}

/// Per-row driver state stored in the `kinds` column: the motion's anchor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AnimKind {
    Tween {
        from: f32,
        start_ms: u64,
        duration_ms: u32,
        curve: Curve,
    },
    /// Released at `start_ms` from `from` with `velocity` (units per
    /// second); settles at `settle_ms`.
    Spring {
        params: SpringParams,
        from: f32,
        velocity: f32,
        start_ms: u64,
        settle_ms: u64,
    },
}

/// How close to its target, in position and velocity, a spring must stay
/// to settle; the row then snaps to its target.
pub const DEFAULT_REST_EPSILON: f32 = 1e-3;

#[derive(Debug, Clone)]
pub struct AnimationTable {
    keys: Vec<AnimKey>,
    props: Vec<PropId>,
    values: Vec<f32>,
    velocities: Vec<f32>,
    targets: Vec<f32>,
    kinds: Vec<AnimKind>,
    phases: Vec<AnimPhase>,
    /// Time the row was last advanced to; never moves back.
    updated_ms: Vec<u64>,
    index: HashMap<(AnimKey, PropId), u32>,
    bindings: signals::Bindings,
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
            phases: Vec::new(),
            updated_ms: Vec::new(),
            index: HashMap::new(),
            bindings: signals::Bindings::default(),
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
        self.kinds.push(AnimKind::Tween {
            from: value,
            start_ms: now_ms,
            duration_ms: 0,
            curve: Curve::Linear,
        });
        self.phases.push(AnimPhase::Settled);
        self.updated_ms.push(now_ms);
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
        self.phases.swap_remove(row);
        self.updated_ms.swap_remove(row);
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
        self.phases[row] = AnimPhase::Settled;
        self.debug_check_row(row);
    }

    /// Starts or retargets an animation toward `target`.
    ///
    /// The row is first advanced to `now_ms`, then the new motion starts
    /// from its value and velocity there, unchanged. A tween retargeted to
    /// the same target keeps its original timing, and so does a spring
    /// asked for the same target with the same parameters; a tween always
    /// starts from the current value at rest. Unknown rows snap to
    /// `target` (call [`set`](Self::set) first to animate in).
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
        let now_ms = now_ms.max(self.updated_ms[row]);
        self.advance_row(row, now_ms);
        let same_target = (self.targets[row] - target).abs() <= f32::EPSILON;
        let phase = self.phases[row];
        match motion {
            Motion::Spring(params) => {
                let same_spring = matches!(
                    self.kinds[row],
                    AnimKind::Spring { params: p, .. } if p == params
                );
                let continues = phase == AnimPhase::Settled || same_spring;
                if same_target && continues {
                    return;
                }
                self.release_spring(row, target, params, now_ms);
            }
            Motion::Tween {
                duration_ms,
                delay_ms,
                curve,
            } => {
                let running_tween = matches!(self.kinds[row], AnimKind::Tween { .. })
                    && phase != AnimPhase::Settled;
                if same_target && running_tween {
                    return;
                }
                if same_target && (self.values[row] - target).abs() <= f32::EPSILON {
                    self.settle(row);
                    return;
                }
                self.kinds[row] = AnimKind::Tween {
                    from: self.values[row],
                    start_ms: now_ms.saturating_add(u64::from(delay_ms)),
                    duration_ms,
                    curve,
                };
                self.targets[row] = target;
                self.phases[row] = AnimPhase::Running;
                self.evaluate(row, now_ms);
            }
        }
    }

    /// Start a spring toward `target` from the row's value and velocity
    /// now, which it keeps exactly.
    fn release_spring(&mut self, row: usize, target: f32, params: SpringParams, now_ms: u64) {
        let (from, velocity) = (self.values[row], self.velocities[row]);
        self.targets[row] = target;
        let x0 = f64::from(from) - f64::from(target);
        let settle_in = spring::settle_ms(params, x0, f64::from(velocity), self.rest_epsilon);
        if settle_in == 0 {
            self.settle(row);
            return;
        }
        self.kinds[row] = AnimKind::Spring {
            params,
            from,
            velocity,
            start_ms: now_ms,
            settle_ms: now_ms + settle_in,
        };
        self.phases[row] = AnimPhase::Running;
        self.updated_ms[row] = now_ms;
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

    pub fn phase(&self, key: AnimKey, prop: PropId) -> Option<AnimPhase> {
        self.row(key, prop).map(|row| self.phases[row])
    }

    pub fn is_animating(&self, key: AnimKey, prop: PropId) -> bool {
        self.row(key, prop)
            .is_some_and(|row| self.phases[row] != AnimPhase::Settled)
    }

    /// Whether any of `props` of `key` is still moving or delayed.
    pub fn any_animating(&self, key: AnimKey, props: &[PropId]) -> bool {
        props.iter().any(|prop| self.is_animating(key, *prop))
    }

    /// Advances every moving row to `now_ms`. Returns whether any row is
    /// still moving afterwards.
    pub fn tick(&mut self, now_ms: u64) -> bool {
        let mut any = false;
        for row in 0..self.keys.len() {
            if self.phases[row] != AnimPhase::Settled {
                self.advance_row(row, now_ms);
                any |= self.phases[row] != AnimPhase::Settled;
            }
        }
        debug_assert_eq!(self.verify_integrity(), Ok(()));
        any
    }

    pub fn has_active(&self) -> bool {
        self.phases.iter().any(|p| *p != AnimPhase::Settled)
    }

    /// Earliest time the table needs another tick, or `None` when idle.
    ///
    /// A running row reports the time it was last advanced to (a frame is
    /// due now); a delayed tween reports its start time.
    pub fn next_deadline(&self) -> Option<u64> {
        let mut best: Option<u64> = None;
        for row in 0..self.keys.len() {
            let due = match (self.phases[row], self.kinds[row]) {
                (AnimPhase::Settled, _) => continue,
                (AnimPhase::Delayed, AnimKind::Tween { start_ms, .. }) => start_ms,
                _ => self.updated_ms[row],
            };
            best = Some(best.map_or(due, |b| b.min(due)));
        }
        best
    }

    /// Removes settled rows within `epsilon` of their target. Returns how
    /// many were removed. Callers fall back to the target (style) value
    /// once a row is gone.
    pub fn retire_settled(&mut self, epsilon: f32) -> usize {
        let mut removed = 0;
        let mut row = 0;
        while row < self.keys.len() {
            let settled = self.phases[row] == AnimPhase::Settled
                && (self.values[row] - self.targets[row]).abs() <= epsilon;
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
    /// Its signal bindings stay.
    pub fn remove_key(&mut self, key: AnimKey) -> usize {
        self.retain(|k, _| k != key)
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
    /// `(key, prop)`, and that its phase matches its value and clock. O(1);
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
    /// row's `(key, prop)` to that row, settled rows rest exactly on their
    /// target, and no row was advanced past the point its phase should
    /// have changed. Whole-table mutations call this through
    /// `debug_assert!`, so release builds skip it. O(rows).
    pub fn verify_integrity(&self) -> Result<(), IntegrityError> {
        count_integrity_steps(self.keys.len());
        self.verify_lengths()?;
        for row in 0..self.keys.len() {
            self.verify_row_entries(row)?;
        }
        self.bindings.verify()
    }

    fn verify_lengths(&self) -> Result<(), IntegrityError> {
        let rows = self.keys.len();
        let columns = [
            ("props", self.props.len()),
            ("values", self.values.len()),
            ("velocities", self.velocities.len()),
            ("targets", self.targets.len()),
            ("kinds", self.kinds.len()),
            ("phases", self.phases.len()),
            ("updated_ms", self.updated_ms.len()),
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
        let now = self.updated_ms[row];
        match (self.phases[row], self.kinds[row]) {
            (AnimPhase::Settled, _) => {
                // Ticks skip settled rows, so one off its target stays there.
                let (value, target) = (self.values[row], self.targets[row]);
                let on_target = value == target || (value.is_nan() && target.is_nan());
                if !(on_target && self.velocities[row] == 0.0) {
                    return Err(IntegrityError::SettledOffTarget { row });
                }
            }
            (AnimPhase::Delayed, AnimKind::Tween { start_ms, .. }) if now < start_ms => {}
            (AnimPhase::Delayed, _) => return Err(IntegrityError::DelayedPastStart { row }),
            (
                AnimPhase::Running,
                AnimKind::Tween {
                    start_ms,
                    duration_ms,
                    ..
                },
            ) => {
                if now < start_ms || now - start_ms >= u64::from(duration_ms) {
                    return Err(IntegrityError::RunningOutsideMotion { row });
                }
            }
            (AnimPhase::Running, AnimKind::Spring { settle_ms, .. }) => {
                if now >= settle_ms {
                    return Err(IntegrityError::RunningOutsideMotion { row });
                }
            }
        }
        Ok(())
    }

    fn settle(&mut self, row: usize) {
        self.values[row] = self.targets[row];
        self.velocities[row] = 0.0;
        self.phases[row] = AnimPhase::Settled;
    }

    fn advance_row(&mut self, row: usize, now_ms: u64) {
        if now_ms > self.updated_ms[row] {
            self.evaluate(row, now_ms);
        }
    }

    /// Set the row's value, velocity, and phase to those of its motion at
    /// `now_ms`, which must not be before its clock.
    fn evaluate(&mut self, row: usize, now_ms: u64) {
        self.updated_ms[row] = now_ms;
        if self.phases[row] == AnimPhase::Settled {
            return;
        }
        let target = self.targets[row];
        match self.kinds[row] {
            AnimKind::Tween {
                from,
                start_ms,
                duration_ms,
                curve,
            } => {
                if now_ms < start_ms {
                    self.values[row] = from;
                    self.velocities[row] = 0.0;
                    self.phases[row] = AnimPhase::Delayed;
                    return;
                }
                let elapsed = now_ms - start_ms;
                if elapsed >= u64::from(duration_ms) {
                    self.settle(row);
                    return;
                }
                let t = elapsed as f32 / duration_ms as f32;
                self.values[row] = from + (target - from) * curve.eval(t);
                self.velocities[row] =
                    (target - from) * curve.slope(t) * 1000.0 / duration_ms as f32;
                self.phases[row] = AnimPhase::Running;
            }
            AnimKind::Spring {
                params,
                from,
                velocity,
                start_ms,
                settle_ms,
            } => {
                if now_ms >= settle_ms {
                    self.settle(row);
                    return;
                }
                let t_s = (now_ms - start_ms) as f64 / 1000.0;
                let x0 = f64::from(from) - f64::from(target);
                let (x, v) = spring::state(params, x0, f64::from(velocity), t_s);
                let (value, v) = ((f64::from(target) + x) as f32, v as f32);
                if !(value.is_finite() && v.is_finite()) {
                    self.settle(row);
                    return;
                }
                self.values[row] = value;
                self.velocities[row] = v;
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
    SettledOffTarget {
        row: usize,
    },
    /// A delayed row whose clock reached its start, or that is no tween.
    DelayedPastStart {
        row: usize,
    },
    /// A running row whose clock is before its tween starts or at or past
    /// where its motion ends.
    RunningOutsideMotion {
        row: usize,
    },
    /// The signal binding columns differ in length.
    BindingLength {
        keys: usize,
        bindings: usize,
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
            Self::SettledOffTarget { row } => {
                write!(f, "settled row {row} is not at rest on its target")
            }
            Self::DelayedPastStart { row } => {
                write!(f, "delayed row {row} has reached its start")
            }
            Self::RunningOutsideMotion { row } => {
                write!(f, "running row {row} is outside its motion")
            }
            Self::BindingLength { keys, bindings } => {
                write!(f, "{keys} binding keys for {bindings} bindings")
            }
        }
    }
}

impl std::error::Error for IntegrityError {}
