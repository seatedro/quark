use super::*;

// ---------------------------------------------------------------------------
// ElementContext
// ---------------------------------------------------------------------------

pub struct ElementContext<'a> {
    pub theme: &'a Theme,
    pub scale_factor: f32,
    pub font_system: &'a mut glyphon::FontSystem,
    pub mouse_position: Option<(f32, f32)>,
    /// Pointer hit entries in paint order; built in prepaint.
    pub hit_table: HitTable,
    /// Event handlers keyed by semantic node; registered in paint.
    pub handlers: InputHandlers,
    pub focus: Option<FocusId>,
    pub signal_store: &'a SignalStore,
    pub clock_ms: u64,
    pub debug_wireframe: bool,
    pub text_input_hit_areas: Vec<TextInputHitArea>,
    pub selectable_text_runs: Vec<SelectableTextRegion>,
    pub scrollbar_tracks: Vec<ScrollbarTrack>,
    pub tooltip_regions: Vec<TooltipRegion>,
    pub accessibility: AccessibilityFrame,
    pub semantic: SemanticFrame,
    hovered: Vec<HitId>,
    /// Intersections of the clips pushed so far; the last one is current.
    clip_stack: Vec<Rect>,
    z_index_stack: Vec<i32>,
    element_offset_stack: Vec<(f32, f32)>,
    text_color_stack: Vec<Color>,
    icon_color_stack: Vec<Color>,
    frame_text_measure_cache: HashMap<TextMeasureKey, f32>,
    persistent_text_measure_cache: Option<&'a mut TextMeasureCache>,
    accessibility_text_hidden_stack: Vec<bool>,
    semantic_parent_stack: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct TextMeasureKey {
    text: String,
    font_size_bits: u32,
    font_kind: u8,
    font_weight: u8,
}

#[derive(Debug, Default)]
pub(crate) struct TextMeasureCache {
    widths: HashMap<TextMeasureKey, f32>,
    order: VecDeque<TextMeasureKey>,
}

impl TextMeasureCache {
    const MAX_ENTRIES: usize = 65_536;

    pub(crate) fn clear(&mut self) {
        self.widths.clear();
        self.order.clear();
    }

    fn get(&self, key: &TextMeasureKey) -> Option<f32> {
        self.widths.get(key).copied()
    }

    fn insert(&mut self, key: TextMeasureKey, width: f32) {
        if !self.widths.contains_key(&key) {
            self.order.push_back(key.clone());
            while self.widths.len() >= Self::MAX_ENTRIES {
                let Some(oldest) = self.order.pop_front() else {
                    break;
                };
                self.widths.remove(&oldest);
            }
        }
        self.widths.insert(key, width);
    }
}

impl<'a> ElementContext<'a> {
    pub fn new(
        theme: &'a Theme,
        scale_factor: f32,
        font_system: &'a mut glyphon::FontSystem,
        mouse_position: Option<(f32, f32)>,
        signal_store: &'a SignalStore,
    ) -> Self {
        Self {
            theme,
            scale_factor,
            font_system,
            mouse_position,
            hit_table: HitTable::default(),
            handlers: InputHandlers::default(),
            focus: None,
            signal_store,
            clock_ms: 0,
            debug_wireframe: false,
            text_input_hit_areas: Vec::new(),
            selectable_text_runs: Vec::new(),
            scrollbar_tracks: Vec::new(),
            tooltip_regions: Vec::new(),
            accessibility: AccessibilityFrame::default(),
            semantic: SemanticFrame::default(),
            hovered: Vec::new(),
            clip_stack: Vec::new(),
            z_index_stack: vec![0],
            element_offset_stack: vec![(0.0, 0.0)],
            text_color_stack: Vec::new(),
            icon_color_stack: Vec::new(),
            frame_text_measure_cache: HashMap::new(),
            persistent_text_measure_cache: None,
            accessibility_text_hidden_stack: Vec::new(),
            semantic_parent_stack: Vec::new(),
        }
    }

    pub(crate) fn with_text_measure_cache(mut self, cache: &'a mut TextMeasureCache) -> Self {
        self.persistent_text_measure_cache = Some(cache);
        self
    }

    pub fn measure_text_width(
        &mut self,
        text: &str,
        font_size: f32,
        font_kind: quark_render::FontKind,
        font_weight: quark_render::FontWeight,
    ) -> f32 {
        if text.is_empty() {
            return 0.0;
        }
        let key = TextMeasureKey {
            text: text.to_owned(),
            font_size_bits: font_size.to_bits(),
            font_kind: font_kind_measure_tag(font_kind),
            font_weight: font_weight_measure_tag(font_weight),
        };
        if let Some(cache) = self.persistent_text_measure_cache.as_ref()
            && let Some(width) = cache.get(&key)
        {
            return width;
        }
        if let Some(width) = self.frame_text_measure_cache.get(&key).copied() {
            return width;
        }
        let width = measure_text_width(self.font_system, text, font_size, font_kind, font_weight);
        if let Some(cache) = self.persistent_text_measure_cache.as_mut() {
            cache.insert(key, width);
        } else {
            self.frame_text_measure_cache.insert(key, width);
        }
        width
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

    pub fn is_focused(&self, target: FocusId) -> bool {
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

    fn accessible_semantic_ancestor(&self) -> Option<accesskit::NodeId> {
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
            .copied()
            .unwrap_or(quark::hit::UNCLIPPED)
    }

    /// Clip later hit entries to `rect`, intersected with the current clip,
    /// mirroring `Scene::clip` for hit testing.
    pub fn push_clip(&mut self, rect: Rect) {
        let clip = self
            .current_clip()
            .intersection(rect)
            .unwrap_or(quark::hit::EMPTY_CLIP);
        self.clip_stack.push(clip);
    }

    pub fn pop_clip(&mut self) {
        self.clip_stack.pop();
    }

    /// Register a hit entry at the current z and clip. Bind it to its
    /// semantic node with [`Self::bind_hit`] once that node exists.
    pub fn insert_hit(&mut self, bounds: Bounds, flags: HitFlags, cursor: CursorHint) -> HitId {
        let (z, clip) = (self.current_z_index(), self.current_clip());
        self.hit_table.push(bounds, clip, z, flags, cursor)
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
