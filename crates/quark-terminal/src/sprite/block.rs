//! Block Elements, U+2580 to U+259F (Ghostty's `draw/block.zig`).
//!
//! ▀▁▂▃▄▅▆▇█▉▊▋▌▍▎▏
//! ▐░▒▓▔▕▖▗▘▙▚▛▜▝▞▟

use super::canvas::{Canvas, Rect};
use super::common::{
    Alignment, FIVE_EIGHTHS, Fraction, HALF, Horizontal, ONE_EIGHTH, ONE_QUARTER, Quads,
    SEVEN_EIGHTHS, Shade, THREE_EIGHTHS, THREE_QUARTERS, Vertical, fill,
};
use crate::metrics::CellMetrics;

pub(super) fn draw_2580_259f(cp: u32, canvas: &mut Canvas, _w: u32, _h: u32, m: &CellMetrics) {
    use Alignment as A;
    let quads = |tl, tr, bl, br| Quads { tl, tr, bl, br };
    match cp {
        0x2580 => block(m, canvas, A::UPPER, 1.0, HALF),
        0x2581 => block(m, canvas, A::LOWER, 1.0, ONE_EIGHTH),
        0x2582 => block(m, canvas, A::LOWER, 1.0, ONE_QUARTER),
        0x2583 => block(m, canvas, A::LOWER, 1.0, THREE_EIGHTHS),
        0x2584 => block(m, canvas, A::LOWER, 1.0, HALF),
        0x2585 => block(m, canvas, A::LOWER, 1.0, FIVE_EIGHTHS),
        0x2586 => block(m, canvas, A::LOWER, 1.0, THREE_QUARTERS),
        0x2587 => block(m, canvas, A::LOWER, 1.0, SEVEN_EIGHTHS),
        0x2588 => full_block_shade(m, canvas, Shade::On),
        0x2589 => block(m, canvas, A::LEFT, SEVEN_EIGHTHS, 1.0),
        0x258a => block(m, canvas, A::LEFT, THREE_QUARTERS, 1.0),
        0x258b => block(m, canvas, A::LEFT, FIVE_EIGHTHS, 1.0),
        0x258c => block(m, canvas, A::LEFT, HALF, 1.0),
        0x258d => block(m, canvas, A::LEFT, THREE_EIGHTHS, 1.0),
        0x258e => block(m, canvas, A::LEFT, ONE_QUARTER, 1.0),
        0x258f => block(m, canvas, A::LEFT, ONE_EIGHTH, 1.0),
        0x2590 => block(m, canvas, A::RIGHT, HALF, 1.0),
        0x2591 => full_block_shade(m, canvas, Shade::Light),
        0x2592 => full_block_shade(m, canvas, Shade::Medium),
        0x2593 => full_block_shade(m, canvas, Shade::Dark),
        0x2594 => block(m, canvas, A::UPPER, 1.0, ONE_EIGHTH),
        0x2595 => block(m, canvas, A::RIGHT, ONE_EIGHTH, 1.0),
        0x2596 => quadrant(m, canvas, quads(false, false, true, false)),
        0x2597 => quadrant(m, canvas, quads(false, false, false, true)),
        0x2598 => quadrant(m, canvas, quads(true, false, false, false)),
        0x2599 => quadrant(m, canvas, quads(true, false, true, true)),
        0x259a => quadrant(m, canvas, quads(true, false, false, true)),
        0x259b => quadrant(m, canvas, quads(true, true, true, false)),
        0x259c => quadrant(m, canvas, quads(true, true, false, true)),
        0x259d => quadrant(m, canvas, quads(false, true, false, false)),
        0x259e => quadrant(m, canvas, quads(false, true, true, false)),
        0x259f => quadrant(m, canvas, quads(false, true, true, true)),
        _ => {}
    }
}

pub(super) fn block(
    m: &CellMetrics,
    canvas: &mut Canvas,
    alignment: Alignment,
    width: f64,
    height: f64,
) {
    block_shade(m, canvas, alignment, width, height, Shade::On);
}

/// A `width` by `height` (fractions of the cell) block placed by
/// `alignment`.
pub(super) fn block_shade(
    m: &CellMetrics,
    canvas: &mut Canvas,
    alignment: Alignment,
    width: f64,
    height: f64,
    shade: Shade,
) {
    let w = (f64::from(m.cell_width) * width).round() as u32;
    let h = (f64::from(m.cell_height) * height).round() as u32;
    let x = match alignment.horizontal {
        Horizontal::Left => 0,
        Horizontal::Right => m.cell_width - w,
        Horizontal::Center => (m.cell_width - w) / 2,
    };
    let y = match alignment.vertical {
        Vertical::Top => 0,
        Vertical::Bottom => m.cell_height - h,
        Vertical::Middle => (m.cell_height - h) / 2,
    };
    canvas.rect(
        Rect {
            x: x as i32,
            y: y as i32,
            width: w as i32,
            height: h as i32,
        },
        shade as u8,
    );
}

/// The whole cell at `shade`. Shades are uniform coverage, as Ghostty
/// draws them, rather than a dither pattern.
pub(super) fn full_block_shade(m: &CellMetrics, canvas: &mut Canvas, shade: Shade) {
    canvas.box_(0, 0, m.cell_width as i32, m.cell_height as i32, shade as u8);
}

fn quadrant(m: &CellMetrics, canvas: &mut Canvas, q: Quads) {
    use Fraction as F;
    if q.tl {
        fill(m, canvas, F::ZERO, F::HALF, F::ZERO, F::HALF);
    }
    if q.tr {
        fill(m, canvas, F::HALF, F::FULL, F::ZERO, F::HALF);
    }
    if q.bl {
        fill(m, canvas, F::ZERO, F::HALF, F::HALF, F::FULL);
    }
    if q.br {
        fill(m, canvas, F::HALF, F::FULL, F::HALF, F::FULL);
    }
}
