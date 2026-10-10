pub(super) const SHADOW_SHADER: &str = r#"
struct ViewportUniform {
    resolution: vec2<f32>,
    time: f32,
    // 1 when the target holds encoded sRGB values (web-compatible
    // compositing), 0 when it blends in linear light.
    encoded: f32,
};

@group(0) @binding(0)
var<uniform> viewport: ViewportUniform;

// Colors arrive as straight-alpha sRGB-encoded channels in 0..1; a linear
// target blends them decoded.
fn to_target(c: vec4<f32>) -> vec4<f32> {
    if (viewport.encoded > 0.5) {
        return c;
    }
    let lo = c.rgb / 12.92;
    let hi = pow((c.rgb + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
    return vec4<f32>(select(hi, lo, c.rgb <= vec3<f32>(0.04045)), c.a);
}

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
    let unit = vec2<f32>(
        f32(input.vertex_id & 1u),
        f32((input.vertex_id >> 1u) & 1u)
    );
    let pixel_pos = input.draw_bounds.xy + unit * input.draw_bounds.zw;
    let ndc = pixel_pos / viewport.resolution * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0);

    var out: VertexOutput;
    out.position = vec4<f32>(ndc, 0.0, 1.0);
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

// Rounded-rect SDF (distance from point p to the rounded rect centered at
// origin with given half_size and corner_radius).
fn rounded_rect_sdf(p: vec2<f32>, half_size: vec2<f32>, radius: f32) -> f32 {
    let q = abs(p) - half_size + vec2<f32>(radius);
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - radius;
}

fn shadow_pick_corner_radius(p: vec2<f32>, radii: vec4<f32>) -> f32 {
    if (p.x < 0.0) {
        return select(radii.w, radii.x, p.y < 0.0);
    } else {
        return select(radii.z, radii.y, p.y < 0.0);
    }
}

fn shadow_clip_alpha(
    pixel_pos: vec2<f32>,
    clip_bounds: vec4<f32>,
    clip_radii: vec4<f32>,
) -> f32 {
    if (clip_radii.x <= 0.0 && clip_radii.y <= 0.0 && clip_radii.z <= 0.0 && clip_radii.w <= 0.0) {
        return 1.0;
    }
    let clip_half = clip_bounds.zw * 0.5;
    let clip_center = clip_bounds.xy + clip_half;
    let cp = pixel_pos - clip_center;
    let cr = shadow_pick_corner_radius(cp, clip_radii);
    let clip_sdf = rounded_rect_sdf(cp, clip_half, cr);
    return saturate(0.5 - clip_sdf);
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

    alpha = alpha * shadow_clip_alpha(input.position.xy, input.clip_bounds, input.clip_radii);

    let final_alpha = input.color.a * alpha;
    if (final_alpha < 0.001) {
        discard;
    }
    return vec4<f32>(input.color.rgb * final_alpha, final_alpha);
}
"#;

// ---------------------------------------------------------------------------
// SDF quad shader
// ---------------------------------------------------------------------------

pub(super) const QUAD_SHADER: &str = r#"
struct ViewportUniform {
    resolution: vec2<f32>,
    time: f32,
    // 1 when the target holds encoded sRGB values (web-compatible
    // compositing), 0 when it blends in linear light.
    encoded: f32,
};

@group(0) @binding(0)
var<uniform> viewport: ViewportUniform;

// Colors arrive as straight-alpha sRGB-encoded channels in 0..1; a linear
// target blends them decoded.
fn to_target(c: vec4<f32>) -> vec4<f32> {
    if (viewport.encoded > 0.5) {
        return c;
    }
    let lo = c.rgb / 12.92;
    let hi = pow((c.rgb + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
    return vec4<f32>(select(hi, lo, c.rgb <= vec3<f32>(0.04045)), c.a);
}

struct VertexInput {
    @builtin(vertex_index) vertex_id: u32,
    @location(0) bounds: vec4<f32>,
    @location(1) background: vec4<f32>,
    @location(2) border_color: vec4<f32>,
    @location(3) corner_radii: vec4<f32>,
    @location(4) border_widths: vec4<f32>,
    @location(5) clip_bounds: vec4<f32>,
    @location(6) clip_radii: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) bounds: vec4<f32>,
    @location(1) @interpolate(flat) background: vec4<f32>,
    @location(2) @interpolate(flat) border_color: vec4<f32>,
    @location(3) @interpolate(flat) corner_radii: vec4<f32>,
    @location(4) @interpolate(flat) border_widths: vec4<f32>,
    @location(5) @interpolate(flat) clip_bounds: vec4<f32>,
    @location(6) @interpolate(flat) clip_radii: vec4<f32>,
};

@vertex
fn vs_quad(input: VertexInput) -> VertexOutput {
    let unit = vec2<f32>(
        f32(input.vertex_id & 1u),
        f32((input.vertex_id >> 1u) & 1u)
    );
    let pixel_pos = input.bounds.xy + unit * input.bounds.zw;
    let ndc = pixel_pos / viewport.resolution * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0);

    var out: VertexOutput;
    out.position = vec4<f32>(ndc, 0.0, 1.0);
    out.bounds = input.bounds;
    out.background = to_target(input.background);
    out.border_color = to_target(input.border_color);
    out.corner_radii = input.corner_radii;
    out.border_widths = input.border_widths;
    out.clip_bounds = input.clip_bounds;
    out.clip_radii = input.clip_radii;
    return out;
}

fn pick_corner_radius(p: vec2<f32>, radii: vec4<f32>) -> f32 {
    // radii: tl, tr, br, bl
    if (p.x < 0.0) {
        return select(radii.w, radii.x, p.y < 0.0);
    } else {
        return select(radii.z, radii.y, p.y < 0.0);
    }
}

fn quad_sdf(p: vec2<f32>, half_size: vec2<f32>, radius: f32) -> f32 {
    let d = abs(p) - half_size + vec2<f32>(radius);
    return length(max(d, vec2<f32>(0.0))) + min(max(d.x, d.y), 0.0) - radius;
}

fn over(below: vec4<f32>, above: vec4<f32>) -> vec4<f32> {
    let a = above.a + below.a * (1.0 - above.a);
    if (a <= 0.0) {
        return vec4<f32>(0.0);
    }
    let c = (above.rgb * above.a + below.rgb * below.a * (1.0 - above.a)) / a;
    return vec4<f32>(c, a);
}

// Anti-aliased rounded-clip alpha. Returns 1.0 when no rounded clip is
// active (all radii zero), otherwise fades with the clip-rect SDF.
fn rounded_clip_alpha(
    pixel_pos: vec2<f32>,
    clip_bounds: vec4<f32>,
    clip_radii: vec4<f32>,
) -> f32 {
    if (clip_radii.x <= 0.0 && clip_radii.y <= 0.0 && clip_radii.z <= 0.0 && clip_radii.w <= 0.0) {
        return 1.0;
    }
    let clip_half = clip_bounds.zw * 0.5;
    let clip_center = clip_bounds.xy + clip_half;
    let cp = pixel_pos - clip_center;
    let cr = pick_corner_radius(cp, clip_radii);
    let clip_sdf = quad_sdf(cp, clip_half, cr);
    return saturate(0.5 - clip_sdf);
}

@fragment
fn fs_quad(input: VertexOutput) -> @location(0) vec4<f32> {
    let half_size = input.bounds.zw * 0.5;
    let center = input.bounds.xy + half_size;
    let p = input.position.xy - center;

    let corner_radius = pick_corner_radius(p, input.corner_radii);
    let outer_sdf = quad_sdf(p, half_size, corner_radius);

    let aa = 0.5;
    let outer_alpha = saturate(aa - outer_sdf);
    if (outer_alpha <= 0.0) {
        discard;
    }

    let clip_alpha = rounded_clip_alpha(input.position.xy, input.clip_bounds, input.clip_radii);
    if (clip_alpha <= 0.0) {
        discard;
    }

    // border_widths: top, right, bottom, left. The fill region is the outer
    // rect inset by each side's width; each inner corner radius shrinks by
    // the wider of its two adjacent sides (a circular stand-in for CSS's
    // elliptical inner corner).
    let bw = input.border_widths;
    // Premultiplied: a border quad's fill is transparent, and blending
    // toward it in straight alpha would darken the border's color along
    // its anti-aliased inner edge, a grey fringe inside rounded corners.
    let background = vec4<f32>(input.background.rgb * input.background.a, input.background.a);
    var color: vec4<f32>;
    if (max(max(bw.x, bw.y), max(bw.z, bw.w)) > 0.0) {
        let inner_min = input.bounds.xy + vec2<f32>(bw.w, bw.x);
        let inner_max = input.bounds.xy + input.bounds.zw - vec2<f32>(bw.y, bw.z);
        let inner_size = inner_max - inner_min;
        var fill_blend = 0.0;
        if (inner_size.x > 0.0 && inner_size.y > 0.0) {
            let inner_half = inner_size * 0.5;
            let ip = input.position.xy - (inner_min + inner_half);
            let inner_radii = max(
                vec4<f32>(0.0),
                input.corner_radii - vec4<f32>(
                    max(bw.x, bw.w),
                    max(bw.x, bw.y),
                    max(bw.z, bw.y),
                    max(bw.z, bw.w),
                ),
            );
            let inner_radius = min(
                pick_corner_radius(ip, inner_radii),
                min(inner_half.x, inner_half.y),
            );
            fill_blend = saturate(aa - quad_sdf(ip, inner_half, inner_radius));
        }
        let blended = over(input.background, input.border_color);
        let border = vec4<f32>(blended.rgb * blended.a, blended.a);
        color = mix(border, background, fill_blend);
    } else {
        color = background;
    }

    return color * (outer_alpha * clip_alpha);
}
"#;

// ---------------------------------------------------------------------------
// Procedural effect shader — noise gradient + linear gradient
// ---------------------------------------------------------------------------

pub(super) const EFFECT_SHADER: &str = r#"
struct ViewportUniform {
    resolution: vec2<f32>,
    time: f32,
    // 1 when the target holds encoded sRGB values (web-compatible
    // compositing), 0 when it blends in linear light.
    encoded: f32,
};

@group(0) @binding(0)
var<uniform> viewport: ViewportUniform;

// Colors arrive as straight-alpha sRGB-encoded channels in 0..1; a linear
// target blends them decoded.
fn to_target(c: vec4<f32>) -> vec4<f32> {
    if (viewport.encoded > 0.5) {
        return c;
    }
    let lo = c.rgb / 12.92;
    let hi = pow((c.rgb + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
    return vec4<f32>(select(hi, lo, c.rgb <= vec3<f32>(0.04045)), c.a);
}

struct VertexInput {
    @builtin(vertex_index) vertex_id: u32,
    @location(0) bounds: vec4<f32>,
    @location(1) color_a: vec4<f32>,
    @location(2) color_b: vec4<f32>,
    @location(3) params: vec4<f32>,   // [effect_type, param1, param2, corner_radius]
    @location(4) clip_bounds: vec4<f32>,
    @location(5) clip_radii: vec4<f32>,
    @location(6) extra: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) bounds: vec4<f32>,
    @location(1) @interpolate(flat) color_a: vec4<f32>,
    @location(2) @interpolate(flat) color_b: vec4<f32>,
    @location(3) @interpolate(flat) params: vec4<f32>,
    @location(4) @interpolate(flat) clip_bounds: vec4<f32>,
    @location(5) @interpolate(flat) clip_radii: vec4<f32>,
    @location(6) @interpolate(flat) extra: vec4<f32>,
};

@vertex
fn vs_effect(input: VertexInput) -> VertexOutput {
    let unit = vec2<f32>(
        f32(input.vertex_id & 1u),
        f32((input.vertex_id >> 1u) & 1u)
    );
    let pixel_pos = input.bounds.xy + unit * input.bounds.zw;
    let ndc = pixel_pos / viewport.resolution * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0);

    var out: VertexOutput;
    out.position = vec4<f32>(ndc, 0.0, 1.0);
    out.bounds = input.bounds;
    out.color_a = to_target(input.color_a);
    out.color_b = to_target(input.color_b);
    out.params = input.params;
    out.clip_bounds = input.clip_bounds;
    out.clip_radii = input.clip_radii;
    out.extra = input.extra;
    return out;
}

// ---- Simplex noise (2D) ----

fn mod289_v3(x: vec3<f32>) -> vec3<f32> {
    return x - floor(x * (1.0 / 289.0)) * 289.0;
}

fn mod289_v2(x: vec2<f32>) -> vec2<f32> {
    return x - floor(x * (1.0 / 289.0)) * 289.0;
}

fn permute(x: vec3<f32>) -> vec3<f32> {
    return mod289_v3(((x * 34.0) + vec3<f32>(10.0)) * x);
}

fn simplex_noise(v: vec2<f32>) -> f32 {
    let C = vec4<f32>(
        0.211324865405187,   // (3.0 - sqrt(3.0)) / 6.0
        0.366025403784439,   // 0.5 * (sqrt(3.0) - 1.0)
        -0.577350269189626,  // -1.0 + 2.0 * C.x
        0.024390243902439    // 1.0 / 41.0
    );

    // First corner.
    var i = floor(v + dot(v, C.yy));
    let x0 = v - i + dot(i, C.xx);

    // Other corners.
    let i1 = select(vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), x0.x > x0.y);
    var x12 = x0.xyxy + C.xxzz;
    x12 = vec4<f32>(x12.xy - i1, x12.zw);

    // Permutations.
    i = mod289_v2(i);
    let p = permute(permute(
        i.y + vec3<f32>(0.0, i1.y, 1.0))
      + i.x + vec3<f32>(0.0, i1.x, 1.0));

    var m = max(vec3<f32>(0.5) - vec3<f32>(
        dot(x0, x0),
        dot(x12.xy, x12.xy),
        dot(x12.zw, x12.zw)
    ), vec3<f32>(0.0));
    m = m * m;
    m = m * m;

    // Gradients.
    let x_ = 2.0 * fract(p * C.www) - vec3<f32>(1.0);
    let h = abs(x_) - vec3<f32>(0.5);
    let ox = floor(x_ + vec3<f32>(0.5));
    let a0 = x_ - ox;

    // Approximate normalisation.
    m = m * (vec3<f32>(1.79284291400159) - vec3<f32>(0.85373472095314) * (a0 * a0 + h * h));

    // Compute final noise value at P.
    let g = vec3<f32>(
        a0.x * x0.x + h.x * x0.y,
        a0.y * x12.x + h.y * x12.y,
        a0.z * x12.z + h.z * x12.w
    );

    return 130.0 * dot(m, g);
}

// ---- Rounded-rect SDF for masking ----

fn effect_sdf(p: vec2<f32>, half_size: vec2<f32>, radius: f32) -> f32 {
    let q = abs(p) - half_size + vec2<f32>(radius);
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - radius;
}

fn effect_pick_corner_radius(p: vec2<f32>, radii: vec4<f32>) -> f32 {
    if (p.x < 0.0) {
        return select(radii.w, radii.x, p.y < 0.0);
    } else {
        return select(radii.z, radii.y, p.y < 0.0);
    }
}

fn effect_clip_alpha(
    pixel_pos: vec2<f32>,
    clip_bounds: vec4<f32>,
    clip_radii: vec4<f32>,
) -> f32 {
    if (clip_radii.x <= 0.0 && clip_radii.y <= 0.0 && clip_radii.z <= 0.0 && clip_radii.w <= 0.0) {
        return 1.0;
    }
    let clip_half = clip_bounds.zw * 0.5;
    let clip_center = clip_bounds.xy + clip_half;
    let cp = pixel_pos - clip_center;
    let cr = effect_pick_corner_radius(cp, clip_radii);
    let clip_sdf = effect_sdf(cp, clip_half, cr);
    return saturate(0.5 - clip_sdf);
}

// ---- Fragment shader ----

@fragment
fn fs_effect(input: VertexOutput) -> @location(0) vec4<f32> {
    let half_size = input.bounds.zw * 0.5;
    let center = input.bounds.xy + half_size;
    let p = input.position.xy - center;
    let corner_radius = input.params.w;

    // Rounded-rect mask.
    let sdf = effect_sdf(p, half_size, corner_radius);
    let mask_self = saturate(0.5 - sdf);
    let mask_clip = effect_clip_alpha(input.position.xy, input.clip_bounds, input.clip_radii);
    let mask = mask_self * mask_clip;
    if (mask <= 0.0) {
        discard;
    }

    // Normalised UV within the element bounds.
    let uv = (input.position.xy - input.bounds.xy) / input.bounds.zw;

    let effect_type = u32(input.params.x);
    var color: vec4<f32>;

    switch (effect_type) {
        // Type 0: Noise gradient — simplex noise blended between two colors.
        case 0u: {
            let scale = input.params.y;
            let noise_coord = input.position.xy * scale + vec2<f32>(viewport.time * 3.0);
            let n = simplex_noise(noise_coord) * 0.5 + 0.5;
            // Layer a second octave for richer texture.
            let n2 = simplex_noise(noise_coord * 2.0 + vec2<f32>(17.3, 31.7)) * 0.5 + 0.5;
            let combined = n * 0.7 + n2 * 0.3;
            // Blend from color_a (top) to color_b (bottom) modulated by noise.
            let gradient = uv.y;
            let t = saturate(gradient + (combined - 0.5) * 0.4);
            color = mix(input.color_a, input.color_b, t);
        }
        // Type 1: Linear gradient with angle.
        case 1u: {
            let angle = input.params.y;
            let dir = vec2<f32>(cos(angle), sin(angle));
            let t = saturate(dot(uv - vec2<f32>(0.5), dir) + 0.5);
            color = mix(input.color_a, input.color_b, t);
        }
        // Type 2: Radial gradient — color_a at center, color_b at edge.
        case 2u: {
            let center = vec2<f32>(0.5, 0.5);
            let d = length((uv - center) * 2.0);
            let t = saturate(d);
            color = mix(input.color_a, input.color_b, t);
        }
        // Type 3: Animated shimmer — diagonal highlight sweep.
        case 3u: {
            let speed = input.params.y;
            // Diagonal position: combine x and y into a single sweep axis.
            let diag = (uv.x + uv.y) * 0.5;
            // Animate the highlight band across the diagonal.
            let phase = fract(viewport.time * speed * 0.3);
            let band_center = phase * 1.6 - 0.3; // sweep from left to right with overshoot
            let band = 1.0 - smoothstep(0.0, 0.15, abs(diag - band_center));
            color = mix(input.color_a, input.color_b, band);
        }
        // Type 4: Vignette — darken/tint edges.
        case 4u: {
            let intensity = input.params.y;
            let center = vec2<f32>(0.5, 0.5);
            let d = length((uv - center) * 2.0);
            let vignette_factor = smoothstep(0.2, 1.2, d) * intensity;
            // Start from transparent, blend toward color_a at edges.
            color = vec4<f32>(input.color_a.rgb, input.color_a.a * vignette_factor);
        }
        // Type 5: Color tint — flat semi-transparent overlay.
        case 5u: {
            color = input.color_a;
        }
        // Type 6: Stripes — bands of color_a covering `duty` of each
        // period across the stripes, color_b between, from the top-left
        // corner. Coverage is the band's overlap with the pixel along the
        // stripe normal, so edges antialias at any angle.
        case 6u: {
            let angle = input.params.y;
            let period = input.params.z;
            let duty = input.extra.x;
            let normal = vec2<f32>(cos(angle), sin(angle));
            let d = dot(input.position.xy - input.bounds.xy, normal);
            let s = d - period * floor(d / period);
            let band = period * duty;
            let coverage = saturate(s + 0.5) - saturate(s - band + 0.5)
                + saturate(s - period + 0.5);
            color = mix(input.color_b, input.color_a, saturate(coverage));
        }
        // Fallback: solid color_a.
        default: {
            color = input.color_a;
        }
    }

    let final_alpha = color.a * mask;
    if (final_alpha < 0.001) {
        discard;
    }
    return vec4<f32>(color.rgb * final_alpha, final_alpha);
}
"#;

// ---------------------------------------------------------------------------
// Blit shader — composite an offscreen texture to screen
// ---------------------------------------------------------------------------

pub(super) const BLIT_SHADER: &str = r#"
struct ViewportUniform {
    resolution: vec2<f32>,
    time: f32,
    // 1 when the target holds encoded sRGB values (web-compatible
    // compositing), 0 when it blends in linear light.
    encoded: f32,
};

@group(0) @binding(0)
var<uniform> viewport: ViewportUniform;

fn srgb_decode(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

fn srgb_encode(c: vec3<f32>) -> vec3<f32> {
    let lo = c * 12.92;
    let hi = 1.055 * pow(max(c, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.4)) - vec3<f32>(0.055);
    return select(hi, lo, c <= vec3<f32>(0.0031308));
}

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
    let unit = vec2<f32>(
        f32(input.vertex_id & 1u),
        f32((input.vertex_id >> 1u) & 1u)
    );
    let pixel_pos = input.bounds.xy + unit * input.bounds.zw;
    let ndc = pixel_pos / viewport.resolution * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0);

    // Interpolate UV from uv_rect min→max.
    let uv = mix(input.uv_rect.xy, input.uv_rect.zw, unit);

    var out: VertexOutput;
    out.position = vec4<f32>(ndc, 0.0, 1.0);
    out.uv = uv;
    out.tint = input.tint;
    out.bounds = input.bounds;
    out.radii = input.radii;
    out.space = input.space.x;
    return out;
}

fn blit_rounded_mask(pixel: vec2<f32>, bounds: vec4<f32>, radii: vec4<f32>) -> f32 {
    if (max(max(radii.x, radii.y), max(radii.z, radii.w)) <= 0.0) {
        return 1.0;
    }
    let half_size = bounds.zw * 0.5;
    let p = pixel - (bounds.xy + half_size);
    var r: f32;
    if (p.x < 0.0) {
        r = select(radii.w, radii.x, p.y < 0.0);
    } else {
        r = select(radii.z, radii.y, p.y < 0.0);
    }
    r = min(r, min(half_size.x, half_size.y));
    let q = abs(p) - half_size + vec2<f32>(r);
    let sdf = length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - r;
    return saturate(0.5 - sdf);
}

@fragment
fn fs_blit(input: VertexOutput) -> @location(0) vec4<f32> {
    let tex_color = to_target_space(
        textureSample(t_source, s_source, input.uv),
        input.space,
        viewport.encoded > 0.5,
    );
    let mask = blit_rounded_mask(input.position.xy, input.bounds, input.radii);
    return tex_color * input.tint * mask;
}
"#;

// ---------------------------------------------------------------------------
// Layer composite shader — an offscreen layer drawn through an affine map
// ---------------------------------------------------------------------------

pub(super) const LAYER_SHADER: &str = r#"
struct ViewportUniform {
    resolution: vec2<f32>,
    time: f32,
    // 1 when the target holds encoded sRGB values (web-compatible
    // compositing), 0 when it blends in linear light.
    encoded: f32,
};

@group(0) @binding(0)
var<uniform> viewport: ViewportUniform;

fn srgb_decode(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

fn srgb_encode(c: vec3<f32>) -> vec3<f32> {
    let lo = c * 12.92;
    let hi = 1.055 * pow(max(c, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.4)) - vec3<f32>(0.055);
    return select(hi, lo, c <= vec3<f32>(0.0031308));
}

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
    let unit = vec2<f32>(
        f32(input.vertex_id & 1u),
        f32((input.vertex_id >> 1u) & 1u)
    );
    let local = unit * input.offset_size.zw;
    let m = input.linear;
    let pixel_pos = vec2<f32>(
        m.x * local.x + m.z * local.y + input.offset_size.x,
        m.y * local.x + m.w * local.y + input.offset_size.y,
    );
    let ndc = pixel_pos / viewport.resolution * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0);

    var out: VertexOutput;
    out.position = vec4<f32>(ndc, 0.0, 1.0);
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

fn layer_clip_alpha(pixel: vec2<f32>, bounds: vec4<f32>, radii: vec4<f32>) -> f32 {
    if (max(max(radii.x, radii.y), max(radii.z, radii.w)) <= 0.0) {
        return 1.0;
    }
    let half_size = bounds.zw * 0.5;
    let p = pixel - (bounds.xy + half_size);
    var r: f32;
    if (p.x < 0.0) {
        r = select(radii.w, radii.x, p.y < 0.0);
    } else {
        r = select(radii.z, radii.y, p.y < 0.0);
    }
    let q = abs(p) - half_size + vec2<f32>(r);
    let sdf = length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - r;
    return saturate(0.5 - sdf);
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
    let clip = layer_clip_alpha(input.position.xy, input.clip_bounds, input.clip_radii);
    let mask = mask_alpha(input.position.xy, input.mask_axis, input.mask_offsets, input.mask_alphas);
    return color * (input.opacity_space.x * clip * mask);
}
"#;

// ---------------------------------------------------------------------------
// Path shader — winding number and edge distance over a band's segments
// ---------------------------------------------------------------------------

/// Texels per row of the segment texture; `SEGMENT_TEXTURE_WIDTH` in the
/// renderer must match.
pub(super) const PATH_SHADER: &str = r#"
struct ViewportUniform {
    resolution: vec2<f32>,
    time: f32,
    // 1 when the target holds encoded sRGB values (web-compatible
    // compositing), 0 when it blends in linear light.
    encoded: f32,
};

@group(0) @binding(0)
var<uniform> viewport: ViewportUniform;

// Colors arrive as straight-alpha sRGB-encoded channels in 0..1; a linear
// target blends them decoded.
fn to_target(c: vec4<f32>) -> vec4<f32> {
    if (viewport.encoded > 0.5) {
        return c;
    }
    let lo = c.rgb / 12.92;
    let hi = pow((c.rgb + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
    return vec4<f32>(select(hi, lo, c.rgb <= vec3<f32>(0.04045)), c.a);
}

// One segment [x0, y0, x1, y1] per texel, path-local physical pixels.
@group(1) @binding(0)
var t_segments: texture_2d<f32>;

const SEGMENT_ROW: u32 = 1024u;

struct VertexInput {
    @builtin(vertex_index) vertex_id: u32,
    @location(0) bounds: vec4<f32>,       // band quad [x, y, w, h]
    @location(1) origin_rule: vec4<f32>,  // [origin x, origin y, fill rule, 0]
    @location(2) color: vec4<f32>,
    @location(3) segments: vec4<u32>,     // [first, count, 0, 0]
    @location(4) clip_bounds: vec4<f32>,
    @location(5) clip_radii: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) origin_rule: vec4<f32>,
    @location(1) @interpolate(flat) color: vec4<f32>,
    @location(2) @interpolate(flat) segments: vec4<u32>,
    @location(3) @interpolate(flat) clip_bounds: vec4<f32>,
    @location(4) @interpolate(flat) clip_radii: vec4<f32>,
};

@vertex
fn vs_path(input: VertexInput) -> VertexOutput {
    let unit = vec2<f32>(
        f32(input.vertex_id & 1u),
        f32((input.vertex_id >> 1u) & 1u)
    );
    let pixel_pos = input.bounds.xy + unit * input.bounds.zw;
    let ndc = pixel_pos / viewport.resolution * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0);

    var out: VertexOutput;
    out.position = vec4<f32>(ndc, 0.0, 1.0);
    out.origin_rule = input.origin_rule;
    out.color = to_target(input.color);
    out.segments = input.segments;
    out.clip_bounds = input.clip_bounds;
    out.clip_radii = input.clip_radii;
    return out;
}

fn path_clip_alpha(pixel: vec2<f32>, bounds: vec4<f32>, radii: vec4<f32>) -> f32 {
    if (max(max(radii.x, radii.y), max(radii.z, radii.w)) <= 0.0) {
        return 1.0;
    }
    let half_size = bounds.zw * 0.5;
    let p = pixel - (bounds.xy + half_size);
    var r: f32;
    if (p.x < 0.0) {
        r = select(radii.w, radii.x, p.y < 0.0);
    } else {
        r = select(radii.z, radii.y, p.y < 0.0);
    }
    let q = abs(p) - half_size + vec2<f32>(r);
    let sdf = length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - r;
    return saturate(0.5 - sdf);
}

@fragment
fn fs_path(input: VertexOutput) -> @location(0) vec4<f32> {
    let p = input.position.xy - input.origin_rule.xy;
    var winding = 0;
    var dist = 1.0e9;
    let first = input.segments.x;
    let count = input.segments.y;
    for (var i = 0u; i < count; i = i + 1u) {
        let index = first + i;
        let s = textureLoad(t_segments, vec2<i32>(i32(index % SEGMENT_ROW), i32(index / SEGMENT_ROW)), 0);
        let a = s.xy;
        let ab = s.zw - a;
        let pa = p - a;
        let h = clamp(dot(pa, ab) / max(dot(ab, ab), 1.0e-12), 0.0, 1.0);
        dist = min(dist, length(pa - ab * h));
        // Crossings of the ray from p toward +x.
        if ((a.y <= p.y) != (s.w <= p.y)) {
            let x = a.x + (p.y - a.y) * ab.x / ab.y;
            if (x > p.x) {
                winding = winding + select(-1, 1, s.w > a.y);
            }
        }
    }
    var inside: bool;
    if (input.origin_rule.z > 0.5) {
        inside = (winding & 1) != 0;
    } else {
        inside = winding != 0;
    }
    let signed_dist = select(dist, -dist, inside);
    let coverage = saturate(0.5 - signed_dist)
        * path_clip_alpha(input.position.xy, input.clip_bounds, input.clip_radii);
    let alpha = input.color.a * coverage;
    if (alpha <= 0.0) {
        discard;
    }
    return vec4<f32>(input.color.rgb * alpha, alpha);
}
"#;

// ---------------------------------------------------------------------------
// Separable Gaussian blur shader — 25 taps reaching three sigma
// ---------------------------------------------------------------------------

pub(super) const BLUR_SHADER: &str = r#"
struct ViewportUniform {
    resolution: vec2<f32>,
    time: f32,
    // 1 when the target holds encoded sRGB values (web-compatible
    // compositing), 0 when it blends in linear light.
    encoded: f32,
};

@group(0) @binding(0)
var<uniform> viewport: ViewportUniform;

fn srgb_decode(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

fn srgb_encode(c: vec3<f32>) -> vec3<f32> {
    let lo = c * 12.92;
    let hi = 1.055 * pow(max(c, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.4)) - vec3<f32>(0.055);
    return select(hi, lo, c <= vec3<f32>(0.0031308));
}

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

@group(1) @binding(0)
var t_source: texture_2d<f32>;
@group(1) @binding(1)
var s_source: sampler;

struct VertexInput {
    @builtin(vertex_index) vertex_id: u32,
    @location(0) bounds: vec4<f32>,       // [x, y, w, h] in pixel coords
    @location(1) uv_rect: vec4<f32>,      // [u_min, v_min, u_max, v_max]
    @location(2) blur_params: vec4<f32>,  // [dir_x, dir_y, sigma, 0]
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) blur_params: vec4<f32>,
};

@vertex
fn vs_blur(input: VertexInput) -> VertexOutput {
    let unit = vec2<f32>(
        f32(input.vertex_id & 1u),
        f32((input.vertex_id >> 1u) & 1u)
    );
    let pixel_pos = input.bounds.xy + unit * input.bounds.zw;
    let ndc = pixel_pos / viewport.resolution * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0);

    let uv = mix(input.uv_rect.xy, input.uv_rect.zw, unit);

    var out: VertexOutput;
    out.position = vec4<f32>(ndc, 0.0, 1.0);
    out.uv = uv;
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
"#;
