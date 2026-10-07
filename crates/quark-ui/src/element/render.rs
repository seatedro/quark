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
    #[cfg(feature = "devtools")]
    let started = std::time::Instant::now();
    let mut engine = LayoutEngine::new();
    {
        profile_scope!("layout");
        let root_id = root.request_layout(&mut engine, cx);
        engine.compute_layout(root_id, width, height);
    }
    #[cfg(feature = "devtools")]
    let laid_out = std::time::Instant::now();
    {
        profile_scope!("paint");
        root.prepaint(&engine, cx);
        cx.run_hit_test();
        root.paint(&engine, scene, cx);
    }
    #[cfg(feature = "devtools")]
    cx.devtools.record_phases(started, laid_out);
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
    let mut engine = LayoutEngine::new();
    let root_id = root.request_layout(&mut engine, cx);
    engine.compute_layout(root_id, width, height);
    // Thread the offset through the element lifecycle so bounds are resolved
    // to global coordinates before any hitbox / hit region / scene primitive
    // is recorded. Running the hit test post-prepaint then sees globally-
    // positioned hitboxes, matching the global mouse position.
    root.prepaint_with_offset(&engine, cx, x, y);
    cx.run_hit_test();
    root.paint_with_offset(&engine, scene, cx, x, y);
}
