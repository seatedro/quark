//! Damped springs evaluated in closed form.
//!
//! A spring released at displacement `x0` from its target with velocity
//! `v0` follows `m·x'' + c·x' + k·x = 0`, which has an exact solution in
//! each damping regime. Evaluating it at any time gives the same answer
//! however the time was reached, so a row's motion does not depend on the
//! frames that sampled it.

use std::f64::consts::TAU;

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

impl SpringParams {
    /// A unit-mass spring described the way Apple's platforms do: its
    /// perceptual `duration_s` (the period it would swing at undamped) and
    /// `bounce` in `[-1, 1]`. Zero is critically damped, the fastest motion
    /// that never overshoots; above zero it overshoots and settles back,
    /// more the closer to one; below zero it is overdamped and eases in
    /// more slowly. Values outside the range are clamped.
    pub fn from_duration_bounce(duration_s: f32, bounce: f32) -> Self {
        let duration = f64::from(duration_s).max(1e-3);
        let bounce = f64::from(bounce.clamp(-1.0, 1.0));
        let omega = TAU / duration;
        let zeta = if bounce >= 0.0 {
            1.0 - bounce
        } else {
            1.0 / (1.0 + bounce).max(1e-3)
        };
        Self {
            stiffness: (omega * omega) as f32,
            damping: (2.0 * zeta * omega) as f32,
            mass: 1.0,
        }
    }

    /// The same spring running `factor` times faster: the same shape of
    /// motion in `1 / factor` of the time.
    pub fn time_scaled(self, factor: f32) -> Self {
        let factor = factor.max(1e-3);
        Self {
            stiffness: self.stiffness * factor * factor,
            damping: self.damping * factor,
            mass: self.mass,
        }
    }
}

/// A spring that never settles snaps to its target this long after release.
pub(super) const MAX_SETTLE_MS: u64 = 60_000;

/// `|ζ - 1|` below which the spring takes the critically damped solution;
/// the other two lose precision as they approach it.
const CRITICAL_BAND: f64 = 1e-4;

/// The motion of one spring, in seconds.
#[derive(Clone, Copy)]
struct Model {
    k: f64,
    m: f64,
    omega: f64,
    zeta: f64,
}

impl Model {
    /// `None` for a spring with no restoring force, which never moves
    /// toward its target.
    fn new(p: SpringParams) -> Option<Self> {
        let (k, c, m) = (
            f64::from(p.stiffness),
            f64::from(p.damping.max(0.0)),
            f64::from(p.mass.max(1e-4)),
        );
        if !(k > 0.0 && k.is_finite() && c.is_finite() && m.is_finite()) {
            return None;
        }
        let omega = (k / m).sqrt();
        Some(Self {
            k,
            m,
            omega,
            zeta: c / (2.0 * (k * m).sqrt()),
        })
    }

    /// Displacement and velocity `t` seconds after release from `x0`, `v0`.
    fn state(self, x0: f64, v0: f64, t: f64) -> (f64, f64) {
        let (w, z) = (self.omega, self.zeta);
        if z < 1.0 - CRITICAL_BAND {
            let decay_rate = z * w;
            let wd = w * (1.0 - z * z).sqrt();
            let decay = (-decay_rate * t).exp();
            let (sin, cos) = (wd * t).sin_cos();
            let x = decay * (x0 * cos + (v0 + decay_rate * x0) / wd * sin);
            let v = decay * (v0 * cos - (w * w * x0 + decay_rate * v0) / wd * sin);
            (x, v)
        } else if z > 1.0 + CRITICAL_BAND {
            let spread = w * (z * z - 1.0).sqrt();
            let (r1, r2) = (-z * w + spread, -z * w - spread);
            let c1 = (v0 - r2 * x0) / (r1 - r2);
            let c2 = x0 - c1;
            let (e1, e2) = ((r1 * t).exp(), (r2 * t).exp());
            (c1 * e1 + c2 * e2, r1 * c1 * e1 + r2 * c2 * e2)
        } else {
            let decay = (-w * t).exp();
            let b = v0 + w * x0;
            (decay * (x0 + b * t), decay * (v0 - w * b * t))
        }
    }

    fn energy(self, x: f64, v: f64) -> f64 {
        0.5 * (self.k * x * x + self.m * v * v)
    }
}

/// Displacement and velocity of a spring released `t_s` seconds ago at
/// displacement `x0` with velocity `v0`. A spring with no restoring force
/// is already at rest.
pub(super) fn state(p: SpringParams, x0: f64, v0: f64, t_s: f64) -> (f64, f64) {
    match Model::new(p) {
        Some(model) => model.state(x0, v0, t_s),
        None => (0.0, 0.0),
    }
}

/// Whole milliseconds after release from `x0`, `v0` from which the spring
/// stays within `rest` of its target in both position and velocity.
///
/// Damping only ever removes energy, so once the energy is below what
/// `rest` allows in either quantity it stays there; the earliest such
/// millisecond is found by bisection. A pure function of the release, so
/// the settle time does not depend on when frames sample the spring.
pub(super) fn settle_ms(p: SpringParams, x0: f64, v0: f64, rest: f32) -> u64 {
    let Some(model) = Model::new(p) else {
        return 0;
    };
    if !(x0.is_finite() && v0.is_finite()) {
        return 0;
    }
    let limit = 0.5 * f64::from(rest).powi(2) * model.k.min(model.m);
    let at_rest = |ms: u64| {
        let (x, v) = model.state(x0, v0, ms as f64 / 1000.0);
        model.energy(x, v) <= limit
    };
    if at_rest(0) {
        return 0;
    }
    if !at_rest(MAX_SETTLE_MS) {
        return MAX_SETTLE_MS;
    }
    let (mut moving, mut resting) = (0, MAX_SETTLE_MS);
    while resting - moving > 1 {
        let mid = moving + (resting - moving) / 2;
        if at_rest(mid) {
            resting = mid;
        } else {
            moving = mid;
        }
    }
    resting
}
