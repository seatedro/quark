// Textured quads: images, blurred regions, and offscreen targets copied
// into their parents.

@group(1) @binding(0)
var t_source: texture_2d<f32>;
@group(1) @binding(1)
var s_source: sampler;

struct VertexInput {
    @builtin(vertex_index) vertex_id: u32,
    @location(0) bounds: vec4<f32>,    // screen-space destination [x, y, w, h]
    @location(1) uv_rect: vec4<f32>,   // source UV [u_min, v_min, u_max, v_max]
    @location(2) tint: vec4<f32>,      // tint/opacity
    @location(3) radii: vec4<f32>,     // rounded mask [tl, tr, br, bl] over bounds
    @location(4) space: vec4<f32>,     // [source space, 0, 0, 0]
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) tint: vec4<f32>,
    @location(2) @interpolate(flat) bounds: vec4<f32>,
    @location(3) @interpolate(flat) radii: vec4<f32>,
    @location(4) @interpolate(flat) space: f32,
};

@vertex
fn vs_blit(input: VertexInput) -> VertexOutput {
    let unit = quad_corner(input.vertex_id);
    var out: VertexOutput;
    out.position = clip_position(input.bounds.xy + unit * input.bounds.zw);
    // Interpolate UV from uv_rect min→max.
    out.uv = mix(input.uv_rect.xy, input.uv_rect.zw, unit);
    out.tint = input.tint;
    out.bounds = input.bounds;
    out.radii = input.radii;
    out.space = input.space.x;
    return out;
}

@fragment
fn fs_blit(input: VertexOutput) -> @location(0) vec4<f32> {
    let tex_color = to_target_space(
        textureSample(t_source, s_source, input.uv),
        input.space,
        viewport.encoded > 0.5,
    );
    // No corner of the mask is rounder than half the quad's shorter side.
    let half_side = min(input.bounds.z, input.bounds.w) * 0.5;
    let radii = min(input.radii, vec4<f32>(half_side));
    let mask = rounded_clip_alpha(input.position.xy, input.bounds, radii);
    return tex_color * input.tint * mask;
}
