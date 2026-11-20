// A premultiplied sample from a texture in space `source` (0 the target's
// own, 1 linear light, 2 encoded sRGB), premultiplied in the target's
// space: unpremultiplied, converted, and premultiplied again. Fully
// transparent samples are zero.
fn to_target_space(c: vec4<f32>, source: f32, target_encoded: bool) -> vec4<f32> {
    if (source < 0.5) {
        return c;
    }
    if (c.a <= 0.0) {
        return vec4<f32>(0.0);
    }
    let straight = c.rgb / c.a;
    if (source < 1.5 && target_encoded) {
        return vec4<f32>(srgb_encode(straight) * c.a, c.a);
    }
    if (source > 1.5 && !target_encoded) {
        return vec4<f32>(srgb_decode(straight) * c.a, c.a);
    }
    return c;
}
