// Blurred rounded-rect shadows.

struct VertexInput {
    @builtin(vertex_index) vertex_id: u32,
    @location(0) draw_bounds: vec4<f32>,
    @location(1) shadow_bounds: vec4<f32>,
    @location(2) color: vec4<f32>,
    @location(3) params: vec4<f32>,
    @location(4) clip_bounds: vec4<f32>,
    @location(5) clip_radii: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) shadow_bounds: vec4<f32>,
    @location(1) @interpolate(flat) color: vec4<f32>,
    @location(2) @interpolate(flat) params: vec4<f32>,
    @location(3) @interpolate(flat) clip_bounds: vec4<f32>,
    @location(4) @interpolate(flat) clip_radii: vec4<f32>,
};

@vertex
fn vs_shadow(input: VertexInput) -> VertexOutput {
    let unit = quad_corner(input.vertex_id);
    var out: VertexOutput;
    out.position = clip_position(input.draw_bounds.xy + unit * input.draw_bounds.zw);
    out.shadow_bounds = input.shadow_bounds;
    out.color = to_target(input.color);
    out.params = input.params;
    out.clip_bounds = input.clip_bounds;
    out.clip_radii = input.clip_radii;
    return out;
}

// Attempt to approximate erf using a polynomial fit.
// Abramowitz & Stegun 7.1.26 — max error < 1.5e-7, which is more than
// enough for visual blur.
fn erf_approx(x: f32) -> f32 {
    let sign = select(-1.0, 1.0, x >= 0.0);
    let a = abs(x);
    let t = 1.0 / (1.0 + 0.3275911 * a);
    let t2 = t * t;
    let t3 = t2 * t;
    let t4 = t3 * t;
    let t5 = t4 * t;
    let poly = 0.254829592 * t - 0.284496736 * t2 + 1.421413741 * t3
             - 1.453152027 * t4 + 1.061405429 * t5;
    return sign * (1.0 - poly * exp(-a * a));
}

// Integral of 1D Gaussian from -inf to x with given sigma.
fn gauss_integral(x: f32, sigma: f32) -> f32 {
    return 0.5 + 0.5 * erf_approx(x / (sigma * 1.4142135));
}

@fragment
fn fs_shadow(input: VertexOutput) -> @location(0) vec4<f32> {
    let sigma = input.params.x;
    let corner_radius = input.params.y;
    let half_size = input.shadow_bounds.zw * 0.5;
    let center = input.shadow_bounds.xy + half_size;
    let p = input.position.xy - center;

    // For the blurred shadow, we compute the convolution of the rounded-rect
    // indicator function with a 2D Gaussian. For a box (no rounding), this
    // factors into the product of two 1D Gaussian integrals. For rounded
    // corners we use a hybrid: compute the box integral and multiply by a
    // smooth SDF-based corner correction.

    // Separable box blur integral.
    let ax = gauss_integral(p.x + half_size.x, sigma)
           - gauss_integral(p.x - half_size.x, sigma);
    let ay = gauss_integral(p.y + half_size.y, sigma)
           - gauss_integral(p.y - half_size.y, sigma);
    var alpha = ax * ay;

    // Corner correction: fade out the corners that the box integral
    // over-estimates. We sample the SDF and use the sigma to smooth it.
    if (corner_radius > 0.0) {
        let sdf = rounded_rect_sdf(p, half_size, corner_radius);
        // Outside the rounded rect, attenuate based on how far outside.
        // The smoothstep range is proportional to sigma for a soft edge.
        let corner_fade = 1.0 - smoothstep(-sigma * 0.5, sigma * 1.5, sdf);
        alpha = alpha * corner_fade;
    }

    alpha = alpha * rounded_clip_alpha(input.position.xy, input.clip_bounds, input.clip_radii);

    let final_alpha = input.color.a * alpha;
    if (final_alpha < 0.001) {
        discard;
    }
    return vec4<f32>(input.color.rgb * final_alpha, final_alpha);
}
