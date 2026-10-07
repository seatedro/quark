//! 2D affine transforms for layers and hit testing.

use crate::geometry::Rect;

/// An affine map of the plane: `x' = a·x + c·y + tx`, `y' = b·x + d·y + ty`.
///
/// Scene transforms are absolute: a layer's transform maps its content, as
/// painted in untransformed scene coordinates, to where it lands in its
/// parent's coordinates. Build one about an element's own point with
/// [`Self::around`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform2D {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
    pub tx: f32,
    pub ty: f32,
}

impl Default for Transform2D {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Transform2D {
    pub const IDENTITY: Self = Self {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        tx: 0.0,
        ty: 0.0,
    };

    pub const fn translate(x: f32, y: f32) -> Self {
        Self {
            tx: x,
            ty: y,
            ..Self::IDENTITY
        }
    }

    pub const fn scale(sx: f32, sy: f32) -> Self {
        Self {
            a: sx,
            d: sy,
            ..Self::IDENTITY
        }
    }

    /// Clockwise on screen (y points down) for positive `radians`.
    pub fn rotate(radians: f32) -> Self {
        let (sin, cos) = radians.sin_cos();
        Self {
            a: cos,
            b: sin,
            c: -sin,
            d: cos,
            tx: 0.0,
            ty: 0.0,
        }
    }

    /// `self` first, then `next`.
    pub fn then(self, next: Self) -> Self {
        Self {
            a: next.a * self.a + next.c * self.b,
            b: next.b * self.a + next.d * self.b,
            c: next.a * self.c + next.c * self.d,
            d: next.b * self.c + next.d * self.d,
            tx: next.a * self.tx + next.c * self.ty + next.tx,
            ty: next.b * self.tx + next.d * self.ty + next.ty,
        }
    }

    /// The same map with `(x, y)` as its fixed point instead of the origin,
    /// e.g. a rotation about an element's center.
    pub fn around(self, x: f32, y: f32) -> Self {
        Self::translate(-x, -y)
            .then(self)
            .then(Self::translate(x, y))
    }

    pub fn apply(self, x: f32, y: f32) -> (f32, f32) {
        (
            self.a * x + self.c * y + self.tx,
            self.b * x + self.d * y + self.ty,
        )
    }

    /// `None` for a degenerate map (a zero scale), which shows nothing and
    /// hits nothing.
    pub fn invert(self) -> Option<Self> {
        let det = self.a * self.d - self.b * self.c;
        if det.abs() < 1e-12 || !det.is_finite() {
            return None;
        }
        let inv = 1.0 / det;
        let (a, b, c, d) = (self.d * inv, -self.b * inv, -self.c * inv, self.a * inv);
        Some(Self {
            a,
            b,
            c,
            d,
            tx: -(a * self.tx + c * self.ty),
            ty: -(b * self.tx + d * self.ty),
        })
    }

    pub fn is_identity(self) -> bool {
        self == Self::IDENTITY
    }

    /// True when the map only moves things (no rotation, scale, or skew).
    pub fn is_translation(self) -> bool {
        self.a == 1.0 && self.b == 0.0 && self.c == 0.0 && self.d == 1.0
    }

    /// Axis-aligned bounds of `rect` after the map.
    pub fn map_rect_bounds(self, rect: Rect) -> Rect {
        let corners = [
            self.apply(rect.x, rect.y),
            self.apply(rect.right(), rect.y),
            self.apply(rect.x, rect.bottom()),
            self.apply(rect.right(), rect.bottom()),
        ];
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for (x, y) in corners {
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
        Rect {
            x: x0,
            y: y0,
            width: x1 - x0,
            height: y1 - y0,
        }
    }

    /// The map for content moved by `(dx, dy)`: it lands where the old map
    /// put it, moved by the same amount. How a layer follows
    /// [`Primitive::offset`](crate::scene::Primitive::offset).
    pub fn offset(self, dx: f32, dy: f32) -> Self {
        // translate(d) ∘ self ∘ translate(-d); the linear part is unchanged.
        let (lx, ly) = (self.a * dx + self.c * dy, self.b * dx + self.d * dy);
        Self {
            tx: self.tx + dx - lx,
            ty: self.ty + dy - ly,
            ..self
        }
    }

    /// The same map in a space scaled by `s` (logical points to physical
    /// pixels): only the translation scales.
    pub fn in_scaled_space(self, s: f32) -> Self {
        Self {
            tx: self.tx * s,
            ty: self.ty * s,
            ..self
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: (f32, f32), b: (f32, f32)) -> bool {
        (a.0 - b.0).abs() < 1e-4 && (a.1 - b.1).abs() < 1e-4
    }

    // A layer moved by the element cache must land where the moved content
    // would have landed under the old map, for any map.
    #[test]
    fn offset_map_commutes_with_moving_content() {
        let maps = [
            Transform2D::rotate(0.7).around(30.0, 40.0),
            Transform2D::scale(2.0, 0.5).around(5.0, -3.0),
            Transform2D::translate(4.0, 9.0),
        ];
        for map in maps {
            let moved = map.offset(12.0, -7.0);
            for p in [(0.0, 0.0), (10.0, 3.0), (-4.0, 25.0)] {
                let (x, y) = map.apply(p.0, p.1);
                assert!(
                    close(moved.apply(p.0 + 12.0, p.1 - 7.0), (x + 12.0, y - 7.0)),
                    "{map:?} at {p:?}"
                );
            }
        }
    }

    #[test]
    fn invert_round_trips_points() {
        let map = Transform2D::rotate(1.1)
            .then(Transform2D::scale(1.5, 0.8))
            .around(20.0, 10.0);
        let inverse = map.invert().expect("invertible");
        for p in [(0.0, 0.0), (33.0, -2.0), (7.5, 19.0)] {
            let (x, y) = map.apply(p.0, p.1);
            assert!(close(inverse.apply(x, y), p), "{p:?}");
        }
        assert_eq!(Transform2D::scale(0.0, 1.0).invert(), None);
    }
}
