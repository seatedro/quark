// Paths: winding number and edge distance over a band's segments.

// One segment [x0, y0, x1, y1] per texel, path-local physical pixels.
@group(1) @binding(0)
var t_segments: texture_2d<f32>;

// Texels per row of the segment texture; `SEGMENT_TEXTURE_WIDTH` in the
// renderer must match.
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
    let unit = quad_corner(input.vertex_id);
    var out: VertexOutput;
    out.position = clip_position(input.bounds.xy + unit * input.bounds.zw);
    out.origin_rule = input.origin_rule;
    out.color = to_target(input.color);
    out.segments = input.segments;
    out.clip_bounds = input.clip_bounds;
    out.clip_radii = input.clip_radii;
    return out;
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
        * rounded_clip_alpha(input.position.xy, input.clip_bounds, input.clip_radii);
    let alpha = input.color.a * coverage;
    if (alpha <= 0.0) {
        discard;
    }
    return vec4<f32>(input.color.rgb * alpha, alpha);
}
