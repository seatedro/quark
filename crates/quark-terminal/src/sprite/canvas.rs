//! An alpha-only drawing surface for one sprite, in device pixels relative
//! to the cell's top-left corner, with a margin around the cell for
//! sprites that reach past it. A port of Ghostty's
//! `font/sprite/canvas.zig`, with tiny-skia standing in for z2d: rects and
//! boxes write pixels directly, paths fill and stroke antialiased and
//! composite over what is there.

use tiny_skia::{FillRule, Mask, Path, PathBuilder, Stroke, Transform};

use tiny_skia::{LineCap, LineJoin};

/// Full coverage.
pub(crate) const ON: u8 = 0xff;

/// Pixel-aligned rectangle with a width and height (Ghostty's `Rect`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// How to stroke a path.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct StrokeStyle {
    pub width: f64,
    pub cap: LineCap,
    pub join: LineJoin,
}

impl StrokeStyle {
    /// Butt caps and miter joins, z2d's defaults.
    pub fn new(width: f64) -> Self {
        Self {
            width,
            cap: LineCap::Butt,
            join: LineJoin::Miter,
        }
    }
}

pub(crate) struct Canvas {
    /// Cell size the sprite draws for.
    pub width: u32,
    pub height: u32,
    pad_x: u32,
    pad_y: u32,
    /// Coverage, row-major over the cell and its margin.
    pixels: Vec<u8>,
    /// Scratch mask paths rasterize into before compositing.
    scratch: Mask,
}

impl Canvas {
    /// A cleared canvas for a `width` by `height` cell, with a quarter of
    /// each dimension as margin on every side, as Ghostty pads sprites.
    pub fn new(width: u32, height: u32) -> Self {
        let (pad_x, pad_y) = (width / 4, height / 4);
        let (w, h) = (width + 2 * pad_x, height + 2 * pad_y);
        Self {
            width,
            height,
            pad_x,
            pad_y,
            pixels: vec![0; (w * h) as usize],
            scratch: Mask::new(w.max(1), h.max(1)).expect("non-empty sprite canvas"),
        }
    }

    fn stride(&self) -> u32 {
        self.width + 2 * self.pad_x
    }

    fn rows(&self) -> u32 {
        self.height + 2 * self.pad_y
    }

    fn index(&self, x: i32, y: i32) -> Option<usize> {
        let x = x + self.pad_x as i32;
        let y = y + self.pad_y as i32;
        let (w, h) = (self.stride() as i32, self.rows() as i32);
        ((0..w).contains(&x) && (0..h).contains(&y)).then(|| (y * w + x) as usize)
    }

    /// Sets one pixel, as Ghostty's `pixel`: no blending.
    pub fn pixel(&mut self, x: i32, y: i32, alpha: u8) {
        if let Some(i) = self.index(x, y) {
            self.pixels[i] = alpha;
        }
    }

    /// Sets every pixel of `r` (clipped to the canvas) to `alpha`.
    pub fn rect(&mut self, r: Rect, alpha: u8) {
        let x0 = (r.x + self.pad_x as i32).max(0);
        let y0 = (r.y + self.pad_y as i32).max(0);
        let x1 = (r.x + r.width + self.pad_x as i32).min(self.stride() as i32);
        let y1 = (r.y + r.height + self.pad_y as i32).min(self.rows() as i32);
        let stride = self.stride() as i32;
        for y in y0..y1 {
            let row = (y * stride) as usize;
            self.pixels[row + x0 as usize..row + x1.max(x0) as usize].fill(alpha);
        }
    }

    /// [`Self::rect`] between two corners in any order.
    pub fn box_(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, alpha: u8) {
        self.rect(
            Rect {
                x: x0.min(x1),
                y: y0.min(y1),
                width: (x1 - x0).abs(),
                height: (y1 - y0).abs(),
            },
            alpha,
        );
    }

    /// Fills a path built in cell pixels, antialiased, compositing
    /// `alpha` over the canvas.
    pub fn fill_path(&mut self, path: &Path, rule: FillRule, alpha: u8) {
        self.scratch.clear();
        let shift = Transform::from_translate(self.pad_x as f32, self.pad_y as f32);
        self.scratch.fill_path(path, rule, true, shift);
        self.composite(alpha);
    }

    /// Strokes a path built in cell pixels.
    pub fn stroke_path(&mut self, path: &Path, style: StrokeStyle, alpha: u8) {
        let stroke = Stroke {
            width: style.width as f32,
            line_cap: style.cap,
            line_join: style.join,
            ..Stroke::default()
        };
        if let Some(outline) = path.stroke(&stroke, 1.0) {
            self.fill_path(&outline, FillRule::Winding, alpha);
        }
    }

    /// Strokes the line from `p0` to `p1` with butt caps.
    pub fn line(&mut self, p0: (f64, f64), p1: (f64, f64), thickness: f64, alpha: u8) {
        let mut pb = PathBuilder::new();
        pb.move_to(p0.0 as f32, p0.1 as f32);
        pb.line_to(p1.0 as f32, p1.1 as f32);
        if let Some(path) = pb.finish() {
            self.stroke_path(&path, StrokeStyle::new(thickness), alpha);
        }
    }

    /// Fills the polygon through `points`.
    pub fn polygon(&mut self, points: &[(f64, f64)], alpha: u8) {
        let mut pb = PathBuilder::new();
        for (i, &(x, y)) in points.iter().enumerate() {
            if i == 0 {
                pb.move_to(x as f32, y as f32);
            } else {
                pb.line_to(x as f32, y as f32);
            }
        }
        pb.close();
        if let Some(path) = pb.finish() {
            self.fill_path(&path, FillRule::Winding, alpha);
        }
    }

    pub fn triangle(&mut self, p0: (f64, f64), p1: (f64, f64), p2: (f64, f64), alpha: u8) {
        self.polygon(&[p0, p1, p2], alpha);
    }

    pub fn quad(
        &mut self,
        p0: (f64, f64),
        p1: (f64, f64),
        p2: (f64, f64),
        p3: (f64, f64),
        alpha: u8,
    ) {
        self.polygon(&[p0, p1, p2, p3], alpha);
    }

    /// Source-over of the scratch coverage, scaled by `alpha`. Zero alpha
    /// clears what the path covers instead (z2d's `.off` paint).
    fn composite(&mut self, alpha: u8) {
        let a = u32::from(alpha);
        for (dst, &c) in self.pixels.iter_mut().zip(self.scratch.data()) {
            if c == 0 {
                continue;
            }
            let c = u32::from(c);
            let d = u32::from(*dst);
            *dst = if alpha == 0 {
                (d * (255 - c) / 255) as u8
            } else {
                let src = c * a / 255;
                (src + d * (255 - src) / 255) as u8
            };
        }
    }

    /// Inverts every pixel, margin included.
    pub fn invert(&mut self) {
        for p in &mut self.pixels {
            *p = 255 - *p;
        }
    }

    /// Mirrors the canvas left to right around the cell's center.
    pub fn flip_horizontal(&mut self) {
        let w = self.stride() as usize;
        for row in self.pixels.chunks_exact_mut(w) {
            row.reverse();
        }
    }

    /// The canvas as rectangles of uniform coverage, in cell pixels: runs
    /// of equal coverage along each row, merged with the identical runs
    /// of the rows below. A pixel-aligned shape becomes a handful of
    /// rectangles; an antialiased edge, one short rectangle per row.
    pub fn to_rects(&self) -> Vec<SpriteRect> {
        let (w, h) = (self.stride() as usize, self.rows() as usize);
        let mut done: Vec<SpriteRect> = Vec::new();
        // Rectangles still growing downward, as of the previous row.
        let mut open: Vec<SpriteRect> = Vec::new();
        let mut row_runs: Vec<SpriteRect> = Vec::new();
        let (px, py) = (self.pad_x as i32, self.pad_y as i32);
        for y in 0..h {
            row_runs.clear();
            let row = &self.pixels[y * w..(y + 1) * w];
            let mut x = 0;
            while x < w {
                let a = row[x];
                let start = x;
                while x < w && row[x] == a {
                    x += 1;
                }
                if a != 0 {
                    row_runs.push(SpriteRect {
                        x: start as i32 - px,
                        y: y as i32 - py,
                        width: (x - start) as u16,
                        height: 1,
                        alpha: a,
                    });
                }
            }
            let mut next = Vec::with_capacity(row_runs.len());
            for run in &row_runs {
                match open
                    .iter()
                    .position(|r| r.x == run.x && r.width == run.width && r.alpha == run.alpha)
                {
                    Some(i) => {
                        let mut r = open.swap_remove(i);
                        r.height += 1;
                        next.push(r);
                    }
                    None => next.push(*run),
                }
            }
            done.append(&mut open);
            open = next;
        }
        done.append(&mut open);
        done.sort_by_key(|r| (r.y, r.x));
        done
    }
}

/// A rectangle of uniform coverage in a sprite, in device pixels from the
/// cell's top-left corner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SpriteRect {
    pub x: i32,
    pub y: i32,
    pub width: u16,
    pub height: u16,
    pub alpha: u8,
}

#[cfg(test)]
impl Canvas {
    /// Coverage of the cell and its margin, row-major.
    pub fn pixels_for_test(&self) -> &[u8] {
        &self.pixels
    }
}
