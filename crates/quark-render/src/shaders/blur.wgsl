// Separable Gaussian blur: up to 25 taps reaching three sigma.

@group(1) @binding(0)
var t_source: texture_2d<f32>;
@group(1) @binding(1)
var s_source: sampler;

struct VertexInput {
    @builtin(vertex_index) vertex_id: u32,
    @location(0) bounds: vec4<f32>,       // [x, y, w, h] in pixel coords
    @location(1) uv_rect: vec4<f32>,      // [u_min, v_min, u_max, v_max]
    @location(2) blur_params: vec4<f32>,  // [dir_x, dir_y, sigma, source space]
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) blur_params: vec4<f32>,
};

@vertex
fn vs_blur(input: VertexInput) -> VertexOutput {
    let unit = quad_corner(input.vertex_id);
    var out: VertexOutput;
    out.position = clip_position(input.bounds.xy + unit * input.bounds.zw);
    out.uv = mix(input.uv_rect.xy, input.uv_rect.zw, unit);
    out.blur_params = input.blur_params;
    return out;
}

const BLUR_TAPS: i32 = 12;

fn tap(uv: vec2<f32>, space: f32) -> vec4<f32> {
    return to_target_space(textureSample(t_source, s_source, uv), space, false);
}

@fragment
fn fs_blur(input: VertexOutput) -> @location(0) vec4<f32> {
    let sigma = input.blur_params.z;
    let dir = vec2<f32>(input.blur_params.x, input.blur_params.y);
    let tex_size = vec2<f32>(textureDimensions(t_source));
    // BLUR_TAPS samples each side reach three sigma: one texel apart for
    // small sigma, spread apart for large. Each weight is the Gaussian at
    // the sample's actual distance, so spreading samples keeps the
    // kernel's width. `blur_reach` in the renderer must match.
    // Small blurs reach three sigma in fewer taps.
    let taps = min(BLUR_TAPS, i32(ceil(3.0 * sigma)));
    let step = max(1.0, 3.0 * sigma / f32(BLUR_TAPS));
    let texel = dir / tex_size * step;
    // The blur writes linear light, so an encoded source decodes first.
    let space = input.blur_params.w;
    var color = tap(input.uv, space);
    var total_weight = 1.0;
    for (var k = 1; k <= taps; k = k + 1) {
        let d = f32(k) * step;
        let w = exp(-(d * d) / (2.0 * sigma * sigma));
        color += (tap(input.uv - texel * f32(k), space) + tap(input.uv + texel * f32(k), space)) * w;
        total_weight += 2.0 * w;
    }
    return color / total_weight;
}
