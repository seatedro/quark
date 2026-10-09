//! Native material regions (G3): parts of a window that ask the platform
//! for a backdrop material (a vibrant sidebar, a titlebar) behind the app's
//! own transparent pixels.
//!
//! A div asks with [`Div::material`]. Painting it records a
//! [`MaterialRegionRequest`] on the frame ([`InputFrame::material_regions`]);
//! the host forwards the frame's requests to the native window, which
//! places input-transparent effect views without accessibility nodes. The
//! div paints its own content as usual: leave its background transparent
//! where the material should show, and paint the material's opaque
//! fallback when the platform reports none.

use super::*;
pub use quark::scene::MaterialKind;

/// One region of a frame asking for a native material.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MaterialRegionRequest {
    /// Stable across frames: a hash of the div's id, test id, or key, or of
    /// its order among the frame's regions.
    pub id: u64,
    /// Window coordinates in logical points, after the ancestors' clips.
    pub rect: Rect,
    /// The div's largest corner radius, in logical points.
    pub corner_radius: f32,
    pub kind: MaterialKind,
}

impl ElementContext<'_> {
    /// Ask for `kind` behind `rect` (layout coordinates) this frame, under
    /// the current clips. Native regions are axis-aligned: under a rotation
    /// or scale the request is dropped (the div's fallback paint shows), as
    /// it is when the clips hide it. `id` names it stably; `None` numbers it
    /// among the frame's unnamed regions.
    pub fn add_material_region(
        &mut self,
        kind: MaterialKind,
        rect: Rect,
        corner_radius: f32,
        id: Option<u64>,
    ) {
        // Requests are not recorded by cache boundaries, so a boundary
        // around one paints fresh each frame.
        self.mark_volatile();
        let unnamed = self.material_regions.len() as u64;
        let Some(rect) = self.window_rect_if_untransformed(rect) else {
            return;
        };
        self.material_regions.push(MaterialRegionRequest {
            id: id.unwrap_or_else(|| quark::stable_hash("material") ^ unnamed),
            rect,
            corner_radius,
            kind,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::Styled;

    /// The material regions of `root` painted in a 400x300 window, twice
    /// through one element cache (so a cached subtree replays): the
    /// second frame's.
    fn regions(root: impl Fn() -> AnyElement) -> Vec<MaterialRegionRequest> {
        let mut text = TextSystem::vendored_only(&Default::default());
        let mut layouts = LayoutCache::default();
        let theme = Theme::default_dark();
        let signals = SignalStore::new();
        let mut cache = ElementCache::new();
        let mut frame = None;
        for _ in 0..2 {
            let mut cx = ElementContext::new(&theme, 1.0, &mut text, &mut layouts, None, &signals)
                .with_element_cache(&mut cache);
            render_element(&mut root(), &mut Scene::default(), &mut cx, 400.0, 300.0);
            frame = Some(cx.take_input_frame());
        }
        frame.expect("two frames").material_regions
    }

    /// A 200x100 clipping pane at (10, 20) holding `child` 30 points in.
    fn pane(child: Div) -> AnyElement {
        div()
            .w(400.0)
            .h(300.0)
            .pl(10.0)
            .pt(20.0)
            .child(
                div()
                    .w(200.0)
                    .h(100.0)
                    .pl(30.0)
                    .overflow_hidden()
                    .child(child),
            )
            .into_any()
    }

    fn sidebar() -> Div {
        div()
            .w(50.0)
            .h(300.0)
            .flex_shrink_0()
            .rounded(6.0)
            .test_id("sidebar")
            .material(MaterialKind::Sidebar)
    }

    // The request is in window points, clipped by the pane, named by the
    // div's test id; a cache boundary around it still reports it on the
    // frame it replays.
    #[test]
    fn material_region_reports_its_clipped_window_rect_every_frame() {
        let expected = MaterialRegionRequest {
            id: quark::stable_hash("sidebar"),
            rect: Rect {
                x: 40.0,
                y: 20.0,
                width: 50.0,
                height: 100.0,
            },
            corner_radius: 6.0,
            kind: MaterialKind::Sidebar,
        };
        assert_eq!(regions(|| pane(sidebar())), [expected]);
        let cached_pane = || pane(div().child(cached("pane", 0, sidebar)));
        assert_eq!(regions(cached_pane), [expected], "replayed");
    }

    #[test]
    fn rotated_material_div_asks_for_no_region() {
        assert_eq!(regions(|| pane(sidebar().rotate(0.2))), []);
    }
}
