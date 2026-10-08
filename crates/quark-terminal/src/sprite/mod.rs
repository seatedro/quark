//! Cells drawn procedurally rather than from a font, as Ghostty's sprite
//! face draws them (`src/font/sprite`): box drawing, block elements,
//! braille, powerline separators, branch drawing, a few geometric shapes,
//! and the Symbols for Legacy Computing blocks with their supplement
//! (sextants, octants, wedges).
//!
//! Font glyphs for these rarely fill their cell exactly, so a row of `─`
//! or a picture made of `█` shows seams; a sprite is drawn for the cell
//! size in device pixels, so neighbors meet edge to edge. Each sprite is
//! rasterized once per codepoint and cell metrics into a coverage mask
//! (see [`canvas`]), which the view paints as a few rectangles of uniform
//! coverage in the cell's foreground color, every edge on a device pixel.

mod block;
mod box_drawing;
mod braille;
mod branch;
pub(crate) mod canvas;
mod common;
mod geometric_shapes;
mod legacy_computing;
mod legacy_computing_supplement;
mod powerline;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use canvas::Canvas;
pub(crate) use canvas::SpriteRect;

use crate::metrics::CellMetrics;

/// Draws codepoint `cp` on `canvas`, which is `width` by `height` pixels
/// (one cell, or two for a wide character).
type DrawFn = fn(cp: u32, canvas: &mut Canvas, width: u32, height: u32, m: &CellMetrics);

/// The codepoints each drawing covers, inclusive and sorted.
const RANGES: &[(u32, u32, DrawFn)] = &[
    (0x2500, 0x257f, box_drawing::draw_2500_257f),
    (0x2580, 0x259f, block::draw_2580_259f),
    (0x25e2, 0x25e5, geometric_shapes::draw_25e2_25e5),
    (0x25f8, 0x25fa, geometric_shapes::draw_25f8_25fa),
    (0x25ff, 0x25ff, geometric_shapes::draw_25ff),
    (0x2800, 0x28ff, braille::draw_2800_28ff),
    (0xe0b0, 0xe0b0, powerline::draw_e0b0),
    (0xe0b1, 0xe0b1, powerline::draw_e0b1),
    (0xe0b2, 0xe0b2, powerline::draw_e0b2),
    (0xe0b3, 0xe0b3, powerline::draw_e0b3),
    (0xe0b4, 0xe0b4, powerline::draw_e0b4),
    (0xe0b5, 0xe0b5, powerline::draw_e0b5),
    (0xe0b6, 0xe0b6, powerline::draw_e0b6),
    (0xe0b7, 0xe0b7, powerline::draw_e0b7),
    (0xe0b8, 0xe0b8, powerline::draw_e0b8),
    (0xe0b9, 0xe0b9, powerline::draw_e0b9),
    (0xe0ba, 0xe0ba, powerline::draw_e0ba),
    (0xe0bb, 0xe0bb, powerline::draw_e0bb),
    (0xe0bc, 0xe0bc, powerline::draw_e0bc),
    (0xe0bd, 0xe0bd, powerline::draw_e0bd),
    (0xe0be, 0xe0be, powerline::draw_e0be),
    (0xe0bf, 0xe0bf, powerline::draw_e0bf),
    (0xe0d2, 0xe0d2, powerline::draw_e0d2),
    (0xe0d4, 0xe0d4, powerline::draw_e0d4),
    (0xf5d0, 0xf60d, branch::draw_f5d0_f60d),
    (
        0x1cc1b,
        0x1cc1e,
        legacy_computing_supplement::draw_1cc1b_1cc1e,
    ),
    (
        0x1cc21,
        0x1cc2f,
        legacy_computing_supplement::draw_1cc21_1cc2f,
    ),
    (
        0x1cc30,
        0x1cc3f,
        legacy_computing_supplement::draw_1cc30_1cc3f,
    ),
    (
        0x1cd00,
        0x1cde5,
        legacy_computing_supplement::draw_1cd00_1cde5,
    ),
    (0x1ce00, 0x1ce00, legacy_computing_supplement::draw_1ce00),
    (0x1ce01, 0x1ce01, legacy_computing_supplement::draw_1ce01),
    (0x1ce0b, 0x1ce0b, legacy_computing_supplement::draw_1ce0b),
    (0x1ce0c, 0x1ce0c, legacy_computing_supplement::draw_1ce0c),
    (
        0x1ce16,
        0x1ce19,
        legacy_computing_supplement::draw_1ce16_1ce19,
    ),
    (
        0x1ce51,
        0x1ce8f,
        legacy_computing_supplement::draw_1ce51_1ce8f,
    ),
    (
        0x1ce90,
        0x1ceaf,
        legacy_computing_supplement::draw_1ce90_1ceaf,
    ),
    (0x1fb00, 0x1fb3b, legacy_computing::draw_1fb00_1fb3b),
    (0x1fb3c, 0x1fb67, legacy_computing::draw_1fb3c_1fb67),
    (0x1fb68, 0x1fb6f, legacy_computing::draw_1fb68_1fb6f),
    (0x1fb70, 0x1fb75, legacy_computing::draw_1fb70_1fb75),
    (0x1fb76, 0x1fb7b, legacy_computing::draw_1fb76_1fb7b),
    (0x1fb7c, 0x1fb97, legacy_computing::draw_1fb7c_1fb97),
    (0x1fb98, 0x1fb98, legacy_computing::draw_1fb98),
    (0x1fb99, 0x1fb99, legacy_computing::draw_1fb99),
    (0x1fb9a, 0x1fb9f, legacy_computing::draw_1fb9a_1fb9f),
    (0x1fba0, 0x1fbae, legacy_computing::draw_1fba0_1fbae),
    (0x1fbaf, 0x1fbaf, legacy_computing::draw_1fbaf),
    (0x1fbbd, 0x1fbbd, legacy_computing::draw_1fbbd),
    (0x1fbbe, 0x1fbbe, legacy_computing::draw_1fbbe),
    (0x1fbbf, 0x1fbbf, legacy_computing::draw_1fbbf),
    (0x1fbce, 0x1fbce, legacy_computing::draw_1fbce),
    (0x1fbcf, 0x1fbcf, legacy_computing::draw_1fbcf),
    (0x1fbd0, 0x1fbdf, legacy_computing::draw_1fbd0_1fbdf),
    (0x1fbe0, 0x1fbef, legacy_computing::draw_1fbe0_1fbef),
];

fn draw_fn(cp: u32) -> Option<DrawFn> {
    let i = RANGES.partition_point(|&(_, hi, _)| hi < cp);
    RANGES
        .get(i)
        .filter(|&&(lo, _, _)| lo <= cp)
        .map(|&(_, _, f)| f)
}

/// Whether `text`, one cell's grapheme, is drawn as a sprite: a single
/// codepoint the sprite face covers, or one followed only by a variation
/// selector (which cannot make a sprite an emoji).
pub(crate) fn sprite_char(text: &str) -> Option<u32> {
    let mut chars = text.chars();
    let c = chars.next()?;
    if chars.any(|c| !matches!(c, '\u{fe0e}' | '\u{fe0f}')) {
        return None;
    }
    let cp = u32::from(c);
    draw_fn(cp).map(|_| cp)
}

/// A sprite's coverage as rectangles in device pixels from the cell's
/// top-left corner.
pub(crate) type Sprite = Arc<[SpriteRect]>;

/// The most sprites kept. A screen of distinct sprites at one size is a
/// few hundred; the cap only matters across many size changes.
const CACHE_LIMIT: usize = 4096;

type Key = (u32, u32, CellMetrics);

static CACHE: Mutex<Option<HashMap<Key, Sprite>>> = Mutex::new(None);

fn lock_cache() -> MutexGuard<'static, Option<HashMap<Key, Sprite>>> {
    // A panic while holding the lock can only lose entries.
    CACHE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Sprite `cp` for a cell of metrics `m`, `cells` cells wide, rasterized on
/// first use and shared after. `None` for a codepoint without a sprite.
pub(crate) fn sprite(cp: u32, cells: u32, m: &CellMetrics) -> Option<Sprite> {
    let draw = draw_fn(cp)?;
    let key = (cp, cells, *m);
    if let Some(sprite) = lock_cache().as_ref().and_then(|c| c.get(&key)) {
        return Some(sprite.clone());
    }
    let sprite: Sprite = render(draw, cp, cells, m).to_rects().into();
    let mut cache = lock_cache();
    let cache = cache.get_or_insert_with(HashMap::new);
    if cache.len() >= CACHE_LIMIT {
        cache.clear();
    }
    cache.insert(key, sprite.clone());
    Some(sprite)
}

fn render(draw: DrawFn, cp: u32, cells: u32, m: &CellMetrics) -> Canvas {
    let width = m.cell_width * cells.max(1);
    let mut canvas = Canvas::new(width, m.cell_height);
    draw(cp, &mut canvas, width, m.cell_height, m);
    canvas
}

#[cfg(test)]
pub(crate) fn render_for_test(cp: u32, m: &CellMetrics) -> Option<Canvas> {
    draw_fn(cp).map(|draw| render(draw, cp, 1, m))
}

#[cfg(test)]
mod tests;
