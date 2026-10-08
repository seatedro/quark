//! Symbols for Legacy Computing, U+1FB00 to U+1FBEF (Ghostty's
//! `draw/symbols_for_legacy_computing.zig`).
//!
//! 🬀🬁🬂🬃🬄🬅🬆🬇🬈🬉🬊🬋🬌🬍🬎🬏🬐🬑🬒🬓🬔🬕🬖🬗🬘🬙🬚🬛🬜🬝🬞🬟🬠🬡🬢🬣🬤🬥🬦🬧🬨🬩🬪🬫🬬🬭🬮🬯🬰🬱🬲🬳🬴🬵🬶🬷🬸🬹🬺🬻
//! 🬼🬽🬾🬿🭀🭁🭂🭃🭄🭅🭆🭇🭈🭉🭊🭋🭌🭍🭎🭏🭐🭑🭒🭓🭔🭕🭖🭗🭘🭙🭚🭛🭜🭝🭞🭟🭠🭡🭢🭣🭤🭥🭦🭧
//! 🭨🭩🭪🭫🭬🭭🭮🭯🭰🭱🭲🭳🭴🭵🭶🭷🭸🭹🭺🭻🭼🭽🭾🭿🮀🮁🮂🮃🮄🮅🮆🮇🮈🮉🮊🮋🮌🮍🮎🮏🮐🮑🮒🮔🮕🮖🮗
//! 🮘🮙🮚🮛🮜🮝🮞🮟🮠🮡🮢🮣🮤🮥🮦🮧🮨🮩🮪🮫🮬🮭🮮🮯🮽🮾🮿🯎🯏🯐🯑🯒🯓🯔🯕🯖🯗🯘🯙🯚🯛🯜🯝🯞🯟
//! 🯠🯡🯢🯣🯤🯥🯦🯧🯨🯩🯪🯫🯬🯭🯮🯯

use tiny_skia::{FillRule, PathBuilder};

use super::block::{block, block_shade, full_block_shade};
use super::box_drawing::{Lines, Style, diagonal_down, diagonal_up, lines_char};
use super::canvas::{Canvas, ON, Rect, StrokeStyle};
use super::common::{
    Alignment, Corner, Edge, FIVE_EIGHTHS, Fraction, HALF, Horizontal, ONE_EIGHTH, ONE_QUARTER,
    ONE_THIRD, Quads, SEVEN_EIGHTHS, Shade, THREE_EIGHTHS, THREE_QUARTERS, TWO_THIRDS, Thickness,
    Vertical, arc, fill,
};
use super::geometric_shapes::corner_triangle_shade;
use crate::metrics::CellMetrics;

/// Sextants: the codepoints count up through the 6-bit masks (top left
/// first), skipping empty, full, and the two half blocks.
pub(super) fn draw_1fb00_1fb3b(cp: u32, canvas: &mut Canvas, _w: u32, _h: u32, m: &CellMetrics) {
    use Fraction as F;
    let idx = cp - 0x1fb00;
    let bits = idx + idx / 0x14 + 1;
    let cells = [
        (F::ZERO, F::HALF, F::ZERO, F::ONE_THIRD),
        (F::HALF, F::FULL, F::ZERO, F::ONE_THIRD),
        (F::ZERO, F::HALF, F::ONE_THIRD, F::TWO_THIRDS),
        (F::HALF, F::FULL, F::ONE_THIRD, F::TWO_THIRDS),
        (F::ZERO, F::HALF, F::TWO_THIRDS, F::FULL),
        (F::HALF, F::FULL, F::TWO_THIRDS, F::FULL),
    ];
    for (i, &(x0, x1, y0, y1)) in cells.iter().enumerate() {
        if bits & (1 << i) != 0 {
            fill(m, canvas, x0, x1, y0, y1);
        }
    }
}

/// Smooth mosaics as 3 by 4 pictures, rows separated by spaces; only `#`
/// counts, `/` and `\` just show where the diagonal runs.
const MOSAICS: [&str; 44] = [
    r"... ... #.. ##.",
    r"... ... #\. ###",
    r"... #.. #\. ##.",
    r"... #.. ##. ###",
    r"#.. #.. ##. ##.",
    r"/## ### ### ###",
    r"./# ### ### ###",
    r".## .## ### ###",
    r"..# .## ### ###",
    r".## .## .## ###",
    r"... ./# ### ###",
    r"... ... ..# .##",
    r"... ... ./# ###",
    r"... ..# ./# .##",
    r"... ..# .## ###",
    r"..# ..# .## .##",
    r"##\ ### ### ###",
    r"#\. ### ### ###",
    r"##. ##. ### ###",
    r"#.. ##. ### ###",
    r"##. ##. ##. ###",
    r"... #\. ### ###",
    r"### ### ### \##",
    r"### ### ### .\#",
    r"### ### .## .##",
    r"### ### .## ..#",
    r"### .## .## .##",
    r"##. #.. ... ...",
    r"### #/. ... ...",
    r"##. #/. #.. ...",
    r"### ##. #.. ...",
    r"##. ##. #.. #..",
    r"### ### #/. ...",
    r"### ### ### ##/",
    r"### ### ### #/.",
    r"### ### ##. ##.",
    r"### ### ##. #..",
    r"### ##. ##. ##.",
    r".## ..# ... ...",
    r"### .\# ... ...",
    r".## .\# ..# ...",
    r"### .## ..# ...",
    r".## .## ..# ..#",
    r"### ### .\# ...",
];

/// Smooth mosaics: the polygon through the picture's filled points on the
/// cell's outline (corners, thirds down the sides, centers of top and
/// bottom), skipping points in the middle of a straight run.
pub(super) fn draw_1fb3c_1fb67(cp: u32, canvas: &mut Canvas, _w: u32, _h: u32, m: &CellMetrics) {
    let p = MOSAICS[(cp - 0x1fb3c) as usize].as_bytes();
    let on = |i: usize| p[i] == b'#';
    // Outline points counterclockwise from the top left. A side's middle
    // point is skipped when both its neighbors are on.
    let (top, upper, lower) = (
        0.0,
        Fraction::ONE_THIRD.float(m.cell_height),
        Fraction::TWO_THIRDS.float(m.cell_height),
    );
    let bottom = f64::from(m.cell_height);
    let (left, center, right) = (
        0.0,
        Fraction::HALF.float(m.cell_width),
        f64::from(m.cell_width),
    );
    let points = [
        (on(0), left, top),
        (on(4) && !(on(0) && on(8)), left, upper),
        (on(8) && !(on(4) && on(12)), left, lower),
        (on(12), left, bottom),
        (on(13) && !(on(12) && on(14)), center, bottom),
        (on(14), right, bottom),
        (on(10) && !(on(14) && on(6)), right, lower),
        (on(6) && !(on(10) && on(2)), right, upper),
        (on(2), right, top),
        (on(1) && !(on(2) && on(0)), center, top),
    ];
    let mut pb = PathBuilder::new();
    for &(_, x, y) in points.iter().filter(|p| p.0) {
        // z2d treats a leading line_to as a move_to; tiny-skia would
        // start from the origin instead.
        if pb.is_empty() {
            pb.move_to(x as f32, y as f32);
        } else {
            pb.line_to(x as f32, y as f32);
        }
    }
    pb.close();
    if let Some(path) = pb.finish() {
        canvas.fill_path(&path, FillRule::Winding, ON);
    }
}

pub(super) fn draw_1fb68_1fb6f(cp: u32, canvas: &mut Canvas, _w: u32, _h: u32, m: &CellMetrics) {
    let edge = [Edge::Left, Edge::Top, Edge::Right, Edge::Bottom][(cp & 3) as usize];
    edge_triangle(m, canvas, edge);
    if cp < 0x1fb6c {
        canvas.invert();
        clip_to_cell(canvas);
    }
}

/// Vertical one eighth blocks.
pub(super) fn draw_1fb70_1fb75(cp: u32, canvas: &mut Canvas, _w: u32, _h: u32, m: &CellMetrics) {
    let n = (cp + 1 - 0x1fb70) as usize;
    fill(
        m,
        canvas,
        Fraction::EIGHTHS[n],
        Fraction::EIGHTHS[n + 1],
        Fraction::ZERO,
        Fraction::FULL,
    );
}

/// Horizontal one eighth blocks.
pub(super) fn draw_1fb76_1fb7b(cp: u32, canvas: &mut Canvas, _w: u32, _h: u32, m: &CellMetrics) {
    horizontal_eighth(m, canvas, (cp + 1 - 0x1fb76) as usize);
}

/// The `n`th of the eight rows of the cell.
fn horizontal_eighth(m: &CellMetrics, canvas: &mut Canvas, n: usize) {
    fill(
        m,
        canvas,
        Fraction::ZERO,
        Fraction::FULL,
        Fraction::EIGHTHS[n],
        Fraction::EIGHTHS[n + 1],
    );
}

pub(super) fn draw_1fb7c_1fb97(cp: u32, canvas: &mut Canvas, w: u32, h: u32, m: &CellMetrics) {
    use Alignment as A;
    match cp {
        0x1fb7c..=0x1fb80 => {
            let (a, b) = match cp {
                0x1fb7c => (A::LEFT, A::LOWER),
                0x1fb7d => (A::LEFT, A::UPPER),
                0x1fb7e => (A::RIGHT, A::UPPER),
                0x1fb7f => (A::RIGHT, A::LOWER),
                _ => (A::UPPER, A::LOWER),
            };
            for al in [a, b] {
                if al.horizontal == Horizontal::Center {
                    block(m, canvas, al, 1.0, ONE_EIGHTH);
                } else {
                    block(m, canvas, al, ONE_EIGHTH, 1.0);
                }
            }
        }
        0x1fb81 => {
            for n in [0, 2, 4, 7] {
                horizontal_eighth(m, canvas, n);
            }
        }
        0x1fb82..=0x1fb86 => {
            let f = [
                ONE_QUARTER,
                THREE_EIGHTHS,
                FIVE_EIGHTHS,
                THREE_QUARTERS,
                SEVEN_EIGHTHS,
            ];
            block(m, canvas, A::UPPER, 1.0, f[(cp - 0x1fb82) as usize]);
        }
        0x1fb87..=0x1fb8b => {
            let f = [
                ONE_QUARTER,
                THREE_EIGHTHS,
                FIVE_EIGHTHS,
                THREE_QUARTERS,
                SEVEN_EIGHTHS,
            ];
            block(m, canvas, A::RIGHT, f[(cp - 0x1fb87) as usize], 1.0);
        }
        0x1fb8c => block_shade(m, canvas, A::LEFT, HALF, 1.0, Shade::Medium),
        0x1fb8d => block_shade(m, canvas, A::RIGHT, HALF, 1.0, Shade::Medium),
        0x1fb8e => block_shade(m, canvas, A::UPPER, 1.0, HALF, Shade::Medium),
        0x1fb8f => block_shade(m, canvas, A::LOWER, 1.0, HALF, Shade::Medium),
        0x1fb90 => full_block_shade(m, canvas, Shade::Medium),
        0x1fb91 => {
            full_block_shade(m, canvas, Shade::Medium);
            block(m, canvas, A::UPPER, 1.0, HALF);
        }
        0x1fb92 => {
            full_block_shade(m, canvas, Shade::Medium);
            block(m, canvas, A::LOWER, 1.0, HALF);
        }
        // U+1FB93 is unassigned; it stays empty.
        0x1fb94 => {
            full_block_shade(m, canvas, Shade::Medium);
            block(m, canvas, A::RIGHT, HALF, 1.0);
        }
        0x1fb95 => checkerboard(m, canvas, 0),
        0x1fb96 => checkerboard(m, canvas, 1),
        0x1fb97 => {
            let (w, h) = (w as i32, h as i32);
            canvas.box_(0, h / 4, w, 2 * h / 4, ON);
            canvas.box_(0, 3 * h / 4, w, h, ON);
        }
        _ => {}
    }
}

/// 🮘, diagonal stripes from upper left to lower right.
pub(super) fn draw_1fb98(_cp: u32, canvas: &mut Canvas, _w: u32, _h: u32, m: &CellMetrics) {
    diagonal_fill(m, canvas, false);
}

/// 🮙, diagonal stripes from upper right to lower left.
pub(super) fn draw_1fb99(_cp: u32, canvas: &mut Canvas, _w: u32, _h: u32, m: &CellMetrics) {
    diagonal_fill(m, canvas, true);
}

/// Parallel light diagonals spaced two line widths apart, clipped to the
/// cell. Like Ghostty, they don't line up with the neighboring cells for
/// most sizes.
fn diagonal_fill(m: &CellMetrics, canvas: &mut Canvas, up: bool) {
    let thick = Thickness::Light.height(m.box_thickness);
    let line_count = m.cell_width / (2 * thick);
    let (w, h) = (f64::from(m.cell_width), f64::from(m.cell_height));
    let stride = (w / f64::from(line_count)).round();
    for i in 0..=2 * line_count as i32 {
        let x = f64::from(i - line_count as i32) * stride;
        let (top, bottom) = if up { (w + x, x) } else { (x, w + x) };
        canvas.line((top, 0.0), (bottom, h), f64::from(thick), ON);
    }
    clip_to_cell(canvas);
}

pub(super) fn draw_1fb9a_1fb9f(cp: u32, canvas: &mut Canvas, _w: u32, _h: u32, m: &CellMetrics) {
    match cp {
        0x1fb9a => {
            edge_triangle(m, canvas, Edge::Top);
            edge_triangle(m, canvas, Edge::Bottom);
        }
        0x1fb9b => {
            edge_triangle(m, canvas, Edge::Left);
            edge_triangle(m, canvas, Edge::Right);
        }
        _ => {
            let corner = [Corner::Tl, Corner::Tr, Corner::Br, Corner::Bl][(cp - 0x1fb9c) as usize];
            corner_triangle_shade(m, canvas, corner, Shade::Medium);
        }
    }
}

/// Lines between the midpoints of the cell's sides, per corner, for
/// U+1FBA0..=U+1FBAE as tl, tr, bl, br.
const CORNER_LINES: [&str; 15] = [
    "#...", ".#..", "..#.", "...#", "#.#.", ".#.#", "..##", "##..", "#..#", ".##.", ".###", "#.##",
    "##.#", "###.", "####",
];

pub(super) fn draw_1fba0_1fbae(cp: u32, canvas: &mut Canvas, _w: u32, _h: u32, m: &CellMetrics) {
    let b = CORNER_LINES[(cp - 0x1fba0) as usize].as_bytes();
    let quads = Quads {
        tl: b[0] == b'#',
        tr: b[1] == b'#',
        bl: b[2] == b'#',
        br: b[3] == b'#',
    };
    corner_diagonal_lines(m, canvas, quads);
}

/// 🮯
pub(super) fn draw_1fbaf(_cp: u32, canvas: &mut Canvas, _w: u32, _h: u32, m: &CellMetrics) {
    let lines = Lines {
        up: Style::Heavy,
        down: Style::Heavy,
        left: Style::Light,
        right: Style::Light,
    };
    lines_char(m, canvas, lines);
}

/// 🮽, the inverse of ╳.
pub(super) fn draw_1fbbd(_cp: u32, canvas: &mut Canvas, _w: u32, _h: u32, m: &CellMetrics) {
    diagonal_up(m, canvas);
    diagonal_down(m, canvas);
    canvas.invert();
    clip_to_cell(canvas);
}

/// 🮾
pub(super) fn draw_1fbbe(_cp: u32, canvas: &mut Canvas, _w: u32, _h: u32, m: &CellMetrics) {
    let quads = Quads {
        br: true,
        ..Quads::default()
    };
    corner_diagonal_lines(m, canvas, quads);
    canvas.invert();
    clip_to_cell(canvas);
}

/// 🮿
pub(super) fn draw_1fbbf(_cp: u32, canvas: &mut Canvas, _w: u32, _h: u32, m: &CellMetrics) {
    let quads = Quads {
        tl: true,
        tr: true,
        bl: true,
        br: true,
    };
    corner_diagonal_lines(m, canvas, quads);
    canvas.invert();
    clip_to_cell(canvas);
}

/// 🯎
pub(super) fn draw_1fbce(_cp: u32, canvas: &mut Canvas, _w: u32, _h: u32, m: &CellMetrics) {
    block(m, canvas, Alignment::LEFT, TWO_THIRDS, 1.0);
}

/// 🯏
pub(super) fn draw_1fbcf(_cp: u32, canvas: &mut Canvas, _w: u32, _h: u32, m: &CellMetrics) {
    block(m, canvas, Alignment::LEFT, ONE_THIRD, 1.0);
}

/// Cell diagonals: each glyph is a path through these points.
pub(super) fn draw_1fbd0_1fbdf(cp: u32, canvas: &mut Canvas, _w: u32, _h: u32, m: &CellMetrics) {
    use Alignment as A;
    let path: &[Alignment] = match cp {
        0x1fbd0 => &[A::RIGHT, A::LOWER_LEFT],
        0x1fbd1 => &[A::UPPER_RIGHT, A::LEFT],
        0x1fbd2 => &[A::UPPER_LEFT, A::RIGHT],
        0x1fbd3 => &[A::LEFT, A::LOWER_RIGHT],
        0x1fbd4 => &[A::UPPER_LEFT, A::LOWER],
        0x1fbd5 => &[A::UPPER, A::LOWER_RIGHT],
        0x1fbd6 => &[A::UPPER_RIGHT, A::LOWER],
        0x1fbd7 => &[A::UPPER, A::LOWER_LEFT],
        0x1fbd8 => &[A::UPPER_LEFT, A::CENTER, A::UPPER_RIGHT],
        0x1fbd9 => &[A::UPPER_RIGHT, A::CENTER, A::LOWER_RIGHT],
        0x1fbda => &[A::LOWER_LEFT, A::CENTER, A::LOWER_RIGHT],
        0x1fbdb => &[A::UPPER_LEFT, A::CENTER, A::LOWER_LEFT],
        0x1fbdc => &[A::UPPER_LEFT, A::LOWER, A::UPPER_RIGHT],
        0x1fbdd => &[A::UPPER_RIGHT, A::LEFT, A::LOWER_RIGHT],
        0x1fbde => &[A::LOWER_LEFT, A::UPPER, A::LOWER_RIGHT],
        _ => &[A::UPPER_LEFT, A::RIGHT, A::LOWER_LEFT],
    };
    let thick = f64::from(Thickness::Light.height(m.box_thickness));
    // Separate strokes, as Ghostty draws them, so the joint gets no miter.
    for pair in path.windows(2) {
        canvas.line(anchor(m, pair[0]), anchor(m, pair[1]), thick, ON);
    }
}

pub(super) fn draw_1fbe0_1fbef(cp: u32, canvas: &mut Canvas, _w: u32, _h: u32, m: &CellMetrics) {
    use Alignment as A;
    match cp {
        0x1fbe4 => block(m, canvas, A::UPPER, HALF, HALF),
        0x1fbe5 => block(m, canvas, A::LOWER, HALF, HALF),
        0x1fbe6 => block(m, canvas, A::LEFT, HALF, HALF),
        0x1fbe7 => block(m, canvas, A::RIGHT, HALF, HALF),
        _ => {
            let position = [
                A::UPPER,
                A::RIGHT,
                A::LOWER,
                A::LEFT,
                A::UPPER_RIGHT,
                A::LOWER_LEFT,
                A::LOWER_RIGHT,
                A::UPPER_LEFT,
            ];
            // U+1FBE0..=U+1FBE3 outlined, U+1FBE8..=U+1FBEF filled.
            let i = (cp - 0x1fbe0) as usize;
            if i < 4 {
                circle(m, canvas, position[i], false);
            } else {
                circle(m, canvas, position[i - 8], true);
            }
        }
    }
}

/// Zeroes everything outside the cell, as Ghostty's sprites that set
/// their clip margins to the padding come out.
pub(super) fn clip_to_cell(canvas: &mut Canvas) {
    let (w, h) = (canvas.width as i32, canvas.height as i32);
    // The margin is a quarter of the cell, so this reaches past it.
    let b = w.max(h);
    let bands = [
        (-b, -b, w + 2 * b, b),
        (-b, h, w + 2 * b, b),
        (-b, 0, b, h),
        (w, 0, b, h),
    ];
    for (x, y, width, height) in bands {
        canvas.rect(
            Rect {
                x,
                y,
                width,
                height,
            },
            0,
        );
    }
}

/// A triangle from the cell's center (rounded to a pixel) to `edge`.
fn edge_triangle(m: &CellMetrics, canvas: &mut Canvas, edge: Edge) {
    let middle = (f64::from(m.cell_height) / 2.0).round();
    let center = (f64::from(m.cell_width) / 2.0).round();
    let (right, lower) = (f64::from(m.cell_width), f64::from(m.cell_height));
    let (p0, p1) = match edge {
        Edge::Top => ((right, 0.0), (0.0, 0.0)),
        Edge::Left => ((0.0, 0.0), (0.0, lower)),
        Edge::Bottom => ((0.0, lower), (right, lower)),
        Edge::Right => ((right, lower), (right, 0.0)),
    };
    canvas.triangle((center, middle), p0, p1, ON);
}

/// Light lines cutting off the chosen corners between the midpoints of
/// the sides (rounded up for odd sizes).
fn corner_diagonal_lines(m: &CellMetrics, canvas: &mut Canvas, corners: Quads) {
    let thick = f64::from(Thickness::Light.height(m.box_thickness));
    let (w, h) = (f64::from(m.cell_width), f64::from(m.cell_height));
    let cx = f64::from(m.cell_width / 2 + m.cell_width % 2);
    let cy = f64::from(m.cell_height / 2 + m.cell_height % 2);
    let lines = [
        (corners.tl, (cx, 0.0), (0.0, cy)),
        (corners.tr, (cx, 0.0), (w, cy)),
        (corners.bl, (cx, h), (0.0, cy)),
        (corners.br, (cx, h), (w, cy)),
    ];
    for (on, p0, p1) in lines {
        if on {
            canvas.line(p0, p1, thick, ON);
        }
    }
}

/// The point of the cell an alignment names: a corner, the middle of a
/// side, or the center.
fn anchor(m: &CellMetrics, a: Alignment) -> (f64, f64) {
    let (w, h) = (f64::from(m.cell_width), f64::from(m.cell_height));
    let x = match a.horizontal {
        Horizontal::Left => 0.0,
        Horizontal::Right => w,
        Horizontal::Center => w / 2.0,
    };
    let y = match a.vertical {
        Vertical::Top => 0.0,
        Vertical::Bottom => h,
        Vertical::Middle => h / 2.0,
    };
    (x, y)
}

/// A four by four checkerboard (rows scaled to keep the squares square),
/// with the top left square filled when `parity` is 0.
fn checkerboard(m: &CellMetrics, canvas: &mut Canvas, parity: u32) {
    let (w, h) = (m.cell_width, m.cell_height);
    let x_size = 4;
    let y_size = (4.0 * (f64::from(h) / f64::from(w))).round() as u32;
    for x in 0..x_size {
        let (x0, x1) = (w * x / x_size, w * (x + 1) / x_size);
        for y in 0..y_size {
            let (y0, y1) = (h * y / y_size, h * (y + 1) / y_size);
            if (x + y) % 2 == parity {
                let r = Rect {
                    x: x0 as i32,
                    y: y0 as i32,
                    width: x1.saturating_sub(x0) as i32,
                    height: y1.saturating_sub(y0) as i32,
                };
                canvas.rect(r, ON);
            }
        }
    }
}

/// A circle of half the smaller cell side centered on the anchor
/// `position`, filled or outlined inside that radius, clipped to the cell.
pub(super) fn circle(m: &CellMetrics, canvas: &mut Canvas, position: Alignment, filled: bool) {
    let (x, y) = anchor(m, position);
    let r = 0.5 * f64::from(m.cell_width).min(f64::from(m.cell_height));
    let thick = f64::from(Thickness::Light.height(m.box_thickness));
    let mut pb = PathBuilder::new();
    let tau = std::f64::consts::TAU;
    if filled {
        arc(&mut pb, x, y, r, 0.0, tau);
    } else {
        arc(&mut pb, x, y, r - thick / 2.0, 0.0, tau);
    }
    pb.close();
    if let Some(path) = pb.finish() {
        if filled {
            canvas.fill_path(&path, FillRule::Winding, ON);
        } else {
            canvas.stroke_path(&path, StrokeStyle::new(thick), ON);
        }
    }
    clip_to_cell(canvas);
}
