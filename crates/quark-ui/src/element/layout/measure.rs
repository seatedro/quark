use super::super::*;
use taffy::AvailableSpace;

// ---------------------------------------------------------------------------
// MeasureFunc — stored per-node for intrinsic sizing
// ---------------------------------------------------------------------------

pub(in crate::element) type MeasureFn = Box<
    dyn Fn(taffy::Size<Option<f32>>, taffy::Size<taffy::AvailableSpace>) -> taffy::Size<f32>
        + Send
        + Sync,
>;

pub(in crate::element) enum NodeMeasure {
    /// Leaf with an intrinsic measure function.
    Measure(MeasureFn),
    /// Text that wraps to the width layout gives it.
    Text(TextMeasure),
    /// A cache boundary whose content is laid out in `subtrees[index]`.
    Subtree(usize),
    /// A cache boundary replayed from the cache: it answers the measure
    /// queries recorded with it, `memo[range]`, and flags `slot` stale on
    /// any other query.
    Replay { memo: Range<usize>, slot: u32 },
}

/// What measured text borrows while layout computes: the window's text
/// system and layout cache, so wrapped sizes are shaped (and cached) by the
/// same system paint uses.
pub struct MeasureContext<'a> {
    pub text: &'a mut TextSystem,
    pub layouts: &'a mut LayoutCache,
    /// Device pixels per point: layout snaps box edges to this grid.
    pub scale: f32,
}

impl ElementContext<'_> {
    /// The context's text state, for computing a layout.
    pub fn measure_context(&mut self) -> MeasureContext<'_> {
        MeasureContext {
            text: self.text,
            layouts: self.layouts,
            scale: self.scale_factor,
        }
    }
}

/// `scale`, or 1 if it is not positive and finite.
fn grid(scale: f32) -> f32 {
    if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    }
}

/// `v` points rounded to the nearest device pixel at `scale`.
pub(in crate::element) fn round_to_pixels(v: f32, scale: f32) -> f32 {
    let scale = grid(scale);
    (v * scale).round() / scale
}

/// `v` points rounded up to whole device pixels at `scale`.
pub(in crate::element) fn ceil_to_pixels(v: f32, scale: f32) -> f32 {
    let scale = grid(scale);
    (v * scale).ceil() / scale
}

/// Float error can leave a box a hair under the max-content width it was
/// sized to, in device pixels.
const FIT_SLACK_PX: f32 = 0.01;

fn floor_to_pixels(v: f32, scale: f32) -> f32 {
    let scale = grid(scale);
    (v * scale).floor() / scale
}

impl MeasureContext<'_> {
    fn layout(&mut self, query: &TextQuery) -> Option<Arc<TextLayout>> {
        self.layouts.layout_query(self.text, query).ok()
    }
}

/// A text leaf that wraps automatically. Its node keeps the descriptor
/// across frames while the unwrapped layout is the same one, so an
/// unchanged frame neither dirties nor measures it.
pub(in crate::element) struct TextMeasure {
    /// The text shaped without wrapping, at the frame's scale: its inputs
    /// are what a wrapped probe shapes, its width the max-content width.
    unwrapped: Arc<TextLayout>,
    /// Height of one line; the element is never shorter.
    line_height: f32,
    /// Lines shown; the element is as tall as these and clips the rest.
    max_lines: Option<usize>,
    /// Min-content width, once asked for.
    min_content: Option<f32>,
}

impl TextMeasure {
    pub(in crate::element) fn new(unwrapped: Arc<TextLayout>, line_height: f32) -> Self {
        Self {
            unwrapped,
            line_height,
            max_lines: None,
            min_content: None,
        }
    }

    pub(in crate::element) fn max_lines(mut self, max_lines: Option<usize>) -> Self {
        self.max_lines = max_lines;
        self
    }

    /// Whether `self` measures as `other` does: the same shaped text, line
    /// height, and line limit.
    pub(super) fn measures_as(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.unwrapped, &other.unwrapped)
            && self.line_height == other.line_height
            && self.max_lines == other.max_lines
    }

    /// Device pixels per point the text was shaped at.
    fn scale(&self) -> f32 {
        self.unwrapped.scale_factor()
    }

    fn max_content(&self) -> f32 {
        ceil_to_pixels(self.unwrapped.size().0, self.scale())
    }

    fn min_content(&mut self) -> f32 {
        let max = self.max_content();
        let scale = self.scale();
        *self.min_content.get_or_insert_with(|| {
            ceil_to_pixels(self.unwrapped.min_content_width(), scale).min(max)
        })
    }

    /// A known width as given, otherwise the width offered up to the
    /// max-content width; the height of the text wrapped at that width.
    ///
    /// An offered width is not raised to the min-content width: a leaf's
    /// final query offers its resolved width as available space without
    /// marking it known, and the height must be the one paint wraps to at
    /// that width. Flex items still stop shrinking at the min-content width
    /// through taffy's automatic minimum size.
    pub(super) fn measure(
        &mut self,
        known: taffy::Size<Option<f32>>,
        available: taffy::Size<AvailableSpace>,
        cx: &mut MeasureContext,
    ) -> taffy::Size<f32> {
        let width = known.width.unwrap_or_else(|| match available.width {
            AvailableSpace::MinContent => self.min_content(),
            AvailableSpace::MaxContent => self.max_content(),
            AvailableSpace::Definite(offered) => offered.min(self.max_content()),
        });
        let height = known.height.unwrap_or_else(|| {
            let wrapped = self
                .wrap_width(width)
                .and_then(|wrap| cx.layout(&self.unwrapped.query().wrap_width(Some(wrap))));
            let layout = wrapped.as_deref().unwrap_or(&self.unwrapped);
            text_height(layout, self.max_lines, self.line_height)
        });
        taffy::Size { width, height }
    }

    /// The width the text is shaped at in a box `width` wide: `None` when
    /// it fits unwrapped.
    ///
    /// The width is floored to device pixels: layout snaps box edges to
    /// device pixels, and the snapped width is never below the floor of the
    /// measured one, so the wrapped lines fit the painted box.
    pub(super) fn wrap_width(&self, width: f32) -> Option<f32> {
        let slack = FIT_SLACK_PX / grid(self.scale());
        (width < self.max_content() - slack).then(|| floor_to_pixels(width, self.scale()).max(1.0))
    }
}

/// Height of `layout` showing at most `max_lines`, never shorter than one
/// line, in whole device pixels.
pub(in crate::element) fn text_height(
    layout: &TextLayout,
    max_lines: Option<usize>,
    line_height: f32,
) -> f32 {
    let height = match max_lines.and_then(|n| layout.line(n)) {
        Some(first_hidden) => first_hidden.top,
        None => layout.size().1,
    };
    ceil_to_pixels(height.max(line_height), layout.scale_factor())
}

/// One measure query a cache boundary answered: what the parent offered
/// and the size it got back.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(in crate::element) struct MeasureMemo {
    pub(super) known: taffy::Size<Option<f32>>,
    pub(super) available: taffy::Size<AvailableSpace>,
    pub(super) size: taffy::Size<f32>,
}

pub(super) fn find_memo(
    memo: &[MeasureMemo],
    known: taffy::Size<Option<f32>>,
    available: taffy::Size<AvailableSpace>,
) -> Option<taffy::Size<f32>> {
    if let (Some(width), Some(height)) = (known.width, known.height) {
        return Some(taffy::Size { width, height });
    }
    memo.iter()
        .find(|m| m.known == known && m.available == available)
        .map(|m| m.size)
}
