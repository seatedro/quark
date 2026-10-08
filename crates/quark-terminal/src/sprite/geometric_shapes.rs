//! Geometric Shapes that tile, U+25E2 to U+25E5, U+25F8 to U+25FA, and
//! U+25FF (Ghostty's `draw/geometric_shapes.zig`).
//!
//! ◢◣◤◥ ◸◹◺ ◿

use tiny_skia::{FillRule, PathBuilder};

use super::canvas::{Canvas, ON};
use super::common::{Corner, Shade, Thickness};
use crate::metrics::CellMetrics;

pub(super) fn draw_25e2_25e5(cp: u32, canvas: &mut Canvas, _w: u32, _h: u32, m: &CellMetrics) {
    let corner = match cp {
        0x25e2 => Corner::Br,
        0x25e3 => Corner::Bl,
        0x25e4 => Corner::Tl,
        0x25e5 => Corner::Tr,
        _ => return,
    };
    corner_triangle_shade(m, canvas, corner, Shade::On);
}

pub(super) fn draw_25f8_25fa(cp: u32, canvas: &mut Canvas, _w: u32, _h: u32, m: &CellMetrics) {
    let corner = match cp {
        0x25f8 => Corner::Tl,
        0x25f9 => Corner::Tr,
        0x25fa => Corner::Bl,
        _ => return,
    };
    corner_triangle_outline(m, canvas, corner);
}

pub(super) fn draw_25ff(_cp: u32, canvas: &mut Canvas, _w: u32, _h: u32, m: &CellMetrics) {
    corner_triangle_outline(m, canvas, Corner::Br);
}

/// The right triangle filling half the cell, its right angle at `corner`.
fn corner_triangle(m: &CellMetrics, corner: Corner) -> [(f64, f64); 3] {
    let (w, h) = (f64::from(m.cell_width), f64::from(m.cell_height));
    match corner {
        Corner::Tl => [(0.0, 0.0), (0.0, h), (w, 0.0)],
        Corner::Tr => [(0.0, 0.0), (w, h), (w, 0.0)],
        Corner::Bl => [(0.0, 0.0), (0.0, h), (w, h)],
        Corner::Br => [(0.0, h), (w, h), (w, 0.0)],
    }
}

pub(super) fn corner_triangle_shade(
    m: &CellMetrics,
    canvas: &mut Canvas,
    corner: Corner,
    shade: Shade,
) {
    canvas.polygon(&corner_triangle(m, corner), shade as u8);
}

/// The outline of [`corner_triangle`], stroked inside its edges.
pub(super) fn corner_triangle_outline(m: &CellMetrics, canvas: &mut Canvas, corner: Corner) {
    let thick = f64::from(Thickness::Light.height(m.box_thickness));
    let outer = corner_triangle(m, corner);
    // Ghostty strokes the triangle inset by half the width with miter
    // joins, which covers exactly the band between the triangle and the
    // triangle inset by the full width. Filling that band directly avoids
    // tiny-skia's lower miter limit beveling the sharp corners.
    let inner = inset_triangle(outer, thick);
    let mut pb = PathBuilder::new();
    for tri in [outer, inner] {
        pb.move_to(tri[0].0 as f32, tri[0].1 as f32);
        pb.line_to(tri[1].0 as f32, tri[1].1 as f32);
        pb.line_to(tri[2].0 as f32, tri[2].1 as f32);
        pb.close();
    }
    if let Some(path) = pb.finish() {
        canvas.fill_path(&path, FillRule::EvenOdd, ON);
    }
}

/// `tri` with every edge moved `d` toward its interior.
fn inset_triangle(tri: [(f64, f64); 3], d: f64) -> [(f64, f64); 3] {
    let [a, b, c] = tri;
    // Orientation picks the side of each edge the interior lies on.
    let area = (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0);
    let sign = area.signum();
    // Each edge as a point on its inset line and its direction.
    let edge = |p: (f64, f64), q: (f64, f64)| {
        let (dx, dy) = (q.0 - p.0, q.1 - p.1);
        let len = dx.hypot(dy);
        let n = (-dy / len * sign * d, dx / len * sign * d);
        ((p.0 + n.0, p.1 + n.1), (dx, dy))
    };
    let edges = [edge(a, b), edge(b, c), edge(c, a)];
    let meet = |(p, u): ((f64, f64), (f64, f64)), (q, v): ((f64, f64), (f64, f64))| {
        let t = ((q.0 - p.0) * v.1 - (q.1 - p.1) * v.0) / (u.0 * v.1 - u.1 * v.0);
        (p.0 + t * u.0, p.1 + t * u.1)
    };
    [
        meet(edges[2], edges[0]),
        meet(edges[0], edges[1]),
        meet(edges[1], edges[2]),
    ]
}
