//! Row presentation: how wide a row's box is, where it sits in the
//! viewport, and what fills it. A chat bubble is an ordinary row with a
//! capped width, end alignment, a background, and rounded corners, so its
//! text stays in document selection, find, and hit testing.
//!
//! [`RowGeometry::resolve`] is the one place a row's box and content
//! column are worked out. Foreground and background measurement,
//! materialization, the element, and [`RowStyle::content_width`] all call
//! it, so a narrowed row is measured at the width it paints at.

use std::hash::{Hash, Hasher};

use quark::Color;

use super::DocumentStyle;

/// Where a row narrower than the viewport sits. Start and End follow the
/// document's left-to-right column, whatever the direction of the row's
/// text.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum RowAlign {
    #[default]
    Start,
    End,
    Center,
}

/// A row's box and fill, carried in [`RowChrome::style`](super::RowChrome::style).
/// The default fills the viewport, takes the [`DocumentStyle`] padding,
/// and leaves the background to the [`RowDecorator`](super::RowDecorator).
///
/// `max_width` and `padding` change the row's height: after changing them
/// in a [`Document`](super::Document)'s source, report the row with
/// [`Document::update`](super::Document::update).
/// [`MarkdownDocument::set_chrome`](super::MarkdownDocument::set_chrome)
/// does that itself, and repaints without remeasuring when only the
/// alignment, background, or radius changed.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct RowStyle {
    /// Width of the whole row box, padding and header included, in
    /// logical points; `None` fills the viewport. A short row still takes
    /// its whole capped width. A nonfinite or nonpositive cap reads as
    /// none.
    pub max_width: Option<f32>,
    pub align: RowAlign,
    /// Fills the row box, in place of the decorator's
    /// [`background`](super::RowDecorator::background); `None` leaves the
    /// fill to the decorator.
    pub background: Option<Color>,
    /// Rounds the box: its fill, its leading edge, and everything painted
    /// in it are clipped to the rounded outline. At most half the box's
    /// smaller side.
    pub corner_radius: f32,
    /// Logical points, `[top, right, bottom, left]`. `None` inherits
    /// [`DocumentStyle`]'s `pad_y`/`pad_x` values. Horizontal padding that
    /// leaves the content less than a point shrinks in proportion.
    pub padding: Option<[f32; 4]>,
}

impl RowStyle {
    /// Width of the row's content column at `viewport_width`: what its
    /// blocks wrap at and what adornment builders get as
    /// [`AdornmentCx::width`](super::AdornmentCx::width). Measure a
    /// wrapping adornment's height at it.
    pub fn content_width(&self, viewport_width: f32, style: &DocumentStyle) -> f32 {
        RowGeometry::resolve(self, viewport_width, style).content_width
    }

    /// Whether rows styled `self` and `other` have the same height at
    /// every width: only the cap and the padding change it.
    pub(super) fn same_layout(&self, other: &Self) -> bool {
        let pads = |style: &Self| style.padding.map(|p| p.map(|v| sanitize(v).to_bits()));
        self.cap().map(f32::to_bits) == other.cap().map(f32::to_bits) && pads(self) == pads(other)
    }

    fn cap(&self) -> Option<f32> {
        self.max_width.filter(|w| w.is_finite() && *w > 0.0)
    }
}

/// Hashes the values the row resolves to, so styles that paint alike hash
/// alike: a cap of zero is no cap, a negative radius none.
impl Hash for RowStyle {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.cap().map(f32::to_bits).hash(state);
        self.align.hash(state);
        self.background.map(|c| (c.r, c.g, c.b, c.a)).hash(state);
        sanitize(self.corner_radius).to_bits().hash(state);
        self.padding
            .map(|p| p.map(|v| sanitize(v).to_bits()))
            .hash(state);
    }
}

/// A row's box and content column at one viewport width: x in viewport
/// coordinates, the paddings below and above the row's flow.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct RowGeometry {
    /// The row box's left edge and width.
    pub x: f32,
    pub width: f32,
    /// The content column's left edge and width. The column is never
    /// narrower than a point, which the text engine needs; a box narrower
    /// than that lies outside the viewport anyway.
    pub content_x: f32,
    pub content_width: f32,
    /// Space above the header band and below the row's last item.
    pub pad_top: f32,
    pub pad_bottom: f32,
    radius: f32,
}

impl RowGeometry {
    pub fn resolve(row: &RowStyle, viewport_width: f32, style: &DocumentStyle) -> Self {
        let viewport = sanitize(viewport_width);
        let width = row.cap().map_or(viewport, |cap| cap.min(viewport));
        let x = match row.align {
            RowAlign::Start => 0.0,
            RowAlign::End => viewport - width,
            RowAlign::Center => (viewport - width) * 0.5,
        };
        let [top, right, bottom, left] = row
            .padding
            .unwrap_or([style.pad_y, style.pad_x, style.pad_y, style.pad_x])
            .map(sanitize);
        // Padding gives way before the content's last point does.
        let room = width - width.min(1.0);
        let (left, right) = if left + right > room {
            let scale = room / (left + right);
            (left * scale, right * scale)
        } else {
            (left, right)
        };
        Self {
            x,
            width,
            content_x: x + left,
            content_width: (width - (left + right)).max(1.0),
            pad_top: top,
            pad_bottom: bottom,
            radius: sanitize(row.corner_radius),
        }
    }

    /// The corner radius of the box when it is `height` tall.
    pub fn radius(&self, height: f32) -> f32 {
        self.radius.min(self.width.min(height) * 0.5).max(0.0)
    }
}

/// `v` when it is a finite positive length, else zero.
fn sanitize(v: f32) -> f32 {
    if v.is_finite() && v > 0.0 { v } else { 0.0 }
}
