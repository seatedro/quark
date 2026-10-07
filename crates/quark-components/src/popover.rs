//! Popover panels and their placement next to an anchor.
//!
//! Layout does not know where an element ends up on screen until it has
//! run, so placement happens in two ways:
//!
//! - [`anchored`] wraps a popover so it is placed after layout, against the
//!   bounds of the element it sits in. Select and combobox lists use it.
//! - [`place_popover`] is the pure rule both use: put the popover on the
//!   preferred side, flip to the opposite side when it would leave the
//!   viewport there and fits better on the other, then clamp it inside the
//!   viewport.

use quark::{Rect, SemanticRole};

use quark_render::Scene;
use quark_ui::design::{Shadow, Sp, Sz};
use quark_ui::element::{
    AnyElement, Bounds, Div, Element, ElementContext, IntoAnyElement, LayoutEngine,
    LayoutId, div,
};
use quark_ui::style::{ElementStyle, Styled};
use quark_ui::theme::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PopoverSide {
    Top,
    Bottom,
    Left,
    Right,
}

impl PopoverSide {
    fn opposite(self) -> Self {
        match self {
            Self::Top => Self::Bottom,
            Self::Bottom => Self::Top,
            Self::Left => Self::Right,
            Self::Right => Self::Left,
        }
    }
}

/// Where [`place_popover`] put a popover: its top-left corner in the
/// anchor's coordinate space and the side it ended up on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PopoverPlacement {
    pub x: f32,
    pub y: f32,
    pub side: PopoverSide,
}

/// Place a `size` popover `gap` points from `anchor` on `preferred`, inside
/// a viewport of `viewport` points with its origin at 0,0.
///
/// The popover flips to the opposite side when it overflows the viewport
/// on `preferred` and the opposite side has more room. Then it is clamped
/// into the viewport on both axes; a popover larger than the viewport keeps
/// its top or left edge visible.
pub fn place_popover(
    anchor: Rect,
    size: (f32, f32),
    viewport: (f32, f32),
    preferred: PopoverSide,
    gap: f32,
) -> PopoverPlacement {
    let (w, h) = size;
    let (vw, vh) = viewport;
    // Room between the anchor and each viewport edge, less the gap.
    let room = |side: PopoverSide| match side {
        PopoverSide::Top => anchor.y - gap,
        PopoverSide::Bottom => vh - (anchor.y + anchor.height) - gap,
        PopoverSide::Left => anchor.x - gap,
        PopoverSide::Right => vw - (anchor.x + anchor.width) - gap,
    };
    let need = |side: PopoverSide| match side {
        PopoverSide::Top | PopoverSide::Bottom => h,
        PopoverSide::Left | PopoverSide::Right => w,
    };
    let side = if room(preferred) < need(preferred)
        && room(preferred.opposite()) > room(preferred)
    {
        preferred.opposite()
    } else {
        preferred
    };
    let (x, y) = match side {
        PopoverSide::Bottom => (anchor.x, anchor.y + anchor.height + gap),
        PopoverSide::Top => (anchor.x, anchor.y - gap - h),
        PopoverSide::Right => (anchor.x + anchor.width + gap, anchor.y),
        PopoverSide::Left => (anchor.x - gap - w, anchor.y),
    };
    // `max` after `min`: an oversized popover pins to the top or left edge.
    PopoverPlacement {
        x: x.min(vw - w).max(0.0),
        y: y.min(vh - h).max(0.0),
        side,
    }
}

/// A popover placed after layout next to the element it is a child of.
///
/// Put it as the last child of the anchor element. It takes no space in the
/// anchor's layout; its child is laid out at its own size, then moved by
/// [`place_popover`] against the anchor's bounds and `viewport`. Give the
/// child a z index (as [`popover_panel`] does) so it paints and hit tests
/// above later siblings.
pub struct Anchored {
    child: AnyElement,
    side: PopoverSide,
    viewport: (f32, f32),
    gap: f32,
}

pub fn anchored(child: impl IntoAnyElement, side: PopoverSide, viewport: (f32, f32)) -> Anchored {
    Anchored {
        child: child.into_any(),
        side,
        viewport,
        gap: 4.0,
    }
}

impl Anchored {
    /// Distance between the anchor and the popover, in points.
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

pub struct AnchoredLayout {
    child: LayoutId,
    /// The child's move from its laid-out position, set in prepaint.
    offset: (f32, f32),
}

impl Element for Anchored {
    type LayoutState = AnchoredLayout;
    type PrepaintState = ();

    fn request_layout(
        &mut self,
        engine: &mut LayoutEngine,
        cx: &mut ElementContext,
    ) -> (LayoutId, AnchoredLayout) {
        let child = self.child.request_layout(engine, cx);
        // Out of flow and the anchor's size, so its bounds are the anchor's.
        let style = LayoutStyle(ElementStyle::default())
            .absolute()
            .left(0.0)
            .top(0.0)
            .size_full()
            .0
            .layout;
        let id = engine.request_layout(style, &[child]);
        (
            id,
            AnchoredLayout {
                child,
                offset: (0.0, 0.0),
            },
        )
    }

    fn prepaint(
        &mut self,
        bounds: Bounds,
        layout: &mut AnchoredLayout,
        engine: &LayoutEngine,
        cx: &mut ElementContext,
    ) {
        let child = engine.layout_bounds(layout.child);
        let placed = place_popover(
            bounds,
            (child.width, child.height),
            self.viewport,
            self.side,
            self.gap,
        );
        // The child is laid out at the anchor's origin (see `request_layout`
        // and `popover_panel`), so the move is relative to `bounds`.
        layout.offset = (placed.x - bounds.x, placed.y - bounds.y);
        self.child
            .prepaint_with_offset(engine, cx, layout.offset.0, layout.offset.1);
    }

    fn paint(
        &mut self,
        _bounds: Bounds,
        layout: &mut AnchoredLayout,
        _prepaint: &mut (),
        engine: &LayoutEngine,
        scene: &mut Scene,
        cx: &mut ElementContext,
    ) {
        self.child
            .paint_with_offset(engine, scene, cx, layout.offset.0, layout.offset.1);
    }
}

impl IntoAnyElement for Anchored {
    fn into_any(self) -> AnyElement {
        AnyElement::new(self)
    }
}

/// The elevated panel a popover draws, at the top-left of its containing
/// block: wrap it in [`anchored`] to place it, or position it yourself.
pub fn popover_panel(theme: &Theme) -> Div {
    let tc = &theme.colors;
    let m = &theme.metrics;
    div()
        .absolute()
        .left(0.0)
        .top(0.0)
        .z_index(200)
        .flex_col()
        .test_id("popover")
        .semantic_role(SemanticRole::Group)
        .focus_scope("popover")
        .key_context("popover")
        .bg(tc.elevated_surface)
        .border(tc.border)
        .rounded(m.panel_radius)
        .shadow_preset(Shadow::POPOVER)
}

pub fn popover_section() -> Div {
    div().flex_col().w_full()
}

pub fn popover_divider(theme: &Theme) -> Div {
    let tc = &theme.colors;
    let scale = theme.metrics.ui_scale();
    div()
        .w_full()
        .py((Sp::XS * scale).round())
        .child(div().w_full().h(Sz::SEPARATOR_W).bg(tc.border_variant))
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

    // Each row: anchor, popover size, preferred side, expected x, y, side.
    // Viewport 400x300, gap 4.
    #[test]
    fn popovers_flip_away_from_edges_and_clamp_inside_the_viewport() {
        use PopoverSide::*;
        let cases = [
            // Fits below: stays below, left-aligned with the anchor.
            (rect(10.0, 10.0, 100.0, 20.0), (120.0, 80.0), Bottom, (10.0, 34.0, Bottom)),
            // Near the bottom edge: flips above.
            (rect(10.0, 250.0, 100.0, 20.0), (120.0, 80.0), Bottom, (10.0, 166.0, Top)),
            // Near the top edge, preferring top: flips below.
            (rect(10.0, 20.0, 100.0, 20.0), (120.0, 80.0), Top, (10.0, 44.0, Bottom)),
            // Too tall for either side: stays on the roomier side, clamped.
            (rect(10.0, 100.0, 100.0, 20.0), (120.0, 280.0), Bottom, (10.0, 20.0, Bottom)),
            // Taller than the viewport: pinned to the top edge.
            (rect(10.0, 100.0, 100.0, 20.0), (120.0, 500.0), Bottom, (10.0, 0.0, Bottom)),
            // Near the right edge: slides left to stay inside.
            (rect(350.0, 10.0, 40.0, 20.0), (120.0, 80.0), Bottom, (280.0, 34.0, Bottom)),
            // Preferring right at the right edge: flips left.
            (rect(330.0, 10.0, 40.0, 20.0), (120.0, 80.0), Right, (206.0, 10.0, Left)),
        ];
        for (anchor, size, side, expected) in cases {
            let placed = place_popover(anchor, size, (400.0, 300.0), side, 4.0);
            assert_eq!(
                (placed.x, placed.y, placed.side),
                expected,
                "anchor {anchor:?} size {size:?} preferring {side:?}"
            );
        }
    }
}
