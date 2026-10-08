//! Tab groups split into rows and columns: the layout inside each region of
//! a [`crate::DockState`].
//!
//! A region holds a [`PaneNode`] tree. Leaves are [`TabGroup`]s, each an
//! ordered list of panels with one active; inner nodes split their children
//! along an axis by weight. Dropping a tab on a group's edge splits the
//! group, and a group emptied by a move or close is pruned so the tree stays
//! canonical: no empty groups below the root, no split with one child, and
//! no split directly inside a split of the same axis.

use serde::{Deserialize, Serialize};

use crate::dock::PanelId;
use crate::split::{Axis, DIVIDER_THICKNESS};

/// Identity of a tab group or split, unique across every host of a
/// [`crate::DockState`]. Not stable across [`crate::DockState::restore`],
/// which numbers them afresh.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PaneId(pub u32);

/// Where in a tab group a dragged tab lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropZone {
    /// Into the tab strip before tab `index`; past the last tab appends.
    Tabs(usize),
    /// Into the group, appended to its tabs.
    Center,
    /// A new group beside this one, splitting it.
    Left,
    Right,
    Top,
    Bottom,
}

/// Fraction of a group's body, from each edge, that splits it on a drop.
const EDGE_FRACTION: f32 = 0.3;

impl DropZone {
    /// The zone at `(x, y)` in a body `width` by `height`: the edge it is
    /// within [`EDGE_FRACTION`] of, the nearest edge first, else the center.
    pub fn in_body(width: f32, height: f32, x: f32, y: f32) -> Self {
        if width <= 0.0 || height <= 0.0 {
            return Self::Center;
        }
        let (fx, fy) = (x / width, y / height);
        let edges = [
            (fx, Self::Left),
            (1.0 - fx, Self::Right),
            (fy, Self::Top),
            (1.0 - fy, Self::Bottom),
        ];
        edges
            .into_iter()
            .filter(|(d, _)| *d < EDGE_FRACTION)
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map_or(Self::Center, |(_, zone)| zone)
    }

    /// For an edge zone: the axis of the split it makes, and whether the
    /// new group goes first.
    pub(crate) fn edge(self) -> Option<(Axis, bool)> {
        match self {
            Self::Left => Some((Axis::Horizontal, true)),
            Self::Right => Some((Axis::Horizontal, false)),
            Self::Top => Some((Axis::Vertical, true)),
            Self::Bottom => Some((Axis::Vertical, false)),
            Self::Tabs(_) | Self::Center => None,
        }
    }

    /// The part of a `width` by `height` body the drop would fill, as
    /// `(x, y, width, height)`: the half an edge drop gives the new group,
    /// or the whole body.
    pub fn preview(self, width: f32, height: f32) -> (f32, f32, f32, f32) {
        let (hw, hh) = ((width / 2.0).floor(), (height / 2.0).floor());
        match self {
            Self::Left => (0.0, 0.0, hw, height),
            Self::Right => (width - hw, 0.0, hw, height),
            Self::Top => (0.0, 0.0, width, hh),
            Self::Bottom => (0.0, height - hh, width, hh),
            Self::Tabs(_) | Self::Center => (0.0, 0.0, width, height),
        }
    }
}

/// A dragged tab's destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaneDrop {
    pub pane: PaneId,
    pub zone: DropZone,
}

/// A leaf: panels shown as tabs, one active.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TabGroup {
    pub id: PaneId,
    pub panels: Vec<PanelId>,
    pub active: usize,
}

impl TabGroup {
    pub(crate) fn new(id: PaneId) -> Self {
        Self {
            id,
            panels: Vec::new(),
            active: 0,
        }
    }

    pub fn active_panel(&self) -> Option<PanelId> {
        self.panels.get(self.active).copied()
    }

    /// Remove the panel at `index`. The neighbor after it becomes active (or
    /// before it, at the end).
    pub(crate) fn remove(&mut self, index: usize) -> Option<PanelId> {
        if index >= self.panels.len() {
            return None;
        }
        let removed = self.panels.remove(index);
        if index < self.active || self.active >= self.panels.len() {
            self.active = self.active.saturating_sub(1);
        }
        Some(removed)
    }

    /// Insert `panel` before `index` (clamped) and make it active.
    pub(crate) fn insert(&mut self, index: usize, panel: PanelId) {
        self.insert_all(index, &[panel], 0);
    }

    /// Insert `panels` in order before `index` (clamped), making
    /// `panels[active]` active.
    pub(crate) fn insert_all(&mut self, index: usize, panels: &[PanelId], active: usize) {
        let index = index.min(self.panels.len());
        self.panels.splice(index..index, panels.iter().copied());
        self.active = index + active.min(panels.len().saturating_sub(1));
    }
}

/// Children of a split, laid out along `axis` in proportion to `weights`,
/// which sum to one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaneSplit {
    pub id: PaneId,
    pub axis: Axis,
    pub children: Vec<PaneNode>,
    pub weights: Vec<f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PaneNode {
    Tabs(TabGroup),
    Split(PaneSplit),
}

impl PaneNode {
    /// Every tab group, in reading order.
    pub fn groups(&self) -> Vec<&TabGroup> {
        let mut out = Vec::new();
        self.collect_groups(&mut out);
        out
    }

    fn collect_groups<'a>(&'a self, out: &mut Vec<&'a TabGroup>) {
        match self {
            Self::Tabs(group) => out.push(group),
            Self::Split(split) => {
                for child in &split.children {
                    child.collect_groups(out);
                }
            }
        }
    }

    pub fn group(&self, id: PaneId) -> Option<&TabGroup> {
        match self {
            Self::Tabs(group) => (group.id == id).then_some(group),
            Self::Split(split) => split.children.iter().find_map(|c| c.group(id)),
        }
    }

    /// Whether any group holds a panel; walks the tree without collecting.
    pub(crate) fn has_panels(&self) -> bool {
        match self {
            Self::Tabs(group) => !group.panels.is_empty(),
            Self::Split(split) => split.children.iter().any(Self::has_panels),
        }
    }

    pub(crate) fn group_mut(&mut self, id: PaneId) -> Option<&mut TabGroup> {
        match self {
            Self::Tabs(group) => (group.id == id).then_some(group),
            Self::Split(split) => split.children.iter_mut().find_map(|c| c.group_mut(id)),
        }
    }

    pub(crate) fn split_mut(&mut self, id: PaneId) -> Option<&mut PaneSplit> {
        let Self::Split(split) = self else {
            return None;
        };
        if split.id == id {
            return Some(split);
        }
        split.children.iter_mut().find_map(|c| c.split_mut(id))
    }

    pub(crate) fn split(&self, id: PaneId) -> Option<&PaneSplit> {
        match self {
            Self::Tabs(_) => None,
            Self::Split(split) if split.id == id => Some(split),
            Self::Split(split) => split.children.iter().find_map(|c| c.split(id)),
        }
    }

    pub(crate) fn id(&self) -> PaneId {
        match self {
            Self::Tabs(group) => group.id,
            Self::Split(split) => split.id,
        }
    }

    /// Put `group` beside the group `target` along `axis`, first or
    /// second. A parent split of the same axis takes it as a sibling, with
    /// half of the target's weight; otherwise the target is replaced by a
    /// new split `split_id` of the two. Returns false when `target` is not
    /// in this tree.
    pub(crate) fn insert_beside(
        &mut self,
        target: PaneId,
        axis: Axis,
        first: bool,
        group: TabGroup,
        split_id: PaneId,
    ) -> bool {
        if let Self::Split(split) = self {
            if split.axis == axis
                && let Some(i) = split.children.iter().position(|c| c.id() == target)
                && matches!(split.children[i], Self::Tabs(_))
            {
                let half = split.weights[i] / 2.0;
                split.weights[i] = half;
                let at = if first { i } else { i + 1 };
                split.children.insert(at, Self::Tabs(group));
                split.weights.insert(at, half);
                return true;
            }
            let mut group = Some(group);
            for child in &mut split.children {
                if child.contains_group(target) {
                    let Some(g) = group.take() else { break };
                    return child.insert_beside(target, axis, first, g, split_id);
                }
            }
            return false;
        }
        if self.id() != target {
            return false;
        }
        let old = std::mem::replace(self, Self::Tabs(TabGroup::new(target)));
        let new = Self::Tabs(group);
        let children = if first {
            vec![new, old]
        } else {
            vec![old, new]
        };
        *self = Self::Split(PaneSplit {
            id: split_id,
            axis,
            children,
            weights: vec![0.5, 0.5],
        });
        true
    }

    /// Where to put the node `id` back beside its nearest group once it is
    /// gone: the first group of the sibling after it (the node going
    /// first), else the last group of the one before, and the split's axis.
    pub(crate) fn neighbor(&self, id: PaneId) -> Option<(PaneId, Axis, bool)> {
        let Self::Split(split) = self else {
            return None;
        };
        if let Some(i) = split.children.iter().position(|c| c.id() == id) {
            if let Some(next) = split.children.get(i + 1) {
                return Some((next.groups()[0].id, split.axis, true));
            }
            let before = &split.children[i.checked_sub(1)?];
            return Some((before.groups().last()?.id, split.axis, false));
        }
        split.children.iter().find_map(|c| c.neighbor(id))
    }

    fn contains_group(&self, id: PaneId) -> bool {
        match self {
            Self::Tabs(group) => group.id == id,
            Self::Split(split) => split.children.iter().any(|c| c.contains_group(id)),
        }
    }

    /// Drop empty groups, unwrap splits left with one child, and merge
    /// splits into a parent of the same axis. `None` when nothing is left.
    pub(crate) fn prune(self) -> Option<Self> {
        match self {
            Self::Tabs(group) => (!group.panels.is_empty()).then_some(Self::Tabs(group)),
            Self::Split(split) => {
                let mut children = Vec::with_capacity(split.children.len());
                let mut weights = Vec::with_capacity(split.children.len());
                for (child, weight) in split.children.into_iter().zip(split.weights) {
                    match child.prune() {
                        Some(Self::Split(inner)) if inner.axis == split.axis => {
                            for (c, w) in inner.children.into_iter().zip(inner.weights) {
                                children.push(c);
                                weights.push(w * weight);
                            }
                        }
                        Some(child) => {
                            children.push(child);
                            weights.push(weight);
                        }
                        None => {}
                    }
                }
                match children.len() {
                    0 => None,
                    1 => children.pop(),
                    _ => {
                        let total: f32 = weights.iter().sum();
                        for w in &mut weights {
                            *w /= total;
                        }
                        Some(Self::Split(PaneSplit {
                            id: split.id,
                            axis: split.axis,
                            children,
                            weights,
                        }))
                    }
                }
            }
        }
    }

    /// Give every node a fresh id from `next`, recording `(old, new)` in
    /// `map`, and replace weights that are not usable (wrong count, not
    /// finite, not positive) with equal ones.
    pub(crate) fn renumber(&mut self, next: &mut u32, map: &mut Vec<(PaneId, PaneId)>) {
        let id = PaneId(*next);
        *next += 1;
        map.push((self.id(), id));
        match self {
            Self::Tabs(group) => {
                group.id = id;
                group.active = group.active.min(group.panels.len().saturating_sub(1));
            }
            Self::Split(split) => {
                split.id = id;
                let n = split.children.len();
                if split.weights.len() != n
                    || split.weights.iter().any(|w| !(w.is_finite() && *w > 0.0))
                {
                    split.weights = vec![1.0; n];
                }
                for child in &mut split.children {
                    child.renumber(next, map);
                }
            }
        }
    }

    /// Lay the tree out in `rect`, reporting each group's rect.
    pub(crate) fn layout(&self, rect: Rect, out: &mut Vec<(PaneId, Rect)>) {
        match self {
            Self::Tabs(group) => out.push((group.id, rect)),
            Self::Split(split) => {
                let horizontal = split.axis == Axis::Horizontal;
                let extent = if horizontal { rect.width } else { rect.height };
                let mut at = 0.0;
                for (child, size) in split
                    .children
                    .iter()
                    .zip(child_sizes(&split.weights, extent))
                {
                    let child_rect = if horizontal {
                        Rect::new(rect.x + at, rect.y, size, rect.height)
                    } else {
                        Rect::new(rect.x, rect.y + at, rect.width, size)
                    };
                    child.layout(child_rect, out);
                    at += size + DIVIDER_THICKNESS;
                }
            }
        }
    }
}

/// An axis aligned rectangle in points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    pub(crate) const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub(crate) fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.width && y < self.y + self.height
    }
}

/// Whole point sizes of a split's children in `extent`, after the dividers
/// between them. The last child takes the rounding remainder, so the sizes
/// fill the extent exactly.
pub(crate) fn child_sizes(weights: &[f32], extent: f32) -> Vec<f32> {
    let n = weights.len();
    let avail = (extent - n.saturating_sub(1) as f32 * DIVIDER_THICKNESS).max(0.0);
    let mut sizes: Vec<f32> = weights.iter().map(|w| (w * avail).round()).collect();
    if let Some((last, rest)) = sizes.split_last_mut() {
        *last = (avail - rest.iter().sum::<f32>()).max(0.0);
    }
    sizes
}

/// Smallest a child gets when a divider of a split moves: `nominal`, or
/// an equal share of `avail` among `count` children when they cannot all
/// have that much.
pub(crate) fn divider_min(nominal: f32, avail: f32, count: usize) -> f32 {
    nominal.min(avail / count.max(1) as f32)
}

/// Where divider `divider` of children `sizes` is, in points from the
/// split's start (dividers before it included), and the least and most it
/// can be: what [`push_divider`] reaches by shrinking every child on one
/// side to `min`, or leaving one already below it where it is.
pub(crate) fn divider_span(sizes: &[f32], min: f32, divider: usize) -> (f32, f32, f32) {
    let split = (divider + 1).min(sizes.len());
    let (before, after) = sizes.split_at(split);
    let slack = |sizes: &[f32]| sizes.iter().map(|s| (s - min).max(0.0)).sum::<f32>();
    let at = before.iter().sum::<f32>() + divider as f32 * DIVIDER_THICKNESS;
    (at, at - slack(before), at + slack(after))
}

/// Move divider `divider` (between children `divider` and `divider + 1`)
/// by `delta` points. The child in front of the motion shrinks first; once
/// it is at `min`, the next one beyond it does, and so on. The child behind
/// the divider grows by what they gave.
pub(crate) fn push_divider(sizes: &mut [f32], min: f32, divider: usize, delta: f32) {
    if divider + 1 >= sizes.len() || delta == 0.0 {
        return;
    }
    let (grow, donors): (usize, Vec<usize>) = if delta > 0.0 {
        (divider, (divider + 1..sizes.len()).collect())
    } else {
        (divider + 1, (0..=divider).rev().collect())
    };
    let mut want = delta.abs();
    for i in donors {
        let give = (sizes[i] - min).clamp(0.0, want);
        sizes[i] -= give;
        sizes[grow] += give;
        want -= give;
        if want <= 0.0 {
            break;
        }
    }
}

#[cfg(kani)]
mod verification {
    use super::*;

    const N: usize = 4;

    /// Every divider of four children at their minimum or above, moved by
    /// any whole amount. `divider` is concrete: a symbolic one gives
    /// `push_divider`'s donor list a symbolic length, which CBMC cannot
    /// handle in memory.
    fn check(divider: usize) {
        // Whole points below 64, so every difference is exact in `f32`.
        let min = f32::from(kani::any::<u8>() % 64);
        let mut sizes = [0.0f32; N];
        for size in &mut sizes {
            *size = f32::from(kani::any::<u8>() % 64);
            kani::assume(*size >= min);
        }
        let delta = f32::from(kani::any::<i8>());
        let before = sizes;

        push_divider(&mut sizes, min, divider, delta);

        let (grow, donors): (usize, &[usize]) = match delta > 0.0 {
            true => (divider, &[1, 2, 3][divider..]),
            false => (divider + 1, &[2, 1, 0][N - 2 - divider..]),
        };
        // The child behind the divider grows by at most the motion.
        // (Conservation of the total is left to the unit test: proving
        // float sums equal runs CBMC out of memory.)
        assert!(sizes[grow] >= before[grow] && sizes[grow] - before[grow] <= delta.abs());
        let mut earlier_at_min = true;
        for i in 0..N {
            assert!(sizes[i] >= min);
            if i != grow && !donors.contains(&i) {
                assert!(sizes[i] == before[i]);
            }
        }
        for &i in donors {
            assert!(sizes[i] <= before[i]);
            // A child gives only once every child nearer the divider is at
            // its minimum.
            if sizes[i] < before[i] {
                assert!(earlier_at_min);
            }
            earlier_at_min &= sizes[i] == min;
        }
    }

    #[kani::proof]
    #[kani::unwind(6)]
    // Kissat took 607 s where the default CaDiCaL took 1133 s.
    #[kani::solver(kissat)]
    fn push_divider_keeps_minimums_and_drains_nearest_first() {
        check(0);
        check(1);
        check(2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_point_in_a_body_picks_the_nearest_edge_zone() {
        // A 300 x 200 body; edges are the outer 30%.
        let cases = [
            ((150.0, 100.0), DropZone::Center),
            ((20.0, 100.0), DropZone::Left),
            ((280.0, 100.0), DropZone::Right),
            ((150.0, 10.0), DropZone::Top),
            ((150.0, 190.0), DropZone::Bottom),
            // In two edge bands, the nearer edge wins.
            ((10.0, 40.0), DropZone::Left),
            ((60.0, 5.0), DropZone::Top),
        ];
        for ((x, y), zone) in cases {
            assert_eq!(DropZone::in_body(300.0, 200.0, x, y), zone, "({x}, {y})");
        }
    }

    // Catches a divider announcing a range it cannot reach: nominal
    // minimums where children already sit below them, or slack counted
    // from children on the wrong side.
    #[test]
    fn a_divider_spans_what_pushing_its_neighbors_can_reach() {
        // (sizes, divider, (position, least, most)), min 100, 1 point
        // dividers.
        type Case = (&'static [f32], usize, (f32, f32, f32));
        let cases: &[Case] = &[
            (&[200.0, 200.0, 200.0], 0, (200.0, 100.0, 400.0)),
            (&[200.0, 200.0, 200.0], 1, (401.0, 201.0, 501.0)),
            // A child below the minimum gives nothing and takes its share.
            (&[150.0, 60.0], 0, (150.0, 100.0, 150.0)),
            // Too small for anyone to give: the divider cannot move.
            (&[60.0, 60.0, 60.0], 1, (121.0, 121.0, 121.0)),
        ];
        for &(sizes, divider, expected) in cases {
            assert_eq!(
                divider_span(sizes, 100.0, divider),
                expected,
                "{sizes:?} divider {divider}"
            );
        }
    }

    #[test]
    fn a_pushed_divider_cascades_through_children_at_their_minimum() {
        // (sizes, divider, delta, expected), min 100.
        let cases: &[(&[f32], usize, f32, &[f32])] = &[
            (&[200.0, 200.0, 200.0], 0, 50.0, &[250.0, 150.0, 200.0]),
            (&[200.0, 200.0, 200.0], 0, 150.0, &[350.0, 100.0, 150.0]),
            // Every child ahead is at its minimum: the rest is refused.
            (&[200.0, 200.0, 200.0], 0, 500.0, &[400.0, 100.0, 100.0]),
            (&[200.0, 200.0, 200.0], 1, -150.0, &[150.0, 100.0, 350.0]),
        ];
        for &(sizes, divider, delta, expected) in cases {
            let mut out = sizes.to_vec();
            push_divider(&mut out, 100.0, divider, delta);
            assert_eq!(out, expected, "{sizes:?} divider {divider} by {delta}");
        }
    }
}
