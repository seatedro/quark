struct VertexInput {
    @builtin(vertex_index) vertex_idx: u32,
    @location(0) pos: vec2<i32>,
    @location(1) dim: u32,
    @location(2) uv: u32,
    @location(3) color: u32,
    @location(4) content_type_with_srgb: u32,
    @location(5) depth: f32,
    // quark patch: bits 0-7 fill index plus one, bit 8 a backdrop, bits
    // 16-23 its sRGB-encoded luminance.
    @location(6) paint: u32,
}

struct VertexOutput {
    @invariant @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) @interpolate(flat) content_type: u32,
    // Linear correction: x is 1 when on, y the background's linear
    // luminance.
    @location(3) @interpolate(flat) correction: vec2<f32>,
    // Fill index plus one (0: the vertex color), and 1 on an encoded
    // target.
    @location(4) @interpolate(flat) fill_encoded: vec2<u32>,
};

// quark patch: per-renderer glyph fills; see `GlyphFill`.
struct GlyphFill {
    axis: vec4<f32>,
    color_a: vec4<f32>,
    color_b: vec4<f32>,
    params: vec4<f32>,
};

struct Params {
    screen_resolution: vec2<u32>,
    // x: 1 when the target holds sRGB-encoded values.
    _pad: vec2<u32>,
};

@group(0) @binding(0)
var color_atlas_texture: texture_2d<f32>;

@group(0) @binding(1)
var mask_atlas_texture: texture_2d<f32>;

@group(0) @binding(2)
var atlas_sampler: sampler;

@group(1) @binding(0)
var<uniform> params: Params;

// Per-draw offsets, two to a vector: the draw at slot `s` (vertices
// `4 * s` to `4 * s + 4`) moves its glyphs by offset `s`.
@group(1) @binding(1)
var<uniform> offsets: array<vec4<i32>, 1024>;

@group(2) @binding(0)
var<uniform> fills: array<GlyphFill, 64>;

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        return c / 12.92;
    } else {
        return pow((c + 0.055) / 1.055, 2.4);
    }
}

fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.0031308 {
        return c * 12.92;
    } else {
        return 1.055 * pow(c, 1.0 / 2.4) - 0.055;
    }
}

fn decode(c: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(srgb_to_linear(c.r), srgb_to_linear(c.g), srgb_to_linear(c.b));
}

fn encode(c: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(linear_to_srgb(c.r), linear_to_srgb(c.g), linear_to_srgb(c.b));
}

fn luminance(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

@vertex
fn vs_main(in_vert: VertexInput) -> VertexOutput {
    let slot = in_vert.vertex_idx >> 2u;
    let pair = offsets[slot >> 1u];
    var pos = in_vert.pos + select(pair.xy, pair.zw, (slot & 1u) == 1u);
    let width = in_vert.dim & 0xffffu;
    let height = (in_vert.dim & 0xffff0000u) >> 16u;
    let color = in_vert.color;
    var uv = vec2<u32>(in_vert.uv & 0xffffu, (in_vert.uv & 0xffff0000u) >> 16u);
    let v = in_vert.vertex_idx;

    let corner_position = vec2<u32>(
        in_vert.vertex_idx & 1u,
        (in_vert.vertex_idx >> 1u) & 1u,
    );

    let corner_offset = vec2<u32>(width, height) * corner_position;

    uv = uv + corner_offset;
    pos = pos + vec2<i32>(corner_offset);

    var vert_output: VertexOutput;

    vert_output.position = vec4<f32>(
        2.0 * vec2<f32>(pos) / vec2<f32>(params.screen_resolution) - 1.0,
        in_vert.depth,
        1.0,
    );

    vert_output.position.y *= -1.0;

    let content_type = in_vert.content_type_with_srgb & 0xffffu;
    let upper = (in_vert.content_type_with_srgb & 0xffff0000u) >> 16u;
    let encoded = params._pad.x & 1u;
    // An encoded target keeps colors as authored.
    let srgb = (upper & 1u) * (1u - encoded);
    vert_output.correction = vec2<f32>(
        f32((upper >> 1u) & 1u),
        srgb_to_linear(f32(upper >> 8u) / 255.0),
    );
    if (in_vert.paint & 0x100u) != 0u {
        // A backdrop known when drawing overrides the cache key's.
        vert_output.correction = vec2<f32>(
            1.0,
            srgb_to_linear(f32((in_vert.paint >> 16u) & 0xffu) / 255.0),
        );
    }
    if encoded == 1u {
        // Blending encoded values already gives text its sRGB weight.
        vert_output.correction.x = 0.0;
    }
    vert_output.fill_encoded = vec2<u32>(in_vert.paint & 0xffu, encoded);

    switch srgb {
        case 0u: {
            vert_output.color = vec4<f32>(
                f32((color & 0x00ff0000u) >> 16u) / 255.0,
                f32((color & 0x0000ff00u) >> 8u) / 255.0,
                f32(color & 0x000000ffu) / 255.0,
                f32((color & 0xff000000u) >> 24u) / 255.0,
            );
        }
        case 1u: {
            vert_output.color = vec4<f32>(
                srgb_to_linear(f32((color & 0x00ff0000u) >> 16u) / 255.0),
                srgb_to_linear(f32((color & 0x0000ff00u) >> 8u) / 255.0),
                srgb_to_linear(f32(color & 0x000000ffu) / 255.0),
                f32((color & 0xff000000u) >> 24u) / 255.0,
            );
        }
        default: {}
    }

    var dim: vec2<u32> = vec2(0u);
    switch content_type {
        case 0u: {
            dim = textureDimensions(color_atlas_texture);
            break;
        }
        case 1u: {
            dim = textureDimensions(mask_atlas_texture);
            break;
        }
        default: {}
    }

    vert_output.content_type = content_type;

    vert_output.uv = vec2<f32>(uv) / vec2<f32>(dim);

    return vert_output;
}

// The fill's straight color at target pixel `p`, in the target's space.
fn fill_color(index: u32, p: vec2<f32>, encoded: u32) -> vec4<f32> {
    let fill = fills[index];
    let start = fill.axis.xy;
    let axis = fill.axis.zw - start;
    let length2 = max(dot(axis, axis), 1.0e-6);
    var t = 0.0;
    if fill.params.x < 1.5 {
        t = clamp(dot(p - start, axis) / length2, 0.0, 1.0);
    } else {
        let along = dot(p - start, axis) / sqrt(length2);
        let half_width = max(fill.params.y, 1.0e-3);
        t = 1.0 - smoothstep(0.0, half_width, abs(along - fill.params.z));
    }
    // Interpolate in the target's space.
    if encoded == 1u {
        return mix(fill.color_a, fill.color_b, t);
    }
    let a = vec4<f32>(decode(fill.color_a.rgb), fill.color_a.a);
    let b = vec4<f32>(decode(fill.color_b.rgb), fill.color_b.a);
    return mix(a, b, t);
}

@fragment
fn fs_main(in_frag: VertexOutput) -> @location(0) vec4<f32> {
    switch in_frag.content_type {
        case 0u: {
            let c = textureSampleLevel(color_atlas_texture, atlas_sampler, in_frag.uv, 0.0);
            if in_frag.fill_encoded.y == 1u {
                return vec4<f32>(encode(c.rgb), c.a);
            }
            return c;
        }
        case 1u: {
            var a = textureSampleLevel(mask_atlas_texture, atlas_sampler, in_frag.uv, 0.0).x;
            var color = in_frag.color;
            if in_frag.fill_encoded.x > 0u {
                let fill = fill_color(in_frag.fill_encoded.x - 1u, in_frag.position.xy, in_frag.fill_encoded.y);
                color = vec4<f32>(fill.rgb, fill.a * in_frag.color.a);
            }
            if in_frag.correction.x > 0.5 {
                // Ghostty's linear-corrected blending: blend the two
                // luminances in sRGB space, then pick the coverage that
                // gives that luminance when blended linearly, so text has
                // the weight of sRGB blending without its color fringes.
                let fg_l = luminance(color.rgb);
                let bg_l = in_frag.correction.y;
                if abs(fg_l - bg_l) > 0.001 {
                    let blend_l = srgb_to_linear(linear_to_srgb(fg_l) * a + linear_to_srgb(bg_l) * (1.0 - a));
                    a = clamp((blend_l - bg_l) / (fg_l - bg_l), 0.0, 1.0);
                }
            }
            return vec4<f32>(color.rgb, color.a * a);
        }
        default: {
            return vec4<f32>(0.0);
        }
    }
}
