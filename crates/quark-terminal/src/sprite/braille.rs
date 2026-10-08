//! Braille Patterns, U+2800 to U+28FF (Ghostty's `draw/braille.zig`):
//! square dots on a two by four grid, sized and spaced to fill the cell
//! evenly.

use super::canvas::{Canvas, ON};
use crate::metrics::CellMetrics;

pub(super) fn draw_2800_28ff(
    cp: u32,
    canvas: &mut Canvas,
    width: u32,
    height: u32,
    _m: &CellMetrics,
) {
    let (width, height) = (width as i32, height as i32);
    let mut w = (width / 4).min(height / 8);
    let mut x_spacing = width / 4;
    let mut y_spacing = height / 8;
    let mut x_margin = x_spacing / 2;
    let mut y_margin = y_spacing / 2;
    let mut x_left = width - 2 * x_margin - x_spacing - 2 * w;
    let mut y_left = height - 2 * y_margin - 3 * y_spacing - 4 * w;

    // Spend the leftover pixels in order: a visible dot, a margin, wider
    // spacing, wider margins, then bigger dots.
    if x_left >= 2 && y_left >= 4 && w == 0 {
        w += 1;
        x_left -= 2;
        y_left -= 4;
    }
    if x_left >= 2 && x_margin == 0 {
        x_margin = 1;
        x_left -= 2;
    }
    if y_left >= 2 && y_margin == 0 {
        y_margin = 1;
        y_left -= 2;
    }
    if x_left >= 1 {
        x_spacing += 1;
        x_left -= 1;
    }
    if y_left >= 3 {
        y_spacing += 1;
        y_left -= 3;
    }
    if x_left >= 2 {
        x_margin += 1;
        x_left -= 2;
    }
    if y_left >= 2 {
        y_margin += 1;
        y_left -= 2;
    }
    if x_left >= 2 && y_left >= 4 {
        w += 1;
    }

    let x = [x_margin, x_margin + w + x_spacing];
    let mut y = [y_margin; 4];
    for i in 1..4 {
        y[i] = y[i - 1] + w + y_spacing;
    }
    // Bit order of the codepoint: dots 1-3 down the left column, 4-6 down
    // the right, then 7 and 8 along the bottom.
    let dots = [
        (0, 0),
        (0, 1),
        (0, 2),
        (1, 0),
        (1, 1),
        (1, 2),
        (0, 3),
        (1, 3),
    ];
    for (bit, (col, row)) in dots.into_iter().enumerate() {
        if cp & (1 << bit) != 0 {
            let (x, y) = (x[col], y[row]);
            canvas.box_(x, y, x + w, y + w, ON);
        }
    }
}
