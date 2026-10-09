use std::cell::Cell;

use super::*;
use crate::animation::{AnimKey, AnimationTable, Motion, PropId};

// ---------------------------------------------------------------------------
// ElementContext
// ---------------------------------------------------------------------------

/// Everything elements see while laying out and painting. Element geometry
/// is in logical points; `scale_factor` is the window's physical pixels per
/// point, used only to shape text at physical size.
pub struct ElementContext<'a> {
    pub theme: &'a Theme,
    pub scale_factor: f32,
    /// Shapes every text element; the renderer must draw with the same one.
    pub text: &'a mut TextSystem,
    /// Layouts shared by measurement, paint, hit-testing, and selection.
    pub layouts: &'a mut LayoutCache,
    pub mouse_position: Option<(f32, f32)>,
    /// Pointer hit entries in paint order; built in prepaint.
    pub hit_table: HitTable,
    /// Event handlers keyed by semantic node; registered in paint.
    pub handlers: InputHandlers,
    pub focus: Option<FocusId>,
    /// Whether focus shows its ring (CSS `:focus-visible`): it came from
    /// the keyboard, assistive tech, or the app, and no pointer press has
    /// moved or claimed it since. Text fields show their caret and ring
    /// either way.
    pub focus_visible: bool,
    pub signal_store: &'a SignalStore,
    pub clock_ms: u64,
    /// Earliest clock time (ms) an element asked to be painted again at.
    next_frame_ms: Option<u64>,
    /// The window's animation table, when the host keeps one.
    animations: Option<&'a mut AnimationTable>,
    /// Keys that animated a style transition this frame.
    transition_keys: Vec<AnimKey>,
    pub debug_wireframe: bool,
    pub text_input_hit_areas: Vec<TextInputHitArea>,
    /// Elements that take IME input while focused, in paint order; see
    /// [`Self::register_ime_target`].
    pub ime_targets: Vec<ImeTarget>,
    pub selectable_text_runs: Vec<SelectableTextRegion>,
    pub tooltip_regions: Vec<TooltipRegion>,
    pub accessibility: AccessibilityFrame,
    pub semantic: SemanticFrame,
    /// Where this frame's identified elements landed, in paint order so
    /// far; becomes the frame's [`LayoutSnapshot`].
    pub geometry: LayoutSnapshot,
    /// Drop targets added so far; becomes the frame's [`DropTargets`].
    pub drop_targets: DropTargets,
    /// Native material regions painted so far; see
    /// [`Self::add_material_region`].
    pub material_regions: Vec<MaterialRegionRequest>,
    /// Inspector recording, style overrides, and phase timings.
    #[cfg(feature = "devtools")]
    pub devtools: crate::inspector::FrameProbe,
    pub(super) hovered: Vec<HitId>,
    /// Interaction groups and slots of this frame.
    pub(super) interaction: super::interaction::InteractionFrame,
    /// First hit row of each cache boundary recording now, innermost last.
    hit_recording_starts: Vec<usize>,
    /// Intersections of the clips pushed so far; the last one is current.
    clip_stack: Vec<ClipEntry>,
    /// The window's element cache, when the host keeps one.
    pub(super) cache: Option<&'a mut ElementCache>,
    /// Cache boundaries recording hits right now (they nest).
    hit_recordings: u32,
    /// Clip of each hit inserted while recording, relative to the
    /// innermost recording boundary; row `i` is hit row `local_hit_base + i`.
    local_hit_clips: Vec<Rect>,
    /// Ids of the same hits, so the boundary can read them back.
    local_hit_ids: Vec<HitId>,
    local_hit_base: usize,
    /// Reads of focus state, so a cache boundary knows its output depends
    /// on focus.
    focus_reads: Cell<u32>,
    /// Requests that make painted output depend on the clock (repaint
    /// requests, running transitions), so a cache boundary does not keep it.
    volatile_reads: u32,
    z_index_stack: Vec<i32>,
    /// Hit spaces of the transformed subtrees being prepainted, innermost
    /// last.
    hit_spaces: Vec<quark::HitSpace>,
    /// The transforms and clips of the elements being painted, innermost
    /// last, mirroring the scene's layers and clips: what geometry,
    /// accessibility bounds, and text hit regions are mapped through.
    paint_spaces: Vec<PaintSpace>,
    element_offset_stack: Vec<(f32, f32)>,
    text_color_stack: Vec<Color>,
    icon_color_stack: Vec<Color>,
    accessibility_text_hidden_stack: Vec<bool>,
    semantic_parent_stack: Vec<usize>,
    /// Scroll handles of the containers being prepainted, innermost last.
    scroll_stack: Vec<ScrollHandle>,
    /// Scroll handles prepainted inside cache boundaries, for their
    /// recordings; cleared when the outermost recording begins.
    scroll_watches: Vec<ScrollWatch>,
    /// Whether to build accessibility nodes: off while no assistive tech
    /// listens, so frames do not build labels nobody reads.
    accessibility_enabled: bool,
}

impl<'a> ElementContext<'a> {
    pub fn new(
        theme: &'a Theme,
        scale_factor: f32,
        text: &'a mut TextSystem,
        layouts: &'a mut LayoutCache,
        mouse_position: Option<(f32, f32)>,
        signal_store: &'a SignalStore,
    ) -> Self {
        Self {
            theme,
            scale_factor,
            text,
            layouts,
            mouse_position,
            hit_table: HitTable::default(),
            handlers: InputHandlers::default(),
            focus: None,
            focus_visible: true,
            signal_store,
            clock_ms: 0,
            next_frame_ms: None,
            animations: None,
            transition_keys: Vec::new(),
            debug_wireframe: false,
            text_input_hit_areas: Vec::new(),
            ime_targets: Vec::new(),
            selectable_text_runs: Vec::new(),
            tooltip_regions: Vec::new(),
            accessibility: AccessibilityFrame::default(),
            semantic: SemanticFrame::default(),
            geometry: {
                let mut geometry = LayoutSnapshot::default();
                geometry.reset();
                geometry
            },
            drop_targets: DropTargets::default(),
            material_regions: Vec::new(),
            #[cfg(feature = "devtools")]
            devtools: Default::default(),
            hovered: Vec::new(),
            interaction: Default::default(),
            hit_recording_starts: Vec::new(),
            clip_stack: Vec::new(),
            cache: None,
            hit_recordings: 0,
            local_hit_clips: Vec::new(),
            local_hit_ids: Vec::new(),
            local_hit_base: 0,
            focus_reads: Cell::new(0),
            volatile_reads: 0,
            z_index_stack: Vec::new(),
            hit_spaces: Vec::new(),
            paint_spaces: Vec::new(),
            element_offset_stack: Vec::new(),
            text_color_stack: Vec::new(),
            icon_color_stack: Vec::new(),
            accessibility_text_hidden_stack: Vec::new(),
            semantic_parent_stack: Vec::new(),
            scroll_stack: Vec::new(),
            scroll_watches: Vec::new(),
            accessibility_enabled: true,
        }
    }

    /// Shaped layout from the frame's cache, shared with paint and
    /// hit-testing. `None` when quark-text rejects the params (e.g. a zero
    /// font size), which callers treat as empty text.
    ///
    /// The layout is shaped at the context's scale factor whatever
    /// `params.scale_factor` says: its metrics stay logical while its glyphs
    /// are physical, and a scale change misses the cache instead of reusing
    /// a layout shaped for the old scale.
    pub fn layout_text(&mut self, params: &TextParams) -> Option<Arc<TextLayout>> {
        if params.scale_factor == self.scale_factor {
            return self.layouts.layout(self.text, params).ok();
        }
        let params = params.clone().scale_factor(self.scale_factor);
        self.layouts.layout(self.text, &params).ok()
    }

    /// [`Self::layout_text`] for borrowed params: a cache hit allocates
    /// nothing.
    pub fn layout_text_query(&mut self, query: &TextQuery) -> Option<Arc<TextLayout>> {
        let query = query.scale_factor(self.scale_factor);
        self.layouts.layout_query(self.text, &query).ok()
    }

    /// Advance width of single-line text, rounded up to whole pixels.
    pub fn measure_text_width(
        &mut self,
        text: &str,
        font_size: f32,
        font_kind: FontKind,
        font_weight: FontWeight,
    ) -> f32 {
        let style = TextStyle::new(font_size)
            .kind(font_kind)
            .weight(font_weight);
        self.measure_text_width_styled(text, style)
    }

    /// [`Self::measure_text_width`] in a full `style`: tracking, family,
    /// and weight measured exactly as text painted in it.
    pub fn measure_text_width_styled(&mut self, text: &str, style: TextStyle) -> f32 {
        if text.is_empty() {
            return 0.0;
        }
        self.layout_text_query(&TextQuery::new(text, style))
            .map_or(0.0, |layout| layout.size().0.ceil())
    }

    /// Read a signal's value (clones it out). Tracked by the current observer scope.
    pub fn read<T: 'static + Clone>(&self, signal: Signal<T>) -> T {
        self.signal_store.read(signal)
    }

    /// Read a signal's value without registering a dependency.
    pub fn read_untracked<T: 'static + Clone>(&self, signal: Signal<T>) -> T {
        self.signal_store.read_untracked(signal)
    }

    /// Access a signal's value by reference without cloning.
    pub fn with_signal<T: 'static, R>(&self, signal: Signal<T>, f: impl FnOnce(&T) -> R) -> R {
        self.signal_store.with(signal, f)
    }

    /// Replace a signal's value.
    pub fn write<T: 'static>(&mut self, signal: Signal<T>, value: T) {
        self.signal_store.write(signal, value);
    }

    /// Mutate a signal's value in place.
    pub fn update<T: 'static>(&mut self, signal: Signal<T>, f: impl FnOnce(&mut T)) {
        self.signal_store.update(signal, f);
    }

    pub fn with_focus(mut self, focus: Option<FocusId>) -> Self {
        self.focus = focus;
        self
    }

    /// See [`Self::focus_visible`].
    pub fn with_focus_visible(mut self, visible: bool) -> Self {
        self.focus_visible = visible;
        self
    }

    pub fn with_clock(mut self, clock_ms: u64) -> Self {
        self.clock_ms = clock_ms;
        self
    }

    /// Ask for another paint once the clock reaches `at_ms` (a caret blink,
    /// a toast expiring). The earliest request in a frame wins; the host
    /// reads it with [`Self::next_frame_ms`] and schedules that window only.
    pub fn request_frame_at_ms(&mut self, at_ms: u64) {
        self.volatile_reads += 1;
        self.next_frame_ms = Some(self.next_frame_ms.map_or(at_ms, |next| next.min(at_ms)));
    }

    /// The earliest repaint an element asked for this frame, if any,
    /// including the animation table's next deadline (never before the
    /// frame's clock). `None` when nothing is pending.
    pub fn next_frame_ms(&self) -> Option<u64> {
        let animation = self
            .animations
            .as_deref()
            .and_then(AnimationTable::next_deadline)
            .map(|at| at.max(self.clock_ms));
        match (self.next_frame_ms, animation) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// Build accessibility nodes this frame, or skip them (the default is
    /// to build). Hosts turn it off while no assistive tech listens.
    pub fn with_accessibility(mut self, enabled: bool) -> Self {
        self.accessibility_enabled = enabled;
        self
    }

    /// Whether elements should build accessibility nodes this frame.
    pub fn accessibility_enabled(&self) -> bool {
        self.accessibility_enabled
    }

    /// Attach the window's element cache so [`cached`] boundaries reuse
    /// their output across frames and layout reuses one engine.
    pub fn with_element_cache(mut self, cache: &'a mut ElementCache) -> Self {
        self.cache = Some(cache);
        self
    }

    /// Build this frame's hit table, handlers, and semantic tree in the
    /// buffers of an earlier frame (from [`InputRouter::replace_frame`]),
    /// so they are not allocated again.
    pub fn with_input_frame(mut self, mut frame: InputFrame) -> Self {
        frame.hits.reset();
        frame.handlers.clear();
        frame.semantic.clear();
        frame.geometry.reset();
        frame.drop_targets.clear();
        frame.material_regions.clear();
        self.material_regions = frame.material_regions;
        self.hit_table = frame.hits;
        self.handlers = frame.handlers;
        self.semantic = frame.semantic;
        self.geometry = frame.geometry;
        self.drop_targets = frame.drop_targets;
        self
    }

    /// Attach the window's animation table so divs can run transitions.
    /// The table persists across frames; the context only borrows it.
    pub fn with_animations(mut self, table: &'a mut AnimationTable) -> Self {
        self.animations = Some(table);
        self
    }

    /// End the frame for animations: drop transition rows of keyed elements
    /// this frame did not paint, so removed elements do not keep rows. Call
    /// once after painting every root of the frame.
    pub fn finish_frame(&mut self) {
        if let Some(table) = self.animations.as_deref_mut() {
            crate::animation::sweep_transitions(table, &mut self.transition_keys);
        }
        self.transition_keys.clear();
        if let Some(cache) = self.cache.as_deref_mut() {
            std::mem::swap(
                &mut cache.buffers.transition_keys,
                &mut self.transition_keys,
            );
        }
    }

    pub fn animations(&self) -> Option<&AnimationTable> {
        self.animations.as_deref()
    }

    pub(crate) fn animations_mut(&mut self) -> Option<&mut AnimationTable> {
        self.animations.as_deref_mut()
    }

    /// Prepaint children of a container scrolled by `handle` between this
    /// and [`Self::pop_scroll_handle`].
    pub(crate) fn push_scroll_handle(&mut self, handle: &ScrollHandle) {
        self.scroll_stack.push(handle.clone());
    }

    pub(crate) fn pop_scroll_handle(&mut self) {
        self.scroll_stack.pop();
    }

    /// Tell the innermost tracked scroll container that the element keyed
    /// `key` was prepainted at `bounds`, for
    /// [`ScrollHandle::scroll_to_item`]. Free outside scroll containers.
    pub fn record_scroll_item(&mut self, key: &UiKey, bounds: Bounds) {
        if let Some(handle) = self.scroll_stack.last() {
            handle.record_item(quark::stable_hash(key.as_str()), bounds);
        }
    }

    /// Move `(key, prop)` toward `target` with `motion`, starting from its
    /// current value, and return `(value now, still moving)`. A row seen for
    /// the first time snaps to `target`. Without a table, returns the target.
    pub(crate) fn transition(
        &mut self,
        key: AnimKey,
        prop: PropId,
        target: f32,
        motion: Motion,
    ) -> (f32, bool) {
        let Some(table) = self.animations.as_deref_mut() else {
            return (target, false);
        };
        if self.transition_keys.last() != Some(&key) {
            self.transition_keys.push(key);
        }
        table.animate_to(key, prop, target, motion, self.clock_ms);
        let animating = table.is_animating(key, prop);
        if animating {
            self.volatile_reads += 1;
        }
        (table.get(key, prop).unwrap_or(target), animating)
    }

    /// Keep transition rows of `keys` through this frame's sweep: a
    /// replayed cache boundary paints them without calling `transition`.
    pub(super) fn keep_transition_keys(&mut self, keys: &[AnimKey]) {
        if self.animations.is_some() {
            self.transition_keys.extend_from_slice(keys);
        }
    }

    pub(super) fn transition_keys(&self) -> &[AnimKey] {
        &self.transition_keys
    }

    pub(super) fn focus_reads(&self) -> u32 {
        self.focus_reads.get()
    }

    pub(super) fn volatile_reads(&self) -> u32 {
        self.volatile_reads
    }

    pub fn is_focused(&self, target: FocusId) -> bool {
        self.focus_reads.set(self.focus_reads.get().wrapping_add(1));
        self.focus == Some(target)
    }

    /// Whether `target` is focused and its focus should show
    /// ([`Self::focus_visible`]): what focus rings paint on.
    pub fn is_focus_visible(&self, target: FocusId) -> bool {
        self.is_focused(target) && self.focus_visible
    }

    pub fn current_z_index(&self) -> i32 {
        *self.z_index_stack.last().unwrap_or(&0)
    }

    pub fn push_z_index(&mut self, z: i32) {
        self.z_index_stack.push(z);
    }

    pub fn pop_z_index(&mut self) {
        self.z_index_stack.pop();
    }

    pub fn push_text_color(&mut self, color: Color) {
        self.text_color_stack.push(color);
    }

    pub fn pop_text_color(&mut self) {
        self.text_color_stack.pop();
    }

    pub fn text_color_override(&self) -> Option<Color> {
        self.text_color_stack.last().copied()
    }

    pub fn push_icon_color(&mut self, color: Color) {
        self.icon_color_stack.push(color);
    }

    pub fn pop_icon_color(&mut self) {
        self.icon_color_stack.pop();
    }

    pub fn icon_color_override(&self) -> Option<Color> {
        self.icon_color_stack.last().copied()
    }

    pub fn push_accessibility_text_hidden(&mut self, hidden: bool) {
        let inherited = self
            .accessibility_text_hidden_stack
            .last()
            .copied()
            .unwrap_or(false);
        self.accessibility_text_hidden_stack
            .push(inherited || hidden);
    }

    pub fn pop_accessibility_text_hidden(&mut self) {
        self.accessibility_text_hidden_stack.pop();
    }

    pub fn accessibility_text_hidden(&self) -> bool {
        self.accessibility_text_hidden_stack
            .last()
            .copied()
            .unwrap_or(false)
    }

    pub fn current_semantic_parent(&self) -> Option<usize> {
        self.semantic_parent_stack.last().copied()
    }

    pub fn push_semantic_parent(&mut self, index: usize) {
        self.semantic_parent_stack.push(index);
    }

    pub fn pop_semantic_parent(&mut self) {
        self.semantic_parent_stack.pop();
    }

    /// Push an accessibility node under the nearest semantic ancestor that has
    /// an accessibility node, or under the window when none does.
    /// The node's bounds are in layout coordinates; it is published at the
    /// window bounds of their four corners under the current transform.
    pub fn push_accessibility(&mut self, mut node: AccessibilityNode) -> accesskit::NodeId {
        if !self.accessibility_enabled {
            return crate::accessibility::ROOT_ID;
        }
        node.set_transform(self.current_transform());
        let parent = self.accessible_semantic_ancestor();
        self.accessibility.push_child(node, parent)
    }

    /// Push an accessibility node under `parent`, a node this frame already
    /// has, with its bounds in layout coordinates like
    /// [`Self::push_accessibility`]: for a part of an element's own node,
    /// such as the composition shown in a terminal.
    pub fn push_accessibility_child(
        &mut self,
        mut node: AccessibilityNode,
        parent: accesskit::NodeId,
    ) -> accesskit::NodeId {
        if !self.accessibility_enabled {
            return crate::accessibility::ROOT_ID;
        }
        node.set_transform(self.current_transform());
        self.accessibility.push_child(node, Some(parent))
    }

    /// Register the element painting now as an IME target: while `focus`
    /// has keyboard focus, the host turns IME on and places the candidate
    /// window at `caret` (layout coordinates). Text fields register
    /// themselves; call this from `paint` for other elements that take
    /// composition, such as a terminal.
    ///
    /// The caret is dropped (the target stays) under a transform that
    /// flattens it or where the current clips hide it. A cache boundary
    /// around the call paints fresh each frame, so the registration is
    /// never replayed with stale geometry.
    pub fn register_ime_target(&mut self, focus: FocusId, caret: Option<Rect>) {
        let space = self.current_paint_space();
        let caret = caret
            .filter(|_| space.transform.invert().is_some())
            .map(|caret| window_rect(space.transform, caret))
            .filter(|caret| {
                let clip = space.window_clip;
                caret.x <= clip.right()
                    && caret.right() >= clip.x
                    && caret.y <= clip.bottom()
                    && caret.bottom() >= clip.y
            });
        self.volatile_reads += 1;
        self.ime_targets.push(ImeTarget {
            focus_target: focus,
            caret,
        });
    }

    /// Like [`Self::push_accessibility`], and records that the node represents
    /// semantic node `semantic_index`, so descendants nest beneath it.
    pub fn push_accessibility_for_semantic(
        &mut self,
        node: AccessibilityNode,
        semantic_index: usize,
    ) -> accesskit::NodeId {
        let id = self.push_accessibility(node);
        if self.accessibility_enabled {
            self.accessibility.bind_semantic(semantic_index, id);
        }
        id
    }

    pub(super) fn accessible_semantic_ancestor(&self) -> Option<accesskit::NodeId> {
        let mut current = self.current_semantic_parent();
        while let Some(index) = current {
            if let Some(id) = self.accessibility.semantic_owner(index) {
                return Some(id);
            }
            // Parents are pushed before children; requiring a smaller index
            // guarantees the walk ends even on a malformed frame.
            current = self
                .semantic
                .nodes()
                .get(index)
                .and_then(|node| node.parent)
                .filter(|parent| *parent < index);
        }
        None
    }

    pub fn current_element_offset(&self) -> (f32, f32) {
        *self.element_offset_stack.last().unwrap_or(&(0.0, 0.0))
    }

    pub fn push_element_offset(&mut self, offset_x: f32, offset_y: f32) {
        let (base_x, base_y) = self.current_element_offset();
        self.element_offset_stack
            .push((base_x + offset_x, base_y + offset_y));
    }

    pub fn pop_element_offset(&mut self) {
        self.element_offset_stack.pop();
    }

    pub fn current_clip(&self) -> Rect {
        self.clip_stack
            .last()
            .map_or(quark::hit::UNCLIPPED, |entry| entry.effective)
    }

    fn current_local_clip(&self) -> Rect {
        self.clip_stack
            .last()
            .map_or(quark::hit::UNCLIPPED, |entry| entry.local)
    }

    /// Clip later hit entries to `rect`, intersected with the current clip,
    /// mirroring `Scene::clip` for hit testing.
    pub fn push_clip(&mut self, rect: Rect) {
        self.clip_stack.push(ClipEntry {
            effective: intersect(self.current_clip(), rect),
            local: intersect(self.current_local_clip(), rect),
        });
    }

    pub fn pop_clip(&mut self) {
        self.clip_stack.pop();
    }

    /// Hit entries inserted until [`Self::pop_transform`] are drawn through
    /// `transform` (layout coordinates to where they land): the pointer is
    /// mapped back through it before testing them. Clips pushed inside are
    /// in layout coordinates; the current clip still clips the subtree.
    pub fn push_transform(&mut self, transform: quark::Transform2D) {
        let parent = self.hit_spaces.last().copied();
        let space = self
            .hit_table
            .push_space(parent, transform, self.current_clip());
        self.hit_spaces.push(space);
        self.clip_stack.push(ClipEntry {
            effective: quark::hit::UNCLIPPED,
            local: quark::hit::UNCLIPPED,
        });
        // Cache boundaries replay hits without their spaces, so a subtree
        // with a transform inside is painted fresh each frame.
        self.volatile_reads += 1;
    }

    pub fn pop_transform(&mut self) {
        if self.hit_spaces.pop().is_some() {
            self.clip_stack.pop();
        }
    }

    fn current_paint_space(&self) -> PaintSpace {
        self.paint_spaces
            .last()
            .copied()
            .unwrap_or(PaintSpace::WINDOW)
    }

    /// Where layout coordinates painted now land in the window: the
    /// transforms of the elements being painted, innermost first. Identity
    /// outside transformed subtrees and outside paint.
    pub fn current_transform(&self) -> quark::Transform2D {
        self.current_paint_space().transform
    }

    /// Paint what follows, until [`Self::pop_paint_transform`], through
    /// `transform` (layout coordinates to where they land in the parent):
    /// the paint-time twin of [`Self::push_transform`], pushed with the
    /// scene layer that draws it.
    pub fn push_paint_transform(&mut self, transform: quark::Transform2D) {
        let space = self.current_paint_space();
        self.paint_spaces.push(PaintSpace {
            transform: transform.then(space.transform),
            window_clip: space.window_clip,
            // A transformed subtree is never replayed (see
            // `push_transform`), so its recording-relative clips are moot.
            local_clip: quark::hit::UNCLIPPED,
        });
    }

    pub fn pop_paint_transform(&mut self) {
        self.paint_spaces.pop();
    }

    /// Clip what follows, until [`Self::pop_paint_clip`], to `rect` (in
    /// layout coordinates), with the scene clip that draws it.
    pub fn push_paint_clip(&mut self, rect: Rect) {
        let space = self.current_paint_space();
        self.paint_spaces.push(PaintSpace {
            transform: space.transform,
            window_clip: intersect(space.window_clip, window_rect(space.transform, rect)),
            local_clip: intersect(space.local_clip, rect),
        });
    }

    pub fn pop_paint_clip(&mut self) {
        self.paint_spaces.pop();
    }

    /// Record that the element named by `id`, `test_id`, or `handle` was
    /// painted at `layout` (layout coordinates, before the current
    /// transform), for [`LayoutSnapshot`] lookups. Does nothing without a
    /// name.
    pub fn record_geometry(
        &mut self,
        id: Option<&UiNodeId>,
        test_id: Option<&TestId>,
        handle: Option<ElementHandle>,
        layout: Rect,
    ) {
        let key = GeometryKey {
            id: id.cloned(),
            test_id: test_id.cloned(),
            handle,
        };
        if key.is_empty() {
            return;
        }
        self.push_geometry(key, layout, quark::hit::UNCLIPPED);
    }

    /// Publish `target` (layout coordinates, before the current transform)
    /// as a drop target of this frame, under the current transform, clips,
    /// and z-index; see [`DropTargets`]. Call it while painting.
    pub fn add_drop_target(&mut self, target: DropTarget) {
        let space = self.current_paint_space();
        self.drop_targets.push(DropTargetRow {
            target,
            transform: space.transform,
            window_clip: space.window_clip,
            z: self.current_z_index(),
        });
        // Cache boundaries do not record drop targets, so one adding them
        // is painted fresh each frame.
        self.volatile_reads += 1;
    }

    /// `rect` (layout coordinates) in window coordinates within the current
    /// clips, when no rotation or scale applies; `None` under one or when
    /// the clips hide it.
    pub(super) fn window_rect_if_untransformed(&self, rect: Rect) -> Option<Rect> {
        let space = self.current_paint_space();
        if !space.transform.is_identity() {
            return None;
        }
        rect.intersection(space.window_clip)
    }

    /// Push a geometry row clipped to `clip` (layout coordinates) within
    /// the current clips: how a replayed cache boundary republishes its
    /// rows under the current transform.
    pub(super) fn push_geometry(&mut self, key: GeometryKey, layout: Rect, clip: Rect) {
        let space = self.current_paint_space();
        self.geometry.push(GeometryRow {
            key,
            layout,
            transform: space.transform,
            window_clip: intersect(space.window_clip, window_rect(space.transform, clip)),
            local_clip: intersect(space.local_clip, clip),
        });
    }

    /// Start recording the geometry rows a cache boundary paints: their
    /// clips are kept relative to the boundary. Returns the first row.
    pub(super) fn begin_geometry_recording(&mut self) -> usize {
        let space = self.current_paint_space();
        self.paint_spaces.push(PaintSpace {
            local_clip: quark::hit::UNCLIPPED,
            ..space
        });
        self.geometry.len()
    }

    /// End a recording: the rows' clips become relative to the enclosing
    /// recording boundary, if any.
    pub(super) fn end_geometry_recording(&mut self, start: usize) {
        self.paint_spaces.pop();
        let outer = self.current_paint_space().local_clip;
        self.geometry.clip_local_from(start, outer);
    }

    /// Register a hit entry at the current z and clip. Bind it to its
    /// semantic node with [`Self::bind_hit`] once that node exists.
    pub fn insert_hit(&mut self, bounds: Bounds, flags: HitFlags, cursor: CursorHint) -> HitId {
        let z = self.current_z_index();
        self.insert_hit_clipped(bounds, quark::hit::UNCLIPPED, z, flags, cursor)
    }

    /// Insert a hit entry clipped to `clip` within the current clip, at
    /// layer `z`: how a replayed cache boundary re-registers its hits.
    pub(super) fn insert_hit_clipped(
        &mut self,
        bounds: Bounds,
        clip: Rect,
        z: i32,
        flags: HitFlags,
        cursor: CursorHint,
    ) -> HitId {
        let local = intersect(self.current_local_clip(), clip);
        let clip = intersect(self.current_clip(), clip);
        let space = self.hit_spaces.last().copied();
        let id = self
            .hit_table
            .push_in(bounds, clip, z, flags, cursor, space);
        if self.hit_recordings > 0 {
            self.local_hit_clips.push(local);
            self.local_hit_ids.push(id);
        }
        id
    }

    /// Start recording the hits a cache boundary inserts: their clips are
    /// kept relative to the boundary, so a replay can clip them again
    /// under a different ancestor clip. Returns the first recorded row.
    pub(super) fn begin_hit_recording(&mut self) -> usize {
        if self.hit_recordings == 0 {
            self.local_hit_clips.clear();
            self.local_hit_ids.clear();
            self.scroll_watches.clear();
            self.local_hit_base = self.hit_table.len();
        }
        self.hit_recordings += 1;
        self.hit_recording_starts.push(self.hit_table.len());
        self.clip_stack.push(ClipEntry {
            effective: self.current_clip(),
            local: quark::hit::UNCLIPPED,
        });
        self.hit_table.len()
    }

    /// Ids and boundary-relative clips of hit rows `start..`, inserted since
    /// the matching [`Self::begin_hit_recording`].
    pub(super) fn recorded_hits(&self, start: usize) -> (&[HitId], &[Rect]) {
        let from = start - self.local_hit_base;
        (&self.local_hit_ids[from..], &self.local_hit_clips[from..])
    }

    /// End a recording: the rows' clips become relative to the enclosing
    /// recording boundary, if any.
    pub(super) fn end_hit_recording(&mut self, start: usize) {
        self.clip_stack.pop();
        self.hit_recordings -= 1;
        self.hit_recording_starts.pop();
        if self.hit_recordings > 0 {
            let outer = self.current_local_clip();
            for clip in &mut self.local_hit_clips[start - self.local_hit_base..] {
                *clip = intersect(*clip, outer);
            }
        }
    }

    /// Where this frame's prepaint output stands, for
    /// [`Self::rewind_prepaint`].
    pub(super) fn prepaint_mark(&self) -> PrepaintMark {
        PrepaintMark {
            hits: self.hit_table.len(),
            local_hits: self.local_hit_ids.len(),
            scroll_watches: self.scroll_watches.len(),
            interaction: self.interaction.mark(),
        }
    }

    /// Forget what prepaint registered since `mark` (hit rows, and the
    /// recordings and groups of the elements that inserted them), so a
    /// subtree can be prepainted again at another offset.
    pub(super) fn rewind_prepaint(&mut self, mark: PrepaintMark) {
        self.hit_table.truncate(mark.hits);
        self.local_hit_ids.truncate(mark.local_hits);
        self.local_hit_clips.truncate(mark.local_hits);
        self.scroll_watches.truncate(mark.scroll_watches);
        self.interaction.rewind(mark.interaction);
    }

    /// First hit row of the innermost cache boundary recording now.
    pub(super) fn innermost_hit_recording(&self) -> Option<usize> {
        self.hit_recording_starts.last().copied()
    }

    /// Make what is being painted depend on more than a cache boundary's
    /// inputs, so the boundary does not keep it.
    pub(super) fn mark_volatile(&mut self) {
        self.volatile_reads += 1;
    }

    /// Note a scroll handle prepainted now, when a cache boundary records.
    pub(super) fn watch_scroll(&mut self, watch: impl FnOnce() -> ScrollWatch) {
        if self.hit_recordings > 0 {
            self.scroll_watches.push(watch());
        }
    }

    /// Handles noted by [`Self::watch_scroll`] in this recording.
    pub(super) fn scroll_watches(&self) -> &[ScrollWatch] {
        &self.scroll_watches
    }

    pub fn bind_hit(&mut self, id: HitId, node: usize) {
        self.hit_table.set_node(id, node);
    }

    pub fn is_hovered(&self, id: HitId) -> bool {
        self.hovered.contains(&id)
    }

    pub fn run_hit_test(&mut self) {
        if self.has_interaction_slots() {
            self.resolve_interaction();
            return;
        }
        match self.mouse_position {
            Some((x, y)) => self.hit_table.stack_at_into(x, y, &mut self.hovered),
            None => self.hovered.clear(),
        }
    }

    /// Move this frame's hit table, handlers, and semantic tree out for an
    /// [`InputRouter`].
    pub fn take_input_frame(&mut self) -> InputFrame {
        self.drop_targets.set_frame(self.geometry.frame());
        InputFrame {
            hits: std::mem::take(&mut self.hit_table),
            handlers: std::mem::take(&mut self.handlers),
            semantic: std::mem::take(&mut self.semantic),
            geometry: std::mem::take(&mut self.geometry),
            drop_targets: std::mem::take(&mut self.drop_targets),
            material_regions: std::mem::take(&mut self.material_regions),
        }
    }
}

/// See [`ElementContext::prepaint_mark`].
#[derive(Clone, Copy)]
pub(super) struct PrepaintMark {
    hits: usize,
    local_hits: usize,
    scroll_watches: usize,
    interaction: (usize, usize),
}

/// One level of the paint space stack.
#[derive(Clone, Copy)]
struct PaintSpace {
    /// Layout coordinates to window coordinates.
    transform: quark::Transform2D,
    /// Intersection of the clips so far, in window coordinates (the bounds
    /// of a transformed clip).
    window_clip: Rect,
    /// Intersection of the clips pushed since the innermost recording
    /// cache boundary, in layout coordinates.
    local_clip: Rect,
}

impl PaintSpace {
    const WINDOW: Self = Self {
        transform: quark::Transform2D::IDENTITY,
        window_clip: quark::hit::UNCLIPPED,
        local_clip: quark::hit::UNCLIPPED,
    };
}

/// One level of the hit clip stack: the clip in window space, and the clip
/// relative to the innermost recording cache boundary.
#[derive(Clone, Copy)]
struct ClipEntry {
    effective: Rect,
    local: Rect,
}

fn intersect(a: Rect, b: Rect) -> Rect {
    a.intersection(b).unwrap_or(quark::hit::EMPTY_CLIP)
}

/// The context's working stacks, kept by the window's [`ElementCache`]
/// between frames so a frame reuses last frame's capacity. They are empty
/// whenever a frame is not being rendered.
#[derive(Default)]
pub(super) struct FrameBuffers {
    clip_stack: Vec<ClipEntry>,
    z_index_stack: Vec<i32>,
    element_offset_stack: Vec<(f32, f32)>,
    text_color_stack: Vec<Color>,
    icon_color_stack: Vec<Color>,
    accessibility_text_hidden_stack: Vec<bool>,
    semantic_parent_stack: Vec<usize>,
    hovered: Vec<HitId>,
    interaction: super::interaction::InteractionFrame,
    hit_recording_starts: Vec<usize>,
    local_hit_clips: Vec<Rect>,
    local_hit_ids: Vec<HitId>,
    paint_spaces: Vec<PaintSpace>,
    scroll_stack: Vec<ScrollHandle>,
    scroll_watches: Vec<ScrollWatch>,
    /// Kept apart: the context needs its keys until `finish_frame`.
    transition_keys: Vec<AnimKey>,
}

/// Swap each listed field between the buffers and the context.
macro_rules! swap_buffers {
    ($buffers:expr, $cx:expr) => {
        std::mem::swap(&mut $buffers.clip_stack, &mut $cx.clip_stack);
        std::mem::swap(&mut $buffers.z_index_stack, &mut $cx.z_index_stack);
        std::mem::swap(
            &mut $buffers.element_offset_stack,
            &mut $cx.element_offset_stack,
        );
        std::mem::swap(&mut $buffers.text_color_stack, &mut $cx.text_color_stack);
        std::mem::swap(&mut $buffers.icon_color_stack, &mut $cx.icon_color_stack);
        std::mem::swap(
            &mut $buffers.accessibility_text_hidden_stack,
            &mut $cx.accessibility_text_hidden_stack,
        );
        std::mem::swap(
            &mut $buffers.semantic_parent_stack,
            &mut $cx.semantic_parent_stack,
        );
        std::mem::swap(&mut $buffers.hovered, &mut $cx.hovered);
        std::mem::swap(&mut $buffers.interaction, &mut $cx.interaction);
        std::mem::swap(
            &mut $buffers.hit_recording_starts,
            &mut $cx.hit_recording_starts,
        );
        std::mem::swap(&mut $buffers.local_hit_clips, &mut $cx.local_hit_clips);
        std::mem::swap(&mut $buffers.local_hit_ids, &mut $cx.local_hit_ids);
        std::mem::swap(&mut $buffers.paint_spaces, &mut $cx.paint_spaces);
        std::mem::swap(&mut $buffers.scroll_stack, &mut $cx.scroll_stack);
        std::mem::swap(&mut $buffers.scroll_watches, &mut $cx.scroll_watches);
    };
}

impl ElementContext<'_> {
    /// Borrow the cache's stacks for a render (they are empty between
    /// renders), and its transition key buffer once per frame.
    pub(super) fn load_buffers(&mut self) {
        let Some(cache) = self.cache.as_deref_mut() else {
            return;
        };
        let mut buffers = std::mem::take(&mut cache.buffers);
        swap_buffers!(buffers, self);
        if self.transition_keys.is_empty() {
            std::mem::swap(&mut buffers.transition_keys, &mut self.transition_keys);
            self.transition_keys.clear();
        }
        if let Some(cache) = self.cache.as_deref_mut() {
            cache.buffers = buffers;
        }
    }

    /// Return the stacks after a render; they are balanced, so empty.
    pub(super) fn store_buffers(&mut self) {
        let Some(cache) = self.cache.as_deref_mut() else {
            return;
        };
        let mut buffers = std::mem::take(&mut cache.buffers);
        swap_buffers!(buffers, self);
        buffers.clip_stack.clear();
        buffers.z_index_stack.clear();
        buffers.element_offset_stack.clear();
        buffers.text_color_stack.clear();
        buffers.icon_color_stack.clear();
        buffers.accessibility_text_hidden_stack.clear();
        buffers.semantic_parent_stack.clear();
        buffers.hovered.clear();
        buffers.interaction.clear();
        buffers.hit_recording_starts.clear();
        buffers.local_hit_clips.clear();
        buffers.local_hit_ids.clear();
        buffers.paint_spaces.clear();
        buffers.scroll_stack.clear();
        buffers.scroll_watches.clear();
        if let Some(cache) = self.cache.as_deref_mut() {
            cache.buffers = buffers;
        }
    }
}
