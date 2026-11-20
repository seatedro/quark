//! Easing curves for tweens.

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

    /// Rate of eased progress per unit of linear progress at `t`: how fast a
    /// tween moves there, so a spring that takes over keeps its speed.
    pub fn slope(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Curve::Linear => 1.0,
            Curve::EaseOutCubic => 3.0 * (1.0 - t).powi(2),
            Curve::EaseOutQuint => 5.0 * (1.0 - t).powi(4),
            Curve::EaseInOutCubic => {
                if t < 0.5 {
                    12.0 * t * t
                } else {
                    3.0 * (2.0 - 2.0 * t).powi(2)
                }
            }
            // A control point on the time axis makes the tangent vertical
            // at an end, so take the secant over a small step instead.
            Curve::CubicBezier { .. } => {
                const H: f32 = 1e-3;
                let (lo, hi) = ((t - H).max(0.0), (t + H).min(1.0));
                (self.eval(hi) - self.eval(lo)) / (hi - lo)
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
