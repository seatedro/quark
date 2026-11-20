//! The pointer gesture under way over a dock. There is one pointer, so one
//! gesture at a time: a press or a hover starts one in place of any other,
//! and only its own events end it.

use super::{MovePayload, PanelId};
use crate::pane_tree::{PaneDrop, PaneId};

#[derive(Debug, Clone, PartialEq, Default)]
pub(super) enum Gesture {
    #[default]
    Idle,
    /// Divider `divider` of `split` is pressed, the split's weights `origin`
    /// at the press: every move lays the split out from them again.
    Divider {
        split: PaneId,
        divider: usize,
        origin: Vec<f32>,
    },
    /// A tab dragged inside its window, over `target` or over nowhere it
    /// can land.
    Tab {
        panel: PanelId,
        target: Option<PaneDrop>,
    },
    /// A drag the app routes, a group's grip or a drag between windows,
    /// over `target`.
    Routed {
        payload: MovePayload,
        target: PaneDrop,
    },
}

impl Gesture {
    /// A drag the app routes moved over `hover`; over nowhere, it ends.
    pub(super) fn route(self, hover: Option<(MovePayload, PaneDrop)>) -> Self {
        match (self, hover) {
            (_, Some((payload, target))) => Self::Routed { payload, target },
            (Self::Routed { .. }, None) => Self::Idle,
            (gesture, None) => gesture,
        }
    }

    /// The tab drag was dropped or cancelled.
    pub(super) fn end_tab(self) -> Self {
        match self {
            Self::Tab { .. } => Self::Idle,
            gesture => gesture,
        }
    }

    /// The pressed divider was let go.
    pub(super) fn release_divider(self) -> Self {
        match self {
            Self::Divider { .. } => Self::Idle,
            gesture => gesture,
        }
    }

    /// The weights a move of `divider` of `split` lays out from, while it
    /// is the divider pressed.
    pub(super) fn divider_origin(&self, split: PaneId, divider: usize) -> Option<&[f32]> {
        match self {
            Self::Divider {
                split: pressed,
                divider: at,
                origin,
            } if (*pressed, *at) == (split, divider) => Some(origin),
            _ => None,
        }
    }

    /// What the gesture would move, and where: the dock previews it while
    /// the move is allowed.
    pub(super) fn landing(&self) -> Option<(MovePayload, PaneDrop)> {
        match *self {
            Self::Tab {
                panel,
                target: Some(target),
            } => Some((MovePayload::Panel(panel), target)),
            Self::Routed { payload, target } => Some((payload, target)),
            Self::Idle | Self::Divider { .. } | Self::Tab { target: None, .. } => None,
        }
    }
}
