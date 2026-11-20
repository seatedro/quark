// Layer composites: an offscreen layer drawn through an affine map, faded,
// clipped, and masked.

@group(1) @binding(0)
var t_layer: texture_2d<f32>;
@group(1) @binding(1)
var s_layer: sampler;

struct VertexInput {
    @builtin(vertex_index) vertex_id: u32,
    // Layer texel (u, v) lands at (a·u + c·v + tx, b·u + d·v + ty).
    @location(0) linear: vec4<f32>,        // [a, b, c, d]
    @location(1) offset_size: vec4<f32>,   // [tx, ty, layer width, layer height]
    @location(2) params: vec4<f32>,        // [1 / texture width, 1 / texture height, opacity, source space]
    @location(3) clip_bounds: vec4<f32>,
    @location(4) clip_radii: vec4<f32>,
    @location(5) mask_axis: vec4<f32>,     // [x0, y0, x1, y1] in target pixels
    @location(6) mask_offsets: vec4<f32>,
    @location(7) mask_alphas: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) opacity_space: vec2<f32>,
    @location(2) @interpolate(flat) clip_bounds: vec4<f32>,
    @location(3) @interpolate(flat) clip_radii: vec4<f32>,
    @location(4) @interpolate(flat) mask_axis: vec4<f32>,
    @location(5) @interpolate(flat) mask_offsets: vec4<f32>,
    @location(6) @interpolate(flat) mask_alphas: vec4<f32>,
};

@vertex
fn vs_layer(input: VertexInput) -> VertexOutput {
    let local = quad_corner(input.vertex_id) * input.offset_size.zw;
    let m = input.linear;
    let pixel_pos = vec2<f32>(
        m.x * local.x + m.z * local.y + input.offset_size.x,
        m.y * local.x + m.w * local.y + input.offset_size.y,
    );

    var out: VertexOutput;
    out.position = clip_position(pixel_pos);
    out.uv = local * input.params.xy;
    out.opacity_space = input.params.zw;
    out.clip_bounds = input.clip_bounds;
    out.clip_radii = input.clip_radii;
    out.mask_axis = input.mask_axis;
    out.mask_offsets = input.mask_offsets;
    out.mask_alphas = input.mask_alphas;
    return out;
}

// The alpha mask's opacity at target pixel `p`: stops interpolated along
// the axis, the first and last held beyond it.
fn mask_alpha(p: vec2<f32>, axis: vec4<f32>, offsets: vec4<f32>, alphas: vec4<f32>) -> f32 {
    let d = axis.zw - axis.xy;
    let length2 = dot(d, d);
    var t = 1.0;
    if (length2 > 0.0) {
        t = dot(p - axis.xy, d) / length2;
    }
    if (t <= offsets.x) {
        return alphas.x;
    }
    if (t <= offsets.y) {
        return mix(alphas.x, alphas.y, (t - offsets.x) / max(offsets.y - offsets.x, 1.0e-6));
    }
    if (t <= offsets.z) {
        return mix(alphas.y, alphas.z, (t - offsets.y) / max(offsets.z - offsets.y, 1.0e-6));
    }
    if (t <= offsets.w) {
        return mix(alphas.z, alphas.w, (t - offsets.z) / max(offsets.w - offsets.z, 1.0e-6));
    }
    return alphas.w;
}

@fragment
fn fs_layer(input: VertexOutput) -> @location(0) vec4<f32> {
    // The layer holds premultiplied color with a transparent border, so
    // bilinear sampling antialiases the edges of a rotated layer.
    let color = to_target_space(
        textureSample(t_layer, s_layer, input.uv),
        input.opacity_space.y,
        viewport.encoded > 0.5,
    );
    let clip = rounded_clip_alpha(input.position.xy, input.clip_bounds, input.clip_radii);
    let mask = mask_alpha(input.position.xy, input.mask_axis, input.mask_offsets, input.mask_alphas);
    return color * (input.opacity_space.x * clip * mask);
}
