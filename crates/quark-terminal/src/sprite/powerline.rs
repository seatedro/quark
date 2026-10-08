//! Powerline separators, U+E0B0 to U+E0BF, U+E0D2, and U+E0D4
//! (Ghostty's `draw/powerline.zig`): the geometric glyphs, not the
//! stylized ones.

use tiny_skia::{FillRule, PathBuilder};

use super::box_drawing::{diagonal_down, diagonal_up};
use super::canvas::{Canvas, ON, StrokeStyle};
use super::common::Thickness;
use crate::metrics::CellMetrics;

/// Control point offset for a cubic approximating a quarter circle.
const ARC_C: f64 = (std::f64::consts::SQRT_2 - 1.0) * 4.0 / 3.0;

fn size(w: u32, h: u32) -> (f64, f64) {
    (f64::from(w), f64::from(h))
}

pub(super) fn draw_e0b0(_cp: u32, canvas: &mut Canvas, w: u32, h: u32, _m: &CellMetrics) {
    let (w, h) = size(w, h);
    canvas.triangle((0.0, 0.0), (w, h / 2.0), (0.0, h), ON);
}

pub(super) fn draw_e0b1(_cp: u32, canvas: &mut Canvas, w: u32, h: u32, m: &CellMetrics) {
    let (w, h) = size(w, h);
    let mut pb = PathBuilder::new();
    pb.move_to(0.0, 0.0);
    pb.line_to(w as f32, (h / 2.0) as f32);
    pb.line_to(0.0, h as f32);
    if let Some(path) = pb.finish() {
        let thick = f64::from(Thickness::Light.height(m.box_thickness));
        canvas.stroke_path(&path, StrokeStyle::new(thick), ON);
    }
}

pub(super) fn draw_e0b2(_cp: u32, canvas: &mut Canvas, w: u32, h: u32, _m: &CellMetrics) {
    let (w, h) = size(w, h);
    canvas.triangle((w, 0.0), (0.0, h / 2.0), (w, h), ON);
}

pub(super) fn draw_e0b3(cp: u32, canvas: &mut Canvas, w: u32, h: u32, m: &CellMetrics) {
    draw_e0b1(cp, canvas, w, h, m);
    canvas.flip_horizontal();
}

pub(super) fn draw_e0b4(_cp: u32, canvas: &mut Canvas, w: u32, h: u32, _m: &CellMetrics) {
    let (w, h) = size(w, h);
    let r = w.min(h / 2.0);
    let mut pb = PathBuilder::new();
    pb.move_to(0.0, 0.0);
    pb.cubic_to(
        (r * ARC_C) as f32,
        0.0,
        r as f32,
        (r - r * ARC_C) as f32,
        r as f32,
        r as f32,
    );
    pb.line_to(r as f32, (h - r) as f32);
    pb.cubic_to(
        r as f32,
        (h - r + r * ARC_C) as f32,
        (r * ARC_C) as f32,
        h as f32,
        0.0,
        h as f32,
    );
    pb.close();
    if let Some(path) = pb.finish() {
        canvas.fill_path(&path, FillRule::Winding, ON);
    }
}

pub(super) fn draw_e0b5(_cp: u32, canvas: &mut Canvas, w: u32, h: u32, m: &CellMetrics) {
    let (w, h) = size(w, h);
    let r = w.min(h / 2.0);
    let thick = f64::from(m.box_thickness);
    // The straight ends keep the offset ends horizontal, so their butt
    // caps are square to the cell's left edge.
    let mut points = vec![(0.0, 0.0), (1.0, 0.0)];
    flatten_cubic(&mut points, (r * ARC_C, 0.0), (r, r - r * ARC_C), (r, r));
    points.push((r, h - r));
    flatten_cubic(
        &mut points,
        (r, h - r + r * ARC_C),
        (r * ARC_C, h),
        (1.0, h),
    );
    points.push((0.0, h));
    inner_stroke(canvas, &points, thick);
}

pub(super) fn draw_e0b6(cp: u32, canvas: &mut Canvas, w: u32, h: u32, m: &CellMetrics) {
    draw_e0b4(cp, canvas, w, h, m);
    canvas.flip_horizontal();
}

pub(super) fn draw_e0b7(cp: u32, canvas: &mut Canvas, w: u32, h: u32, m: &CellMetrics) {
    draw_e0b5(cp, canvas, w, h, m);
    canvas.flip_horizontal();
}

pub(super) fn draw_e0b8(_cp: u32, canvas: &mut Canvas, w: u32, h: u32, _m: &CellMetrics) {
    let (w, h) = size(w, h);
    canvas.triangle((0.0, 0.0), (w, h), (0.0, h), ON);
}

pub(super) fn draw_e0b9(_cp: u32, canvas: &mut Canvas, _w: u32, _h: u32, m: &CellMetrics) {
    diagonal_down(m, canvas);
}

pub(super) fn draw_e0ba(_cp: u32, canvas: &mut Canvas, w: u32, h: u32, _m: &CellMetrics) {
    let (w, h) = size(w, h);
    canvas.triangle((w, 0.0), (w, h), (0.0, h), ON);
}

pub(super) fn draw_e0bb(_cp: u32, canvas: &mut Canvas, _w: u32, _h: u32, m: &CellMetrics) {
    diagonal_up(m, canvas);
}

pub(super) fn draw_e0bc(_cp: u32, canvas: &mut Canvas, w: u32, h: u32, _m: &CellMetrics) {
    let (w, h) = size(w, h);
    canvas.triangle((0.0, 0.0), (w, 0.0), (0.0, h), ON);
}

pub(super) fn draw_e0bd(_cp: u32, canvas: &mut Canvas, _w: u32, _h: u32, m: &CellMetrics) {
    diagonal_up(m, canvas);
}

pub(super) fn draw_e0be(_cp: u32, canvas: &mut Canvas, w: u32, h: u32, _m: &CellMetrics) {
    let (w, h) = size(w, h);
    canvas.triangle((0.0, 0.0), (w, 0.0), (w, h), ON);
}

pub(super) fn draw_e0bf(_cp: u32, canvas: &mut Canvas, _w: u32, _h: u32, m: &CellMetrics) {
    diagonal_down(m, canvas);
}

pub(super) fn draw_e0d2(_cp: u32, canvas: &mut Canvas, w: u32, h: u32, m: &CellMetrics) {
    let (w, h) = size(w, h);
    let half_thick = f64::from(m.box_thickness) / 2.0;
    let (top, bottom) = (h / 2.0 - half_thick, h / 2.0 + half_thick);
    canvas.quad((0.0, 0.0), (w, 0.0), (w / 2.0, top), (0.0, top), ON);
    canvas.quad((0.0, h), (w, h), (w / 2.0, bottom), (0.0, bottom), ON);
}

pub(super) fn draw_e0d4(cp: u32, canvas: &mut Canvas, w: u32, h: u32, m: &CellMetrics) {
    draw_e0d2(cp, canvas, w, h, m);
    canvas.flip_horizontal();
}

/// Appends the cubic from the last point of `points` through controls
/// `c1` and `c2` to `p3`, as short line segments.
fn flatten_cubic(points: &mut Vec<(f64, f64)>, c1: (f64, f64), c2: (f64, f64), p3: (f64, f64)) {
    const STEPS: usize = 32;
    let p0 = *points.last().expect("a start point");
    for i in 1..=STEPS {
        let t = i as f64 / STEPS as f64;
        let u = 1.0 - t;
        let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
        points.push((
            a * p0.0 + b * c1.0 + c * c2.0 + d * p3.0,
            a * p0.1 + b * c1.1 + c * c2.1 + d * p3.1,
        ));
    }
}

/// Strokes the open polyline `points` with butt caps after moving it half
/// the line width to its right (in the cell's y-down space), so the
/// stroke lies on that side of the original line (Ghostty's
/// `innerStrokePath`, which offsets the path the same way).
fn inner_stroke(canvas: &mut Canvas, points: &[(f64, f64)], thick: f64) {
    let d = thick / 2.0;
    // Repeated points (the straight side vanishes when the cell is
    // exactly two radii tall) have no direction.
    let mut points = points.to_vec();
    points.dedup();
    // Unit normals of each segment, pointing to its right.
    let normals: Vec<(f64, f64)> = points
        .windows(2)
        .map(|s| {
            let (dx, dy) = (s[1].0 - s[0].0, s[1].1 - s[0].1);
            let len = dx.hypot(dy);
            (-dy / len, dx / len)
        })
        .collect();
    let mut pb = PathBuilder::new();
    for (i, &(x, y)) in points.iter().enumerate() {
        let a = normals[i.saturating_sub(1)];
        let b = normals[i.min(normals.len() - 1)];
        // Miter offset: where the two offset segments meet.
        let k = d / (1.0 + a.0 * b.0 + a.1 * b.1);
        let (ox, oy) = (x + (a.0 + b.0) * k, y + (a.1 + b.1) * k);
        if i == 0 {
            pb.move_to(ox as f32, oy as f32);
        } else {
            pb.line_to(ox as f32, oy as f32);
        }
    }
    if let Some(path) = pb.finish() {
        canvas.stroke_path(&path, StrokeStyle::new(thick), ON);
    }
}
