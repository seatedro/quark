//! Values spread over several rows: a rect's four edges, a color's four
//! premultiplied Oklab channels, and a layer's opacity, offset, and scale.

use super::{AnimKey, AnimationTable, Motion, PropId};
use crate::color::{Color, Oklab};
use crate::geometry::Rect;
use crate::scene::LayerTransform;

/// Props from here up belong to quark's own helpers and to quark-ui's
/// style transitions; components animate their own props below it.
pub const RESERVED_PROP_BASE: u16 = 0xE000;

/// The rows [`AnimationTable::animate_rect`] animates: the left, top,
/// right, and bottom edges.
pub const RECT_PROPS: [PropId; 4] = [
    PropId(RESERVED_PROP_BASE),
    PropId(RESERVED_PROP_BASE + 1),
    PropId(RESERVED_PROP_BASE + 2),
    PropId(RESERVED_PROP_BASE + 3),
];

/// The rows [`AnimationTable::animate_layer`] animates: opacity, offset x
/// and y, and scale x and y.
pub const LAYER_PROPS: [PropId; 5] = [
    PropId(RESERVED_PROP_BASE + 4),
    PropId(RESERVED_PROP_BASE + 5),
    PropId(RESERVED_PROP_BASE + 6),
    PropId(RESERVED_PROP_BASE + 7),
    PropId(RESERVED_PROP_BASE + 8),
];

/// The four rows from `base` that animate a color: its premultiplied Oklab
/// lightness, a, b, and alpha.
pub const fn color_props(base: PropId) -> [PropId; 4] {
    [
        base,
        PropId(base.0 + 1),
        PropId(base.0 + 2),
        PropId(base.0 + 3),
    ]
}

/// How [`AnimationTable::animate_rect`] moves a rect's edges.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RectMotion {
    pub motion: Motion,
    /// How many times faster the edges facing the direction of travel
    /// move than the others; one moves every edge alike.
    pub lead: f32,
}

impl RectMotion {
    pub const fn uniform(motion: Motion) -> Self {
        Self { motion, lead: 1.0 }
    }

    /// The leading edges move `lead` times faster, so a moving rect
    /// stretches toward where it is going and its trailing edges catch up.
    pub const fn leading(motion: Motion, lead: f32) -> Self {
        Self { motion, lead }
    }
}

impl From<Motion> for RectMotion {
    fn from(motion: Motion) -> Self {
        Self::uniform(motion)
    }
}

fn edges(rect: Rect) -> [f32; 4] {
    [rect.x, rect.y, rect.right(), rect.bottom()]
}

fn layer_values(layer: LayerTransform) -> [f32; 5] {
    [
        layer.opacity,
        layer.offset_x,
        layer.offset_y,
        layer.scale_x,
        layer.scale_y,
    ]
}

impl AnimationTable {
    /// Snap the edges of `key`'s rect to `rect`.
    pub fn set_rect(&mut self, key: AnimKey, rect: Rect, now_ms: u64) {
        for (prop, value) in RECT_PROPS.into_iter().zip(edges(rect)) {
            self.set(key, prop, value, now_ms);
        }
    }

    /// Animate `key`'s rect to `target`, each edge on its own row. An edge
    /// whose target did not change is left alone, so a resize that keeps
    /// the left edge never moves it, and an edge already heading for its
    /// target keeps its motion. Along each axis, the edge facing the way
    /// the rect's center travels leads by `motion.lead`. Unknown rects snap
    /// (call [`Self::set_rect`] first to animate in).
    pub fn animate_rect(
        &mut self,
        key: AnimKey,
        target: Rect,
        motion: impl Into<RectMotion>,
        now_ms: u64,
    ) {
        let RectMotion { motion, lead } = motion.into();
        let to = edges(target);
        let at = self.rect(key).map_or(to, edges);
        for axis in 0..2 {
            let (low, high) = (axis, axis + 2);
            let travel = (to[low] + to[high]) - (at[low] + at[high]);
            let leading = match travel.partial_cmp(&0.0) {
                Some(std::cmp::Ordering::Greater) => Some(high),
                Some(std::cmp::Ordering::Less) => Some(low),
                _ => None,
            };
            for edge in [low, high] {
                let prop = RECT_PROPS[edge];
                if self.target(key, prop) == Some(to[edge]) {
                    continue;
                }
                let motion = if leading == Some(edge) {
                    motion.time_scaled(lead)
                } else {
                    motion
                };
                self.animate_to(key, prop, to[edge], motion, now_ms);
            }
        }
    }

    /// `key`'s rect from its edge rows, or `None` while any is missing.
    pub fn rect(&self, key: AnimKey) -> Option<Rect> {
        let [left, top, right, bottom] = RECT_PROPS.map(|prop| self.get(key, prop));
        let (left, top, right, bottom) = (left?, top?, right?, bottom?);
        Some(Rect {
            x: left,
            y: top,
            width: right - left,
            height: bottom - top,
        })
    }

    /// Snap the color at `base` (see [`color_props`]) to `color`.
    pub fn set_color(&mut self, key: AnimKey, base: PropId, color: Color, now_ms: u64) {
        let channels = color.to_oklab().premultiplied();
        for (prop, value) in color_props(base).into_iter().zip(channels) {
            self.set(key, prop, value, now_ms);
        }
    }

    /// Animate the color at `base` toward `target` in premultiplied Oklab,
    /// so hues pass through no grey and a fade from transparent keeps the
    /// target's hue. Unknown colors snap.
    pub fn animate_color(
        &mut self,
        key: AnimKey,
        base: PropId,
        target: Color,
        motion: Motion,
        now_ms: u64,
    ) {
        let channels = target.to_oklab().premultiplied();
        for (prop, value) in color_props(base).into_iter().zip(channels) {
            self.animate_to(key, prop, value, motion, now_ms);
        }
    }

    /// The color at `base`, or `None` while any channel is missing.
    /// Springs may overshoot; channels clamp to valid colors.
    pub fn color(&self, key: AnimKey, base: PropId) -> Option<Color> {
        let [l, a, b, alpha] = color_props(base).map(|prop| self.get(key, prop));
        let channels = [l?, a?, b?, alpha?];
        Some(Color::from_oklab(Oklab::from_premultiplied(channels)))
    }

    /// Snap `key`'s layer (its [`LAYER_PROPS`]) to `layer`; the origin is
    /// the caller's.
    pub fn set_layer(&mut self, key: AnimKey, layer: LayerTransform, now_ms: u64) {
        for (prop, value) in LAYER_PROPS.into_iter().zip(layer_values(layer)) {
            self.set(key, prop, value, now_ms);
        }
    }

    /// Animate `key`'s layer toward `target`'s opacity, offset, and scale;
    /// the origin is the caller's. Unknown rows snap.
    pub fn animate_layer(
        &mut self,
        key: AnimKey,
        target: LayerTransform,
        motion: Motion,
        now_ms: u64,
    ) {
        for (prop, value) in LAYER_PROPS.into_iter().zip(layer_values(target)) {
            self.animate_to(key, prop, value, motion, now_ms);
        }
    }

    /// `key`'s layer with its origin at zero, or `None` when it has no
    /// layer rows. A missing row reads as the identity's value.
    pub fn layer(&self, key: AnimKey) -> Option<LayerTransform> {
        let values = LAYER_PROPS.map(|prop| self.get(key, prop));
        if values.iter().all(Option::is_none) {
            return None;
        }
        let mut current = layer_values(LayerTransform::IDENTITY);
        for (slot, value) in current.iter_mut().zip(values) {
            if let Some(value) = value {
                *slot = value;
            }
        }
        let [opacity, offset_x, offset_y, scale_x, scale_y] = current;
        Some(LayerTransform {
            opacity,
            offset_x,
            offset_y,
            scale_x,
            scale_y,
            origin: [0.0; 2],
        })
    }
}
