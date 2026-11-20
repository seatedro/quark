//! WGSL sources of every pipeline, assembled at compile time from shared
//! snippets so color conversion, quad vertex math, and rounded-rect SDFs
//! each have one definition:
//!
//! - `srgb.wgsl`: the sRGB transfer functions.
//! - `viewport.wgsl`: the viewport uniform at group 0, quad corners,
//!   clip-space positions, and `to_target` for straight sRGB colors.
//! - `sample.wgsl`: `to_target_space` for premultiplied texture samples.
//! - `rounded_rect.wgsl`: the rounded-rect SDF and rounded clip coverage.
//!
//! WGSL module declarations are order independent, so a shader is the
//! snippets it uses followed by its own body.

pub(crate) const SHADOW_SHADER: &str = concat!(
    include_str!("srgb.wgsl"),
    include_str!("viewport.wgsl"),
    include_str!("rounded_rect.wgsl"),
    include_str!("shadow.wgsl"),
);

pub(crate) const QUAD_SHADER: &str = concat!(
    include_str!("srgb.wgsl"),
    include_str!("viewport.wgsl"),
    include_str!("rounded_rect.wgsl"),
    include_str!("quad.wgsl"),
);

pub(crate) const EFFECT_SHADER: &str = concat!(
    include_str!("srgb.wgsl"),
    include_str!("viewport.wgsl"),
    include_str!("rounded_rect.wgsl"),
    include_str!("effect.wgsl"),
);

pub(crate) const BLIT_SHADER: &str = concat!(
    include_str!("srgb.wgsl"),
    include_str!("viewport.wgsl"),
    include_str!("sample.wgsl"),
    include_str!("rounded_rect.wgsl"),
    include_str!("blit.wgsl"),
);

pub(crate) const LAYER_SHADER: &str = concat!(
    include_str!("srgb.wgsl"),
    include_str!("viewport.wgsl"),
    include_str!("sample.wgsl"),
    include_str!("rounded_rect.wgsl"),
    include_str!("layer.wgsl"),
);

pub(crate) const PATH_SHADER: &str = concat!(
    include_str!("srgb.wgsl"),
    include_str!("viewport.wgsl"),
    include_str!("rounded_rect.wgsl"),
    include_str!("path.wgsl"),
);

pub(crate) const BLUR_SHADER: &str = concat!(
    include_str!("srgb.wgsl"),
    include_str!("viewport.wgsl"),
    include_str!("sample.wgsl"),
    include_str!("blur.wgsl"),
);

/// The glyph shader of the text engine.
pub(crate) const TEXT_SHADER: &str = concat!(
    include_str!("srgb.wgsl"),
    include_str!("../text_engine/shader.wgsl"),
);

#[cfg(test)]
mod tests {
    use wgpu::naga::front::wgsl;
    use wgpu::naga::valid::{Capabilities, ValidationFlags, Validator};

    // Shaders compile on first use by a GPU, and GPU tests skip without
    // an adapter: a snippet that no longer fits one of its shaders must
    // still fail here.
    #[test]
    fn every_assembled_shader_parses_and_validates() {
        let shaders = [
            ("shadow", super::SHADOW_SHADER),
            ("quad", super::QUAD_SHADER),
            ("effect", super::EFFECT_SHADER),
            ("blit", super::BLIT_SHADER),
            ("layer", super::LAYER_SHADER),
            ("path", super::PATH_SHADER),
            ("blur", super::BLUR_SHADER),
            ("text", super::TEXT_SHADER),
        ];
        for (name, source) in shaders {
            let module = wgsl::parse_str(source)
                .unwrap_or_else(|error| panic!("{name}: {}", error.emit_to_string(source)));
            Validator::new(ValidationFlags::all(), Capabilities::default())
                .validate(&module)
                .unwrap_or_else(|error| panic!("{name}: {}", error.emit_to_string(source)));
        }
    }
}
