//! Pointer drags over a dock: a tab, a whole group by its grip, and a
//! divider between groups.

use std::rc::Rc;

use quark_ui::Action;
use quark_ui::element::{
    ClickEvent, CursorHint, DRAG_PREVIEW_THRESHOLD, DragHandler, DragHandoff, DragPreview,
    DragReleaseResult, ElementGeometry, ElementHandle, LayoutSnapshot,
};

use super::tabs::TabWidths;
use super::{
    DOCK_ID, Dock, DockDestination, DockEvent, DockRegion, EventMap, HostId, MovePayload,
    PaneDividerEvent, PanelId, TabPolicy, crossing_refusal, drop_is_noop,
};
use crate::pane_tree::{DropZone, PaneDrop, PaneId, Rect};
use crate::split::Axis;

/// Where a tab being dragged came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Origin {
    pub(super) region: DockRegion,
    pub(super) pane: PaneId,
    pub(super) index: usize,
    pub(super) count: usize,
}

/// Whether a tab from `from` may land on `target` in `to`, both in the
/// host whose regions have `policies`: what a drag in progress shows. The
/// drop itself is checked again by [`super::DockState::prepare`].
fn drop_allowed(
    policies: &[TabPolicy; 4],
    confined: bool,
    from: Origin,
    to: DockRegion,
    target: PaneDrop,
) -> bool {
    if from.region != to
        && crossing_refusal(
            policies[from.region.index()],
            policies[to.index()],
            confined,
        )
        .is_some()
    {
        return false;
    }
    !drop_is_noop(from.pane, from.index, from.count, target)
}

/// One tab group as laid out this frame, for finding drop targets.
#[derive(Debug, Clone, Copy)]
pub(super) struct GroupHit {
    pub(super) pane: PaneId,
    pub(super) region: DockRegion,
    pub(super) rect: Rect,
    /// Height of its tab strip; zero when it shows none.
    pub(super) strip: f32,
    pub(super) tabs: TabWidths,
}

/// How a tab drag finds its dock's root in a frame's geometry.
#[derive(Debug, Clone, Copy)]
pub(super) enum DockRoot {
    Handle(ElementHandle),
    Id,
}

impl DockRoot {
    fn find(self, geometry: &LayoutSnapshot) -> Option<ElementGeometry> {
        match self {
            Self::Handle(handle) => geometry.by_handle(handle),
            Self::Id => geometry.by_id(DOCK_ID),
        }
        .ok()
    }
}

/// What a tab drag needs from the frame it started in. Its rects are in
/// the dock's own coordinates.
pub(super) struct DragContext {
    hits: Vec<GroupHit>,
    policies: [TabPolicy; 4],
    root: DockRoot,
    host: HostId,
    /// Of all the above: a cached tab strip's drag handlers hold the
    /// context they were built with, so the strip rebuilds when it changes.
    pub(super) hash: u64,
}

impl DragContext {
    pub(super) fn new(
        hits: Vec<GroupHit>,
        policies: [TabPolicy; 4],
        root: DockRoot,
        host: HostId,
    ) -> Self {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::hash::DefaultHasher::new();
        for hit in &hits {
            let r = hit.rect;
            (hit.pane, hit.region).hash(&mut hasher);
            for v in [r.x, r.y, r.width, r.height, hit.strip] {
                v.to_bits().hash(&mut hasher);
            }
            hit.tabs.hash_into(&mut hasher);
        }
        policies.map(|p| (p.can_leave, p.accepts)).hash(&mut hasher);
        match root {
            DockRoot::Handle(handle) => Some(handle),
            DockRoot::Id => None,
        }
        .hash(&mut hasher);
        host.hash(&mut hasher);
        Self {
            hits,
            policies,
            root,
            host,
            hash: hasher.finish(),
        }
    }

    fn target_at(&self, x: f32, y: f32) -> Option<(DockRegion, PaneDrop)> {
        let hit = self.hits.iter().find(|h| h.rect.contains(x, y))?;
        let (lx, ly) = (x - hit.rect.x, y - hit.rect.y);
        let zone = if ly < hit.strip {
            DropZone::Tabs(hit.tabs.index_at(lx))
        } else {
            DropZone::in_body(
                hit.rect.width,
                hit.rect.height - hit.strip,
                lx,
                ly - hit.strip,
            )
        };
        Some((
            hit.region,
            PaneDrop {
                pane: hit.pane,
                zone,
            },
        ))
    }
}

/// How far a tab or group drag has gone from its press. Past the drag
/// threshold the app hears once, by [`DockEvent::DragOut`], so it can hand
/// the drag to a session that follows it out of the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Travel {
    Near,
    Out,
}

impl Travel {
    /// The pointer moved to window point `at`, the press being at `press`.
    fn after_move(self, press: (f32, f32), at: (f32, f32)) -> Self {
        match self {
            Self::Near if (at.0 - press.0).hypot(at.1 - press.1) >= DRAG_PREVIEW_THRESHOLD => {
                Self::Out
            }
            travel => travel,
        }
    }
}

/// What tab and group drags share: the frame they started in, the press,
/// how far the pointer has gone from it, and the picture under it.
pub(super) struct PayloadDrag {
    map: EventMap,
    ctx: Rc<DragContext>,
    /// Window point of the press.
    press: (f32, f32),
    /// The dock's root in the frame the drag routes through, to map the
    /// pointer into the dock's coordinates.
    dock: Option<ElementGeometry>,
    travel: Travel,
    preview: DragPreview,
}

impl PayloadDrag {
    pub(super) fn new(
        map: EventMap,
        ctx: Rc<DragContext>,
        press: &ClickEvent,
        preview: DragPreview,
    ) -> Self {
        Self {
            map,
            ctx,
            press: (press.x, press.y),
            dock: None,
            travel: Travel::Near,
            preview,
        }
    }

    fn set_geometry(&mut self, geometry: &LayoutSnapshot) {
        self.dock = self.ctx.root.find(geometry);
    }

    /// The pointer moved to window point `at`: [`DockEvent::DragOut`] of
    /// `payload` as it first passes the drag threshold.
    fn travel(&mut self, at: (f32, f32), payload: MovePayload) -> Option<Action> {
        let travel = self.travel.after_move(self.press, at);
        let out = travel != self.travel;
        self.travel = travel;
        out.then(|| (self.map)(DockEvent::DragOut { payload, at }))
    }

    /// The drop target under window point `(x, y)`. Without the dock in the
    /// frame, as if it filled the window.
    fn target_at(&self, (x, y): (f32, f32)) -> Option<(DockRegion, PaneDrop)> {
        let (x, y) = self.dock.and_then(|g| g.to_local(x, y)).unwrap_or((x, y));
        self.ctx.target_at(x, y)
    }
}

/// What a dragged tab is over.
#[derive(Debug, Clone, Copy, PartialEq)]
enum TabTarget {
    /// Nowhere it can land, or where it already is: no preview and no
    /// refusal.
    Nowhere,
    Allowed(PaneDrop),
    /// A target the areas' policies or the tab's confinement refuse.
    Refused(PaneDrop),
}

impl TabTarget {
    /// What a tab from `from` is over at `hit`, in the host whose regions
    /// have `policies`.
    fn at(
        hit: Option<(DockRegion, PaneDrop)>,
        policies: &[TabPolicy; 4],
        confined: bool,
        from: Origin,
    ) -> Self {
        let Some((region, target)) = hit else {
            return Self::Nowhere;
        };
        if drop_allowed(policies, confined, from, region, target) {
            Self::Allowed(target)
        } else if target.pane == from.pane {
            // Over its own slot, or a split its group cannot give it, the
            // tab is simply not moving.
            Self::Nowhere
        } else {
            Self::Refused(target)
        }
    }

    /// The target the dock hears about, refused ones included.
    fn hovered(self) -> Option<PaneDrop> {
        match self {
            Self::Nowhere => None,
            Self::Allowed(target) | Self::Refused(target) => Some(target),
        }
    }

    /// Where a release drops the tab.
    fn drop(self) -> Option<PaneDrop> {
        match self {
            Self::Allowed(target) => Some(target),
            Self::Nowhere | Self::Refused(_) => None,
        }
    }

    fn cursor(self) -> CursorHint {
        match self {
            Self::Nowhere => CursorHint::Default,
            Self::Allowed(_) => CursorHint::Grabbing,
            Self::Refused(_) => CursorHint::NotAllowed,
        }
    }
}

/// Selects a tab on press, reports the drop target under the pointer as it
/// moves, and drops the tab there on release. Shows the tab under the
/// pointer, held where it was pressed.
pub(super) struct TabDrag {
    drag: PayloadDrag,
    panel: PanelId,
    confined: bool,
    from: Origin,
    target: TabTarget,
    /// The preview has the tab's measured size and the press's place on it.
    held: bool,
}

impl TabDrag {
    pub(super) fn new(drag: PayloadDrag, panel: PanelId, confined: bool, from: Origin) -> Self {
        Self {
            drag,
            panel,
            confined,
            from,
            target: TabTarget::Nowhere,
            held: false,
        }
    }

    fn payload(&self) -> MovePayload {
        MovePayload::Panel(self.panel)
    }

    /// Report the target under window point `(x, y)` when it changed.
    fn hover(&mut self, x: f32, y: f32) -> Vec<Action> {
        let hit = self.drag.target_at((x, y));
        let target = TabTarget::at(hit, &self.drag.ctx.policies, self.confined, self.from);
        if target == self.target {
            return Vec::new();
        }
        self.target = target;
        vec![(self.drag.map)(DockEvent::TabHover {
            panel: self.panel,
            target: target.hovered(),
        })]
    }
}

impl DragHandler for TabDrag {
    fn set_geometry(&mut self, geometry: &LayoutSnapshot) {
        self.drag.set_geometry(geometry);
        // Once, from the frame pressed in: later frames may have moved the
        // tab (selected, or dropped somewhere) while it stays held.
        if !self.held
            && let Ok(tab) = geometry.by_id(&Dock::tab_id(self.panel))
        {
            let (b, press) = (tab.bounds, self.drag.press);
            self.drag.preview.set_size(b.width, b.height);
            self.drag.preview.set_hotspot(press.0 - b.x, press.1 - b.y);
            self.held = true;
        }
    }

    fn on_press(&mut self) -> Vec<Action> {
        vec![(self.drag.map)(DockEvent::Select {
            pane: self.from.pane,
            index: self.from.index,
        })]
    }

    fn on_move(&mut self, wx: f32, wy: f32) -> Vec<Action> {
        let out = self.drag.travel((wx, wy), self.payload());
        let mut actions = self.hover(wx, wy);
        actions.extend(out);
        actions
    }

    fn on_release(&mut self) -> DragReleaseResult {
        DragReleaseResult {
            actions: vec![(self.drag.map)(DockEvent::TabDrop {
                panel: self.panel,
                target: self.target.drop(),
            })],
        }
    }

    /// Drop nowhere: the tab stays where it was.
    fn on_cancel(&mut self) -> Vec<Action> {
        vec![(self.drag.map)(DockEvent::TabDrop {
            panel: self.panel,
            target: None,
        })]
    }

    /// The tab follows the pointer into other windows: the session carries
    /// [`MovePayload::Panel`], and this window stops showing a target.
    fn on_handoff(&mut self) -> Option<DragHandoff> {
        self.target = TabTarget::Nowhere;
        Some(DragHandoff {
            payload: Box::new(self.payload()),
            actions: vec![(self.drag.map)(DockEvent::TabHover {
                panel: self.panel,
                target: None,
            })],
        })
    }

    fn preview(&self) -> Option<&DragPreview> {
        Some(&self.drag.preview)
    }

    fn cursor(&self) -> CursorHint {
        self.target.cursor()
    }
}

/// Drags a whole group by its grip: shows where it would land, moves it
/// there on release ([`DockEvent::Transfer`]), and hands off to a drag
/// session carrying [`MovePayload::Group`].
pub(super) struct GroupDrag {
    drag: PayloadDrag,
    pane: PaneId,
    region: DockRegion,
    target: Option<DockDestination>,
}

impl GroupDrag {
    pub(super) fn new(drag: PayloadDrag, pane: PaneId, region: DockRegion) -> Self {
        Self {
            drag,
            pane,
            region,
            target: None,
        }
    }

    fn payload(&self) -> MovePayload {
        MovePayload::Group(self.pane)
    }

    fn hover(&self, target: Option<DockDestination>) -> Action {
        (self.drag.map)(DockEvent::Hover {
            payload: self.payload(),
            target,
        })
    }
}

impl DragHandler for GroupDrag {
    fn set_geometry(&mut self, geometry: &LayoutSnapshot) {
        self.drag.set_geometry(geometry);
    }

    fn on_move(&mut self, wx: f32, wy: f32) -> Vec<Action> {
        let out = self.drag.travel((wx, wy), self.payload());
        let target = self
            .drag
            .target_at((wx, wy))
            .filter(|(region, t)| !(*region == self.region && t.pane == self.pane))
            .map(|(_, t)| DockDestination {
                host: self.drag.ctx.host,
                pane: t.pane,
                zone: t.zone,
            });
        let mut actions = Vec::new();
        if target != self.target {
            self.target = target;
            actions.push(self.hover(target));
        }
        actions.extend(out);
        actions
    }

    fn on_release(&mut self) -> DragReleaseResult {
        let mut actions = vec![self.hover(None)];
        if let Some(destination) = self.target {
            actions.push((self.drag.map)(DockEvent::Transfer {
                payload: self.payload(),
                destination,
            }));
        }
        DragReleaseResult { actions }
    }

    fn on_cancel(&mut self) -> Vec<Action> {
        vec![self.hover(None)]
    }

    fn on_handoff(&mut self) -> Option<DragHandoff> {
        self.target = None;
        Some(DragHandoff {
            payload: Box::new(self.payload()),
            actions: vec![self.hover(None)],
        })
    }

    fn preview(&self) -> Option<&DragPreview> {
        Some(&self.drag.preview)
    }

    fn cursor(&self) -> CursorHint {
        CursorHint::Grabbing
    }
}

/// The cursor over, and while dragging, a divider between groups along
/// `axis`.
pub(super) fn resize_cursor(axis: Axis) -> CursorHint {
    match axis {
        Axis::Horizontal => CursorHint::ResizeCol,
        Axis::Vertical => CursorHint::ResizeRow,
    }
}

/// Drags divider `divider` of `split`, whose groups line up along `axis`.
pub(super) struct PaneDividerDrag {
    map: EventMap,
    split: PaneId,
    divider: usize,
    axis: Axis,
    /// The press, along the axis.
    origin: f32,
    extent: f32,
}

impl PaneDividerDrag {
    pub(super) fn new(
        map: EventMap,
        split: PaneId,
        divider: usize,
        axis: Axis,
        press: &ClickEvent,
        extent: f32,
    ) -> Self {
        Self {
            map,
            split,
            divider,
            axis,
            origin: Self::along(axis, press.x, press.y),
            extent,
        }
    }

    fn along(axis: Axis, x: f32, y: f32) -> f32 {
        match axis {
            Axis::Horizontal => x,
            Axis::Vertical => y,
        }
    }

    fn event(&self, event: PaneDividerEvent) -> Action {
        (self.map)(DockEvent::PaneDivider {
            split: self.split,
            divider: self.divider,
            event,
        })
    }
}

impl DragHandler for PaneDividerDrag {
    fn on_press(&mut self) -> Vec<Action> {
        vec![self.event(PaneDividerEvent::Press)]
    }

    fn on_move(&mut self, x: f32, y: f32) -> Vec<Action> {
        vec![self.event(PaneDividerEvent::Drag {
            delta: Self::along(self.axis, x, y) - self.origin,
            extent: self.extent,
        })]
    }

    fn on_release(&mut self) -> DragReleaseResult {
        DragReleaseResult {
            actions: vec![self.event(PaneDividerEvent::Release)],
        }
    }

    /// The panes follow the pointer as it moves, so cancelling keeps the
    /// weights dragged to, like a release.
    fn on_cancel(&mut self) -> Vec<Action> {
        self.on_release().actions
    }

    fn cursor(&self) -> CursorHint {
        resize_cursor(self.axis)
    }
}
