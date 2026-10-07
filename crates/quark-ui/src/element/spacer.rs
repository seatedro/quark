use super::*;

// ---------------------------------------------------------------------------
// Spacer — flexible empty space
// ---------------------------------------------------------------------------

pub struct Spacer;

pub fn spacer() -> Spacer {
    Spacer
}

impl Element for Spacer {
    type LayoutState = ();
    type PrepaintState = ();

    fn request_layout(
        &mut self,
        engine: &mut LayoutEngine,
        _cx: &mut ElementContext,
    ) -> (LayoutId, ()) {
        let id = engine.request_layout(
            taffy::Style {
                flex_grow: 1.0,
                ..Default::default()
            },
            &[],
        );
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
        _bounds: Bounds,
        _layout_state: &mut (),
        _prepaint_state: &mut (),
        _engine: &LayoutEngine,
        _scene: &mut Scene,
        _cx: &mut ElementContext,
    ) {
    }
}

impl IntoAnyElement for Spacer {
    fn into_any(self) -> AnyElement {
        element_into_any(self)
    }
}
