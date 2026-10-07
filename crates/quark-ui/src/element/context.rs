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
    pub selectable_text_runs: Vec<SelectableTextRegion>,
    pub scrollbar_tracks: Vec<ScrollbarTrack>,
    pub tooltip_regions: Vec<TooltipRegion>,
    pub accessibility: AccessibilityFrame,
    pub semantic: SemanticFrame,
    /// Inspector recording, style overrides, and phase timings.
    #[cfg(feature = "devtools")]
    pub devtools: crate::inspector::FrameProbe,
    hovered: Vec<HitId>,
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
    element_offset_stack: Vec<(f32, f32)>,
    text_color_stack: Vec<Color>,
    icon_color_stack: Vec<Color>,
    accessibility_text_hidden_stack: Vec<bool>,
    semantic_parent_stack: Vec<usize>,
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
            signal_store,
            clock_ms: 0,
            next_frame_ms: None,
            animations: None,
            transition_keys: Vec::new(),
            debug_wireframe: false,
            text_input_hit_areas: Vec::new(),
            selectable_text_runs: Vec::new(),
            scrollbar_tracks: Vec::new(),
            tooltip_regions: Vec::new(),
            accessibility: AccessibilityFrame::default(),
            semantic: SemanticFrame::default(),
            #[cfg(feature = "devtools")]
            devtools: Default::default(),
            hovered: Vec::new(),
            clip_stack: Vec::new(),
            cache: None,
            hit_recordings: 0,
            local_hit_clips: Vec::new(),
            local_hit_ids: Vec::new(),
            local_hit_base: 0,
            focus_reads: Cell::new(0),
            volatile_reads: 0,
            z_index_stack: vec![0],
            element_offset_stack: vec![(0.0, 0.0)],
            text_color_stack: Vec::new(),
            icon_color_stack: Vec::new(),
            accessibility_text_hidden_stack: Vec::new(),
            semantic_parent_stack: Vec::new(),
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

    /// Advance width of single-line text, rounded up to whole pixels.
    pub fn measure_text_width(
        &mut self,
        text: &str,
        font_size: f32,
        font_kind: FontKind,
        font_weight: FontWeight,
    ) -> f32 {
        if text.is_empty() {
            return 0.0;
        }
        let style = TextStyle::new(font_size)
            .kind(font_kind)
            .weight(font_weight);
        self.layout_text(&TextParams::new(text, style))
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

    /// Attach the window's element cache so [`cached`] boundaries reuse
    /// their output across frames and layout reuses one engine.
    pub fn with_element_cache(mut self, cache: &'a mut ElementCache) -> Self {
        self.cache = Some(cache);
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
    }

    pub fn animations(&self) -> Option<&AnimationTable> {
        self.animations.as_deref()
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

    pub fn current_z_index(&self) -> i32 {
        *self.z_index_stack.last().unwrap_or(&0)
    }

    pub fn push_z_index(&mut self, z: i32) {
        self.z_index_stack.push(z);
    }

    pub fn pop_z_index(&mut self) {
        if self.z_index_stack.len() > 1 {
            self.z_index_stack.pop();
        }
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
    pub fn push_accessibility(&mut self, node: AccessibilityNode) -> accesskit::NodeId {
        let parent = self.accessible_semantic_ancestor();
        self.accessibility.push_child(node, parent)
    }

    /// Like [`Self::push_accessibility`], and records that the node represents
    /// semantic node `semantic_index`, so descendants nest beneath it.
    pub fn push_accessibility_for_semantic(
        &mut self,
        node: AccessibilityNode,
        semantic_index: usize,
    ) -> accesskit::NodeId {
        let id = self.push_accessibility(node);
        self.accessibility.bind_semantic(semantic_index, id);
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
        if self.element_offset_stack.len() > 1 {
            self.element_offset_stack.pop();
        }
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
        let id = self.hit_table.push(bounds, clip, z, flags, cursor);
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
            self.local_hit_base = self.hit_table.len();
        }
        self.hit_recordings += 1;
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
        if self.hit_recordings > 0 {
            let outer = self.current_local_clip();
            for clip in &mut self.local_hit_clips[start - self.local_hit_base..] {
                *clip = intersect(*clip, outer);
            }
        }
    }

    pub fn bind_hit(&mut self, id: HitId, node: usize) {
        self.hit_table.set_node(id, node);
    }

    pub fn is_hovered(&self, id: HitId) -> bool {
        self.hovered.contains(&id)
    }

    pub fn run_hit_test(&mut self) {
        self.hovered = match self.mouse_position {
            Some((x, y)) => self.hit_table.stack_at(x, y),
            None => Vec::new(),
        };
    }

    /// Move this frame's hit table, handlers, and semantic tree out for an
    /// [`InputRouter`].
    pub fn take_input_frame(&mut self) -> InputFrame {
        InputFrame {
            hits: std::mem::take(&mut self.hit_table),
            handlers: std::mem::take(&mut self.handlers),
            semantic: std::mem::take(&mut self.semantic),
        }
    }
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
