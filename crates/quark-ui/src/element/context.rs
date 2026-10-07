use super::*;

// ---------------------------------------------------------------------------
// ElementContext
// ---------------------------------------------------------------------------

pub struct ElementContext<'a> {
    pub theme: &'a Theme,
    pub scale_factor: f32,
    /// Shapes every text element; the renderer must draw with the same one.
    pub text: &'a mut TextSystem,
    /// Layouts shared by measurement, paint, hit-testing, and selection.
    pub layouts: &'a mut LayoutCache,
    pub mouse_position: Option<(f32, f32)>,
    pub hits: Vec<HitRegion>,
    pub scroll_regions: Vec<ScrollRegion>,
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
    hitboxes: Vec<Hitbox>,
    hovered_hitboxes: Vec<HitboxId>,
    next_hitbox_id: usize,
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
            hits: Vec::new(),
            scroll_regions: Vec::new(),
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
            hitboxes: Vec::new(),
            hovered_hitboxes: Vec::new(),
            next_hitbox_id: 0,
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
    pub fn layout_text(&mut self, params: &TextParams) -> Option<Arc<TextLayout>> {
        self.layouts.layout(self.text, params).ok()
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

    pub fn push_click_handler(
        &mut self,
        bounds: Bounds,
        cursor: CursorHint,
        handler: ClickHandler,
    ) {
        self.hits.push(HitRegion::new(bounds, cursor, handler));
    }

    pub fn insert_hitbox(&mut self, bounds: Bounds, behavior: HitboxBehavior) -> HitboxId {
        let id = HitboxId::new(self.next_hitbox_id);
        self.next_hitbox_id += 1;
        self.hitboxes.push(Hitbox {
            id,
            bounds,
            behavior,
            z_index: self.current_z_index(),
        });
        id
    }

    pub fn is_hovered(&self, id: HitboxId) -> bool {
        self.hovered_hitboxes.contains(&id)
    }

    pub fn run_hit_test(&mut self) {
        self.hovered_hitboxes = quark::hit::resolve_hovered(&self.hitboxes, self.mouse_position);
    }
}
