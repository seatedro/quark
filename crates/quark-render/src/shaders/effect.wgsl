// Procedural effect quads: noise and linear gradients, radial gradients,
// shimmer, vignette, tint, and stripes.

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
    let unit = quad_corner(input.vertex_id);
    var out: VertexOutput;
    out.position = clip_position(input.bounds.xy + unit * input.bounds.zw);
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

// ---- Fragment shader ----

@fragment
fn fs_effect(input: VertexOutput) -> @location(0) vec4<f32> {
    let half_size = input.bounds.zw * 0.5;
    let center = input.bounds.xy + half_size;
    let p = input.position.xy - center;
    let corner_radius = input.params.w;

    // Rounded-rect mask.
    let sdf = rounded_rect_sdf(p, half_size, corner_radius);
    let mask_self = saturate(0.5 - sdf);
    let mask_clip = rounded_clip_alpha(input.position.xy, input.clip_bounds, input.clip_radii);
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
