// The target being drawn, at group 0 of every quad pipeline, and the
// vertex math every instanced quad shares.

struct ViewportUniform {
    resolution: vec2<f32>,
    time: f32,
    // 1 when the target holds encoded sRGB values (web-compatible
    // compositing), 0 when it blends in linear light.
    encoded: f32,
};

@group(0) @binding(0)
var<uniform> viewport: ViewportUniform;

// The corner of its quad that triangle-strip vertex `id` draws: (0, 0) top
// left to (1, 1) bottom right.
fn quad_corner(id: u32) -> vec2<f32> {
    return vec2<f32>(f32(id & 1u), f32((id >> 1u) & 1u));
}

// Target pixel `p` in clip space.
fn clip_position(p: vec2<f32>) -> vec4<f32> {
    let ndc = p / viewport.resolution * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0);
    return vec4<f32>(ndc, 0.0, 1.0);
}

// Colors arrive as straight-alpha sRGB-encoded channels in 0..1; a linear
// target blends them decoded.
fn to_target(c: vec4<f32>) -> vec4<f32> {
    if (viewport.encoded > 0.5) {
        return c;
    }
    return vec4<f32>(srgb_decode(c.rgb), c.a);
}
