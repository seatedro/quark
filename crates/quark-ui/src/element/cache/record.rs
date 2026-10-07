//! Recording a built subtree's output into its cache row, and replaying it.

use accesskit::NodeId;
use quark_render::Primitive;

use crate::animation::AnimKey;

use super::*;

/// Semantic node or hit binding with no node (or the boundary's parent).
const NONE: u32 = u32::MAX;

/// State a subtree inherits from its ancestors that its output depends on.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(super) struct Inherited {
    z: i32,
    paint: PaintState,
}

/// The paint-time part of [`Inherited`].
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct PaintState {
    text_color: Option<Color>,
    icon_color: Option<Color>,
    text_hidden: bool,
}

impl PaintState {
    fn of(cx: &ElementContext) -> Self {
        Self {
            text_color: cx.text_color_override(),
            icon_color: cx.icon_color_override(),
            text_hidden: cx.accessibility_text_hidden(),
        }
    }
}

/// Where a recorded accessibility node hangs.
#[derive(Debug, Clone, Copy)]
enum A11yParent {
    /// The boundary's accessible ancestor, whatever it is at replay.
    Boundary,
    Window,
    /// A recorded node, by index.
    Node(u32),
}

/// Everything a subtree registered in one frame, relative to its origin.
/// Columns are reused across recordings, so a steady row records without
/// growing them.
#[derive(Default)]
pub(super) struct PaintRecord {
    scene: Vec<Primitive>,
    hit_bounds: Vec<Rect>,
    hit_clip: Vec<Rect>,
    hit_z: Vec<i32>,
    hit_flags: Vec<HitFlags>,
    hit_cursor: Vec<CursorHint>,
    /// Semantic node of each hit, relative to the first recorded node.
    hit_node: Vec<u32>,
    hit_identity: Vec<Option<HitIdentity>>,
    /// This frame's ids of the hit rows, between prepaint and paint.
    hit_ids: Vec<HitId>,
    semantic: Vec<SemanticNode>,
    /// Relative parent of each semantic node, `NONE` for the boundary's.
    semantic_parent: Vec<u32>,
    handlers: InputHandlers,
    a11y: Vec<AccessibilityNode>,
    a11y_parent: Vec<A11yParent>,
    /// `(semantic node, accessibility node)` pairs, both relative.
    a11y_owner: Vec<(u32, u32)>,
    /// This frame's ids of the replayed accessibility nodes.
    a11y_ids: Vec<NodeId>,
    tooltips: Vec<TooltipRegion>,
    selectable: Vec<SelectableTextRegion>,
    scrollbars: Vec<ScrollbarTrack>,
    transitions: Vec<AnimKey>,
}

impl PaintRecord {
    pub(super) fn clear(&mut self) {
        self.scene.clear();
        self.hit_bounds.clear();
        self.hit_clip.clear();
        self.hit_z.clear();
        self.hit_flags.clear();
        self.hit_cursor.clear();
        self.hit_node.clear();
        self.hit_identity.clear();
        self.hit_ids.clear();
        self.semantic.clear();
        self.semantic_parent.clear();
        self.handlers = InputHandlers::default();
        self.a11y.clear();
        self.a11y_parent.clear();
        self.a11y_owner.clear();
        self.a11y_ids.clear();
        self.tooltips.clear();
        self.selectable.clear();
        self.scrollbars.clear();
        self.transitions.clear();
    }
}

fn take_record(cx: &mut ElementContext, row: u32) -> PaintRecord {
    let cache = cx.cache.as_deref_mut().expect("recording needs a cache");
    std::mem::take(&mut cache.paint[row as usize])
}

fn put_record(cx: &mut ElementContext, row: u32, record: PaintRecord) {
    let cache = cx.cache.as_deref_mut().expect("recording needs a cache");
    cache.paint[row as usize] = record;
}

/// Lengths of the context's outputs when a subtree started painting.
struct PaintMarks {
    scene: usize,
    semantic: usize,
    a11y: usize,
    handlers: HandlerMarks,
    tooltips: usize,
    selectable: usize,
    scrollbars: usize,
    text_inputs: usize,
    semantic_parent: Option<usize>,
    a11y_parent: Option<NodeId>,
    state: PaintState,
}

/// Counters a phase started from, to tell what the subtree itself did.
#[derive(Clone, Copy)]
struct PhaseStart {
    volatile: u32,
    focus: u32,
    transitions: usize,
}

impl PhaseStart {
    fn of(cx: &ElementContext) -> Self {
        Self {
            volatile: cx.volatile_reads(),
            focus: cx.focus_reads(),
            transitions: cx.transition_keys().len(),
        }
    }
}

/// A subtree being recorded, from its prepaint to the end of its paint.
pub(super) struct Recording {
    row: u32,
    hash: u64,
    origin: (f32, f32),
    hit_start: usize,
    z: i32,
    phase: PhaseStart,
    volatile: u32,
    focus_reads: u32,
    extent: Option<Rect>,
    paint: Option<PaintMarks>,
}

impl Recording {
    pub(super) fn begin_prepaint(
        row: u32,
        hash: u64,
        bounds: Bounds,
        cx: &mut ElementContext,
    ) -> Self {
        Self {
            row,
            hash,
            origin: (bounds.x, bounds.y),
            hit_start: cx.begin_hit_recording(),
            z: cx.current_z_index(),
            phase: PhaseStart::of(cx),
            volatile: 0,
            focus_reads: 0,
            extent: None,
            paint: None,
        }
    }

    /// Close a phase: count what the subtree read and keep the transition
    /// keys it touched.
    fn end_phase(&mut self, record: &mut PaintRecord, cx: &ElementContext) {
        self.volatile += cx.volatile_reads().wrapping_sub(self.phase.volatile);
        self.focus_reads += cx.focus_reads().wrapping_sub(self.phase.focus);
        record
            .transitions
            .extend_from_slice(&cx.transition_keys()[self.phase.transitions..]);
    }

    pub(super) fn end_prepaint(&mut self, cx: &mut ElementContext) {
        let mut record = take_record(cx, self.row);
        record.clear();
        self.end_phase(&mut record, cx);
        let (ox, oy) = self.origin;
        let (ids, clips) = cx.recorded_hits(self.hit_start);
        for (&id, &clip) in ids.iter().zip(clips) {
            let table = &cx.hit_table;
            let bounds = table.bounds(id).expect("recorded hit").offset(-ox, -oy);
            self.extent = Some(match self.extent {
                Some(extent) => union(extent, bounds),
                None => bounds,
            });
            record.hit_bounds.push(bounds);
            record.hit_clip.push(clip.offset(-ox, -oy));
            record.hit_z.push(table.z(id).unwrap_or(0));
            record.hit_flags.push(table.flags(id).unwrap_or_default());
            record.hit_cursor.push(table.cursor(id).unwrap_or_default());
            record.hit_ids.push(id);
        }
        put_record(cx, self.row, record);
        cx.end_hit_recording(self.hit_start);
    }

    pub(super) fn begin_paint(&mut self, scene: &Scene, cx: &ElementContext) {
        self.phase = PhaseStart::of(cx);
        self.paint = Some(PaintMarks {
            scene: scene.len(),
            semantic: cx.semantic.nodes().len(),
            a11y: cx.accessibility.len(),
            handlers: cx.handlers.marks(),
            tooltips: cx.tooltip_regions.len(),
            selectable: cx.selectable_text_runs.len(),
            scrollbars: cx.scrollbar_tracks.len(),
            text_inputs: cx.text_input_hit_areas.len(),
            semantic_parent: cx.current_semantic_parent(),
            a11y_parent: cx.accessible_semantic_ancestor(),
            state: PaintState::of(cx),
        });
    }

    /// Copy the subtree's paint output into its row and decide whether the
    /// row may replay. `memo` replaces the row's measure memo when given.
    pub(super) fn commit(
        mut self,
        scene: &Scene,
        memo: Option<&[MeasureMemo]>,
        cx: &mut ElementContext,
    ) {
        let marks = self.paint.take().expect("commit after begin_paint");
        let mut record = take_record(cx, self.row);
        self.end_phase(&mut record, cx);
        let (ox, oy) = self.origin;
        let mut complete = cx.text_input_hit_areas.len() == marks.text_inputs;

        record
            .scene
            .extend(scene.primitives[marks.scene..].iter().map(|p| {
                let mut p = p.clone();
                p.offset(-ox, -oy);
                p
            }));

        let base = marks.semantic;
        for node in &cx.semantic.nodes()[base..] {
            let parent = match node.parent {
                Some(parent) if parent >= base => (parent - base) as u32,
                parent if parent == marks.semantic_parent => NONE,
                _ => {
                    complete = false;
                    NONE
                }
            };
            let mut node = node.clone();
            node.parent = None;
            node.bounds = node.bounds.offset(-ox, -oy);
            record.semantic.push(node);
            record.semantic_parent.push(parent);
        }
        for &id in &record.hit_ids {
            let node = match cx.hit_table.node(id) {
                Some(node) if node >= base => (node - base) as u32,
                Some(_) => {
                    complete = false;
                    NONE
                }
                None => NONE,
            };
            record.hit_node.push(node);
            record.hit_identity.push(cx.hit_table.identity(id));
        }
        complete &= cx
            .handlers
            .copy_since(marks.handlers, base, &mut record.handlers);

        let nodes = cx.accessibility.nodes_from(marks.a11y);
        for (index, node) in nodes.iter().enumerate() {
            let parent = match node.parent() {
                parent if parent == marks.a11y_parent => A11yParent::Boundary,
                None => A11yParent::Window,
                Some(id) => match nodes[..index].iter().rposition(|n| n.id() == id) {
                    Some(at) => A11yParent::Node(at as u32),
                    None => {
                        complete = false;
                        A11yParent::Window
                    }
                },
            };
            let mut node = node.clone();
            node.offset(-ox, -oy);
            record.a11y.push(node);
            record.a11y_parent.push(parent);
        }
        for semantic in 0..record.semantic.len() {
            if let Some(owner) = cx.accessibility.semantic_owner(base + semantic) {
                match nodes.iter().position(|n| n.id() == owner) {
                    Some(at) => record.a11y_owner.push((semantic as u32, at as u32)),
                    None => complete = false,
                }
            }
        }

        record
            .tooltips
            .extend(
                cx.tooltip_regions[marks.tooltips..]
                    .iter()
                    .map(|t| TooltipRegion {
                        bounds: t.bounds.offset(-ox, -oy),
                        text: t.text.clone(),
                    }),
            );
        record.selectable.extend(
            cx.selectable_text_runs[marks.selectable..]
                .iter()
                .map(|r| offset_selectable(r, -ox, -oy)),
        );
        record.scrollbars.extend(
            cx.scrollbar_tracks[marks.scrollbars..]
                .iter()
                .map(|t| offset_scrollbar(t, -ox, -oy)),
        );

        let pointer_inside = match (self.extent, cx.mouse_position) {
            (Some(extent), Some((x, y))) => extent.offset(ox, oy).contains(x, y),
            _ => false,
        };
        let focus = (self.focus_reads > 0).then_some(cx.focus);
        let (scale, accessibility) = (cx.scale_factor, cx.accessibility_enabled());
        let row = self.row as usize;
        put_record(cx, self.row, record);
        let cache = cx.cache.as_deref_mut().expect("recording needs a cache");
        cache.inputs[row] = EntryInputs {
            hash: self.hash,
            scale,
            theme: cache.theme_generation,
            accessibility,
            focus,
            reusable: complete && self.volatile == 0 && !pointer_inside,
        };
        cache.inherited[row] = Inherited {
            z: self.z,
            paint: marks.state,
        };
        cache.hit_extent[row] = self.extent;
        if let Some(memo) = memo {
            cache.memo[row].clear();
            cache.memo[row].extend_from_slice(memo);
        }
    }
}

fn offset_selectable(region: &SelectableTextRegion, dx: f32, dy: f32) -> SelectableTextRegion {
    SelectableTextRegion {
        bounds: region.bounds.offset(dx, dy),
        text_origin: (region.text_origin.0 + dx, region.text_origin.1 + dy),
        ..region.clone()
    }
}

fn offset_scrollbar(track: &ScrollbarTrack, dx: f32, dy: f32) -> ScrollbarTrack {
    ScrollbarTrack {
        track_rect: track.track_rect.offset(dx, dy),
        thumb_top: track.thumb_top + dy,
        ..track.clone()
    }
}

/// Re-insert `row`'s hits at the boundary's new origin. False when the
/// row cannot replay here: another z layer, or the pointer over its hits.
pub(super) fn replay_prepaint(row: u32, bounds: Bounds, cx: &mut ElementContext) -> bool {
    let (ox, oy) = (bounds.x, bounds.y);
    let Some(cache) = cx.cache.as_deref() else {
        return false;
    };
    let r = row as usize;
    let pointer_over = match (cache.hit_extent[r], cx.mouse_position) {
        (Some(extent), Some((x, y))) => extent.offset(ox, oy).contains(x, y),
        _ => false,
    };
    if pointer_over || cache.inherited[r].z != cx.current_z_index() {
        return false;
    }
    let mut record = take_record(cx, row);
    record.hit_ids.clear();
    for i in 0..record.hit_bounds.len() {
        let id = cx.insert_hit_clipped(
            record.hit_bounds[i].offset(ox, oy),
            record.hit_clip[i].offset(ox, oy),
            record.hit_z[i],
            record.hit_flags[i],
            record.hit_cursor[i],
        );
        record.hit_ids.push(id);
    }
    put_record(cx, row, record);
    true
}

/// Replay `row`'s paint output at the boundary's origin. False when the
/// inherited paint state differs from the recording's.
pub(super) fn replay_paint(
    row: u32,
    bounds: Bounds,
    scene: &mut Scene,
    cx: &mut ElementContext,
) -> bool {
    let (ox, oy) = (bounds.x, bounds.y);
    let Some(cache) = cx.cache.as_deref() else {
        return false;
    };
    if cache.inherited[row as usize].paint != PaintState::of(cx) {
        return false;
    }
    let mut record = take_record(cx, row);

    scene.primitives.extend(record.scene.iter().map(|p| {
        let mut p = p.clone();
        p.offset(ox, oy);
        p
    }));

    let base = cx.semantic.nodes().len();
    let parent = cx.current_semantic_parent();
    for (node, &relative) in record.semantic.iter().zip(&record.semantic_parent) {
        let mut node = node.clone();
        node.bounds = node.bounds.offset(ox, oy);
        node.parent = if relative == NONE {
            parent
        } else {
            Some(base + relative as usize)
        };
        cx.semantic.push(node);
    }
    for (i, &id) in record.hit_ids.iter().enumerate() {
        if record.hit_node[i] != NONE {
            cx.bind_hit(id, base + record.hit_node[i] as usize);
        }
        cx.hit_table.set_identity(id, record.hit_identity[i]);
    }
    cx.handlers.extend_shifted(&record.handlers, base);

    let ancestor = cx.accessible_semantic_ancestor();
    record.a11y_ids.clear();
    for (node, parent) in record.a11y.iter().zip(&record.a11y_parent) {
        let parent = match *parent {
            A11yParent::Boundary => ancestor,
            A11yParent::Window => None,
            A11yParent::Node(at) => Some(record.a11y_ids[at as usize]),
        };
        let mut node = node.clone();
        node.offset(ox, oy);
        let id = cx.accessibility.push_child(node, parent);
        record.a11y_ids.push(id);
    }
    for &(semantic, a11y) in &record.a11y_owner {
        cx.accessibility
            .bind_semantic(base + semantic as usize, record.a11y_ids[a11y as usize]);
    }

    cx.tooltip_regions
        .extend(record.tooltips.iter().map(|t| TooltipRegion {
            bounds: t.bounds.offset(ox, oy),
            text: t.text.clone(),
        }));
    cx.selectable_text_runs.extend(
        record
            .selectable
            .iter()
            .map(|r| offset_selectable(r, ox, oy)),
    );
    cx.scrollbar_tracks.extend(
        record
            .scrollbars
            .iter()
            .map(|t| offset_scrollbar(t, ox, oy)),
    );
    cx.keep_transition_keys(&record.transitions);

    put_record(cx, row, record);
    true
}

fn union(a: Rect, b: Rect) -> Rect {
    let (x, y) = (a.x.min(b.x), a.y.min(b.y));
    Rect {
        x,
        y,
        width: (a.x + a.width).max(b.x + b.width) - x,
        height: (a.y + a.height).max(b.y + b.height) - y,
    }
}
