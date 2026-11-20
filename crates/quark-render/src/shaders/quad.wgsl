// SDF quads: rects, rounded rects, and borders.

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
    let unit = quad_corner(input.vertex_id);
    var out: VertexOutput;
    out.position = clip_position(input.bounds.xy + unit * input.bounds.zw);
    out.bounds = input.bounds;
    out.background = to_target(input.background);
    out.border_color = to_target(input.border_color);
    out.corner_radii = input.corner_radii;
    out.border_widths = input.border_widths;
    out.clip_bounds = input.clip_bounds;
    out.clip_radii = input.clip_radii;
    return out;
}

fn over(below: vec4<f32>, above: vec4<f32>) -> vec4<f32> {
    let a = above.a + below.a * (1.0 - above.a);
    if (a <= 0.0) {
        return vec4<f32>(0.0);
    }
    let c = (above.rgb * above.a + below.rgb * below.a * (1.0 - above.a)) / a;
    return vec4<f32>(c, a);
}

@fragment
fn fs_quad(input: VertexOutput) -> @location(0) vec4<f32> {
    let half_size = input.bounds.zw * 0.5;
    let center = input.bounds.xy + half_size;
    let p = input.position.xy - center;

    let corner_radius = pick_corner_radius(p, input.corner_radii);
    let outer_sdf = rounded_rect_sdf(p, half_size, corner_radius);

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
            fill_blend = saturate(aa - rounded_rect_sdf(ip, inner_half, inner_radius));
        }
        let blended = over(input.background, input.border_color);
        let border = vec4<f32>(blended.rgb * blended.a, blended.a);
        color = mix(border, background, fill_blend);
    } else {
        color = background;
    }

    return color * (outer_alpha * clip_alpha);
}
