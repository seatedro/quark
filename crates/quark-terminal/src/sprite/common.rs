//! Helpers shared by the sprite drawings, after Ghostty's
//! `font/sprite/draw/common.zig`.

use tiny_skia::PathBuilder;

use super::canvas::{Canvas, ON};
use crate::metrics::CellMetrics;

pub(crate) const ONE_EIGHTH: f64 = 0.125;
pub(crate) const ONE_QUARTER: f64 = 0.25;
pub(crate) const ONE_THIRD: f64 = 1.0 / 3.0;
pub(crate) const THREE_EIGHTHS: f64 = 0.375;
pub(crate) const HALF: f64 = 0.5;
pub(crate) const FIVE_EIGHTHS: f64 = 0.625;
pub(crate) const TWO_THIRDS: f64 = 2.0 / 3.0;
pub(crate) const THREE_QUARTERS: f64 = 0.75;
pub(crate) const SEVEN_EIGHTHS: f64 = 0.875;

/// Line weights, relative to the font's box thickness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Thickness {
    Light,
    Heavy,
}

impl Thickness {
    pub fn height(self, base: u32) -> u32 {
        match self {
            Self::Light => base,
            Self::Heavy => base * 2,
        }
    }
}

/// Shades, as coverage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum Shade {
    Light = 0x40,
    Medium = 0x80,
    Dark = 0xc0,
    On = 0xff,
}

/// Which quadrants of a cell are filled.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Quads {
    pub tl: bool,
    pub tr: bool,
    pub bl: bool,
    pub br: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Corner {
    Tl,
    Tr,
    Bl,
    Br,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Edge {
    Top,
    Left,
    Bottom,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Horizontal {
    Left,
    Right,
    Center,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Vertical {
    Top,
    Bottom,
    Middle,
}

/// Where a figure sits in its cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Alignment {
    pub horizontal: Horizontal,
    pub vertical: Vertical,
}

impl Alignment {
    pub const CENTER: Self = Self::new(Horizontal::Center, Vertical::Middle);
    pub const UPPER: Self = Self::new(Horizontal::Center, Vertical::Top);
    pub const LOWER: Self = Self::new(Horizontal::Center, Vertical::Bottom);
    pub const LEFT: Self = Self::new(Horizontal::Left, Vertical::Middle);
    pub const RIGHT: Self = Self::new(Horizontal::Right, Vertical::Middle);
    pub const UPPER_LEFT: Self = Self::new(Horizontal::Left, Vertical::Top);
    pub const UPPER_RIGHT: Self = Self::new(Horizontal::Right, Vertical::Top);
    pub const LOWER_LEFT: Self = Self::new(Horizontal::Left, Vertical::Bottom);
    pub const LOWER_RIGHT: Self = Self::new(Horizontal::Right, Vertical::Bottom);

    pub const fn new(horizontal: Horizontal, vertical: Vertical) -> Self {
        Self {
            horizontal,
            vertical,
        }
    }
}

/// A fraction of the way across a cell. `min` and `max` round so that
/// adjoining blocks meet exactly: with a 7 pixel cell, `0..half` and
/// `half..1` are both 4 pixels wide, overlapping by one, rather than one
/// being 3.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Fraction(pub f64);

impl Fraction {
    pub const ZERO: Self = Self(0.0);
    pub const ONE_EIGHTH: Self = Self(ONE_EIGHTH);
    pub const ONE_QUARTER: Self = Self(ONE_QUARTER);
    pub const ONE_THIRD: Self = Self(ONE_THIRD);
    pub const THREE_EIGHTHS: Self = Self(THREE_EIGHTHS);
    pub const HALF: Self = Self(HALF);
    pub const FIVE_EIGHTHS: Self = Self(FIVE_EIGHTHS);
    pub const TWO_THIRDS: Self = Self(TWO_THIRDS);
    pub const THREE_QUARTERS: Self = Self(THREE_QUARTERS);
    pub const SEVEN_EIGHTHS: Self = Self(SEVEN_EIGHTHS);
    pub const FULL: Self = Self(1.0);

    pub const EIGHTHS: [Self; 9] = [
        Self::ZERO,
        Self::ONE_EIGHTH,
        Self::ONE_QUARTER,
        Self::THREE_EIGHTHS,
        Self::HALF,
        Self::FIVE_EIGHTHS,
        Self::THREE_QUARTERS,
        Self::SEVEN_EIGHTHS,
        Self::FULL,
    ];
    pub const QUARTERS: [Self; 5] = [
        Self::ZERO,
        Self::ONE_QUARTER,
        Self::HALF,
        Self::THREE_QUARTERS,
        Self::FULL,
    ];
    pub const HALVES: [Self; 3] = [Self::ZERO, Self::HALF, Self::FULL];

    /// As the left or top edge of a block.
    pub fn min(self, size: u32) -> i32 {
        let s = f64::from(size);
        (s - ((1.0 - self.0) * s).round()) as i32
    }

    /// As the right or bottom edge of a block.
    pub fn max(self, size: u32) -> i32 {
        (self.0 * f64::from(size)).round() as i32
    }

    /// Unrounded, for paths.
    pub fn float(self, size: u32) -> f64 {
        self.0 * f64::from(size)
    }
}

/// Fills the part of the cell between two vertical and two horizontal
/// fraction lines.
pub(crate) fn fill(
    m: &CellMetrics,
    canvas: &mut Canvas,
    x0: Fraction,
    x1: Fraction,
    y0: Fraction,
    y1: Fraction,
) {
    canvas.box_(
        x0.min(m.cell_width),
        y0.min(m.cell_height),
        x1.max(m.cell_width),
        y1.max(m.cell_height),
        ON,
    );
}

/// A vertical line of `thickness` down the middle of the cell.
pub(crate) fn vline_middle(m: &CellMetrics, canvas: &mut Canvas, thickness: Thickness) {
    let thick = thickness.height(m.box_thickness);
    vline(
        canvas,
        0,
        m.cell_height as i32,
        (m.cell_width.saturating_sub(thick) / 2) as i32,
        thick,
    );
}

/// A horizontal line of `thickness` across the middle of the cell.
pub(crate) fn hline_middle(m: &CellMetrics, canvas: &mut Canvas, thickness: Thickness) {
    let thick = thickness.height(m.box_thickness);
    hline(
        canvas,
        0,
        m.cell_width as i32,
        (m.cell_height.saturating_sub(thick) / 2) as i32,
        thick,
    );
}

/// A vertical line with its left edge at `x`, from `y1` to `y2`.
pub(crate) fn vline(canvas: &mut Canvas, y1: i32, y2: i32, x: i32, thickness: u32) {
    canvas.box_(x, y1, x + thickness as i32, y2, ON);
}

/// A horizontal line with its top edge at `y`, from `x1` to `x2`.
pub(crate) fn hline(canvas: &mut Canvas, x1: i32, x2: i32, y: i32, thickness: u32) {
    canvas.box_(x1, y, x2, y + thickness as i32, ON);
}

/// Appends a circular arc around `(cx, cy)` from angle `a0` to `a1`
/// (radians, clockwise in the cell's y-down space, increasing like
/// cairo's and z2d's `arc`), joined to the current point by a line.
pub(crate) fn arc(pb: &mut PathBuilder, cx: f64, cy: f64, r: f64, a0: f64, mut a1: f64) {
    use std::f64::consts::{FRAC_PI_2, TAU};
    while a1 < a0 {
        a1 += TAU;
    }
    let point = |a: f64| (cx + r * a.cos(), cy + r * a.sin());
    let start = point(a0);
    if pb.is_empty() {
        pb.move_to(start.0 as f32, start.1 as f32);
    } else {
        pb.line_to(start.0 as f32, start.1 as f32);
    }
    let segments = ((a1 - a0) / FRAC_PI_2).ceil().max(1.0) as usize;
    let step = (a1 - a0) / segments as f64;
    // Control points of a cubic approximating each segment.
    let k = 4.0 / 3.0 * (step / 4.0).tan();
    for i in 0..segments {
        let (s, e) = (a0 + step * i as f64, a0 + step * (i + 1) as f64);
        let (p0, p3) = (point(s), point(e));
        let c1 = (p0.0 - k * r * s.sin(), p0.1 + k * r * s.cos());
        let c2 = (p3.0 + k * r * e.sin(), p3.1 - k * r * e.cos());
        pb.cubic_to(
            c1.0 as f32,
            c1.1 as f32,
            c2.0 as f32,
            c2.1 as f32,
            p3.0 as f32,
            p3.1 as f32,
        );
    }
}
