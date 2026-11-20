//! The per-instance vertex data of every quad pipeline, laid out as the
//! shaders in `crate::shaders` read it, and the viewport uniform.

use bytemuck::{Pod, Zeroable};

/// Per-instance vertex data: one quad per instance, attributes at
/// consecutive shader locations from 0.
pub(super) trait Instance: Pod {
    const ATTRIBUTES: &'static [wgpu::VertexAttribute];

    fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: size_of::<Self>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: Self::ATTRIBUTES,
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(super) struct QuadInstance {
    pub(super) bounds: [f32; 4],
    pub(super) background: [f32; 4],
    pub(super) border_color: [f32; 4],
    pub(super) corner_radii: [f32; 4],
    pub(super) border_widths: [f32; 4],
    /// Rounded-clip rect [x, y, w, h] — the rect whose radii apply to the SDF clip test.
    pub(super) clip_bounds: [f32; 4],
    /// Rounded-clip corner radii [tl, tr, br, bl]. All zero = no rounded clip.
    pub(super) clip_radii: [f32; 4],
}

impl Instance for QuadInstance {
    const ATTRIBUTES: &'static [wgpu::VertexAttribute] = &wgpu::vertex_attr_array![
        0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4, 4 => Float32x4,
        5 => Float32x4, 6 => Float32x4
    ];
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(super) struct ShadowInstance {
    /// Expanded quad bounds (x, y, w, h) — covers the full blur extent.
    pub(super) draw_bounds: [f32; 4],
    /// Original shadow-casting rect (x, y, w, h) before expansion.
    pub(super) shadow_bounds: [f32; 4],
    /// Shadow color (sRGB-encoded, straight alpha; see `color_to_unit`).
    pub(super) color: [f32; 4],
    /// [blur_sigma, corner_radius, 0, 0]
    pub(super) params: [f32; 4],
    /// Rounded-clip rect [x, y, w, h] — the rect whose radii apply for the SDF clip test.
    pub(super) clip_bounds: [f32; 4],
    /// Rounded-clip corner radii [tl, tr, br, bl]. All zero = no rounded clip.
    pub(super) clip_radii: [f32; 4],
}

impl Instance for ShadowInstance {
    const ATTRIBUTES: &'static [wgpu::VertexAttribute] = &wgpu::vertex_attr_array![
        0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4, 4 => Float32x4,
        5 => Float32x4
    ];
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(super) struct EffectQuadInstance {
    /// Element bounds: [x, y, width, height].
    pub(super) bounds: [f32; 4],
    /// First color (sRGB-encoded, straight alpha; see `color_to_unit`).
    pub(super) color_a: [f32; 4],
    /// Second color (sRGB-encoded, straight alpha).
    pub(super) color_b: [f32; 4],
    /// [effect_type, param1, param2, corner_radius].
    pub(super) params: [f32; 4],
    /// Rounded-clip rect [x, y, w, h] — the rect whose radii apply for the SDF clip test.
    pub(super) clip_bounds: [f32; 4],
    /// Rounded-clip corner radii [tl, tr, br, bl]. All zero = no rounded clip.
    pub(super) clip_radii: [f32; 4],
    /// More effect parameters: stripes keep [duty, 0, 0, 0].
    pub(super) extra: [f32; 4],
}

/// Effect type of [`StripesPrimitive`](crate::scene::StripesPrimitive)
/// quads in the effect shader.
pub(super) const EFFECT_STRIPES: f32 = 6.0;

impl Instance for EffectQuadInstance {
    const ATTRIBUTES: &'static [wgpu::VertexAttribute] = &wgpu::vertex_attr_array![
        0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4, 4 => Float32x4,
        5 => Float32x4, 6 => Float32x4
    ];
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(super) struct BlitInstance {
    /// Screen-space destination bounds: [x, y, width, height].
    pub(super) bounds: [f32; 4],
    /// Source texture UV rect: [u_min, v_min, u_max, v_max].
    pub(super) uv_rect: [f32; 4],
    /// Premultiplied tint/opacity multiplier (usually [alpha; 4]).
    pub(super) tint: [f32; 4],
    /// Rounded mask over `bounds`: [tl, tr, br, bl]. All zero = none.
    pub(super) radii: [f32; 4],
    /// [source space, 0, 0, 0]; see [`SourceSpace`].
    pub(super) space: [f32; 4],
}

impl Instance for BlitInstance {
    const ATTRIBUTES: &'static [wgpu::VertexAttribute] = &wgpu::vertex_attr_array![
        0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4, 4 => Float32x4
    ];
}

/// The values in a sampled texture, relative to the target sampling it.
/// The blit, layer, and blur shaders convert samples by it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SourceSpace {
    /// The target's own space.
    Same = 0,
    /// Linear light (sRGB textures): an encoded target encodes it.
    Linear = 1,
    /// Encoded values (non-sRGB views of encoded targets): a linear target
    /// decodes it.
    Encoded = 2,
}

impl SourceSpace {
    /// The space of a target that is `source_encoded`, sampled into one
    /// that is `target_encoded`.
    pub(super) fn between(source_encoded: bool, target_encoded: bool) -> Self {
        match (source_encoded, target_encoded) {
            (a, b) if a == b => Self::Same,
            (true, _) => Self::Encoded,
            (false, _) => Self::Linear,
        }
    }

    /// The flag the shaders read.
    pub(super) fn code(self) -> f32 {
        f32::from(self as u8)
    }

    /// A [`BlitInstance::space`] of this space.
    pub(super) fn attribute(self) -> [f32; 4] {
        [self.code(), 0.0, 0.0, 0.0]
    }
}

/// An offscreen layer composited through an affine map.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(super) struct LayerInstance {
    /// [a, b, c, d] of the map from layer texels to target pixels.
    pub(super) linear: [f32; 4],
    /// [tx, ty, layer width, layer height].
    pub(super) offset_size: [f32; 4],
    /// [1 / texture width, 1 / texture height, opacity, source space].
    pub(super) params: [f32; 4],
    pub(super) clip_bounds: [f32; 4],
    pub(super) clip_radii: [f32; 4],
    /// Alpha mask axis in target pixels, [x0, y0, x1, y1]; all zero for
    /// no mask (with every stop alpha one).
    pub(super) mask_axis: [f32; 4],
    /// Four stop offsets along the axis, increasing; unused stops repeat
    /// the last.
    pub(super) mask_offsets: [f32; 4],
    pub(super) mask_alphas: [f32; 4],
}

impl LayerInstance {
    /// Mask fields for `mask` drawn with `shift` added to its points.
    pub(super) fn mask_fields(
        mask: Option<&crate::scene::AlphaMask>,
        shift: (f32, f32),
    ) -> ([f32; 4], [f32; 4], [f32; 4]) {
        let Some(crate::scene::AlphaMask::Linear { start, end, stops }) = mask else {
            return ([0.0; 4], [0.0; 4], [1.0; 4]);
        };
        let stops = stops.as_slice();
        let (mut offsets, mut alphas) = ([0.0; 4], [1.0; 4]);
        for i in 0..4 {
            if let Some(stop) = stops.get(i).or(stops.last()) {
                offsets[i] = stop.offset;
                alphas[i] = stop.alpha;
            }
        }
        (
            [
                start[0] + shift.0,
                start[1] + shift.1,
                end[0] + shift.0,
                end[1] + shift.1,
            ],
            offsets,
            alphas,
        )
    }
}

impl Instance for LayerInstance {
    const ATTRIBUTES: &'static [wgpu::VertexAttribute] = &wgpu::vertex_attr_array![
        0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4, 4 => Float32x4,
        5 => Float32x4, 6 => Float32x4, 7 => Float32x4
    ];
}

/// One band of a path shape; see `path.rs`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(super) struct PathInstance {
    /// Band quad in target pixels: [x, y, w, h].
    pub(super) bounds: [f32; 4],
    /// [origin x, origin y, fill rule (0 nonzero, 1 even-odd), 0].
    pub(super) origin_rule: [f32; 4],
    /// Linear RGBA, straight alpha.
    pub(super) color: [f32; 4],
    /// [first segment, segment count, 0, 0].
    pub(super) segments: [u32; 4],
    pub(super) clip_bounds: [f32; 4],
    pub(super) clip_radii: [f32; 4],
}

impl Instance for PathInstance {
    const ATTRIBUTES: &'static [wgpu::VertexAttribute] = &wgpu::vertex_attr_array![
        0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Uint32x4, 4 => Float32x4,
        5 => Float32x4
    ];
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(super) struct BlurInstance {
    /// Destination bounds in the target texture: [x, y, width, height].
    pub(super) bounds: [f32; 4],
    /// Source UV rect: [u_min, v_min, u_max, v_max].
    pub(super) uv_rect: [f32; 4],
    /// [direction_x, direction_y, blur_sigma, source space]
    /// direction = (1,0) for horizontal, (0,1) for vertical. The blur
    /// filters linear light, so an encoded source decodes as it is read.
    pub(super) blur_params: [f32; 4],
}

impl Instance for BlurInstance {
    const ATTRIBUTES: &'static [wgpu::VertexAttribute] =
        &wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x4];
}

/// The viewport of one target, at group 0 of every quad pipeline.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(super) struct ViewportUniform {
    resolution: [f32; 2],
    time: f32,
    /// 1 when the target holds encoded sRGB values; see `to_target` in
    /// the shaders.
    encoded: f32,
}

impl ViewportUniform {
    pub(super) fn new(width: u32, height: u32, time: f32, encoded: bool) -> Self {
        Self {
            resolution: [width as f32, height as f32],
            time,
            encoded: f32::from(u8::from(encoded)),
        }
    }
}
