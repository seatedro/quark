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
//! Colors interpolate in premultiplied Oklab; see [`quark::color`].
//!
//! Keyed divs can also enter and leave with a [`Transition`]
//! ([`Div::presence`](crate::element::Div::presence)): the window's
//! [`PresenceTable`](crate::element::PresenceTable) keeps painting a div
//! that left the tree until its exit settles.

use std::ops::BitOr;

pub use quark::animation::{
    AnimKey, AnimPhase, AnimationTable, Curve, LAYER_PROPS, Motion, PropId, RECT_PROPS, RectMotion,
    SpringParams, color_props,
};
pub use quark::scene::LayerTransform;

pub use crate::element::{PresencePhase, PresenceTable, Transition};

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

/// Colors take four rows from their base prop; see [`color_props`].
pub(crate) const BACKGROUND: PropId = PropId(TRANSITION_PROP_BASE);
pub(crate) const BORDER_COLOR: PropId = PropId(TRANSITION_PROP_BASE + 4);
pub(crate) const OPACITY: PropId = PropId(TRANSITION_PROP_BASE + 8);
pub(crate) const TRANSLATE_X: PropId = PropId(TRANSITION_PROP_BASE + 9);
pub(crate) const TRANSLATE_Y: PropId = PropId(TRANSITION_PROP_BASE + 10);
pub(crate) const ROTATE: PropId = PropId(TRANSITION_PROP_BASE + 11);
pub(crate) const SCALE_X: PropId = PropId(TRANSITION_PROP_BASE + 12);
pub(crate) const SCALE_Y: PropId = PropId(TRANSITION_PROP_BASE + 13);

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
