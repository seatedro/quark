// Rounded rectangles: corner radii are [top left, top right, bottom right,
// bottom left], and rects [x, y, width, height].

// Signed distance from `p` to the rounded rect centered at the origin with
// `half_size` and corner `radius`; negative inside.
fn rounded_rect_sdf(p: vec2<f32>, half_size: vec2<f32>, radius: f32) -> f32 {
    let q = abs(p) - half_size + vec2<f32>(radius);
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - radius;
}

// The radius of the corner of `radii` in the quadrant of `p`, a point
// relative to the rect's center.
fn pick_corner_radius(p: vec2<f32>, radii: vec4<f32>) -> f32 {
    if (p.x < 0.0) {
        return select(radii.w, radii.x, p.y < 0.0);
    } else {
        return select(radii.z, radii.y, p.y < 0.0);
    }
}

// Antialiased coverage of target pixel `pixel` by the rect `bounds` with
// corner `radii`. Instances without a rounded clip give no positive radius,
// which covers everything.
fn rounded_clip_alpha(pixel: vec2<f32>, bounds: vec4<f32>, radii: vec4<f32>) -> f32 {
    if (max(max(radii.x, radii.y), max(radii.z, radii.w)) <= 0.0) {
        return 1.0;
    }
    let half_size = bounds.zw * 0.5;
    let p = pixel - (bounds.xy + half_size);
    return saturate(0.5 - rounded_rect_sdf(p, half_size, pick_corner_radius(p, radii)));
}
