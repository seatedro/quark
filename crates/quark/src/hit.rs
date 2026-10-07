//! Pointer hit testing: one table per frame, in paint order, that answers
//! "what is under the pointer" for hover, click, wheel, and drag alike.

use crate::geometry::Rect;

/// Requested cursor shape for a hit region.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CursorHint {
    #[default]
    Default,
    Pointer,
    Text,
    ResizeCol,
    ResizeRow,
    ResizeNs,
    ResizeEw,
    ResizeNesw,
    ResizeNwse,
    Move,
    Grab,
    Grabbing,
    NotAllowed,
    Wait,
    Progress,
    Crosshair,
    Help,
}

/// Opaque identity payload for hover routing. Lets halogen-owned code
/// answer "which file/toast/entry is hovered?" without pattern-matching on
/// the app's action enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HitIdentity {
    File(usize),
    Toast(usize),
    OverlayEntry(usize),
    OverlayBackdrop,
}

/// A pointer click at a specific position, in hit table coordinates.
#[derive(Debug, Clone, Copy)]
pub struct ClickEvent {
    pub x: f32,
    pub y: f32,
}

/// A tooltip-bearing rectangle collected during paint. The host app drains
/// these off the built UI frame each tick and renders the tooltip when the
/// cursor lingers inside `bounds`.
#[derive(Debug, Clone)]
pub struct TooltipRegion {
    pub bounds: Rect,
    pub text: String,
}

/// Index of an entry in a [`HitTable`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HitId(u32);

impl HitId {
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// What a hit entry responds to.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct HitFlags(u8);

impl HitFlags {
    pub const NONE: Self = Self(0);
    /// Entries below this one (by z, then paint order) see neither hover nor
    /// clicks at points this entry covers.
    pub const BLOCKS_MOUSE: Self = Self(1);
    pub const HOVER: Self = Self(1 << 1);
    pub const CLICK: Self = Self(1 << 2);
    pub const DRAG: Self = Self(1 << 3);
    pub const SCROLL: Self = Self(1 << 4);
    pub const TEXT: Self = Self(1 << 5);

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl std::ops::BitOr for HitFlags {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self {
        self.union(rhs)
    }
}

impl std::ops::BitOrAssign for HitFlags {
    fn bitor_assign(&mut self, rhs: Self) {
        *self = self.union(rhs);
    }
}

/// Clip of an entry with no clipping ancestor.
pub const UNCLIPPED: Rect = Rect {
    x: -1.0e9,
    y: -1.0e9,
    width: 2.0e9,
    height: 2.0e9,
};

/// Clip of an entry whose ancestor clips do not overlap. `Rect::contains`
/// is edge-inclusive, so a zero-size rect would still contain its origin.
pub const EMPTY_CLIP: Rect = Rect {
    x: 0.0,
    y: 0.0,
    width: -1.0,
    height: -1.0,
};

const NO_NODE: u32 = u32::MAX;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HitTableIntegrityError {
    ColumnLength {
        column: &'static str,
        len: usize,
        expected: usize,
    },
}

/// Every pointer-interactive rectangle of one frame, one row per entry,
/// stored column-wise. Rows are in paint order; `push` order is the
/// tiebreak between entries of equal z.
#[derive(Debug, Clone, Default)]
pub struct HitTable {
    bounds: Vec<Rect>,
    /// Intersection of every ancestor clip, in the same space as `bounds`.
    clip: Vec<Rect>,
    z: Vec<i32>,
    /// Semantic node index, or `NO_NODE` until the owner binds it in paint.
    node: Vec<u32>,
    flags: Vec<HitFlags>,
    cursor: Vec<CursorHint>,
    identity: Vec<Option<HitIdentity>>,
}

impl HitTable {
    pub fn push(
        &mut self,
        bounds: Rect,
        clip: Rect,
        z: i32,
        flags: HitFlags,
        cursor: CursorHint,
    ) -> HitId {
        let id = HitId(self.bounds.len() as u32);
        self.bounds.push(bounds);
        self.clip.push(clip);
        self.z.push(z);
        self.node.push(NO_NODE);
        self.flags.push(flags);
        self.cursor.push(cursor);
        self.identity.push(None);
        debug_assert_eq!(self.verify_integrity(), Ok(()));
        id
    }

    pub fn set_node(&mut self, id: HitId, node: usize) {
        if let Some(slot) = self.node.get_mut(id.index()) {
            *slot = u32::try_from(node).unwrap_or(NO_NODE);
        }
    }

    pub fn set_identity(&mut self, id: HitId, identity: Option<HitIdentity>) {
        if let Some(slot) = self.identity.get_mut(id.index()) {
            *slot = identity;
        }
    }

    pub fn len(&self) -> usize {
        self.bounds.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bounds.is_empty()
    }

    pub fn bounds(&self, id: HitId) -> Rect {
        self.bounds[id.index()]
    }

    /// Every entry, in paint order.
    pub fn ids(&self) -> impl Iterator<Item = HitId> + use<> {
        (0..self.bounds.len() as u32).map(HitId)
    }

    /// Intersection of the entry's ancestor clips.
    pub fn clip(&self, id: HitId) -> Rect {
        self.clip[id.index()]
    }

    pub fn z(&self, id: HitId) -> i32 {
        self.z[id.index()]
    }

    pub fn node(&self, id: HitId) -> Option<usize> {
        let node = self.node[id.index()];
        (node != NO_NODE).then_some(node as usize)
    }

    pub fn flags(&self, id: HitId) -> HitFlags {
        self.flags[id.index()]
    }

    pub fn cursor(&self, id: HitId) -> CursorHint {
        self.cursor[id.index()]
    }

    pub fn identity(&self, id: HitId) -> Option<HitIdentity> {
        self.identity[id.index()]
    }

    /// Entries under `(x, y)`, topmost first: higher z wins, then later
    /// paint order. A point counts only when both the bounds and the clip
    /// contain it. The list ends at the first `BLOCKS_MOUSE` entry
    /// (inclusive), so nothing beneath a blocker is hovered or clicked.
    pub fn stack_at(&self, x: f32, y: f32) -> Vec<HitId> {
        let mut stack: Vec<HitId> = (0..self.bounds.len())
            .filter(|&i| self.bounds[i].contains(x, y) && self.clip[i].contains(x, y))
            .map(|i| HitId(i as u32))
            .collect();
        stack.sort_unstable_by(|a, b| (self.z[b.index()], b.0).cmp(&(self.z[a.index()], a.0)));
        if let Some(blocker) = stack
            .iter()
            .position(|id| self.flags(*id).contains(HitFlags::BLOCKS_MOUSE))
        {
            stack.truncate(blocker + 1);
        }
        stack
    }

    pub fn verify_integrity(&self) -> Result<(), HitTableIntegrityError> {
        let expected = self.bounds.len();
        let columns = [
            ("clip", self.clip.len()),
            ("z", self.z.len()),
            ("node", self.node.len()),
            ("flags", self.flags.len()),
            ("cursor", self.cursor.len()),
            ("identity", self.identity.len()),
        ];
        for (column, len) in columns {
            if len != expected {
                return Err(HitTableIntegrityError::ColumnLength {
                    column,
                    len,
                    expected,
                });
            }
        }
        Ok(())
    }
}
