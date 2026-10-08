//! Pure geometry primitives used by quark hit-testing and scene emission.

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    pub fn contains(self, x: f32, y: f32) -> bool {
        x >= self.x && y >= self.y && x <= self.right() && y <= self.bottom()
    }

    pub fn right(self) -> f32 {
        self.x + self.width
    }

    pub fn bottom(self) -> f32 {
        self.y + self.height
    }

    pub fn inset(self, amount: f32) -> Self {
        Self {
            x: self.x + amount,
            y: self.y + amount,
            width: (self.width - amount * 2.0).max(0.0),
            height: (self.height - amount * 2.0).max(0.0),
        }
    }

    pub fn pad(self, left: f32, top: f32, right: f32, bottom: f32) -> Self {
        Self {
            x: self.x + left,
            y: self.y + top,
            width: (self.width - left - right).max(0.0),
            height: (self.height - top - bottom).max(0.0),
        }
    }

    pub fn center(self, child_w: f32, child_h: f32) -> Self {
        Self {
            x: self.x + ((self.width - child_w).max(0.0) * 0.5),
            y: self.y + ((self.height - child_h).max(0.0) * 0.5),
            width: child_w.min(self.width - 24.0),
            height: child_h.min(self.height - 24.0),
        }
    }

    pub fn offset(self, dx: f32, dy: f32) -> Self {
        Self {
            x: self.x + dx,
            y: self.y + dy,
            ..self
        }
    }

    pub fn intersection(self, other: Self) -> Option<Self> {
        let left = self.x.max(other.x);
        let top = self.y.max(other.y);
        let right = self.right().min(other.right());
        let bottom = self.bottom().min(other.bottom());
        let width = right - left;
        let height = bottom - top;
        if width <= 0.0 || height <= 0.0 {
            None
        } else {
            Some(Self {
                x: left,
                y: top,
                width,
                height,
            })
        }
    }
}

#[cfg(kani)]
mod verification {
    use super::*;

    /// Whole-point edges, possibly negative sizes, so every sum is exact.
    fn any_rect() -> Rect {
        let coord = || f32::from(kani::any::<i8>() % 16);
        Rect {
            x: coord(),
            y: coord(),
            width: coord(),
            height: coord(),
        }
    }

    fn inside(r: Rect, x: f32, y: f32) -> bool {
        x > r.x && y > r.y && x < r.right() && y < r.bottom()
    }

    /// The intersection is symmetric, has positive area, and holds exactly
    /// the points both rects hold; `None` means the rects share no
    /// interior point.
    #[kani::proof]
    fn intersection_holds_exactly_the_points_of_both() {
        let (a, b) = (any_rect(), any_rect());
        // Half points too, so a point can sit strictly between two edges.
        let x = f32::from(kani::any::<i8>() % 64) / 2.0;
        let y = f32::from(kani::any::<i8>() % 64) / 2.0;

        let overlap = a.intersection(b);

        assert!(overlap == b.intersection(a));
        match overlap {
            Some(r) => {
                assert!(r.width > 0.0 && r.height > 0.0);
                assert!(r.contains(x, y) == (a.contains(x, y) && b.contains(x, y)));
            }
            None => {
                assert!(!(inside(a, x, y) && inside(b, x, y)));
            }
        }
    }
}
