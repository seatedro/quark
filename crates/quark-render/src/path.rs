//! Path geometry for the GPU path pipeline.
//!
//! A path draws as horizontal bands. Each band is one instanced quad whose
//! fragment shader walks the line segments that reach its rows: the
//! crossings of a ray from the pixel give the winding number (inside or
//! not), and the nearest segment gives the distance to the edge, so
//! coverage is `0.5 - signed distance`, the same analytic antialiasing the
//! quad shader uses. Strokes are first expanded into fill outlines by
//! kurbo, which gives every join and cap. kurbo already ships in every
//! build through resvg, so paths add no dependency; a tessellator (lyon)
//! would add several crates and still need a separate antialiasing pass.
//!
//! The cost is per pixel times segments in its band, so bands are short:
//! a chart line of a few hundred points stays well under a millisecond on
//! a GPU.

use kurbo::{BezPath, PathEl, Point};

use kurbo::ParamCurveArclen;

use crate::scene::{
    FillRule, LineCap, LineJoin, MAX_PATTERN_SEGMENTS, Path, PathVerb, Rect, StrokePattern,
    StrokeStyle,
};

/// Rows per band. Shorter bands give each pixel fewer segments to visit
/// and repeat more segments across bands.
const BAND_ROWS: f32 = 16.0;

/// Curve flattening tolerance in physical pixels.
const TOLERANCE: f64 = 0.2;

/// One band of a shape, in path-local physical pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Band {
    pub rect: Rect,
    /// Range of the band's segments in the frame's segment list.
    pub start: u32,
    pub count: u32,
}

/// A path scaled to physical pixels and relative to its origin, as kurbo
/// geometry.
pub(crate) fn to_kurbo(path: &Path, scale: f32) -> BezPath {
    let s = f64::from(scale);
    let p = |q: [f32; 2]| Point::new(f64::from(q[0]) * s, f64::from(q[1]) * s);
    let mut out = BezPath::new();
    for verb in path.verbs() {
        match *verb {
            PathVerb::MoveTo(to) => out.move_to(p(to)),
            PathVerb::LineTo(to) => out.line_to(p(to)),
            PathVerb::QuadTo(c, to) => out.quad_to(p(c), p(to)),
            PathVerb::CubicTo(c1, c2, to) => out.curve_to(p(c1), p(c2), p(to)),
            PathVerb::Close => out.close_path(),
        }
    }
    out
}

/// Length of the drawn part of a dot, in physical pixels: round caps make
/// it a disc as wide as the stroke, and a nonzero length gives the cap a
/// direction.
const DOT_LENGTH: f64 = 0.01;

/// The fill outline of `style` stroked along `path` (already scaled). A
/// pattern dashes the path first, restarting on every subpath; one that
/// is invalid for the width, or would cut the path into more than
/// [`MAX_PATTERN_SEGMENTS`] pieces, strokes solid instead.
pub(crate) fn stroke_outline(path: &BezPath, style: &StrokeStyle, scale: f32) -> BezPath {
    let join = match style.join {
        LineJoin::Miter => kurbo::Join::Miter,
        LineJoin::Round => kurbo::Join::Round,
        LineJoin::Bevel => kurbo::Join::Bevel,
    };
    let mut cap = match style.cap {
        LineCap::Butt => kurbo::Cap::Butt,
        LineCap::Round => kurbo::Cap::Round,
        LineCap::Square => kurbo::Cap::Square,
    };
    let s = f64::from(scale);
    let pattern = style.pattern.validated(style.width);
    let dashes = match pattern {
        StrokePattern::Solid => None,
        StrokePattern::Dashed { dash, gap, offset } => Some((
            f64::from(offset) * s,
            [f64::from(dash) * s, f64::from(gap) * s],
        )),
        StrokePattern::Dotted { spacing, offset } => {
            cap = kurbo::Cap::Round;
            let spacing = f64::from(spacing) * s;
            // The pattern starts a dot at phase zero; start it `offset`
            // before, so the first dot is centered `offset` along the path.
            let phase = (spacing - f64::from(offset) * s).rem_euclid(spacing);
            Some((phase, [DOT_LENGTH, spacing - DOT_LENGTH]))
        }
    };
    let mut stroke = kurbo::Stroke::new(f64::from(style.width * scale))
        .with_join(join)
        .with_caps(cap)
        .with_miter_limit(f64::from(style.miter_limit.max(1.0)));
    if let Some((offset, pattern)) = dashes {
        let period = pattern[0] + pattern[1];
        let length: f64 = path.segments().map(|seg| seg.arclen(TOLERANCE)).sum();
        if length / period <= MAX_PATTERN_SEGMENTS as f64 {
            stroke = stroke.with_dashes(offset, pattern);
        }
    }
    kurbo::stroke(path, &stroke, &kurbo::StrokeOpts::default(), TOLERANCE)
}

/// Flatten `path` into closed polygons and append its bands to `bands` and
/// their segments (`[x0, y0, x1, y1]`) to `segments`. Every subpath is
/// closed, as a fill closes it. Returns false when nothing would draw.
pub(crate) fn push_bands(
    path: &BezPath,
    segments: &mut Vec<[f32; 4]>,
    bands: &mut Vec<Band>,
    scratch: &mut Vec<[f32; 4]>,
) -> bool {
    scratch.clear();
    let mut start: Option<Point> = None;
    let mut last = Point::ZERO;
    let close = |scratch: &mut Vec<[f32; 4]>, from: Point, to: Point| {
        if from != to {
            scratch.push([from.x as f32, from.y as f32, to.x as f32, to.y as f32]);
        }
    };
    kurbo::flatten(path, TOLERANCE, |el| match el {
        PathEl::MoveTo(p) => {
            if let Some(s) = start {
                close(scratch, last, s);
            }
            start = Some(p);
            last = p;
        }
        PathEl::LineTo(p) => {
            close(scratch, last, p);
            last = p;
        }
        PathEl::ClosePath => {
            if let Some(s) = start {
                close(scratch, last, s);
                last = s;
            }
        }
        // flatten emits only lines.
        PathEl::QuadTo(..) | PathEl::CurveTo(..) => {}
    });
    if let Some(s) = start {
        close(scratch, last, s);
    }
    let finite = |s: &[f32; 4]| s.iter().all(|v| v.is_finite());
    scratch.retain(finite);
    if scratch.is_empty() {
        return false;
    }

    let (top, bottom) = scratch.iter().fold((f32::MAX, f32::MIN), |(lo, hi), s| {
        (lo.min(s[1]).min(s[3]), hi.max(s[1]).max(s[3]))
    });
    // Rows whose centers lie within half a pixel of the shape are the ones
    // with coverage; they start at row floor(top).
    let mut y0 = top.floor();
    let first = bands.len();
    while y0 < bottom {
        let y1 = (y0 + BAND_ROWS).min(bottom.ceil() + 1.0);
        let begin = segments.len();
        let (mut x0, mut x1) = (f32::MAX, f32::MIN);
        // A segment matters to a row when it crosses it (winding) or comes
        // within a pixel of it (edge distance).
        for s in scratch.iter() {
            if s[1].min(s[3]) <= y1 + 1.0 && s[1].max(s[3]) >= y0 - 1.0 {
                segments.push(*s);
                x0 = x0.min(s[0]).min(s[2]);
                x1 = x1.max(s[0]).max(s[2]);
            }
        }
        let count = segments.len() - begin;
        if count > 0 {
            // Pixels left or right of every segment are outside the shape;
            // a pixel of margin keeps the antialiased edge.
            bands.push(Band {
                rect: Rect {
                    x: x0.floor() - 1.0,
                    y: y0,
                    width: x1.ceil() - x0.floor() + 2.0,
                    height: y1 - y0,
                },
                start: begin as u32,
                count: count as u32,
            });
        }
        y0 = y1;
    }
    bands.len() > first
}

/// The rule the shader applies, as the instance encodes it.
pub(crate) fn rule_code(rule: FillRule) -> f32 {
    match rule {
        FillRule::NonZero => 0.0,
        FillRule::EvenOdd => 1.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Coverage at `(x, y)` computed the way the shader does, from the
    /// segments of the band holding that row.
    fn coverage(bands: &[Band], segments: &[[f32; 4]], x: f32, y: f32, even_odd: bool) -> f32 {
        let Some(band) = bands
            .iter()
            .find(|b| y >= b.rect.y && y < b.rect.bottom() && x >= b.rect.x && x < b.rect.right())
        else {
            return 0.0;
        };
        let mut winding = 0;
        let mut dist = f32::MAX;
        for s in &segments[band.start as usize..(band.start + band.count) as usize] {
            let (ax, ay, bx, by) = (s[0], s[1], s[2], s[3]);
            let (px, py) = (x - ax, y - ay);
            let (dx, dy) = (bx - ax, by - ay);
            let h = ((px * dx + py * dy) / (dx * dx + dy * dy).max(1e-12)).clamp(0.0, 1.0);
            dist = dist.min(((px - dx * h).powi(2) + (py - dy * h).powi(2)).sqrt());
            if (ay <= y) != (by <= y) {
                let cx = ax + (y - ay) * dx / dy;
                if cx > x {
                    winding += if by > ay { 1 } else { -1 };
                }
            }
        }
        let inside = if even_odd {
            winding % 2 != 0
        } else {
            winding != 0
        };
        let signed = if inside { -dist } else { dist };
        (0.5 - signed).clamp(0.0, 1.0)
    }

    fn bands_of(path: &BezPath) -> (Vec<Band>, Vec<[f32; 4]>) {
        let (mut segments, mut bands) = (Vec::new(), Vec::new());
        push_bands(path, &mut segments, &mut bands, &mut Vec::new());
        (bands, segments)
    }

    // Two nested squares drawn the same direction: nonzero fills the hole,
    // even-odd leaves it empty. Across band boundaries (y 16, 32) too.
    #[test]
    fn band_segments_reproduce_nonzero_and_even_odd_fills() {
        let mut builder = Path::builder();
        for (x, y, size) in [(0.0, 0.0, 48.0), (12.0, 12.0, 24.0)] {
            builder
                .move_to(x, y)
                .line_to(x + size, y)
                .line_to(x + size, y + size)
                .line_to(x, y + size)
                .close();
        }
        let (bands, segments) = bands_of(&to_kurbo(&builder.build(), 1.0));
        let hole = (24.5, 24.5);
        let ring = (5.5, 31.5);
        let outside = (60.5, 16.5);
        assert_eq!(coverage(&bands, &segments, hole.0, hole.1, false), 1.0);
        assert_eq!(coverage(&bands, &segments, hole.0, hole.1, true), 0.0);
        assert_eq!(coverage(&bands, &segments, ring.0, ring.1, true), 1.0);
        assert_eq!(
            coverage(&bands, &segments, outside.0, outside.1, false),
            0.0
        );
        // Half a pixel past the left edge is half covered.
        assert!((coverage(&bands, &segments, 0.0, 20.5, false) - 0.5).abs() < 1e-3);
    }

    // Catches a stroke expansion that ignores the cap: a square cap reaches
    // half the width past the end, a butt cap stops at it.
    #[test]
    fn stroke_caps_set_how_far_the_outline_reaches_past_the_end() {
        let line = to_kurbo(&Path::polyline([(10.0, 10.0), (40.0, 10.0)]), 1.0);
        for (cap, past_end) in [(LineCap::Butt, 0.0), (LineCap::Square, 1.0)] {
            let style = StrokeStyle::new(8.0).cap(cap);
            let (bands, segments) = bands_of(&stroke_outline(&line, &style, 1.0));
            // 3 px past the end, on the center line.
            let at = coverage(&bands, &segments, 43.5, 10.5, false);
            assert_eq!(at, past_end, "{cap:?}");
            assert_eq!(coverage(&bands, &segments, 25.5, 10.5, false), 1.0);
            assert_eq!(coverage(&bands, &segments, 25.5, 16.5, false), 0.0);
        }
    }
}
