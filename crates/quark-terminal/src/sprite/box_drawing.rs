//! Box Drawing, U+2500 to U+257F (Ghostty's `draw/box.zig`).
//!
//! ─━│┃┄┅┆┇┈┉┊┋┌┍┎┏┐┑┒┓└┕┖┗┘┙┚┛├┝┞┟┠┡┢┣┤┥┦┧┨┩┪┫┬┭┮┯┰┱┲┳┴┵┶┷┸┹┺┻┼┽┾┿
//! ╀╁╂╃╄╅╆╇╈╉╊╋╌╍╎╏═║╒╓╔╕╖╗╘╙╚╛╜╝╞╟╠╡╢╣╤╥╦╧╨╩╪╫╬╭╮╯╰╱╲╳╴╵╶╷╸╹╺╻╼╽╾╿
//!
//! Lines run from the center of the cell to its edges, so a light line
//! of one cell meets its neighbor's at the same pixels.

use tiny_skia::PathBuilder;

use super::canvas::{Canvas, ON, StrokeStyle};
use super::common::{Corner, Thickness, hline, hline_middle, vline, vline_middle};
use crate::metrics::CellMetrics;

/// The style of each arm of a line-drawing character.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct Lines {
    pub up: Style,
    pub right: Style,
    pub down: Style,
    pub left: Style,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum Style {
    #[default]
    None,
    Light,
    Heavy,
    Double,
}

/// Arms of U+2500..=U+254B and U+2550..=U+256C as up, right, down, left;
/// `.` none, `l` light, `h` heavy, `d` double. Dashes, arcs, and
/// diagonals are drawn by hand and marked `-`.
#[rustfmt::skip]
const LINES: [&str; 0x80] = [
    ".l.l", ".h.h", "l.l.", "h.h.", "-", "-", "-", "-",
    "-", "-", "-", "-", ".ll.", ".hl.", ".lh.", ".hh.",
    "..ll", "..lh", "..hl", "..hh", "ll..", "lh..", "hl..", "hh..",
    "l..l", "l..h", "h..l", "h..h", "lll.", "lhl.", "hll.", "llh.",
    "hlh.", "hhl.", "lhh.", "hhh.", "l.ll", "l.lh", "h.ll", "l.hl",
    "h.hl", "h.lh", "l.hh", "h.hh", ".lll", ".llh", ".hll", ".hlh",
    ".lhl", ".lhh", ".hhl", ".hhh", "ll.l", "ll.h", "lh.l", "lh.h",
    "hl.l", "hl.h", "hh.l", "hh.h", "llll", "lllh", "lhll", "lhlh",
    "hlll", "llhl", "hlhl", "hllh", "hhll", "llhh", "lhhl", "hhlh",
    "lhhh", "hlhh", "hhhl", "hhhh", "-", "-", "-", "-",
    ".d.d", "d.d.", ".dl.", ".ld.", ".dd.", "..ld", "..dl", "..dd",
    "ld..", "dl..", "dd..", "l..d", "d..l", "d..d", "ldl.", "dld.",
    "ddd.", "l.ld", "d.dl", "d.dd", ".dld", ".ldl", ".ddd", "ld.d",
    "dl.l", "dd.d", "ldld", "dldl", "dddd", "-", "-", "-",
    "-", "-", "-", "-", "...l", "l...", ".l..", "..l.",
    "...h", "h...", ".h..", "..h.", ".h.l", "l.h.", ".l.h", "h.l.",
];

fn parse(spec: &str) -> Lines {
    let style = |c: u8| match c {
        b'l' => Style::Light,
        b'h' => Style::Heavy,
        b'd' => Style::Double,
        _ => Style::None,
    };
    let b = spec.as_bytes();
    Lines {
        up: style(b[0]),
        right: style(b[1]),
        down: style(b[2]),
        left: style(b[3]),
    }
}

pub(super) fn draw_2500_257f(cp: u32, canvas: &mut Canvas, _w: u32, _h: u32, m: &CellMetrics) {
    let light = Thickness::Light.height(m.box_thickness);
    let heavy = Thickness::Heavy.height(m.box_thickness);
    match cp {
        0x2504 => dash_horizontal(m, canvas, 3, light, light.max(4)),
        0x2505 => dash_horizontal(m, canvas, 3, heavy, light.max(4)),
        0x2506 => dash_vertical(m, canvas, 3, light, light.max(4)),
        0x2507 => dash_vertical(m, canvas, 3, heavy, light.max(4)),
        0x2508 => dash_horizontal(m, canvas, 4, light, light.max(4)),
        0x2509 => dash_horizontal(m, canvas, 4, heavy, light.max(4)),
        0x250a => dash_vertical(m, canvas, 4, light, light.max(4)),
        0x250b => dash_vertical(m, canvas, 4, heavy, light.max(4)),
        0x254c => dash_horizontal(m, canvas, 2, light, light),
        0x254d => dash_horizontal(m, canvas, 2, heavy, heavy),
        0x254e => dash_vertical(m, canvas, 2, light, heavy),
        0x254f => dash_vertical(m, canvas, 2, heavy, heavy),
        0x256d => arc(m, canvas, Corner::Br, Thickness::Light),
        0x256e => arc(m, canvas, Corner::Bl, Thickness::Light),
        0x256f => arc(m, canvas, Corner::Tl, Thickness::Light),
        0x2570 => arc(m, canvas, Corner::Tr, Thickness::Light),
        0x2571 => diagonal_up(m, canvas),
        0x2572 => diagonal_down(m, canvas),
        0x2573 => {
            diagonal_up(m, canvas);
            diagonal_down(m, canvas);
        }
        _ => match LINES.get((cp - 0x2500) as usize) {
            Some(spec) if spec.len() == 4 => lines_char(m, canvas, parse(spec)),
            _ => {}
        },
    }
}

/// Draws each arm from the edge to where it meets the others, so light
/// and heavy arms join without notches and double arms leave the
/// crossing open where both strokes are double.
pub(super) fn lines_char(m: &CellMetrics, canvas: &mut Canvas, lines: Lines) {
    use Style::{Double, Heavy, Light, None};
    let light = Thickness::Light.height(m.box_thickness);
    let heavy = Thickness::Heavy.height(m.box_thickness);
    let (cw, ch) = (m.cell_width, m.cell_height);

    let h_light_top = ch.saturating_sub(light) / 2;
    let h_light_bottom = h_light_top + light;
    let h_heavy_top = ch.saturating_sub(heavy) / 2;
    let h_heavy_bottom = h_heavy_top + heavy;
    let h_double_top = h_light_top.saturating_sub(light);
    let h_double_bottom = h_light_bottom + light;

    let v_light_left = cw.saturating_sub(light) / 2;
    let v_light_right = v_light_left + light;
    let v_heavy_left = cw.saturating_sub(heavy) / 2;
    let v_heavy_right = v_heavy_left + heavy;
    let v_double_left = v_light_left.saturating_sub(light);
    let v_double_right = v_light_right + light;

    let (up, right, down, left) = (lines.up, lines.right, lines.down, lines.left);

    let up_bottom = if left == Heavy || right == Heavy {
        h_heavy_bottom
    } else if left != right || down == up {
        if left == Double || right == Double {
            h_double_bottom
        } else {
            h_light_bottom
        }
    } else if left == None && right == None {
        h_light_bottom
    } else {
        h_light_top
    };
    let down_top = if left == Heavy || right == Heavy {
        h_heavy_top
    } else if left != right || up == down {
        if left == Double || right == Double {
            h_double_top
        } else {
            h_light_top
        }
    } else if left == None && right == None {
        h_light_top
    } else {
        h_light_bottom
    };
    let left_right = if up == Heavy || down == Heavy {
        v_heavy_right
    } else if up != down || left == right {
        if up == Double || down == Double {
            v_double_right
        } else {
            v_light_right
        }
    } else if up == None && down == None {
        v_light_right
    } else {
        v_light_left
    };
    let right_left = if up == Heavy || down == Heavy {
        v_heavy_left
    } else if up != down || right == left {
        if up == Double || down == Double {
            v_double_left
        } else {
            v_light_left
        }
    } else if up == None && down == None {
        v_light_left
    } else {
        v_light_right
    };

    let mut b = |x0: u32, y0: u32, x1: u32, y1: u32| {
        canvas.box_(x0 as i32, y0 as i32, x1 as i32, y1 as i32, ON);
    };
    match up {
        None => {}
        Light => b(v_light_left, 0, v_light_right, up_bottom),
        Heavy => b(v_heavy_left, 0, v_heavy_right, up_bottom),
        Double => {
            let left_bottom = if left == Double {
                h_light_top
            } else {
                up_bottom
            };
            let right_bottom = if right == Double {
                h_light_top
            } else {
                up_bottom
            };
            b(v_double_left, 0, v_light_left, left_bottom);
            b(v_light_right, 0, v_double_right, right_bottom);
        }
    }
    match right {
        None => {}
        Light => b(right_left, h_light_top, cw, h_light_bottom),
        Heavy => b(right_left, h_heavy_top, cw, h_heavy_bottom),
        Double => {
            let top_left = if up == Double {
                v_light_right
            } else {
                right_left
            };
            let bottom_left = if down == Double {
                v_light_right
            } else {
                right_left
            };
            b(top_left, h_double_top, cw, h_light_top);
            b(bottom_left, h_light_bottom, cw, h_double_bottom);
        }
    }
    match down {
        None => {}
        Light => b(v_light_left, down_top, v_light_right, ch),
        Heavy => b(v_heavy_left, down_top, v_heavy_right, ch),
        Double => {
            let left_top = if left == Double {
                h_light_bottom
            } else {
                down_top
            };
            let right_top = if right == Double {
                h_light_bottom
            } else {
                down_top
            };
            b(v_double_left, left_top, v_light_left, ch);
            b(v_light_right, right_top, v_double_right, ch);
        }
    }
    match left {
        None => {}
        Light => b(0, h_light_top, left_right, h_light_bottom),
        Heavy => b(0, h_heavy_top, left_right, h_heavy_bottom),
        Double => {
            let top_right = if up == Double {
                v_light_left
            } else {
                left_right
            };
            let bottom_right = if down == Double {
                v_light_left
            } else {
                left_right
            };
            b(0, h_double_top, top_right, h_light_top);
            b(0, h_light_bottom, bottom_right, h_double_bottom);
        }
    }
}

/// The slope-preserving overshoot past the corners, so diagonals of
/// neighboring cells join.
fn overshoot(m: &CellMetrics) -> (f64, f64, f64, f64) {
    let (w, h) = (f64::from(m.cell_width), f64::from(m.cell_height));
    (w, h, (w / h).min(1.0), (h / w).min(1.0))
}

pub(super) fn diagonal_up(m: &CellMetrics, canvas: &mut Canvas) {
    let (w, h, sx, sy) = overshoot(m);
    let thick = f64::from(Thickness::Light.height(m.box_thickness));
    canvas.line(
        (w + 0.5 * sx, -0.5 * sy),
        (-0.5 * sx, h + 0.5 * sy),
        thick,
        ON,
    );
}

pub(super) fn diagonal_down(m: &CellMetrics, canvas: &mut Canvas) {
    let (w, h, sx, sy) = overshoot(m);
    let thick = f64::from(Thickness::Light.height(m.box_thickness));
    canvas.line(
        (-0.5 * sx, -0.5 * sy),
        (w + 0.5 * sx, h + 0.5 * sy),
        thick,
        ON,
    );
}

/// A rounded corner joining the arms toward `corner`, its curve a quarter
/// ellipse of half the smaller cell side.
pub(super) fn arc(m: &CellMetrics, canvas: &mut Canvas, corner: Corner, thickness: Thickness) {
    let thick = thickness.height(m.box_thickness);
    let (w, h, t) = (
        f64::from(m.cell_width),
        f64::from(m.cell_height),
        f64::from(thick),
    );
    let cx = f64::from(m.cell_width.saturating_sub(thick) / 2) + t / 2.0;
    let cy = f64::from(m.cell_height.saturating_sub(thick) / 2) + t / 2.0;
    let r = w.min(h) / 2.0;
    // How far from the center the curve's inner control points sit.
    let s = 0.25;
    let (y_edge, dy) = match corner {
        Corner::Tl | Corner::Tr => (0.0, -1.0),
        Corner::Bl | Corner::Br => (h, 1.0),
    };
    let (x_edge, dx) = match corner {
        Corner::Tl | Corner::Bl => (0.0, -1.0),
        Corner::Tr | Corner::Br => (w, 1.0),
    };
    let mut pb = PathBuilder::new();
    pb.move_to(cx as f32, y_edge as f32);
    pb.line_to(cx as f32, (cy + dy * r) as f32);
    pb.cubic_to(
        cx as f32,
        (cy + dy * s * r) as f32,
        (cx + dx * s * r) as f32,
        cy as f32,
        (cx + dx * r) as f32,
        cy as f32,
    );
    pb.line_to(x_edge as f32, cy as f32);
    if let Some(path) = pb.finish() {
        canvas.stroke_path(&path, StrokeStyle::new(t), ON);
    }
}

/// `count` dashes across the cell with half gaps at both ends, so a row of
/// them tiles evenly; spare pixels widen dashes, not gaps.
fn dash_horizontal(m: &CellMetrics, canvas: &mut Canvas, count: u32, thick: u32, gap: u32) {
    if m.cell_width < 2 * count {
        hline_middle(m, canvas, Thickness::Light);
        return;
    }
    let gap = gap.min(m.cell_width / (2 * count)) as i32;
    let count = count as i32;
    let total_dash = m.cell_width as i32 - count * gap;
    let dash = total_dash.div_euclid(count);
    let mut extra = total_dash.rem_euclid(count);
    let y = (m.cell_height.saturating_sub(thick) / 2) as i32;
    let mut x = gap / 2;
    for _ in 0..count {
        let mut x1 = x + dash;
        if extra > 0 {
            extra -= 1;
            x1 += 1;
        }
        hline(canvas, x, x1, y, thick);
        x = x1 + gap;
    }
}

/// `count` dashes down the cell with one whole gap at the bottom, so they
/// join solid lines above without a half gap.
fn dash_vertical(m: &CellMetrics, canvas: &mut Canvas, count: u32, thick: u32, gap: u32) {
    if m.cell_height < 2 * count {
        vline_middle(m, canvas, Thickness::Light);
        return;
    }
    let gap = gap.min(m.cell_height / (2 * count)) as i32;
    let count = count as i32;
    let total_dash = m.cell_height as i32 - count * gap;
    let dash = total_dash.div_euclid(count);
    let mut extra = total_dash.rem_euclid(count);
    let x = (m.cell_width.saturating_sub(thick) / 2) as i32;
    let mut y = 0;
    for _ in 0..count {
        let mut y1 = y + dash;
        if extra > 0 {
            extra -= 1;
            y1 += 1;
        }
        vline(canvas, y, y1, x, thick);
        y = y1 + gap;
    }
}
