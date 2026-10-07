use super::*;

// ---------------------------------------------------------------------------
// render_element — top-level entry point
// ---------------------------------------------------------------------------

/// Lay out, prepaint, hit-test, and paint an element tree into the given scene.
/// Returns the hit regions accumulated during paint.
pub fn render_element(
    root: &mut AnyElement,
    scene: &mut Scene,
    cx: &mut ElementContext,
    width: f32,
    height: f32,
) {
    render_element_at(root, scene, cx, 0.0, 0.0, width, height);
}

pub fn render_element_at(
    root: &mut AnyElement,
    scene: &mut Scene,
    cx: &mut ElementContext,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
) {
    #[cfg(feature = "devtools")]
    let started = std::time::Instant::now();
    let mut engine = cache::begin_frame(cx);
    {
        profile_scope!("layout");
        layout_root(root, &mut engine, cx, width, height);
    }
    #[cfg(feature = "devtools")]
    let laid_out = std::time::Instant::now();
    {
        profile_scope!("paint");
        // Thread the offset through the element lifecycle so bounds are
        // resolved to global coordinates before any hitbox / hit region /
        // scene primitive is recorded. Running the hit test post-prepaint
        // then sees globally-positioned hitboxes, matching the global mouse
        // position.
        root.prepaint_with_offset(&engine, cx, x, y);
        cx.run_hit_test();
        root.paint_with_offset(&engine, scene, cx, x, y);
    }
    cache::end_frame(cx, engine);
    #[cfg(feature = "devtools")]
    cx.devtools.record_phases(started, laid_out);
}

/// Lay out `root`. A replayed cache boundary asked a measure query it has
/// no answer for (its parent offers a new size) goes stale: its entry is
/// dropped and the pass runs again, rebuilding it. The last pass replays
/// nothing, so this always ends.
fn layout_root(
    root: &mut AnyElement,
    engine: &mut LayoutEngine,
    cx: &mut ElementContext,
    width: f32,
    height: f32,
) {
    let mut stale = Vec::new();
    for pass in 0..cache::LAYOUT_PASSES {
        cache::begin_layout_pass(cx, pass);
        let root_id = root.request_layout(engine, cx);
        engine.compute_layout(root_id, width, height);
        engine.drain_stale(&mut stale);
        if stale.is_empty() {
            return;
        }
        cache::invalidate(cx, &stale);
        stale.clear();
        engine.clear();
    }
}
