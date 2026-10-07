use super::*;

// ---------------------------------------------------------------------------
// Canvas — custom painting element
// ---------------------------------------------------------------------------

/// A leaf element that delegates painting to a caller-provided closure.
/// Participates in layout via its Taffy style.
type PaintFn = Box<dyn FnOnce(Bounds, &mut Scene, &mut ElementContext)>;

pub struct Canvas {
    style: taffy::Style,
    paint_fn: Option<PaintFn>,
}

/// Create a canvas element that calls `paint` with its resolved bounds.
pub fn canvas(paint: impl FnOnce(Bounds, &mut Scene, &mut ElementContext) + 'static) -> Canvas {
    Canvas {
        style: taffy::Style::default(),
        paint_fn: Some(Box::new(paint)),
    }
}

impl Canvas {
    pub fn w(mut self, v: f32) -> Self {
        self.style.size.width = taffy::Dimension::length(v);
        self
    }

    pub fn h(mut self, v: f32) -> Self {
        self.style.size.height = taffy::Dimension::length(v);
        self
    }

    pub fn flex_1(mut self) -> Self {
        self.style.flex_grow = 1.0;
        self.style.flex_shrink = 1.0;
        self.style.flex_basis = taffy::Dimension::percent(0.0);
        self
    }
}

impl Element for Canvas {
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
            f(bounds, scene, cx);
        }
    }
}

impl IntoAnyElement for Canvas {
    fn into_any(self) -> AnyElement {
        element_into_any(self)
    }
}
