//! Branch drawing for git graphs, U+F5D0 to U+F60D (Ghostty's
//! `draw/branch.zig`, after the set Kitty introduced).

use std::f64::consts::TAU;

use tiny_skia::{FillRule, PathBuilder};

use super::box_drawing::arc;
use super::canvas::{Canvas, ON, StrokeStyle};
use super::common::{self, Corner, Edge, Thickness, hline_middle, vline_middle};
use crate::metrics::CellMetrics;

/// A node: a circle, filled or not, with lines from it to some edges.
#[derive(Debug, Clone, Copy, Default)]
struct BranchNode {
    up: bool,
    right: bool,
    down: bool,
    left: bool,
    filled: bool,
}

pub(super) fn draw_f5d0_f60d(cp: u32, canvas: &mut Canvas, _w: u32, _h: u32, m: &CellMetrics) {
    use Corner::{Bl, Br, Tl, Tr};
    const L: Thickness = Thickness::Light;
    let arcs = |canvas: &mut Canvas, corners: &[Corner]| {
        for &c in corners {
            arc(m, canvas, c, L);
        }
    };
    match cp {
        0xf5d0 => hline_middle(m, canvas, L),
        0xf5d1 => vline_middle(m, canvas, L),
        0xf5d2 => fading_line(m, canvas, Edge::Right, L),
        0xf5d3 => fading_line(m, canvas, Edge::Left, L),
        0xf5d4 => fading_line(m, canvas, Edge::Bottom, L),
        0xf5d5 => fading_line(m, canvas, Edge::Top, L),
        0xf5d6 => arcs(canvas, &[Br]),
        0xf5d7 => arcs(canvas, &[Bl]),
        0xf5d8 => arcs(canvas, &[Tr]),
        0xf5d9 => arcs(canvas, &[Tl]),
        0xf5da => {
            vline_middle(m, canvas, L);
            arcs(canvas, &[Tr]);
        }
        0xf5db => {
            vline_middle(m, canvas, L);
            arcs(canvas, &[Br]);
        }
        0xf5dc => arcs(canvas, &[Tr, Br]),
        0xf5dd => {
            vline_middle(m, canvas, L);
            arcs(canvas, &[Tl]);
        }
        0xf5de => {
            vline_middle(m, canvas, L);
            arcs(canvas, &[Bl]);
        }
        0xf5df => arcs(canvas, &[Tl, Bl]),
        0xf5e0 => {
            arcs(canvas, &[Bl]);
            hline_middle(m, canvas, L);
        }
        0xf5e1 => {
            arcs(canvas, &[Br]);
            hline_middle(m, canvas, L);
        }
        0xf5e2 => arcs(canvas, &[Br, Bl]),
        0xf5e3 => {
            arcs(canvas, &[Tl]);
            hline_middle(m, canvas, L);
        }
        0xf5e4 => {
            arcs(canvas, &[Tr]);
            hline_middle(m, canvas, L);
        }
        0xf5e5 => arcs(canvas, &[Tr, Tl]),
        0xf5e6 => {
            vline_middle(m, canvas, L);
            arcs(canvas, &[Tl, Tr]);
        }
        0xf5e7 => {
            vline_middle(m, canvas, L);
            arcs(canvas, &[Bl, Br]);
        }
        0xf5e8 => {
            hline_middle(m, canvas, L);
            arcs(canvas, &[Bl, Tl]);
        }
        0xf5e9 => {
            hline_middle(m, canvas, L);
            arcs(canvas, &[Tr, Br]);
        }
        0xf5ea => {
            vline_middle(m, canvas, L);
            arcs(canvas, &[Tl, Br]);
        }
        0xf5eb => {
            vline_middle(m, canvas, L);
            arcs(canvas, &[Tr, Bl]);
        }
        0xf5ec => {
            hline_middle(m, canvas, L);
            arcs(canvas, &[Tl, Br]);
        }
        0xf5ed => {
            hline_middle(m, canvas, L);
            arcs(canvas, &[Tr, Bl]);
        }
        0xf5ee..=0xf60d => {
            // Nodes come in filled and empty pairs, cycling through which
            // edges they connect to.
            let i = cp - 0xf5ee;
            let (up, right, down, left) = match i / 2 {
                0 => (false, false, false, false),
                1 => (false, true, false, false),
                2 => (false, false, false, true),
                3 => (false, true, false, true),
                4 => (false, false, true, false),
                5 => (true, false, false, false),
                6 => (true, false, true, false),
                7 => (false, true, true, false),
                8 => (false, false, true, true),
                9 => (true, true, false, false),
                10 => (true, false, false, true),
                11 => (true, true, true, false),
                12 => (true, false, true, true),
                13 => (false, true, true, true),
                14 => (true, true, false, true),
                _ => (true, true, true, true),
            };
            let node = BranchNode {
                up,
                right,
                down,
                left,
                filled: i.is_multiple_of(2),
            };
            branch_node(m, canvas, node, L);
        }
        _ => {}
    }
}

/// Where the middle lines of `thickness` sit: the top and bottom of the
/// horizontal one, then the left and right of the vertical one.
fn middle_lines(m: &CellMetrics, thick: u32) -> (u32, u32, u32, u32) {
    let h_top = m.cell_height.saturating_sub(thick) / 2;
    let v_left = m.cell_width.saturating_sub(thick) / 2;
    (
        h_top,
        h_top.saturating_add(thick),
        v_left,
        v_left.saturating_add(thick),
    )
}

fn branch_node(m: &CellMetrics, canvas: &mut Canvas, node: BranchNode, thickness: Thickness) {
    let thick_px = thickness.height(m.box_thickness);
    let (w, h, t) = (
        f64::from(m.cell_width),
        f64::from(m.cell_height),
        f64::from(thick_px),
    );
    let (h_top, h_bottom, v_left, v_right) = middle_lines(m, thick_px);
    // Centered on the middle lines, which sit off center when the cell
    // can't split them evenly, so the circle lines up with box drawing.
    let cx = f64::from(v_left) + t / 2.0;
    let cy = f64::from(h_top) + t / 2.0;
    let r = cx.min(cy).min(w - cx).min(h - cy);
    let (h_top, h_bottom, v_left, v_right) =
        (h_top as i32, h_bottom as i32, v_left as i32, v_right as i32);
    let (cw, ch) = (m.cell_width as i32, m.cell_height as i32);

    if node.up {
        let y = (cy - r + t / 2.0).ceil() as i32;
        canvas.box_(v_left, 0, v_right, y, ON);
    }
    if node.right {
        let x = (cx + r - t / 2.0).floor() as i32;
        canvas.box_(x, h_top, cw, h_bottom, ON);
    }
    if node.down {
        let y = (cy + r - t / 2.0).floor() as i32;
        canvas.box_(v_left, y, v_right, ch, ON);
    }
    if node.left {
        let x = (cx - r + t / 2.0).ceil() as i32;
        canvas.box_(0, h_top, x, h_bottom, ON);
    }

    let mut pb = PathBuilder::new();
    let radius = if node.filled { r } else { r - t / 2.0 };
    common::arc(&mut pb, cx, cy, radius, 0.0, TAU);
    pb.close();
    let Some(path) = pb.finish() else { return };
    if node.filled {
        canvas.fill_path(&path, FillRule::Winding, ON);
    } else {
        canvas.stroke_path(&path, StrokeStyle::new(t), ON);
    }
}

/// A middle line that fades out linearly toward edge `to`.
fn fading_line(m: &CellMetrics, canvas: &mut Canvas, to: Edge, thickness: Thickness) {
    let thick_px = thickness.height(m.box_thickness);
    let (w, h) = (f64::from(m.cell_width), f64::from(m.cell_height));
    let (h_top, h_bottom, v_left, v_right) = middle_lines(m, thick_px);

    let mut color: f64 = match to {
        Edge::Top | Edge::Left => 0.0,
        Edge::Bottom | Edge::Right => 255.0,
    };
    let inc = 255.0
        / match to {
            Edge::Top => h,
            Edge::Bottom => -h,
            Edge::Left => w,
            Edge::Right => -w,
        };

    match to {
        Edge::Top | Edge::Bottom => {
            for y in 0..m.cell_height {
                for x in v_left..v_right {
                    canvas.pixel(x as i32, y as i32, color.round() as u8);
                }
                color += inc;
            }
        }
        Edge::Left | Edge::Right => {
            for x in 0..m.cell_width {
                for y in h_top..h_bottom {
                    canvas.pixel(x as i32, y as i32, color.round() as u8);
                }
                color += inc;
            }
        }
    }
}
