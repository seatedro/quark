//! Animation for quark-ui: the [`AnimationTable`] from `quark`, plus the
//! declarative style transitions divs opt into with
//! [`Div::transition`](crate::element::Div::transition).
//!
//! One table lives per window (the `quark-app` adapter owns it and hands it
//! to the view and to [`ElementContext`](crate::element::ElementContext)).
//! Components animate their own rows on it with keys derived from their
//! stable identity; transitions use the reserved prop range starting at
//! [`TRANSITION_PROP_BASE`], so component props must stay below it.
//!
//! Colors interpolate in premultiplied Oklab. Oklab is perceptually uniform,
//! so a hover between two theme colors moves at an even visual pace and does
//! not pass through the grey or dark midpoints that sRGB and linear sRGB
//! blends produce between distant hues (linear sRGB also makes dark to light
//! fades look front-loaded). Premultiplying by alpha lets a transparent
//! background fade in as the target hue instead of from black.

use std::ops::BitOr;

pub use quark::animation::{AnimKey, AnimationTable, Curve, Motion, PropId, SpringParams};

use crate::theme::Color;

/// A style property a div can transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Prop {
    Background,
    BorderColor,
    Opacity,
    /// The div's [`translate`](crate::element::Div::translate) offset,
    /// [`rotate`](crate::element::Div::rotate), and
    /// [`scale`](crate::element::Div::scale).
    Transform,
}

impl Prop {
    const fn bit(self) -> u8 {
        1 << self as u8
    }
}

/// A set of [`Prop`]s, built with `|`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PropSet(u8);

impl PropSet {
    pub const ALL: Self = Self(
        Prop::Background.bit()
            | Prop::BorderColor.bit()
            | Prop::Opacity.bit()
            | Prop::Transform.bit(),
    );

    pub const fn contains(self, prop: Prop) -> bool {
        self.0 & prop.bit() != 0
    }
}

impl From<Prop> for PropSet {
    fn from(prop: Prop) -> Self {
        Self(prop.bit())
    }
}

impl BitOr for Prop {
    type Output = PropSet;
    fn bitor(self, rhs: Prop) -> PropSet {
        PropSet(self.bit() | rhs.bit())
    }
}

impl BitOr<Prop> for PropSet {
    type Output = PropSet;
    fn bitor(self, rhs: Prop) -> PropSet {
        PropSet(self.0 | rhs.bit())
    }
}

/// Props at or above this id belong to style transitions; the element
/// context drops them for keys that were not painted in a frame.
pub const TRANSITION_PROP_BASE: u16 = 0xF000;

/// Premultiplied Oklab channels `[L·α, a·α, b·α, α]`, one row each.
pub(crate) const BACKGROUND: [PropId; 4] = prop_range(0);
pub(crate) const BORDER_COLOR: [PropId; 4] = prop_range(4);
pub(crate) const OPACITY: PropId = PropId(TRANSITION_PROP_BASE + 8);
pub(crate) const TRANSLATE_X: PropId = PropId(TRANSITION_PROP_BASE + 9);
pub(crate) const TRANSLATE_Y: PropId = PropId(TRANSITION_PROP_BASE + 10);
pub(crate) const ROTATE: PropId = PropId(TRANSITION_PROP_BASE + 11);
pub(crate) const SCALE_X: PropId = PropId(TRANSITION_PROP_BASE + 12);
pub(crate) const SCALE_Y: PropId = PropId(TRANSITION_PROP_BASE + 13);

const fn prop_range(start: u16) -> [PropId; 4] {
    let base = TRANSITION_PROP_BASE + start;
    [
        PropId(base),
        PropId(base + 1),
        PropId(base + 2),
        PropId(base + 3),
    ]
}

pub(crate) fn is_transition_prop(prop: PropId) -> bool {
    prop.0 >= TRANSITION_PROP_BASE
}

/// Drops transition rows whose key is not in `painted`, so elements that
/// left the tree do not keep rows forever. Other rows are untouched.
pub(crate) fn sweep_transitions(table: &mut AnimationTable, painted: &mut Vec<AnimKey>) {
    if table.is_empty() {
        return;
    }
    painted.sort_unstable();
    painted.dedup();
    table.retain(|key, prop| !is_transition_prop(prop) || painted.binary_search(&key).is_ok());
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

/// `[L·α, a·α, b·α, α]` for an sRGB color with straight alpha.
pub(crate) fn to_premul_oklab(color: Color) -> [f32; 4] {
    let (r, g, b) = (
        srgb_to_linear(color.r),
        srgb_to_linear(color.g),
        srgb_to_linear(color.b),
    );
    let l = (0.412_221_47 * r + 0.536_332_55 * g + 0.051_445_99 * b).cbrt();
    let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
    let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
    let alpha = f32::from(color.a) / 255.0;
    [
        (0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s) * alpha,
        (1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s) * alpha,
        (0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s) * alpha,
        alpha,
    ]
}

/// Inverse of [`to_premul_oklab`]; springs may overshoot, so channels clamp.
pub(crate) fn from_premul_oklab([pl, pa, pb, alpha]: [f32; 4]) -> Color {
    let alpha = alpha.clamp(0.0, 1.0);
    if alpha <= 0.0 {
        return Color::TRANSPARENT;
    }
    let (ok_l, ok_a, ok_b) = (pl / alpha, pa / alpha, pb / alpha);
    let l = (ok_l + 0.396_337_78 * ok_a + 0.215_803_76 * ok_b).powi(3);
    let m = (ok_l - 0.105_561_346 * ok_a - 0.063_854_17 * ok_b).powi(3);
    let s = (ok_l - 0.089_484_18 * ok_a - 1.291_485_5 * ok_b).powi(3);
    Color::rgba(
        linear_to_srgb(4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s),
        linear_to_srgb(-1.268_438 * l + 2.609_757_4 * m - 0.341_319_4 * s),
        linear_to_srgb(-0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s),
        (alpha * 255.0).round() as u8,
    )
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    proptest! {
        // Catches a wrong matrix coefficient or transfer function: a settled
        // transition must paint exactly the style color it started from.
        #[test]
        fn premul_oklab_round_trips_opaque_colors(r: u8, g: u8, b: u8) {
            let color = Color::rgba(r, g, b, 255);
            prop_assert_eq!(from_premul_oklab(to_premul_oklab(color)), color);
        }
    }
}
