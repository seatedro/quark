//! Symbols for Legacy Computing Supplement, U+1CC00 to U+1CEBF
//! (Ghostty's `draw/symbols_for_legacy_computing_supplement.zig`).
//!
//! 𜰛𜰜𜰝𜰞 𜰡𜰢𜰣𜰤𜰥𜰦𜰧𜰨𜰩𜰪𜰫𜰬𜰭𜰮𜰯 𜰰𜰱𜰲𜰳𜰴𜰵𜰶𜰷𜰸𜰹𜰺𜰻𜰼𜰽𜰾𜰿
//! the octants U+1CD00..=U+1CDE5, 𜸀𜸁𜸋𜸌 𜸖𜸗𜸘𜸙,
//! the separated sextants U+1CE51..=U+1CE8F, and the sixteenth blocks
//! U+1CE90..=U+1CEAF.

use tiny_skia::PathBuilder;

use super::box_drawing::{Lines, Style, lines_char};
use super::canvas::{Canvas, ON, StrokeStyle};
use super::common::{Alignment, Corner, Fraction, fill};
use super::legacy_computing::{circle, clip_to_cell};
use crate::metrics::CellMetrics;

pub(super) fn draw_1cc1b_1cc1e(cp: u32, canvas: &mut Canvas, w: u32, h: u32, m: &CellMetrics) {
    let (w, h, t) = (w as i32, h as i32, m.box_thickness as i32);
    let horizontal = Lines {
        left: Style::Light,
        right: Style::Light,
        ..Lines::default()
    };
    match cp {
        0x1cc1b => {
            lines_char(m, canvas, horizontal);
            canvas.box_(w - t, 0, w, h.div_euclid(2), ON);
        }
        0x1cc1c => {
            lines_char(m, canvas, horizontal);
            canvas.box_(w - t, h.div_euclid(2), w, h, ON);
        }
        0x1cc1d => {
            canvas.box_(0, 0, w, t, ON);
            canvas.box_(0, 0, t, h.div_euclid(2), ON);
        }
        _ => {
            canvas.box_(0, h - t, w, h, ON);
            canvas.box_(0, h.div_euclid(2), t, h, ON);
        }
    }
}

/// Separated block quadrants: the low four bits of `cp - 0x1cc20` are
/// tl, tr, bl, br.
pub(super) fn draw_1cc21_1cc2f(cp: u32, canvas: &mut Canvas, w: u32, h: u32, _m: &CellMetrics) {
    let bits = (cp - 0x1cc20) & 0xf;
    let gap = (w / 12).max(1) as i32;
    let mid_x = gap * 2 + (w % 2) as i32;
    let mid_y = gap * 2 + (h % 2) as i32;
    let bw = (w as i32 - gap * 2 - mid_x) / 2;
    let bh = (h as i32 - gap * 2 - mid_y) / 2;
    let (x1, y1) = (gap + bw + mid_x, gap + bh + mid_y);
    let blocks = [(gap, gap), (x1, gap), (gap, y1), (x1, y1)];
    for (i, (x, y)) in blocks.into_iter().enumerate() {
        if bits & (1 << i) != 0 {
            canvas.box_(x, y, x + bw, y + bh, ON);
        }
    }
}

/// Twelfth and quarter circle pieces: quarter ellipses sized to touch the
/// edges of the 4 by 4 (twelfths) or 2 by 2 (quarters) block of cells they
/// tile, as `(x, y, w, h, corner)` arguments of [`circle_piece`].
pub(super) fn draw_1cc30_1cc3f(cp: u32, canvas: &mut Canvas, w: u32, h: u32, m: &CellMetrics) {
    use Corner::*;
    const PIECES: [(f64, f64, f64, f64, Corner); 16] = [
        (0.0, 0.0, 2.0, 2.0, Tl),
        (1.0, 0.0, 2.0, 2.0, Tl),
        (2.0, 0.0, 2.0, 2.0, Tr),
        (3.0, 0.0, 2.0, 2.0, Tr),
        (0.0, 1.0, 2.0, 2.0, Tl),
        (0.0, 0.0, 1.0, 1.0, Tl),
        (1.0, 0.0, 1.0, 1.0, Tr),
        (3.0, 1.0, 2.0, 2.0, Tr),
        (0.0, 2.0, 2.0, 2.0, Bl),
        (0.0, 1.0, 1.0, 1.0, Bl),
        (1.0, 1.0, 1.0, 1.0, Br),
        (3.0, 2.0, 2.0, 2.0, Br),
        (0.0, 3.0, 2.0, 2.0, Bl),
        (1.0, 3.0, 2.0, 2.0, Bl),
        (2.0, 3.0, 2.0, 2.0, Br),
        (3.0, 3.0, 2.0, 2.0, Br),
    ];
    let (x, y, pw, ph, corner) = PIECES[(cp - 0x1cc30) as usize];
    circle_piece(canvas, w, h, m, x, y, pw, ph, corner);
}

/// Octants U+1CD00..=U+1CDE5, generated from Ghostty's `draw/octants.txt`
/// ("BLOCK OCTANT-1235" and so on): bit `n - 1` is octant `n`, numbered
/// left to right, top to bottom.
#[rustfmt::skip]
const OCTANTS: [u8; 0x1cde5 - 0x1cd00 + 1] = [
    0x04, 0x06, 0x07, 0x08, 0x09, 0x0b, 0x0c, 0x0d, 0x0e, 0x10, 0x11, 0x12,
    0x13, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e, 0x1f,
    0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27, 0x29, 0x2a, 0x2b, 0x2c,
    0x2d, 0x2e, 0x2f, 0x30, 0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38,
    0x39, 0x3a, 0x3b, 0x3c, 0x3d, 0x3e, 0x41, 0x42, 0x43, 0x44, 0x45, 0x46,
    0x47, 0x48, 0x49, 0x4a, 0x4b, 0x4c, 0x4d, 0x4e, 0x4f, 0x51, 0x52, 0x53,
    0x54, 0x56, 0x57, 0x58, 0x59, 0x5b, 0x5c, 0x5d, 0x5e, 0x60, 0x61, 0x62,
    0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69, 0x6a, 0x6b, 0x6c, 0x6d, 0x6e,
    0x6f, 0x70, 0x71, 0x72, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7a,
    0x7b, 0x7c, 0x7d, 0x7e, 0x7f, 0x81, 0x82, 0x83, 0x84, 0x85, 0x86, 0x87,
    0x88, 0x89, 0x8a, 0x8b, 0x8c, 0x8d, 0x8e, 0x8f, 0x90, 0x91, 0x92, 0x93,
    0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9a, 0x9b, 0x9c, 0x9d, 0x9e, 0x9f,
    0xa1, 0xa2, 0xa3, 0xa4, 0xa6, 0xa7, 0xa8, 0xa9, 0xab, 0xac, 0xad, 0xae,
    0xb0, 0xb1, 0xb2, 0xb3, 0xb4, 0xb5, 0xb6, 0xb7, 0xb8, 0xb9, 0xba, 0xbb,
    0xbc, 0xbd, 0xbe, 0xbf, 0xc1, 0xc2, 0xc3, 0xc4, 0xc5, 0xc6, 0xc7, 0xc8,
    0xc9, 0xca, 0xcb, 0xcc, 0xcd, 0xce, 0xcf, 0xd0, 0xd1, 0xd2, 0xd3, 0xd4,
    0xd5, 0xd6, 0xd7, 0xd8, 0xd9, 0xda, 0xdb, 0xdc, 0xdd, 0xde, 0xdf, 0xe0,
    0xe1, 0xe2, 0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8, 0xe9, 0xea, 0xeb, 0xec,
    0xed, 0xee, 0xef, 0xf1, 0xf2, 0xf3, 0xf4, 0xf6, 0xf7, 0xf8, 0xf9, 0xfb,
    0xfd, 0xfe,
];

pub(super) fn draw_1cd00_1cde5(cp: u32, canvas: &mut Canvas, _w: u32, _h: u32, m: &CellMetrics) {
    let bits = OCTANTS[(cp - 0x1cd00) as usize];
    let q = Fraction::QUARTERS;
    let h = Fraction::HALVES;
    for i in 0..8 {
        if bits & (1 << i) != 0 {
            let (col, row) = (i % 2, i / 2);
            fill(m, canvas, h[col], h[col + 1], q[row], q[row + 1]);
        }
    }
}

/// 𜸀
pub(super) fn draw_1ce00(_cp: u32, canvas: &mut Canvas, _w: u32, _h: u32, m: &CellMetrics) {
    circle(m, canvas, Alignment::LEFT, false);
    circle(m, canvas, Alignment::RIGHT, false);
}

/// 𜸁
pub(super) fn draw_1ce01(_cp: u32, canvas: &mut Canvas, _w: u32, _h: u32, m: &CellMetrics) {
    circle(m, canvas, Alignment::UPPER, false);
    circle(m, canvas, Alignment::LOWER, false);
}

/// 𜸋
pub(super) fn draw_1ce0b(_cp: u32, canvas: &mut Canvas, w: u32, h: u32, m: &CellMetrics) {
    circle_piece(canvas, w, h, m, 0.0, 0.0, 1.0, 0.5, Corner::Tl);
    circle_piece(canvas, w, h, m, 0.0, 0.0, 1.0, 0.5, Corner::Bl);
}

/// 𜸌
pub(super) fn draw_1ce0c(_cp: u32, canvas: &mut Canvas, w: u32, h: u32, m: &CellMetrics) {
    circle_piece(canvas, w, h, m, 1.0, 0.0, 1.0, 0.5, Corner::Tr);
    circle_piece(canvas, w, h, m, 1.0, 0.0, 1.0, 0.5, Corner::Br);
}

pub(super) fn draw_1ce16_1ce19(cp: u32, canvas: &mut Canvas, w: u32, h: u32, m: &CellMetrics) {
    let (w, h, t) = (w as i32, h as i32, m.box_thickness as i32);
    let vertical = Lines {
        up: Style::Light,
        down: Style::Light,
        ..Lines::default()
    };
    lines_char(m, canvas, vertical);
    let half = w.div_euclid(2);
    match cp {
        0x1ce16 => canvas.box_(half, 0, w, t, ON),
        0x1ce17 => canvas.box_(half, h - t, w, h, ON),
        0x1ce18 => canvas.box_(0, 0, half, t, ON),
        _ => canvas.box_(0, h - t, half, h, ON),
    }
}

/// Separated block sextants: the low six bits of `cp - 0x1ce50` are tl,
/// tr, ml, mr, bl, br.
pub(super) fn draw_1ce51_1ce8f(cp: u32, canvas: &mut Canvas, w: u32, h: u32, _m: &CellMetrics) {
    let bits = (cp - 0x1ce50) & 0x3f;
    let (wi, hi) = (w as i32, h as i32);
    let gap = (w / 12).max(1) as i32;
    let mid_x = gap * 2 + (w % 2) as i32;
    let mid_y = gap * 2 + ((h % 3) as i32).div_euclid(2);
    let bw = (wi - gap * 2 - mid_x) / 2;
    let bh = (hi - gap * 2 - mid_y * 2).div_euclid(3);
    // Leftover height goes to the middle row.
    let bh_m = hi - gap * 2 - mid_y * 2 - bh * 2;
    let x1 = gap + bw + mid_x;
    let y1 = gap + bh + mid_y;
    let y2 = y1 + bh_m + mid_y;
    let blocks = [
        (gap, gap, bh),
        (x1, gap, bh),
        (gap, y1, bh_m),
        (x1, y1, bh_m),
        (gap, y2, bh),
        (x1, y2, bh),
    ];
    for (i, (x, y, height)) in blocks.into_iter().enumerate() {
        if bits & (1 << i) != 0 {
            canvas.box_(x, y, x + bw, y + height, ON);
        }
    }
}

/// Sixteenth blocks and the quarter-wide bars along the edges, as quarter
/// indices `(x0, x1, y0, y1)`.
pub(super) fn draw_1ce90_1ceaf(cp: u32, canvas: &mut Canvas, _w: u32, _h: u32, m: &CellMetrics) {
    let (x0, x1, y0, y1) = match cp - 0x1ce90 {
        i @ 0..=0xf => {
            let (col, row) = (i as usize % 4, i as usize / 4);
            (col, col + 1, row, row + 1)
        }
        i => [
            (2, 4, 3, 4),
            (1, 4, 3, 4),
            (0, 3, 3, 4),
            (0, 2, 3, 4),
            (0, 1, 2, 4),
            (0, 1, 1, 4),
            (0, 1, 0, 3),
            (0, 1, 0, 2),
            (0, 2, 0, 1),
            (0, 3, 0, 1),
            (1, 4, 0, 1),
            (2, 4, 0, 1),
            (3, 4, 0, 2),
            (3, 4, 0, 3),
            (3, 4, 1, 4),
            (3, 4, 2, 4),
        ][i as usize - 0x10],
    };
    let q = Fraction::QUARTERS;
    fill(m, canvas, q[x0], q[x1], q[y0], q[y1]);
}

/// A quarter ellipse stroked at the box thickness, for a piece at cell
/// `(x, y)` of a `w` by `h` cell ellipse quadrant toward `corner`,
/// clipped to the cell.
#[allow(clippy::too_many_arguments)]
fn circle_piece(
    canvas: &mut Canvas,
    width: u32,
    height: u32,
    m: &CellMetrics,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    corner: Corner,
) {
    let wd = f64::from(width) * w;
    let hg = f64::from(height) * h;
    let xp = f64::from(width) * x;
    let yp = f64::from(height) * y;
    // Control point distance for a cubic approximating a quarter circle.
    let c = (std::f64::consts::SQRT_2 - 1.0) * 4.0 / 3.0;
    let (cw, ch) = (c * wd, c * hg);
    let thick = f64::from(m.box_thickness);
    let ht = thick * 0.5;
    let pts: [(f64, f64); 4] = match corner {
        Corner::Tl => [(wd, ht), (wd - cw, ht), (ht, hg - ch), (ht, hg)],
        Corner::Tr => [
            (wd, ht),
            (wd + cw, ht),
            (wd * 2.0 - ht, hg - ch),
            (wd * 2.0 - ht, hg),
        ],
        Corner::Bl => [
            (ht, hg),
            (ht, hg + ch),
            (wd - cw, hg * 2.0 - ht),
            (wd, hg * 2.0 - ht),
        ],
        Corner::Br => [
            (wd * 2.0 - ht, hg),
            (wd * 2.0 - ht, hg + ch),
            (wd + cw, hg * 2.0 - ht),
            (wd, hg * 2.0 - ht),
        ],
    };
    let p = pts.map(|(px, py)| ((px - xp) as f32, (py - yp) as f32));
    let mut pb = PathBuilder::new();
    pb.move_to(p[0].0, p[0].1);
    pb.cubic_to(p[1].0, p[1].1, p[2].0, p[2].1, p[3].0, p[3].1);
    if let Some(path) = pb.finish() {
        canvas.stroke_path(&path, StrokeStyle::new(thick), ON);
    }
    clip_to_cell(canvas);
}
