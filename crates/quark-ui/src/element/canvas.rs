use quark_render::scene::{FillRule, Path, PathPrimitive, StrokeStyle};

use super::*;

// ---------------------------------------------------------------------------
// Canvas — custom painting element
// ---------------------------------------------------------------------------

/// A leaf element that delegates painting to a caller-provided closure.
/// Participates in layout via its Taffy style. The closure is stored
/// inline: the element's pooled box holds it, so building a canvas does
/// not allocate.
pub struct Canvas<F> {
    style: taffy::Style,
    paint_fn: Option<F>,
    clips: bool,
}

/// Create a canvas element that calls `paint` with its resolved bounds.
pub fn canvas<F>(paint: F) -> Canvas<F>
where
    F: FnOnce(Bounds, &mut Scene, &mut ElementContext) + 'static,
{
    Canvas {
        style: taffy::Style::default(),
        paint_fn: Some(paint),
        clips: false,
    }
}

/// A canvas that draws vector paths (charts, sparklines, custom shapes)
/// through a [`CanvasPainter`], in coordinates local to the canvas.
pub fn path_canvas(
    paint: impl FnOnce(&mut CanvasPainter<'_>) + 'static,
) -> Canvas<impl FnOnce(Bounds, &mut Scene, &mut ElementContext) + 'static> {
    canvas(move |bounds, scene, _cx| paint(&mut CanvasPainter { scene, bounds }))
}

/// Draws paths into a canvas. Path coordinates are logical points from the
/// canvas's top-left corner; the renderer fills and strokes them
/// antialiased on the GPU at any scale factor.
pub struct CanvasPainter<'a> {
    scene: &'a mut Scene,
    bounds: Bounds,
}

impl CanvasPainter<'_> {
    /// The canvas's laid-out width and height.
    pub fn size(&self) -> (f32, f32) {
        (self.bounds.width, self.bounds.height)
    }

    pub fn fill(&mut self, path: impl Into<Arc<Path>>, color: Color) {
        self.fill_with_rule(path, color, FillRule::NonZero);
    }

    pub fn fill_with_rule(&mut self, path: impl Into<Arc<Path>>, color: Color, rule: FillRule) {
        let primitive = self.primitive(path.into()).fill(color).fill_rule(rule);
        self.scene.path(primitive);
    }

    pub fn stroke(&mut self, path: impl Into<Arc<Path>>, color: Color, style: StrokeStyle) {
        let primitive = self.primitive(path.into()).stroke(color, style);
        self.scene.path(primitive);
    }

    /// Fill, then stroke on top, as one primitive.
    pub fn fill_and_stroke(
        &mut self,
        path: impl Into<Arc<Path>>,
        fill: Color,
        stroke: Color,
        style: StrokeStyle,
    ) {
        let primitive = self.primitive(path.into()).fill(fill).stroke(stroke, style);
        self.scene.path(primitive);
    }

    /// The scene, for primitives other than paths. Its coordinates are the
    /// window's, not the canvas's.
    pub fn scene(&mut self) -> &mut Scene {
        self.scene
    }

    fn primitive(&self, path: Arc<Path>) -> PathPrimitive {
        PathPrimitive::new(path, [self.bounds.x, self.bounds.y])
    }
}

/// A polyline through `values` spread evenly across `width`, scaled so the
/// smallest value sits at `height` and the largest at 0 (a flat series sits
/// in the middle). Stroke it for a sparkline, or close it along the bottom
/// and fill it for an area chart.
pub fn sparkline_path(values: &[f32], width: f32, height: f32) -> Path {
    let (lo, hi) = values
        .iter()
        .filter(|v| v.is_finite())
        .fold((f32::MAX, f32::MIN), |(lo, hi), &v| (lo.min(v), hi.max(v)));
    let span = hi - lo;
    let step = if values.len() > 1 {
        width / (values.len() - 1) as f32
    } else {
        0.0
    };
    Path::polyline(values.iter().enumerate().map(|(i, &v)| {
        let t = if span > 0.0 && v.is_finite() {
            (v - lo) / span
        } else {
            0.5
        };
        (i as f32 * step, height * (1.0 - t))
    }))
}

impl<F> Canvas<F> {
    pub fn w(mut self, v: f32) -> Self {
        self.style.size.width = taffy::Dimension::length(v);
        self
    }

    pub fn h(mut self, v: f32) -> Self {
        self.style.size.height = taffy::Dimension::length(v);
        self
    }

    /// Clip what the canvas paints to its bounds.
    pub fn clip(mut self) -> Self {
        self.clips = true;
        self
    }

    pub fn flex_1(mut self) -> Self {
        self.style.flex_grow = 1.0;
        self.style.flex_shrink = 1.0;
        self.style.flex_basis = taffy::Dimension::percent(0.0);
        self
    }
}

impl<F: FnOnce(Bounds, &mut Scene, &mut ElementContext) + 'static> Element for Canvas<F> {
    type LayoutState = ();
    type PrepaintState = ();

    fn request_layout(
        &mut self,
        engine: &mut LayoutEngine,
        _cx: &mut ElementContext,
    ) -> (LayoutId, ()) {
        let id = engine.request_layout(self.style.clone(), &[]);
        (id, ())
    }

    fn prepaint(
        &mut self,
        _bounds: Bounds,
        _layout_state: &mut (),
        _engine: &LayoutEngine,
        _cx: &mut ElementContext,
    ) {
    }

    fn paint(
        &mut self,
        bounds: Bounds,
        _layout_state: &mut (),
        _prepaint_state: &mut (),
        _engine: &LayoutEngine,
        scene: &mut Scene,
        cx: &mut ElementContext,
    ) {
        if let Some(f) = self.paint_fn.take() {
            if self.clips {
                scene.clip(bounds);
            }
            f(bounds, scene, cx);
            if self.clips {
                scene.pop_clip();
            }
        }
    }
}

impl<F: FnOnce(Bounds, &mut Scene, &mut ElementContext) + 'static> IntoAnyElement for Canvas<F> {
    fn into_any(self) -> AnyElement {
        element_into_any(self)
    }
}

#[cfg(test)]
mod tests {
    use quark_render::scene::PathVerb;

    use super::*;

    fn points(path: &Path) -> Vec<[f32; 2]> {
        path.verbs()
            .iter()
            .filter_map(|verb| match *verb {
                PathVerb::MoveTo(p) | PathVerb::LineTo(p) => Some(p),
                _ => None,
            })
            .collect()
    }

    // Catches an inverted or unnormalized sparkline: the largest value at
    // the top edge, the smallest at the bottom, evenly spaced, and a flat
    // series centered instead of dividing by zero.
    #[test]
    fn sparkline_spans_the_box_with_high_values_on_top() {
        let cases: &[(&[f32], &[[f32; 2]])] = &[
            (&[2.0, 6.0, 4.0], &[[0.0, 20.0], [50.0, 0.0], [100.0, 10.0]]),
            (&[3.0, 3.0], &[[0.0, 10.0], [100.0, 10.0]]),
            (&[7.0], &[[0.0, 10.0]]),
        ];
        for (values, expected) in cases {
            assert_eq!(
                points(&sparkline_path(values, 100.0, 20.0)),
                *expected,
                "{values:?}"
            );
        }
    }
}
