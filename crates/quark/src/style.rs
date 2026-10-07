//! Style primitives — layout (Taffy) + visual (color, border, corner, shadow).
//!
//! Pure data. The fluent `Styled` trait built on top of these lives in
//! quark-ui because it depends on its design tokens (`Sp`, `Rad`, `ShadowLayer`).

use crate::color::Color;

#[derive(Clone)]
pub struct ShadowStyle {
    pub blur_radius: f32,
    pub offset: [f32; 2],
    pub corner_radius: f32,
    pub color: Color,
}

#[derive(Clone)]
pub struct ElementStyle {
    pub layout: taffy::Style,
    pub background: Option<Color>,
    pub border_color: Option<Color>,
    pub border_widths: [f32; 4],
    /// Per-corner radii: [top-left, top-right, bottom-right, bottom-left].
    pub corner_radii: [f32; 4],
    pub opacity: f32,
    pub z_index: i32,
    pub shadows: Vec<ShadowStyle>,
    /// Text and icon colors this element hands down to its descendants.
    /// `None` inherits from the enclosing element.
    pub text_color: Option<Color>,
    pub icon_color: Option<Color>,
}

impl Default for ElementStyle {
    fn default() -> Self {
        Self {
            layout: taffy::Style {
                display: taffy::Display::Flex,
                ..Default::default()
            },
            background: None,
            border_color: None,
            border_widths: [0.0; 4],
            corner_radii: [0.0; 4],
            opacity: 1.0,
            z_index: 0,
            shadows: Vec::new(),
            text_color: None,
            icon_color: None,
        }
    }
}

impl ElementStyle {
    /// Largest corner radius, for primitives that take a single radius
    /// (shadows, effect quads, blur regions).
    pub fn max_corner_radius(&self) -> f32 {
        self.corner_radii.iter().copied().fold(0.0, f32::max)
    }
}

#[derive(Clone, Default)]
pub struct StyleOverride {
    pub background: Option<Color>,
    pub border_color: Option<Color>,
    pub corner_radius: Option<f32>,
    pub opacity: Option<f32>,
    pub text_color: Option<Color>,
    pub icon_color: Option<Color>,
}

impl StyleOverride {
    pub fn bg(mut self, color: Color) -> Self {
        self.background = Some(color);
        self
    }

    pub fn border_color(mut self, color: Color) -> Self {
        self.border_color = Some(color);
        self
    }

    pub fn rounded(mut self, r: f32) -> Self {
        self.corner_radius = Some(r);
        self
    }

    pub fn opacity(mut self, v: f32) -> Self {
        self.opacity = Some(v);
        self
    }

    pub fn text_color(mut self, color: Color) -> Self {
        self.text_color = Some(color);
        self
    }

    pub fn icon_color(mut self, color: Color) -> Self {
        self.icon_color = Some(color);
        self
    }
}

pub fn apply_override(base: &mut ElementStyle, ov: &StyleOverride) {
    if let Some(bg) = ov.background {
        base.background = Some(bg);
    }
    if let Some(bc) = ov.border_color {
        base.border_color = Some(bc);
    }
    if let Some(cr) = ov.corner_radius {
        base.corner_radii = [cr; 4];
    }
    if let Some(op) = ov.opacity {
        base.opacity = op;
    }
    if let Some(tc) = ov.text_color {
        base.text_color = Some(tc);
    }
    if let Some(ic) = ov.icon_color {
        base.icon_color = Some(ic);
    }
}

#[derive(Debug, Clone, Copy)]
pub enum BackgroundEffect {
    NoiseGradient {
        scale: f32,
        color_a: Color,
        color_b: Color,
    },
    LinearGradient {
        angle: f32,
        color_a: Color,
        color_b: Color,
    },
    RadialGradient {
        color_a: Color,
        color_b: Color,
    },
    Shimmer {
        base: Color,
        highlight: Color,
        speed: f32,
    },
    Vignette {
        color: Color,
        intensity: f32,
    },
    ColorTint {
        color: Color,
    },
}

pub fn noise_gradient(scale: f32, color_a: Color, color_b: Color) -> BackgroundEffect {
    BackgroundEffect::NoiseGradient {
        scale,
        color_a,
        color_b,
    }
}

pub fn linear_gradient(angle: f32, color_a: Color, color_b: Color) -> BackgroundEffect {
    BackgroundEffect::LinearGradient {
        angle,
        color_a,
        color_b,
    }
}

pub fn radial_gradient(center: Color, edge: Color) -> BackgroundEffect {
    BackgroundEffect::RadialGradient {
        color_a: center,
        color_b: edge,
    }
}

pub fn shimmer(base: Color, highlight: Color, speed: f32) -> BackgroundEffect {
    BackgroundEffect::Shimmer {
        base,
        highlight,
        speed,
    }
}

pub fn vignette(color: Color, intensity: f32) -> BackgroundEffect {
    BackgroundEffect::Vignette { color, intensity }
}

pub fn color_tint(color: Color) -> BackgroundEffect {
    BackgroundEffect::ColorTint { color }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn override_sets_text_and_icon_colors_and_keeps_unset_ones() {
        let red = Color::rgba(255, 0, 0, 255);
        let blue = Color::rgba(0, 0, 255, 255);
        let mut style = ElementStyle {
            text_color: Some(blue),
            ..Default::default()
        };

        apply_override(&mut style, &StyleOverride::default().icon_color(red));
        assert_eq!(
            (style.text_color, style.icon_color),
            (Some(blue), Some(red))
        );

        apply_override(&mut style, &StyleOverride::default().text_color(red));
        assert_eq!((style.text_color, style.icon_color), (Some(red), Some(red)));
    }
}
