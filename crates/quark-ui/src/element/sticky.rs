use super::*;

// ---------------------------------------------------------------------------
// StickySection — a header that sticks to the top of its scroll viewport
// ---------------------------------------------------------------------------

/// A section whose header sticks to the top of the enclosing clip (a
/// scroll container's viewport) while the section's body scrolls under
/// it, and is pushed off by the section's bottom edge, as CSS
/// `position: sticky; top: 0` on a section header behaves.
///
/// Taffy has no sticky positioning, so the section lays out like a column
/// (header, then body) and moves the header at paint time; hit testing
/// follows the painted header. The offset depends on the scroll offset, so
/// a section must not sit inside a [`cached`] subtree, which replays its
/// last paint unchanged.
pub struct StickySection {
    style: ElementStyle,
    header: AnyElement,
    body: AnyElement,
    top: f32,
}

/// A [`StickySection`] with `header` over `body`.
pub fn sticky_section(header: impl IntoAnyElement, body: impl IntoAnyElement) -> StickySection {
    let mut style = ElementStyle::default();
    style.layout.flex_direction = taffy::FlexDirection::Column;
    StickySection {
        style,
        header: header.into_any(),
        body: body.into_any(),
        top: 0.0,
    }
}

impl StickySection {
    /// Stick `inset` points below the viewport's top edge instead of at it.
    pub fn sticky_top(mut self, inset: f32) -> Self {
        self.top = inset;
        self
    }
}

impl Styled for StickySection {
    fn element_style_mut(&mut self) -> &mut ElementStyle {
        &mut self.style
    }
}

/// How far down the header moves from its laid-out place: enough to keep
/// its top at `clip_top + inset`, never above its place, and never past
/// the section's bottom edge.
fn sticky_shift(section: Bounds, header: Bounds, clip_top: f32, inset: f32) -> f32 {
    let room = (section.y + section.height) - (header.y + header.height);
    (clip_top + inset - header.y).clamp(0.0, room.max(0.0))
}

impl Element for StickySection {
    /// The header's layout node.
    type LayoutState = LayoutId;
    /// The header's paint-time shift and the z it is raised to.
    type PrepaintState = (f32, i32);

    fn request_layout(
        &mut self,
        engine: &mut LayoutEngine,
        cx: &mut ElementContext,
    ) -> (LayoutId, LayoutId) {
        let header = self.header.request_layout(engine, cx);
        let body = self.body.request_layout(engine, cx);
        let id = engine.request_layout(self.style.layout.clone(), &[header, body]);
        (id, header)
    }

    fn prepaint(
        &mut self,
        bounds: Bounds,
        header: &mut LayoutId,
        engine: &LayoutEngine,
        cx: &mut ElementContext,
    ) -> (f32, i32) {
        let (dx, dy) = cx.current_element_offset();
        let mut header_bounds = engine.layout_bounds(*header);
        header_bounds.x += dx;
        header_bounds.y += dy;
        let shift = sticky_shift(bounds, header_bounds, cx.current_clip().y, self.top);
        self.body.prepaint(engine, cx);
        // The header covers the body it sticks over, for the pointer too.
        let z = cx.current_z_index() + 1;
        cx.push_z_index(z);
        self.header.prepaint_with_offset(engine, cx, 0.0, shift);
        cx.pop_z_index();
        (shift, z)
    }

    fn paint(
        &mut self,
        _bounds: Bounds,
        _header: &mut LayoutId,
        &mut (shift, z): &mut (f32, i32),
        engine: &LayoutEngine,
        scene: &mut Scene,
        cx: &mut ElementContext,
    ) {
        // Header first, so it comes first in the accessibility tree; its
        // raised z keeps it drawn over the body.
        scene.push_z_index(z);
        self.header.paint_with_offset(engine, scene, cx, 0.0, shift);
        scene.pop_z_index();
        self.body.paint(engine, scene, cx);
    }
}

impl IntoAnyElement for StickySection {
    fn into_any(self) -> AnyElement {
        element_into_any(self)
    }
}
