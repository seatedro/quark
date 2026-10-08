//! Element geometry: where each identified element of a frame landed in
//! the window.
//!
//! Paint records one row per element that carries a [`UiNodeId`], a
//! [`TestId`], or an [`ElementHandle`], whether or not it builds an
//! accessibility node. A finished frame's rows are its [`LayoutSnapshot`],
//! published with the frame's input routing data: apps read the last
//! completed frame through `UiContext::geometry` and `ViewContext::geometry`
//! in quark-app, drag handlers get the frame they route through
//! ([`DragHandler::set_geometry`]), and elements read the rows painted so
//! far in [`ElementContext::geometry`].
//!
//! A lookup never lays anything out: it reads what the frame recorded.
//! Before the first frame, and for elements the frame did not paint
//! (removed, scrolled out of a virtual list), it finds nothing.

use std::sync::atomic::{AtomicU64, Ordering};

use super::*;

/// Identity of one painted frame's geometry. Every [`LayoutSnapshot`]
/// reset takes a fresh one, so geometry from different frames never
/// compares equal by stamp.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct FrameId(u64);

/// Source of [`FrameId`]s. Starts at 1: the default snapshot (no frame
/// yet) has stamp 0.
static NEXT_FRAME: AtomicU64 = AtomicU64::new(1);

/// A window-scoped name for one element, attached with
/// `Div::element_handle`. Unlike a [`UiNodeId`] it needs no string and
/// cannot collide with ids other code picks. Allocate one from
/// [`ElementHandles`]; once released, its slot comes back with a new
/// generation, so a stale handle never resolves to the element that got
/// the slot next.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ElementHandle {
    slot: u32,
    generation: u32,
}

/// Hands out [`ElementHandle`]s for one window.
#[derive(Debug, Default)]
pub struct ElementHandles {
    /// Current generation of each slot.
    generations: Vec<u32>,
    free: Vec<u32>,
}

impl ElementHandles {
    pub fn allocate(&mut self) -> ElementHandle {
        match self.free.pop() {
            Some(slot) => ElementHandle {
                slot,
                generation: self.generations[slot as usize],
            },
            None => {
                self.generations.push(0);
                ElementHandle {
                    slot: self.generations.len() as u32 - 1,
                    generation: 0,
                }
            }
        }
    }

    /// Retire `handle`: its slot comes back from a later `allocate` under a
    /// new generation, so the new handle never matches the old one's rows.
    /// Lookups read snapshots, which release does not touch: a snapshot
    /// painted before the release still finds the element by `handle`, and
    /// so does any later frame that still attaches it. Releasing a stale
    /// handle does nothing.
    pub fn release(&mut self, handle: ElementHandle) {
        let Some(generation) = self.generations.get_mut(handle.slot as usize) else {
            return;
        };
        if *generation == handle.generation {
            *generation = generation.wrapping_add(1);
            self.free.push(handle.slot);
        }
    }
}

/// Where one element landed in its frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ElementGeometry {
    /// The element's layout box before any transform: window coordinates
    /// as if no ancestor (or the element itself) were rotated or scaled.
    pub layout: Rect,
    /// Maps layout coordinates to window coordinates: the element's own
    /// transform after those of its ancestors.
    pub transform: Transform2D,
    /// Axis-aligned bounds of the transformed layout box's four corners,
    /// in window points.
    pub bounds: Rect,
    /// The part of `bounds` inside every ancestor clip, `None` when the
    /// element is clipped out entirely. A clip under a rotation counts as
    /// its axis-aligned bounds, so this can overstate what shows.
    pub visible: Option<Rect>,
    pub frame: FrameId,
}

impl ElementGeometry {
    /// A window point relative to the layout box's top left corner, in
    /// layout coordinates: where the element itself would see it. `None`
    /// under a transform that flattens the element (a zero scale), which
    /// shows nothing and takes no input.
    pub fn to_local(&self, x: f32, y: f32) -> Option<(f32, f32)> {
        let (lx, ly) = self.transform.invert()?.apply(x, y);
        Some((lx - self.layout.x, ly - self.layout.y))
    }

    /// Whether the element can take pointer input: its transform is
    /// invertible.
    pub fn is_interactive(&self) -> bool {
        self.transform.invert().is_some()
    }
}

/// Why a lookup found no single element.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LookupError {
    /// The frame painted no element by that name.
    Missing,
    /// The frame painted this many elements by that name; use
    /// [`LayoutSnapshot::all_by_test_id`] (or a unique name) instead.
    Ambiguous(usize),
}

/// The geometry of one painted frame, one row per identified element in
/// paint order, stored column-wise.
#[derive(Debug, Clone, Default)]
pub struct LayoutSnapshot {
    frame: FrameId,
    id: Vec<Option<UiNodeId>>,
    test_id: Vec<Option<TestId>>,
    handle: Vec<Option<ElementHandle>>,
    layout: Vec<Rect>,
    transform: Vec<Transform2D>,
    /// Ancestor clips in window space.
    window_clip: Vec<Rect>,
    /// Clips pushed since the innermost recording cache boundary, in
    /// layout coordinates, so a replay can clip the row again.
    local_clip: Vec<Rect>,
}

/// Identity columns of one row.
#[derive(Debug, Clone, Default)]
pub(crate) struct GeometryKey {
    pub(crate) id: Option<UiNodeId>,
    pub(crate) test_id: Option<TestId>,
    pub(crate) handle: Option<ElementHandle>,
}

impl GeometryKey {
    pub(crate) fn is_empty(&self) -> bool {
        self.id.is_none() && self.test_id.is_none() && self.handle.is_none()
    }
}

/// One row as paint pushes it.
pub(crate) struct GeometryRow {
    pub(crate) key: GeometryKey,
    pub(crate) layout: Rect,
    pub(crate) transform: Transform2D,
    pub(crate) window_clip: Rect,
    pub(crate) local_clip: Rect,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LayoutSnapshotIntegrityError {
    ColumnLength {
        column: &'static str,
        len: usize,
        rows: usize,
    },
}

impl LayoutSnapshot {
    /// The frame these rows came from; the default (empty) snapshot of a
    /// window that has not painted has `FrameId::default()`.
    pub fn frame(&self) -> FrameId {
        self.frame
    }

    pub fn len(&self) -> usize {
        self.layout.len()
    }

    pub fn is_empty(&self) -> bool {
        self.layout.is_empty()
    }

    /// The element with stable id `id` (`Div::id`, or `accessibility_id`).
    pub fn by_id(&self, id: &str) -> Result<ElementGeometry, LookupError> {
        self.unique(|i| self.id[i].as_ref().is_some_and(|v| v.as_str() == id))
    }

    /// The element with test id `id`.
    pub fn by_test_id(&self, id: &str) -> Result<ElementGeometry, LookupError> {
        self.unique(|i| self.test_id[i].as_ref().is_some_and(|v| v.as_str() == id))
    }

    /// The element `handle` is attached to.
    pub fn by_handle(&self, handle: ElementHandle) -> Result<ElementGeometry, LookupError> {
        self.unique(|i| self.handle[i] == Some(handle))
    }

    /// Every element with test id `id`, in paint order.
    pub fn all_by_test_id<'a>(&'a self, id: &'a str) -> impl Iterator<Item = ElementGeometry> + 'a {
        (0..self.len())
            .filter(move |&i| self.test_id[i].as_ref().is_some_and(|v| v.as_str() == id))
            .map(|i| self.geometry(i))
    }

    fn unique(&self, matches: impl Fn(usize) -> bool) -> Result<ElementGeometry, LookupError> {
        let mut found = (0..self.len()).filter(|&i| matches(i));
        let Some(first) = found.next() else {
            return Err(LookupError::Missing);
        };
        match found.count() {
            0 => Ok(self.geometry(first)),
            more => Err(LookupError::Ambiguous(more + 1)),
        }
    }

    fn geometry(&self, row: usize) -> ElementGeometry {
        let transform = self.transform[row];
        let bounds = window_rect(transform, self.layout[row]);
        ElementGeometry {
            layout: self.layout[row],
            transform,
            bounds,
            visible: bounds.intersection(self.window_clip[row]),
            frame: self.frame,
        }
    }

    /// Empty the snapshot for a new frame, keeping its buffers.
    pub(crate) fn reset(&mut self) {
        self.frame = FrameId(NEXT_FRAME.fetch_add(1, Ordering::Relaxed));
        self.id.clear();
        self.test_id.clear();
        self.handle.clear();
        self.layout.clear();
        self.transform.clear();
        self.window_clip.clear();
        self.local_clip.clear();
    }

    pub(crate) fn push(&mut self, row: GeometryRow) {
        self.id.push(row.key.id);
        self.test_id.push(row.key.test_id);
        self.handle.push(row.key.handle);
        self.layout.push(row.layout);
        self.transform.push(row.transform);
        self.window_clip.push(row.window_clip);
        self.local_clip.push(row.local_clip);
        debug_assert_eq!(self.verify_integrity(), Ok(()));
    }

    /// Identity, layout box, and recording-relative clip of `row`, for a
    /// cache boundary's recording.
    pub(crate) fn recorded(&self, row: usize) -> (GeometryKey, Rect, Rect) {
        let key = GeometryKey {
            id: self.id[row].clone(),
            test_id: self.test_id[row].clone(),
            handle: self.handle[row],
        };
        (key, self.layout[row], self.local_clip[row])
    }

    /// Clip rows `start..` by `clip` too: their recording-relative clips
    /// become relative to the enclosing recording.
    pub(crate) fn clip_local_from(&mut self, start: usize, clip: Rect) {
        for local in &mut self.local_clip[start..] {
            *local = local.intersection(clip).unwrap_or(quark::hit::EMPTY_CLIP);
        }
    }

    /// Every column has one entry per row.
    pub fn verify_integrity(&self) -> Result<(), LayoutSnapshotIntegrityError> {
        let rows = self.layout.len();
        let columns = [
            ("id", self.id.len()),
            ("test_id", self.test_id.len()),
            ("handle", self.handle.len()),
            ("transform", self.transform.len()),
            ("window_clip", self.window_clip.len()),
            ("local_clip", self.local_clip.len()),
        ];
        for (column, len) in columns {
            if len != rows {
                return Err(LayoutSnapshotIntegrityError::ColumnLength { column, len, rows });
            }
        }
        Ok(())
    }
}

/// `rect` (layout coordinates) in window coordinates: the bounds of its
/// four transformed corners. An empty clip stays empty.
pub(crate) fn window_rect(transform: Transform2D, rect: Rect) -> Rect {
    if rect.width < 0.0 || rect.height < 0.0 {
        return rect;
    }
    if transform.is_translation() {
        return rect.offset(transform.tx, transform.ty);
    }
    transform.map_rect_bounds(rect)
}

#[cfg(test)]
mod tests {
    use std::f32::consts::FRAC_PI_2;

    use super::*;
    use crate::accessibility::{AccessibilityFrame, dump_accessibility};
    use crate::theme::Theme;

    /// Paint `root` into a 400x300 window; its geometry and accessibility.
    fn paint(root: impl IntoAnyElement) -> (LayoutSnapshot, AccessibilityFrame) {
        let mut text = TextSystem::vendored_only(&Default::default());
        let mut layouts = LayoutCache::default();
        let theme = Theme::default_dark();
        let signals = SignalStore::new();
        let mut cx = ElementContext::new(&theme, 1.0, &mut text, &mut layouts, None, &signals);
        cx.accessibility = AccessibilityFrame::new(400.0, 300.0);
        cx.semantic = SemanticFrame::new(400.0, 300.0);
        let mut root = root.into_any();
        render_element(&mut root, &mut Scene::default(), &mut cx, 400.0, 300.0);
        let accessibility = std::mem::take(&mut cx.accessibility);
        (cx.take_input_frame().geometry, accessibility)
    }

    /// `rect` rounded to hundredths, so rotations compare exactly.
    fn rounded(rect: Rect) -> (f32, f32, f32, f32) {
        let r = |v: f32| (v * 100.0).round() / 100.0 + 0.0;
        (r(rect.x), r(rect.y), r(rect.width), r(rect.height))
    }

    // Catches a duplicate test id resolving to whichever element painted
    // first, which would anchor to the wrong one without a word.
    #[test]
    fn duplicate_test_ids_are_ambiguous_and_all_are_listed() {
        let row = || div().test_id("row").w(100.0).h(20.0);
        let (geometry, _) = paint(div().flex_col().gap(20.0).child(row()).child(row()));

        assert_eq!(geometry.by_test_id("row"), Err(LookupError::Ambiguous(2)));
        let tops: Vec<f32> = geometry.all_by_test_id("row").map(|g| g.bounds.y).collect();
        assert_eq!(tops, [0.0, 40.0]);
    }

    // Catches geometry reported in layout space (ignoring the rotation),
    // or visible bounds that ignore the ancestor clip.
    #[test]
    fn rotated_element_reports_its_turned_bounds_inside_the_clip() {
        let (geometry, _) = paint(
            div()
                .w(100.0)
                .h(80.0)
                .clip()
                .flex_col()
                .child(div().h(40.0))
                .child(div().test_id("bar").w(100.0).h(20.0).rotate(FRAC_PI_2)),
        );

        let bar = geometry.by_test_id("bar").expect("one bar");
        assert_eq!(rounded(bar.layout), (0.0, 40.0, 100.0, 20.0));
        // A quarter turn about its center (50, 50).
        assert_eq!(rounded(bar.bounds), (40.0, 0.0, 20.0, 100.0));
        assert_eq!(bar.visible.map(rounded), Some((40.0, 0.0, 20.0, 80.0)));
    }

    // Catches accessibility bounds left in layout space, or transforms
    // composed in the wrong order: a button turned a quarter about its
    // center inside a parent scaled by half about its own.
    #[test]
    fn accessibility_bounds_compose_nested_transforms() {
        let (_, accessibility) = paint(
            div().w(200.0).h(200.0).scale(0.5).child(
                div()
                    .absolute()
                    .left(50.0)
                    .top(90.0)
                    .w(100.0)
                    .h(20.0)
                    .rotate(FRAC_PI_2)
                    .accessibility_role(AccessibilityRole::Button)
                    .accessibility_label("Turned"),
            ),
        );

        // Turned: 90,50 20x100; then halved about (100, 100).
        let dump = dump_accessibility(&accessibility);
        assert!(dump.contains("Turned | - | 95,75,10,50"), "{dump}");
    }

    // Catches a released handle that still resolves once its slot is
    // reused by another element's handle.
    #[test]
    fn released_handle_does_not_resolve_to_its_slots_next_owner() {
        let mut handles = ElementHandles::default();
        let old = handles.allocate();
        handles.release(old);
        let new = handles.allocate();
        assert_ne!(old, new);

        let mut snapshot = LayoutSnapshot::default();
        snapshot.reset();
        snapshot.push(GeometryRow {
            key: GeometryKey {
                handle: Some(new),
                ..GeometryKey::default()
            },
            layout: Rect {
                x: 10.0,
                y: 20.0,
                width: 30.0,
                height: 40.0,
            },
            transform: Transform2D::IDENTITY,
            window_clip: quark::hit::UNCLIPPED,
            local_clip: quark::hit::UNCLIPPED,
        });
        assert_eq!(snapshot.by_handle(old), Err(LookupError::Missing));
        assert_eq!(
            snapshot.by_handle(new).map(|g| g.bounds.x),
            Ok(10.0),
            "the live handle resolves"
        );
    }
}
