//! Pointer hit testing: one table per frame, in paint order, that answers
//! "what is under the pointer" for hover, click, wheel, and drag alike.

use std::sync::atomic::{AtomicU32, Ordering};

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

/// Opaque identity payload for hover routing. Lets quark-owned code
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

/// An entry in one [`HitTable`]. Each table gets a fresh frame stamp, so
/// an id kept past its frame resolves to nothing instead of aliasing the
/// entry that reuses its index in a later table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HitId {
    frame: u32,
    index: u32,
}

/// Source of [`HitTable`] frame stamps. Starts at 1 so a zeroed id never
/// matches a table.
static NEXT_FRAME: AtomicU32 = AtomicU32::new(1);

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
#[derive(Debug, Clone)]
pub struct HitTable {
    /// Stamp carried by every [`HitId`] this table issues.
    frame: u32,
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

impl Default for HitTable {
    fn default() -> Self {
        Self {
            frame: NEXT_FRAME.fetch_add(1, Ordering::Relaxed),
            bounds: Vec::new(),
            clip: Vec::new(),
            z: Vec::new(),
            node: Vec::new(),
            flags: Vec::new(),
            cursor: Vec::new(),
            identity: Vec::new(),
        }
    }
}

impl HitTable {
    /// Row of `id` in this table, or `None` for an id another table issued.
    pub fn row(&self, id: HitId) -> Option<usize> {
        let index = id.index as usize;
        (id.frame == self.frame && index < self.bounds.len()).then_some(index)
    }

    fn id(&self, index: usize) -> HitId {
        HitId {
            frame: self.frame,
            index: index as u32,
        }
    }

    pub fn push(
        &mut self,
        bounds: Rect,
        clip: Rect,
        z: i32,
        flags: HitFlags,
        cursor: CursorHint,
    ) -> HitId {
        let id = self.id(self.bounds.len());
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
        if let Some(row) = self.row(id) {
            self.node[row] = u32::try_from(node).unwrap_or(NO_NODE);
        }
    }

    pub fn set_identity(&mut self, id: HitId, identity: Option<HitIdentity>) {
        if let Some(row) = self.row(id) {
            self.identity[row] = identity;
        }
    }

    pub fn len(&self) -> usize {
        self.bounds.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bounds.is_empty()
    }

    pub fn bounds(&self, id: HitId) -> Option<Rect> {
        Some(self.bounds[self.row(id)?])
    }

    /// Every entry, in paint order.
    pub fn ids(&self) -> impl Iterator<Item = HitId> + '_ {
        (0..self.bounds.len()).map(|i| self.id(i))
    }

    /// Intersection of the entry's ancestor clips.
    pub fn clip(&self, id: HitId) -> Option<Rect> {
        Some(self.clip[self.row(id)?])
    }

    pub fn z(&self, id: HitId) -> Option<i32> {
        Some(self.z[self.row(id)?])
    }

    pub fn node(&self, id: HitId) -> Option<usize> {
        let node = self.node[self.row(id)?];
        (node != NO_NODE).then_some(node as usize)
    }

    pub fn flags(&self, id: HitId) -> Option<HitFlags> {
        Some(self.flags[self.row(id)?])
    }

    pub fn cursor(&self, id: HitId) -> Option<CursorHint> {
        Some(self.cursor[self.row(id)?])
    }

    pub fn identity(&self, id: HitId) -> Option<HitIdentity> {
        self.identity[self.row(id)?]
    }

    /// Entries under `(x, y)`, topmost first: higher z wins, then later
    /// paint order. A point counts only when both the bounds and the clip
    /// contain it. The list ends at the first `BLOCKS_MOUSE` entry
    /// (inclusive), so nothing beneath a blocker is hovered or clicked.
    pub fn stack_at(&self, x: f32, y: f32) -> Vec<HitId> {
        let mut stack = Vec::new();
        self.stack_at_into(x, y, &mut stack);
        stack
    }

    /// [`Self::stack_at`] into `out`, replacing its contents, so a caller
    /// that keeps `out` reuses its buffer.
    pub fn stack_at_into(&self, x: f32, y: f32, out: &mut Vec<HitId>) {
        out.clear();
        out.extend(
            (0..self.bounds.len())
                .filter(|&i| self.bounds[i].contains(x, y) && self.clip[i].contains(x, y))
                .map(|i| self.id(i)),
        );
        let z = |id: &HitId| self.z[id.index as usize];
        out.sort_unstable_by_key(|id| std::cmp::Reverse((z(id), id.index)));
        if let Some(blocker) = out
            .iter()
            .position(|id| self.flags[id.index as usize].contains(HitFlags::BLOCKS_MOUSE))
        {
            out.truncate(blocker + 1);
        }
    }

    /// Empty the table for a new frame, keeping its buffers. Ids issued
    /// before stop resolving, as with a new table.
    pub fn reset(&mut self) {
        self.frame = NEXT_FRAME.fetch_add(1, Ordering::Relaxed);
        self.bounds.clear();
        self.clip.clear();
        self.z.clear();
        self.node.clear();
        self.flags.clear();
        self.cursor.clear();
        self.identity.clear();
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

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect {
            x,
            y,
            width: w,
            height: h,
        }
    }

    /// `(bounds, clip, z, blocks_mouse)`.
    type Row = (Rect, Rect, i32, bool);

    /// Push `(bounds, clip, z, blocks_mouse)` rows and return the rows of
    /// `stack_at(5, 5)`, topmost first.
    fn stack(rows: &[Row]) -> Vec<usize> {
        let mut table = HitTable::default();
        for &(bounds, clip, z, blocks) in rows {
            let flags = if blocks {
                HitFlags::BLOCKS_MOUSE
            } else {
                HitFlags::HOVER
            };
            table.push(bounds, clip, z, flags, CursorHint::Default);
        }
        table
            .stack_at(5.0, 5.0)
            .into_iter()
            .map(|id| table.row(id).unwrap())
            .collect()
    }

    #[test]
    fn stack_at_orders_by_z_then_paint_and_honors_clip_and_blockers() {
        let on = rect(0.0, 0.0, 10.0, 10.0);
        let off = rect(20.0, 20.0, 10.0, 10.0);
        let cases: &[(&str, &[Row], &[usize])] = &[
            (
                "later paint wins at equal z",
                &[(on, UNCLIPPED, 0, false), (on, UNCLIPPED, 0, false)],
                &[1, 0],
            ),
            (
                "higher z beats later paint",
                &[(on, UNCLIPPED, 1, false), (on, UNCLIPPED, 0, false)],
                &[0, 1],
            ),
            (
                "bounds miss",
                &[(off, UNCLIPPED, 0, false), (on, UNCLIPPED, 0, false)],
                &[1],
            ),
            (
                "clip excludes the point",
                &[(on, off, 0, false), (on, UNCLIPPED, 0, false)],
                &[1],
            ),
            (
                "empty clip excludes its origin",
                &[(on, EMPTY_CLIP, 0, false)],
                &[],
            ),
            (
                "blocker hides what is beneath",
                &[
                    (on, UNCLIPPED, 0, false),
                    (on, UNCLIPPED, 0, true),
                    (on, UNCLIPPED, 1, false),
                ],
                &[2, 1],
            ),
            (
                "clipped blocker blocks nothing",
                &[(on, UNCLIPPED, 0, false), (on, off, 1, true)],
                &[0],
            ),
        ];
        for (name, rows, expected) in cases {
            assert_eq!(stack(rows), *expected, "{name}");
        }
    }

    #[test]
    fn id_from_another_table_resolves_to_nothing() {
        let mut old = HitTable::default();
        let stale = old.push(
            rect(0.0, 0.0, 1.0, 1.0),
            UNCLIPPED,
            0,
            HitFlags::HOVER,
            CursorHint::Pointer,
        );
        let mut new = HitTable::default();
        new.push(
            rect(0.0, 0.0, 1.0, 1.0),
            UNCLIPPED,
            0,
            HitFlags::HOVER,
            CursorHint::Text,
        );
        assert_eq!(new.cursor(stale), None);
        assert_eq!(old.cursor(stale), Some(CursorHint::Pointer));
    }
}
