//! RGBA color primitive shared by quark's style + render layers.
//!
//! Colors blend in premultiplied Oklab ([`Color::lerp_oklab`]). Oklab is
//! perceptually uniform, so a blend between two theme colors moves at an
//! even visual pace and does not pass through the grey or dark midpoints
//! that sRGB and linear sRGB blends produce between distant hues.
//! Premultiplying by alpha lets a transparent color fade in as the other
//! color's hue instead of from black.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

/// A color in Oklab with straight alpha in `[0, 1]`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Oklab {
    pub l: f32,
    pub a: f32,
    pub b: f32,
    pub alpha: f32,
}

impl Oklab {
    /// `[L·α, a·α, b·α, α]`: the space colors interpolate in.
    pub fn premultiplied(self) -> [f32; 4] {
        let alpha = self.alpha;
        [self.l * alpha, self.a * alpha, self.b * alpha, alpha]
    }

    /// Inverse of [`Self::premultiplied`]. Alpha clamps to `[0, 1]`, so
    /// channels that overshot (a spring past its target) stay valid; at
    /// zero alpha the hue is gone and reads as black.
    pub fn from_premultiplied([l, a, b, alpha]: [f32; 4]) -> Self {
        let alpha = alpha.clamp(0.0, 1.0);
        if alpha <= 0.0 {
            return Self::default();
        }
        Self {
            l: l / alpha,
            a: a / alpha,
            b: b / alpha,
            alpha,
        }
    }
}

fn srgb_to_linear(c: u8) -> f32 {
    let c = f32::from(c) / 255.0;
    if c <= 0.040_45 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(c: f32) -> u8 {
    let c = c.clamp(0.0, 1.0);
    let s = if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0).round() as u8
}

impl Color {
    pub const TRANSPARENT: Self = Self {
        r: 0,
        g: 0,
        b: 0,
        a: 0,
    };

    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    pub const fn with_alpha(self, a: u8) -> Self {
        Self {
            r: self.r,
            g: self.g,
            b: self.b,
            a,
        }
    }

    pub fn lerp(self, other: Self, t: f32) -> Self {
        let t = t.clamp(0.0, 1.0);
        let inv = 1.0 - t;
        Self {
            r: (self.r as f32 * inv + other.r as f32 * t).round() as u8,
            g: (self.g as f32 * inv + other.g as f32 * t).round() as u8,
            b: (self.b as f32 * inv + other.b as f32 * t).round() as u8,
            a: (self.a as f32 * inv + other.a as f32 * t).round() as u8,
        }
    }

    pub fn to_oklab(self) -> Oklab {
        let (r, g, b) = (
            srgb_to_linear(self.r),
            srgb_to_linear(self.g),
            srgb_to_linear(self.b),
        );
        let l = (0.412_221_47 * r + 0.536_332_55 * g + 0.051_445_99 * b).cbrt();
        let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
        let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
        Oklab {
            l: 0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
            a: 1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
            b: 0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
            alpha: f32::from(self.a) / 255.0,
        }
    }

    /// The nearest sRGB color; channels outside the gamut clamp.
    pub fn from_oklab(color: Oklab) -> Self {
        let alpha = color.alpha.clamp(0.0, 1.0);
        if alpha <= 0.0 {
            return Self::TRANSPARENT;
        }
        let Oklab { l: ok_l, a, b, .. } = color;
        let l = (ok_l + 0.396_337_78 * a + 0.215_803_76 * b).powi(3);
        let m = (ok_l - 0.105_561_346 * a - 0.063_854_17 * b).powi(3);
        let s = (ok_l - 0.089_484_18 * a - 1.291_485_5 * b).powi(3);
        Self::rgba(
            linear_to_srgb(4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s),
            linear_to_srgb(-1.268_438 * l + 2.609_757_4 * m - 0.341_319_4 * s),
            linear_to_srgb(-0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s),
            (alpha * 255.0).round() as u8,
        )
    }

    /// `self` blended `t` of the way to `other` in premultiplied Oklab.
    /// The ends return the colors themselves.
    pub fn lerp_oklab(self, other: Self, t: f32) -> Self {
        let t = t.clamp(0.0, 1.0);
        if t == 0.0 {
            return self;
        }
        if t == 1.0 {
            return other;
        }
        let (from, to) = (
            self.to_oklab().premultiplied(),
            other.to_oklab().premultiplied(),
        );
        let mixed = [0, 1, 2, 3].map(|i| from[i] + (to[i] - from[i]) * t);
        Self::from_oklab(Oklab::from_premultiplied(mixed))
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    proptest! {
        #![proptest_config(crate::test_support::proptest_config(64))]

        // A settled transition must paint exactly the color it started from.
        #[test]
        fn oklab_round_trips_opaque_colors(r: u8, g: u8, b: u8) {
            let color = Color::rgba(r, g, b, 255);
            let premultiplied = color.to_oklab().premultiplied();
            prop_assert_eq!(Color::from_oklab(Oklab::from_premultiplied(premultiplied)), color);
        }

        // Straight alpha would drag a fade from transparent through black.
        #[test]
        fn lerp_oklab_from_transparent_keeps_the_hue(r: u8, g: u8, b: u8, t in 0.05f32..1.0) {
            let target = Color::rgba(r, g, b, 255);
            let mixed = Color::TRANSPARENT.lerp_oklab(target, t);
            let near = |x: u8, y: u8| x.abs_diff(y) <= 1;
            prop_assert!(near(mixed.r, r) && near(mixed.g, g) && near(mixed.b, b), "{mixed:?}");
            prop_assert!(near(mixed.a, (t * 255.0).round() as u8), "{mixed:?}");
        }
    }
}
