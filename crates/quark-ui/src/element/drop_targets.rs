//! Drop targets: the places a frame says a drag may land, published with
//! the frame like its [`LayoutSnapshot`].
//!
//! An element that accepts drops (a dock's tab group, a list that takes
//! dragged rows) adds a [`DropTarget`] while it paints, through
//! [`ElementContext::add_drop_target`]. The context records it under the
//! current transform, clip, and z-index, and the finished frame carries the
//! rows as its [`DropTargets`] ([`InputFrame::drop_targets`]). A drag that
//! leaves its element's router (see [`DragSession`]) resolves window points
//! against the last completed frame of the window under the pointer, so it
//! needs no element of that window to be alive or to receive input.
//!
//! Every row names the frame that painted it and a model revision the
//! contributor supplies. A target remembered from an earlier frame, or
//! painted from a model that has changed since, is stale: callers check
//! with [`DropTargets::revalidate`] and [`DropPolicy::revision`] rather
//! than committing it blindly.

use super::*;

/// Names one drop target across frames and windows. `scope` names the
/// contributor (one dock among several), `key` the target inside it (a
/// pane). Both are the contributor's choice; quark compares them only for
/// equality.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DropTargetId {
    pub scope: u64,
    pub key: u64,
}

/// One target as an element adds it during paint. Rects are in the
/// element's layout coordinates, the ones it paints in.
#[derive(Debug, Clone, Copy)]
pub struct DropTarget<'a> {
    pub id: DropTargetId,
    /// The revision of the contributor's model this frame was painted
    /// from. A hit carries it, so a drop resolved against a frame painted
    /// before the model changed is refused as stale.
    pub revision: u64,
    /// Where the target takes drops.
    pub layout: Rect,
    /// Insertion places inside the target, such as the gaps of a tab
    /// strip; a hit reports the first one containing the point.
    pub slots: &'a [Rect],
}

/// Where a window point landed among a frame's drop targets.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DropTargetHit {
    pub id: DropTargetId,
    /// The revision the target was painted at.
    pub revision: u64,
    /// The frame whose targets were hit.
    pub frame: FrameId,
    /// The window point that was resolved.
    pub point: (f32, f32),
    /// The same point relative to the target's layout box, in layout
    /// coordinates.
    pub local: (f32, f32),
    /// Index of the insertion slot containing the point, if any.
    pub slot: Option<usize>,
    /// Axis-aligned bounds of the transformed target, in window points.
    pub bounds: Rect,
}

impl DropTargetHit {
    /// Whether `other` names the same place: target, revision, and slot.
    pub fn same_place(&self, other: &DropTargetHit) -> bool {
        self.id == other.id && self.revision == other.revision && self.slot == other.slot
    }
}

/// Why a remembered hit no longer holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StaleTarget {
    /// The current frame has no target under the remembered point.
    Gone,
    /// The current frame puts another target, revision, or slot there.
    Moved,
}

/// The drop targets of one painted frame, in paint order, stored
/// column-wise.
#[derive(Debug, Clone, Default)]
pub struct DropTargets {
    frame: FrameId,
    id: Vec<DropTargetId>,
    revision: Vec<u64>,
    layout: Vec<Rect>,
    transform: Vec<Transform2D>,
    /// Ancestor clips in window space.
    window_clip: Vec<Rect>,
    z: Vec<i32>,
    /// Each row's range in `slot_rects`.
    slots: Vec<(u32, u32)>,
    slot_rects: Vec<Rect>,
}

/// One row as the context pushes it.
pub(crate) struct DropTargetRow<'a> {
    pub(crate) target: DropTarget<'a>,
    pub(crate) transform: Transform2D,
    pub(crate) window_clip: Rect,
    pub(crate) z: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DropTargetsIntegrityError {
    ColumnLength {
        column: &'static str,
        len: usize,
        rows: usize,
    },
    /// A row's slot range is reversed, out of bounds, or not right after
    /// the previous row's.
    SlotRange { row: usize },
}

impl DropTargets {
    /// The frame these targets came from; `FrameId::default()` before a
    /// window has painted.
    pub fn frame(&self) -> FrameId {
        self.frame
    }

    pub fn len(&self) -> usize {
        self.id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.id.is_empty()
    }

    /// The topmost target under window point `(x, y)`: higher z-index
    /// wins, then later paint order, as for pointer hits. A target counts
    /// where its transformed layout box and every ancestor clip contain
    /// the point; one under a transform that flattens it (a zero scale)
    /// takes no drops. A clip under a rotation counts as its axis-aligned
    /// bounds, as in [`ElementGeometry::visible`].
    pub fn at(&self, x: f32, y: f32) -> Option<DropTargetHit> {
        let mut best: Option<(i32, usize, (f32, f32))> = None;
        for row in 0..self.len() {
            if best.is_some_and(|(z, _, _)| z > self.z[row])
                || !self.window_clip[row].contains(x, y)
            {
                continue;
            }
            let Some(inverse) = self.transform[row].invert() else {
                continue;
            };
            let (lx, ly) = inverse.apply(x, y);
            if self.layout[row].contains(lx, ly) {
                // Rows run in paint order, so a later row at the same z
                // replaces the earlier one.
                best = Some((self.z[row], row, (lx, ly)));
            }
        }
        best.map(|(_, row, local)| self.hit(row, (x, y), local))
    }

    /// Where target `id` landed in this frame: the topmost row by that id.
    pub fn geometry(&self, id: DropTargetId) -> Option<ElementGeometry> {
        let row = (0..self.len())
            .filter(|&row| self.id[row] == id)
            .max_by_key(|&row| (self.z[row], row))?;
        let bounds = window_rect(self.transform[row], self.layout[row]);
        Some(ElementGeometry {
            layout: self.layout[row],
            transform: self.transform[row],
            bounds,
            visible: bounds.intersection(self.window_clip[row]),
            frame: self.frame,
        })
    }

    /// Check a hit resolved earlier, possibly against another frame,
    /// against this one: the same point must still land on the same
    /// target, revision, and slot. Returns the hit as this frame sees it.
    /// A layout that shifted under a still pointer is refused rather than
    /// re-aimed, so a drop never lands somewhere the user was not shown.
    pub fn revalidate(&self, hit: &DropTargetHit) -> Result<DropTargetHit, StaleTarget> {
        let (x, y) = hit.point;
        let current = self.at(x, y).ok_or(StaleTarget::Gone)?;
        if current.same_place(hit) {
            Ok(current)
        } else {
            Err(StaleTarget::Moved)
        }
    }

    fn hit(&self, row: usize, point: (f32, f32), (lx, ly): (f32, f32)) -> DropTargetHit {
        let (start, end) = self.slots[row];
        let slot = self.slot_rects[start as usize..end as usize]
            .iter()
            .position(|slot| slot.contains(lx, ly));
        let layout = self.layout[row];
        DropTargetHit {
            id: self.id[row],
            revision: self.revision[row],
            frame: self.frame,
            point,
            local: (lx - layout.x, ly - layout.y),
            slot,
            bounds: window_rect(self.transform[row], layout),
        }
    }

    /// Empty the registry for a new frame, keeping its buffers.
    pub(crate) fn clear(&mut self) {
        self.frame = FrameId::default();
        self.id.clear();
        self.revision.clear();
        self.layout.clear();
        self.transform.clear();
        self.window_clip.clear();
        self.z.clear();
        self.slots.clear();
        self.slot_rects.clear();
    }

    /// Stamp the rows with the frame that painted them.
    pub(crate) fn set_frame(&mut self, frame: FrameId) {
        self.frame = frame;
    }

    pub(crate) fn push(&mut self, row: DropTargetRow) {
        let target = row.target;
        let start = self.slot_rects.len() as u32;
        self.slot_rects.extend_from_slice(target.slots);
        self.slots.push((start, self.slot_rects.len() as u32));
        self.id.push(target.id);
        self.revision.push(target.revision);
        self.layout.push(target.layout);
        self.transform.push(row.transform);
        self.window_clip.push(row.window_clip);
        self.z.push(row.z);
        debug_assert_eq!(self.verify_integrity(), Ok(()));
    }

    /// Every column has one entry per row, and the slot ranges tile
    /// `slot_rects` in row order.
    pub fn verify_integrity(&self) -> Result<(), DropTargetsIntegrityError> {
        let rows = self.id.len();
        let columns = [
            ("revision", self.revision.len()),
            ("layout", self.layout.len()),
            ("transform", self.transform.len()),
            ("window_clip", self.window_clip.len()),
            ("z", self.z.len()),
            ("slots", self.slots.len()),
        ];
        for (column, len) in columns {
            if len != rows {
                return Err(DropTargetsIntegrityError::ColumnLength { column, len, rows });
            }
        }
        let mut next = 0;
        for (row, &(start, end)) in self.slots.iter().enumerate() {
            if start != next || end < start || end as usize > self.slot_rects.len() {
                return Err(DropTargetsIntegrityError::SlotRange { row });
            }
            next = end;
        }
        Ok(())
    }
}

#[cfg(test)]
pub(super) mod tests {
    use std::f32::consts::FRAC_PI_2;

    use super::*;
    use crate::theme::Theme;

    const SCOPE: u64 = 7;

    pub(crate) fn id(key: u64) -> DropTargetId {
        DropTargetId { scope: SCOPE, key }
    }

    /// A `w`x`h` canvas that adds target `key` at `revision`, with `slots`
    /// in the canvas's own coordinates (from its top left).
    pub(crate) fn target(
        key: u64,
        revision: u64,
        slots: &'static [Rect],
        (w, h): (f32, f32),
    ) -> impl IntoAnyElement {
        canvas(move |bounds, _scene, cx| {
            let slots: Vec<Rect> = slots
                .iter()
                .map(|s| Rect {
                    x: bounds.x + s.x,
                    y: bounds.y + s.y,
                    ..*s
                })
                .collect();
            cx.add_drop_target(DropTarget {
                id: id(key),
                revision,
                layout: bounds,
                slots: &slots,
            });
        })
        .w(w)
        .h(h)
    }

    /// Paint `root` into a 400x300 window; the frame's drop targets.
    pub(crate) fn paint(root: impl IntoAnyElement) -> DropTargets {
        let mut text = TextSystem::vendored_only(&Default::default());
        let mut layouts = LayoutCache::default();
        let theme = Theme::default_dark();
        let signals = SignalStore::new();
        let mut cx = ElementContext::new(&theme, 1.0, &mut text, &mut layouts, None, &signals);
        cx.semantic = SemanticFrame::new(400.0, 300.0);
        let mut root = root.into_any();
        render_element(&mut root, &mut Scene::default(), &mut cx, 400.0, 300.0);
        let frame = cx.take_input_frame();
        assert_eq!(frame.drop_targets.frame(), frame.geometry.frame());
        frame.drop_targets
    }

    /// The key of the target at each point, `0` for none.
    fn keys_at(targets: &DropTargets, points: &[(f32, f32)]) -> Vec<u64> {
        points
            .iter()
            .map(|&(x, y)| targets.at(x, y).map_or(0, |hit| hit.id.key))
            .collect()
    }

    fn rect(x: f32, y: f32, width: f32, height: f32) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    // Catches lookup against the untransformed layout box: a 100x20 target
    // turned a quarter about its center covers 40..60 x 0..100, not its
    // layout rows, and an axis-aligned bounds test would accept a point
    // off a 45° turn's corner.
    #[test]
    fn lookup_maps_the_point_through_the_targets_transform() {
        let targets = paint(
            div()
                .w(400.0)
                .h(300.0)
                .flex_col()
                .child(div().h(40.0))
                .child(div().w(100.0).h(20.0).rotate(FRAC_PI_2).child(target(
                    1,
                    0,
                    &[],
                    (100.0, 20.0),
                ))),
        );
        let turned = paint(
            div()
                .w(100.0)
                .h(100.0)
                .rotate(FRAC_PI_2 / 2.0)
                .child(target(2, 0, &[], (100.0, 100.0))),
        );

        assert_eq!(
            keys_at(&targets, &[(50.0, 5.0), (50.0, 95.0), (10.0, 50.0)]),
            [1, 1, 0]
        );
        // The bounding box of the turned square spans about -20.7..120.7;
        // its top left corner region lies outside the turned square.
        assert_eq!(keys_at(&turned, &[(50.0, 50.0), (2.0, 2.0)]), [2, 0]);
    }

    // Catches a target taking drops where an ancestor clip hides it: a
    // scrolled list shows rows 40..140 of a target 300 tall.
    #[test]
    fn lookup_ignores_the_clipped_out_part_of_a_target() {
        let targets = paint(
            div()
                .w(400.0)
                .h(300.0)
                .flex_col()
                .child(div().h(40.0))
                .child(div().w(200.0).h(100.0).flex_col().scroll_y(50.0).child(
                    div().w(200.0).h(300.0).flex_shrink_0().child(target(
                        1,
                        0,
                        &[],
                        (200.0, 300.0),
                    )),
                )),
        );

        assert_eq!(
            keys_at(&targets, &[(10.0, 20.0), (10.0, 60.0), (10.0, 160.0)]),
            [0, 1, 0]
        );
    }

    // Catches stacking by paint order alone: a raised overlay painted
    // first still covers the target painted after it, and a nested target
    // painted later covers its parent.
    #[test]
    fn higher_z_then_later_paint_wins() {
        let targets = paint(
            div()
                .w(400.0)
                .h(300.0)
                .child(
                    div()
                        .absolute()
                        .left(0.0)
                        .top(0.0)
                        .w(100.0)
                        .h(100.0)
                        .z_index(5)
                        .child(target(1, 0, &[], (100.0, 100.0))),
                )
                .child(
                    div()
                        .absolute()
                        .left(0.0)
                        .top(0.0)
                        .w(300.0)
                        .h(300.0)
                        .child(target(2, 0, &[], (300.0, 300.0)))
                        .child(
                            div()
                                .absolute()
                                .left(200.0)
                                .top(200.0)
                                .w(50.0)
                                .h(50.0)
                                .child(target(3, 0, &[], (50.0, 50.0))),
                        ),
                ),
        );

        assert_eq!(
            keys_at(&targets, &[(50.0, 50.0), (150.0, 150.0), (220.0, 220.0)]),
            [1, 2, 3]
        );
    }

    // Catches slots tested in window space: a 100x20 strip at (50, 10)
    // scaled by two about its center covers window 0..200 x 0..40, and its
    // second 50-wide slot window x 100..200.
    #[test]
    fn slots_are_found_in_the_targets_layout_space() {
        const SLOTS: &[Rect] = &[
            Rect {
                x: 0.0,
                y: 0.0,
                width: 50.0,
                height: 20.0,
            },
            Rect {
                x: 50.0,
                y: 0.0,
                width: 50.0,
                height: 20.0,
            },
        ];
        let targets = paint(
            div().w(400.0).h(300.0).child(
                div()
                    .absolute()
                    .left(50.0)
                    .top(10.0)
                    .w(100.0)
                    .h(20.0)
                    .scale(2.0)
                    .child(target(1, 0, SLOTS, (100.0, 20.0))),
            ),
        );

        let slots: Vec<Option<usize>> = [10.0, 150.0]
            .map(|x| targets.at(x, 10.0).and_then(|hit| hit.slot))
            .into();
        assert_eq!(slots, [Some(0), Some(1)]);
        let hit = targets.at(150.0, 10.0).expect("strip");
        assert_eq!(hit.local, (75.0, 5.0));
        assert_eq!(hit.bounds, rect(0.0, 0.0, 200.0, 40.0));
    }

    // Catches a remembered hit committed after the layout moved under a
    // still pointer, or one refused although nothing changed but the
    // frame stamp.
    #[test]
    fn revalidate_refuses_a_hit_the_new_frame_moved_or_removed() {
        let layout = |first: f32, revision: u64| {
            paint(
                div()
                    .w(400.0)
                    .h(300.0)
                    .flex_row()
                    .child(
                        div()
                            .w(first)
                            .h(100.0)
                            .child(target(1, revision, &[], (first, 100.0))),
                    )
                    .child(
                        div()
                            .w(100.0)
                            .h(100.0)
                            .child(target(2, revision, &[], (100.0, 100.0))),
                    ),
            )
        };
        let before = layout(100.0, 0);
        let hit = before.at(50.0, 50.0).expect("target 1");

        let cases = [
            ("repainted", layout(100.0, 0), Ok(1)),
            ("shifted", layout(20.0, 0), Err(StaleTarget::Moved)),
            (
                "repainted at a new revision",
                layout(100.0, 1),
                Err(StaleTarget::Moved),
            ),
            (
                "removed",
                paint(div().w(10.0).h(10.0)),
                Err(StaleTarget::Gone),
            ),
        ];
        for (name, after, expected) in cases {
            assert_ne!(after.frame(), hit.frame, "{name}");
            let got = after.revalidate(&hit).map(|hit| hit.id.key);
            assert_eq!(got, expected, "{name}");
        }
    }
}
