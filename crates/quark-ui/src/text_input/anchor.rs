//! Popups anchored to an editor's caret: the caret's painted position,
//! shared from the editor's element to the app, and an element that places
//! a popup (a completion list, say) next to it.

use std::sync::{Arc, Mutex, PoisonError};

use quark_render::Scene;
use quark_render::scene::Rect;

use crate::element::LayoutId;
use crate::element::{AnyElement, Bounds, Element, ElementContext, IntoAnyElement, LayoutEngine};
use crate::style::{ElementStyle, Styled};

/// Where an editor's caret and field were painted, in window coordinates
/// (ignoring transforms of their ancestors, such as a sliding panel).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CaretGeometry {
    /// One line tall at the caret, even while the caret blinks off or the
    /// field is unfocused.
    pub caret: Rect,
    /// The editor element's box.
    pub field: Rect,
}

/// The caret position an [`super::Editor`]'s element records each frame
/// (see [`super::Editor::caret_anchor`]). The element writes it while
/// prepainting, so a [`caret_popup`] later in the tree reads this frame's
/// position. Clones share it.
#[derive(Debug, Clone, Default)]
pub struct CaretAnchor(Arc<Mutex<Option<CaretGeometry>>>);

impl CaretAnchor {
    /// `None` until the editor's element has been painted.
    pub fn get(&self) -> Option<CaretGeometry> {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn set(&self, geometry: Option<CaretGeometry>) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = geometry;
    }
}

/// Where a popup of `size` goes for `caret` in a `viewport`: below the
/// caret and left-aligned with it, or above it when it does not fit below
/// and there is more room above; then moved inside the viewport.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CaretPlacement {
    pub x: f32,
    pub y: f32,
    pub above: bool,
}

pub fn place_at_caret(
    caret: Rect,
    size: (f32, f32),
    viewport: (f32, f32),
    gap: f32,
) -> CaretPlacement {
    let (w, h) = size;
    let (vw, vh) = viewport;
    let below = vh - (caret.y + caret.height) - gap;
    let above_room = caret.y - gap;
    let above = below < h && above_room > below;
    let y = if above {
        caret.y - gap - h
    } else {
        caret.y + caret.height + gap
    };
    // `max` after `min`: an oversized popup pins to the top or left edge.
    CaretPlacement {
        x: caret.x.min(vw - w).max(0.0),
        y: y.min(vh - h).max(0.0),
        above,
    }
}

/// A popup placed at an editor's caret after layout, out of flow so it
/// moves nothing: put it in the window's overlay layer, after the editor
/// in the tree, and give the child a width and a z index (so it paints and
/// hit-tests above the content). The child is hidden until the editor has
/// been painted once.
pub struct CaretPopup {
    anchor: CaretAnchor,
    child: AnyElement,
    viewport: (f32, f32),
    gap: f32,
    /// The caret the child was placed against in prepaint.
    placed: Option<CaretGeometry>,
}

pub fn caret_popup(
    anchor: &CaretAnchor,
    child: impl IntoAnyElement,
    viewport: (f32, f32),
) -> CaretPopup {
    CaretPopup {
        anchor: anchor.clone(),
        child: child.into_any(),
        viewport,
        gap: 4.0,
        placed: None,
    }
}

impl CaretPopup {
    /// Distance between the caret line and the popup, in points.
    pub fn gap(mut self, gap: f32) -> Self {
        self.gap = gap;
        self
    }
}

/// [`Styled`] over a bare style, to build a layout node's taffy style.
struct LayoutStyle(ElementStyle);

impl Styled for LayoutStyle {
    fn element_style_mut(&mut self) -> &mut ElementStyle {
        &mut self.0
    }
}

pub struct CaretPopupLayout {
    child: LayoutId,
    /// The child's move from its laid-out position, set in prepaint.
    offset: Option<(f32, f32)>,
}

impl Element for CaretPopup {
    type LayoutState = CaretPopupLayout;
    type PrepaintState = ();

    fn request_layout(
        &mut self,
        engine: &mut LayoutEngine,
        cx: &mut ElementContext,
    ) -> (LayoutId, CaretPopupLayout) {
        let child = self.child.request_layout(engine, cx);
        let style = LayoutStyle(ElementStyle::default())
            .absolute()
            .left(0.0)
            .top(0.0)
            .0
            .layout;
        let id = engine.request_layout(style, &[child]);
        (
            id,
            CaretPopupLayout {
                child,
                offset: None,
            },
        )
    }

    fn prepaint(
        &mut self,
        bounds: Bounds,
        layout: &mut CaretPopupLayout,
        engine: &LayoutEngine,
        cx: &mut ElementContext,
    ) {
        self.placed = self.anchor.get();
        let Some(geometry) = self.placed else {
            layout.offset = None;
            return;
        };
        let child = engine.layout_bounds(layout.child);
        let placed = place_at_caret(
            geometry.caret,
            (child.width, child.height),
            self.viewport,
            self.gap,
        );
        let offset = (placed.x - bounds.x, placed.y - bounds.y);
        layout.offset = Some(offset);
        self.child
            .prepaint_with_offset(engine, cx, offset.0, offset.1);
    }

    fn paint(
        &mut self,
        _bounds: Bounds,
        layout: &mut CaretPopupLayout,
        _prepaint: &mut (),
        engine: &LayoutEngine,
        scene: &mut Scene,
        cx: &mut ElementContext,
    ) {
        // Every element has prepainted by now: if the editor comes later in
        // the tree, the caret moved after this popup was placed, so place
        // it again next frame.
        if self.anchor.get() != self.placed {
            cx.request_frame_at_ms(cx.clock_ms);
        }
        if let Some((dx, dy)) = layout.offset {
            self.child.paint_with_offset(engine, scene, cx, dx, dy);
        }
    }
}

impl IntoAnyElement for CaretPopup {
    fn into_any(self) -> AnyElement {
        AnyElement::new(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f32, y: f32, width: f32, height: f32) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    // Each row: caret, popup size, expected placement. Viewport 400x300,
    // gap 4, caret lines 20 tall.
    #[test]
    fn caret_popups_flip_above_near_the_bottom_and_stay_inside_the_viewport() {
        let cases = [
            // Room below: under the caret line, at the caret.
            (
                rect(50.0, 40.0, 2.0, 20.0),
                (120.0, 80.0),
                (50.0, 64.0, false),
            ),
            // A composer at the bottom: above the caret line.
            (
                rect(50.0, 260.0, 2.0, 20.0),
                (120.0, 80.0),
                (50.0, 176.0, true),
            ),
            // Caret near the right edge: pulled left to fit.
            (
                rect(390.0, 40.0, 2.0, 20.0),
                (120.0, 80.0),
                (280.0, 64.0, false),
            ),
            // Too tall for either side: the roomier side, pinned inside.
            (
                rect(50.0, 200.0, 2.0, 20.0),
                (120.0, 260.0),
                (50.0, 0.0, true),
            ),
        ];
        for (caret, size, (x, y, above)) in cases {
            assert_eq!(
                place_at_caret(caret, size, (400.0, 300.0), 4.0),
                CaretPlacement { x, y, above },
                "caret {caret:?}"
            );
        }
    }
}
