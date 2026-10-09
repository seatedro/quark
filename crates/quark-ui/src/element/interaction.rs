//! Interaction groups: content that shows while a row is hovered or holds
//! focus, without the row changing size.
//!
//! A div marked [`Div::interaction_group`] is a group; a descendant marked
//! [`Div::show_when`] is a slot whose visibility follows the nearest
//! enclosing group with the same [`GroupId`]. Ids are names, scoped by
//! nesting: every row of a list can use the same id, and a slot refers to
//! its own row.
//!
//! A hidden slot keeps its layout space. It paints nothing, registers no
//! hits, handlers, semantic or accessibility nodes, so it is neither
//! clickable nor in the Tab order. A slot that holds the focused element
//! always shows, whatever its condition.

/// Name of an interaction group, scoped to the nearest enclosing group
/// with that name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GroupId(u64);

impl GroupId {
    pub const fn new(name: &str) -> Self {
        Self(quark::stable_hash(name))
    }

    /// An id the caller derived itself.
    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }
}

impl From<&str> for GroupId {
    fn from(name: &str) -> Self {
        Self::new(name)
    }
}

/// When a [`Div::show_when`] slot shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GroupCondition {
    /// While the pointer is over the group (or a shown descendant inside
    /// it) or the focused element is inside the group: row actions.
    HoveredOrFocusWithin(GroupId),
    /// The inverse: while the group is neither hovered nor holds focus,
    /// e.g. a timestamp the actions replace.
    Idle(GroupId),
}

impl GroupCondition {
    pub fn group(self) -> GroupId {
        match self {
            Self::HoveredOrFocusWithin(id) | Self::Idle(id) => id,
        }
    }

    /// Whether the slot shows for a group that is `active` (hovered or
    /// holding focus).
    pub fn shows(self, active: bool) -> bool {
        match self {
            Self::HoveredOrFocusWithin(_) => active,
            Self::Idle(_) => !active,
        }
    }
}
