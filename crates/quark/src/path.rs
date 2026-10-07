//! Vector paths for [`PathPrimitive`](crate::scene::PathPrimitive): lines,
//! quadratic and cubic Béziers, and circular arcs, filled and stroked by
//! the renderer.

use crate::geometry::Rect;

/// One drawing command. Points are `[x, y]` in path units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PathVerb {
    MoveTo([f32; 2]),
    LineTo([f32; 2]),
    QuadTo([f32; 2], [f32; 2]),
    CubicTo([f32; 2], [f32; 2], [f32; 2]),
    Close,
}

/// An immutable path. Build one with [`Path::builder`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Path {
    verbs: Vec<PathVerb>,
    /// Bounds of every point, control points included, so it contains the
    /// curve.
    bounds: Rect,
}

impl Path {
    pub fn builder() -> PathBuilder {
        PathBuilder::default()
    }

    /// Open polyline through `points`, e.g. a sparkline.
    pub fn polyline(points: impl IntoIterator<Item = (f32, f32)>) -> Self {
        let mut builder = Self::builder();
        for (i, (x, y)) in points.into_iter().enumerate() {
            if i == 0 {
                builder.move_to(x, y);
            } else {
                builder.line_to(x, y);
            }
        }
        builder.build()
    }

    pub fn verbs(&self) -> &[PathVerb] {
        &self.verbs
    }

    pub fn bounds(&self) -> Rect {
        self.bounds
    }

    pub fn is_empty(&self) -> bool {
        self.verbs.is_empty()
    }
}

/// Builds a [`Path`]. A drawing command without a current point starts a
/// subpath at its first point.
#[derive(Debug, Clone, Default)]
pub struct PathBuilder {
    verbs: Vec<PathVerb>,
    current: Option<[f32; 2]>,
    start: [f32; 2],
}

impl PathBuilder {
    pub fn move_to(&mut self, x: f32, y: f32) -> &mut Self {
        self.verbs.push(PathVerb::MoveTo([x, y]));
        self.current = Some([x, y]);
        self.start = [x, y];
        self
    }

    pub fn line_to(&mut self, x: f32, y: f32) -> &mut Self {
        self.ensure_start([x, y]);
        self.verbs.push(PathVerb::LineTo([x, y]));
        self.current = Some([x, y]);
        self
    }

    pub fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) -> &mut Self {
        self.ensure_start([cx, cy]);
        self.verbs.push(PathVerb::QuadTo([cx, cy], [x, y]));
        self.current = Some([x, y]);
        self
    }

    pub fn cubic_to(&mut self, c1: (f32, f32), c2: (f32, f32), to: (f32, f32)) -> &mut Self {
        self.ensure_start([c1.0, c1.1]);
        self.verbs
            .push(PathVerb::CubicTo([c1.0, c1.1], [c2.0, c2.1], [to.0, to.1]));
        self.current = Some([to.0, to.1]);
        self
    }

    /// A circular arc around `(cx, cy)` from `start` sweeping `sweep`
    /// radians (positive is clockwise on screen). It joins the current
    /// point with a line, or starts a subpath when there is none.
    pub fn arc(&mut self, cx: f32, cy: f32, radius: f32, start: f32, sweep: f32) -> &mut Self {
        let point = |angle: f32| [cx + radius * angle.cos(), cy + radius * angle.sin()];
        let first = point(start);
        match self.current {
            Some(_) => {
                self.line_to(first[0], first[1]);
            }
            None => {
                self.move_to(first[0], first[1]);
            }
        }
        if radius <= 0.0 || sweep == 0.0 || !sweep.is_finite() {
            return self;
        }
        // At most a quarter turn per cubic keeps the error below 0.03% of
        // the radius.
        let sweep = sweep.clamp(-std::f32::consts::TAU, std::f32::consts::TAU);
        let pieces = (sweep.abs() / std::f32::consts::FRAC_PI_2).ceil().max(1.0) as usize;
        let step = sweep / pieces as f32;
        let k = 4.0 / 3.0 * (step / 4.0).tan() * radius;
        let mut angle = start;
        for _ in 0..pieces {
            let next = angle + step;
            let (s0, c0) = angle.sin_cos();
            let (s1, c1) = next.sin_cos();
            let end = point(next);
            self.verbs.push(PathVerb::CubicTo(
                [cx + radius * c0 - k * s0, cy + radius * s0 + k * c0],
                [cx + radius * c1 + k * s1, cy + radius * s1 - k * c1],
                end,
            ));
            self.current = Some(end);
            angle = next;
        }
        self
    }

    /// Close the subpath with a line back to its start.
    pub fn close(&mut self) -> &mut Self {
        if self.current.is_some() {
            self.verbs.push(PathVerb::Close);
            self.current = Some(self.start);
        }
        self
    }

    pub fn build(&mut self) -> Path {
        let verbs = std::mem::take(&mut self.verbs);
        self.current = None;
        let mut bounds: Option<(f32, f32, f32, f32)> = None;
        let mut grow = |p: [f32; 2]| {
            let (x0, y0, x1, y1) = bounds.get_or_insert((p[0], p[1], p[0], p[1]));
            *x0 = x0.min(p[0]);
            *y0 = y0.min(p[1]);
            *x1 = x1.max(p[0]);
            *y1 = y1.max(p[1]);
        };
        for verb in &verbs {
            match *verb {
                PathVerb::MoveTo(p) | PathVerb::LineTo(p) => grow(p),
                PathVerb::QuadTo(c, p) => {
                    grow(c);
                    grow(p);
                }
                PathVerb::CubicTo(c1, c2, p) => {
                    grow(c1);
                    grow(c2);
                    grow(p);
                }
                PathVerb::Close => {}
            }
        }
        let bounds = bounds.map_or(Rect::default(), |(x0, y0, x1, y1)| Rect {
            x: x0,
            y: y0,
            width: x1 - x0,
            height: y1 - y0,
        });
        Path { verbs, bounds }
    }

    fn ensure_start(&mut self, at: [f32; 2]) {
        if self.current.is_none() {
            self.move_to(at[0], at[1]);
        }
    }
}

/// Which points a fill covers when subpaths overlap or wind twice.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FillRule {
    #[default]
    NonZero,
    EvenOdd,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum LineJoin {
    #[default]
    Miter,
    Round,
    Bevel,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum LineCap {
    #[default]
    Butt,
    Round,
    Square,
}

/// How a stroke outlines a path. `width` is in path units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StrokeStyle {
    pub width: f32,
    pub join: LineJoin,
    pub cap: LineCap,
    /// Miter joins longer than `miter_limit * width / 2` become bevels.
    pub miter_limit: f32,
}

impl Default for StrokeStyle {
    fn default() -> Self {
        Self {
            width: 1.0,
            join: LineJoin::Miter,
            cap: LineCap::Butt,
            miter_limit: 4.0,
        }
    }
}

impl StrokeStyle {
    pub fn new(width: f32) -> Self {
        Self {
            width,
            ..Self::default()
        }
    }

    pub fn join(mut self, join: LineJoin) -> Self {
        self.join = join;
        self
    }

    pub fn cap(mut self, cap: LineCap) -> Self {
        self.cap = cap;
        self
    }

    pub fn miter_limit(mut self, limit: f32) -> Self {
        self.miter_limit = limit;
        self
    }

    /// How far the stroke can reach past the path's points.
    pub fn reach(&self) -> f32 {
        let half = self.width.max(0.0) * 0.5;
        let join = match self.join {
            LineJoin::Miter => self.miter_limit.max(1.0),
            LineJoin::Round | LineJoin::Bevel => 1.0,
        };
        let cap = match self.cap {
            LineCap::Square => std::f32::consts::SQRT_2,
            LineCap::Butt | LineCap::Round => 1.0,
        };
        half * join.max(cap)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // An arc must stay on its circle: every cubic endpoint at the radius and
    // the midpoint of each piece within a hair of it.
    #[test]
    fn arc_cubics_stay_on_the_circle() {
        let mut builder = Path::builder();
        builder.arc(10.0, 20.0, 50.0, 0.3, 4.0);
        let path = builder.build();
        let mut from = match path.verbs()[0] {
            PathVerb::MoveTo(p) => p,
            other => panic!("starts with {other:?}"),
        };
        for verb in &path.verbs()[1..] {
            let PathVerb::CubicTo(c1, c2, to) = *verb else {
                panic!("{verb:?}");
            };
            // de Casteljau midpoint.
            let mid = |i: usize| 0.125 * (from[i] + 3.0 * c1[i] + 3.0 * c2[i] + to[i]);
            let radius = |x: f32, y: f32| ((x - 10.0).powi(2) + (y - 20.0).powi(2)).sqrt();
            assert!((radius(to[0], to[1]) - 50.0).abs() < 1e-3);
            assert!((radius(mid(0), mid(1)) - 50.0).abs() < 0.05, "{verb:?}");
            from = to;
        }
        assert_eq!(path.verbs().len(), 4, "a 4 rad sweep takes three pieces");
    }
}
