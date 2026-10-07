use std::borrow::Cow;
use std::hash::{Hash, Hasher};

use super::*;

// ---------------------------------------------------------------------------
// Div — the fundamental container element
// ---------------------------------------------------------------------------

/// A flexbox container. The core building block.
pub struct Div {
    base_style: ElementStyle,
    hover_style: Option<StyleOverride>,
    transitions: Transitions,
    /// Paint-time offset of the div and its subtree; layout ignores it.
    translate: (f32, f32),
    bg_effect: Option<BackgroundEffect>,
    blur_radius: Option<f32>,
    children: pool::ChildList,
    on_click: Option<Action>,
    on_click_handler: Option<ClickHandler>,
    on_drag: Option<DragStart>,
    key_bindings: Vec<(String, Action)>,
    on_scroll: Option<ScrollActionBuilder>,
    cursor: CursorHint,
    scroll_y: f32,
    scroll_total_height: f32,
    scroll_x: f32,
    scroll_total_width: f32,
    on_scroll_x: Option<ScrollActionBuilder>,
    scroll_handle: Option<ScrollHandle>,
    scroll_axes: ScrollAxes,
    hide_scrollbar: bool,
    scrollbar_auto_hide: bool,
    clips: bool,
    block_mouse: bool,
    focus_target: Option<FocusId>,
    tooltip: Option<String>,
    hit_identity: Option<HitIdentity>,
    semantic_id: Option<UiNodeId>,
    semantic_key: Option<UiKey>,
    test_id: Option<TestId>,
    semantic_role: Option<SemanticRole>,
    focus_scope: Option<FocusScopeId>,
    trap_focus: bool,
    tab_stop: Option<TabStop>,
    key_context: Option<KeyContext>,
    event_bindings: Vec<UiEventBinding>,
    accessibility_id: Option<String>,
    accessibility_role: Option<AccessibilityRole>,
    accessibility_label: Option<String>,
    accessibility_value: Option<String>,
    accessibility_description: Option<String>,
    accessibility_selected: Option<bool>,
    accessibility_toggled: Option<bool>,
    accessibility_expanded: Option<bool>,
    accessibility_disabled: bool,
    pub(crate) accessibility_extra: AccessibilityExtra,
}

pub fn div() -> Div {
    Div {
        base_style: ElementStyle::default(),
        hover_style: None,
        transitions: Transitions::default(),
        translate: (0.0, 0.0),
        bg_effect: None,
        blur_radius: None,
        children: pool::ChildList::new(),
        on_click: None,
        on_click_handler: None,
        on_drag: None,
        key_bindings: Vec::new(),
        on_scroll: None,
        cursor: CursorHint::Default,
        scroll_y: 0.0,
        scroll_total_height: 0.0,
        scroll_x: 0.0,
        scroll_total_width: 0.0,
        on_scroll_x: None,
        scroll_handle: None,
        scroll_axes: ScrollAxes::default(),
        hide_scrollbar: false,
        scrollbar_auto_hide: false,
        clips: false,
        block_mouse: false,
        focus_target: None,
        tooltip: None,
        hit_identity: None,
        semantic_id: None,
        semantic_key: None,
        test_id: None,
        semantic_role: None,
        focus_scope: None,
        trap_focus: false,
        tab_stop: None,
        key_context: None,
        event_bindings: Vec::new(),
        accessibility_id: None,
        accessibility_role: None,
        accessibility_label: None,
        accessibility_value: None,
        accessibility_description: None,
        accessibility_selected: None,
        accessibility_toggled: None,
        accessibility_expanded: None,
        accessibility_disabled: false,
        accessibility_extra: AccessibilityExtra::default(),
    }
}

fn role_labels_descendant_text(role: AccessibilityRole) -> bool {
    matches!(
        role,
        AccessibilityRole::Button
            | AccessibilityRole::DefaultButton
            | AccessibilityRole::CheckBox
            | AccessibilityRole::Switch
            | AccessibilityRole::RadioButton
            | AccessibilityRole::Tab
            | AccessibilityRole::TreeItem
            | AccessibilityRole::ListItem
            | AccessibilityRole::ListBoxOption
            | AccessibilityRole::MenuItem
            | AccessibilityRole::MenuItemCheckBox
            | AccessibilityRole::MenuItemRadio
            | AccessibilityRole::MenuListOption
            | AccessibilityRole::ComboBox
            | AccessibilityRole::EditableComboBox
    )
}

impl Styled for Div {
    fn element_style_mut(&mut self) -> &mut ElementStyle {
        &mut self.base_style
    }
}

impl Div {
    // -- Children --

    pub fn child(mut self, child: impl IntoAnyElement) -> Self {
        self.children.push(child.into_any());
        self
    }

    pub fn children(mut self, children: impl IntoIterator<Item = AnyElement>) -> Self {
        self.children.extend(children);
        self
    }

    pub fn optional_child(mut self, child: Option<impl IntoAnyElement>) -> Self {
        if let Some(c) = child {
            self.children.push(c.into_any());
        }
        self
    }

    pub fn children_from<I, E>(mut self, iter: I) -> Self
    where
        I: IntoIterator<Item = E>,
        E: IntoAnyElement,
    {
        for item in iter {
            self.children.push(item.into_any());
        }
        self
    }

    // -- Interaction --

    pub fn on_click(mut self, action: impl Into<Action>) -> Self {
        self.on_click = Some(action.into());
        self.cursor = CursorHint::Pointer;
        self
    }

    pub fn on_click_handler(mut self, handler: ClickHandler) -> Self {
        self.on_click_handler = Some(handler);
        self.cursor = CursorHint::Pointer;
        self
    }

    /// Start a drag on pointer down. The handler `start` returns captures the
    /// pointer and receives every move and the release until the button goes up.
    pub fn on_drag(mut self, start: impl Fn(ClickEvent) -> Box<dyn DragHandler> + 'static) -> Self {
        self.on_drag = Some(DragStart::new(start));
        self
    }

    /// Emit `action` when `binding` (keymap format, e.g. `"enter"`) is pressed
    /// while this element or a descendant has focus.
    pub fn on_key(mut self, binding: impl Into<String>, action: impl Into<Action>) -> Self {
        self.key_bindings.push((binding.into(), action.into()));
        self
    }

    /// Make this element capture the mouse: hover and clicks stop at it, so
    /// nothing beneath it (lower z, or earlier in paint order) is hovered or
    /// clicked where it covers. Use on elevated surfaces and scrims.
    pub fn block_mouse(mut self) -> Self {
        self.block_mouse = true;
        self
    }

    /// Attach an opaque identity payload used by hover/click dispatch to
    /// route behavior without pattern-matching the app's Action enum.
    pub fn hit_identity(mut self, identity: HitIdentity) -> Self {
        self.hit_identity = Some(identity);
        self
    }

    pub fn id(mut self, id: impl Into<UiNodeId>) -> Self {
        self.semantic_id = Some(id.into());
        self
    }

    pub fn key(mut self, key: impl Into<UiKey>) -> Self {
        self.semantic_key = Some(key.into());
        self
    }

    pub fn test_id(mut self, id: impl Into<TestId>) -> Self {
        self.test_id = Some(id.into());
        self
    }

    pub fn semantic_role(mut self, role: SemanticRole) -> Self {
        self.accessibility_role = self.accessibility_role.or(accessibility_role_for(role));
        self.semantic_role = Some(role);
        self
    }

    pub fn cursor(mut self, cursor: CursorHint) -> Self {
        self.cursor = cursor;
        self
    }

    /// Register a scroll action for this div. Scroll wheel events inside
    /// this div's bounds will dispatch through the action builder.
    pub fn on_scroll(mut self, builder: ScrollActionBuilder) -> Self {
        self.on_scroll = Some(builder);
        self
    }

    /// Full style override on hover.
    pub fn hover(mut self, f: impl FnOnce(StyleOverride) -> StyleOverride) -> Self {
        self.hover_style = Some(f(StyleOverride::default()));
        self
    }

    /// Convenience: set only the hover background.
    pub fn hover_bg(self, color: Color) -> Self {
        self.hover(|s| s.bg(color))
    }

    /// Convenience: set only the hover text color (propagates to child text elements).
    pub fn hover_text_color(self, color: Color) -> Self {
        self.hover(|s| s.text_color(color))
    }

    /// Convenience: set only the hover icon color (propagates to child svg icons).
    pub fn hover_icon_color(self, color: Color) -> Self {
        self.hover(|s| s.icon_color(color))
    }

    /// Animate changes to `props` of the resolved style (hover included)
    /// with `motion`, starting from the value on screen. Needs a stable
    /// [`key`](Self::key) and a context with an animation table; without
    /// either the div paints its target values directly.
    pub fn transition(mut self, props: impl Into<PropSet>, motion: Motion) -> Self {
        self.transitions.push(props.into(), motion);
        self
    }

    /// Offset the div and everything in it at paint time, for slides and
    /// nudges that should not reflow layout. Hit testing follows the offset.
    pub fn translate(mut self, x: f32, y: f32) -> Self {
        self.translate = (x, y);
        self
    }

    /// Conditionally apply style/config changes.
    pub fn when(self, condition: bool, f: impl FnOnce(Self) -> Self) -> Self {
        if condition { f(self) } else { self }
    }

    // -- Scroll / clip --

    pub fn scroll_y(mut self, offset: f32) -> Self {
        self.scroll_y = offset;
        self.clips = true;
        // Tell taffy the element is a scroll container so it constrains to
        // the available space instead of expanding to fit all children.
        self.base_style.layout.overflow.y = taffy::Overflow::Hidden;
        self
    }

    pub fn scroll_total(mut self, total_height: f32) -> Self {
        self.scroll_total_height = total_height;
        self
    }

    /// Horizontal [`Self::scroll_y`]: paint children `offset` points to the
    /// left, clipped, with the width constrained like a scroll container.
    pub fn scroll_x(mut self, offset: f32) -> Self {
        self.scroll_x = offset;
        self.clips = true;
        self.base_style.layout.overflow.x = taffy::Overflow::Hidden;
        self
    }

    /// Content width for the horizontal scrollbar and wheel limit, like
    /// [`Self::scroll_total`].
    pub fn scroll_total_x(mut self, total_width: f32) -> Self {
        self.scroll_total_width = total_width;
        self
    }

    /// Horizontal [`Self::on_scroll`]: horizontal wheel motion (and
    /// Shift+wheel) becomes lines for `builder`; its `to_px` gets
    /// horizontal thumb drags.
    pub fn on_scroll_x(mut self, builder: ScrollActionBuilder) -> Self {
        self.on_scroll_x = Some(builder);
        self
    }

    /// Keep this container's scroll offset in `handle`, which wheel,
    /// scrollbar, and key input move without going through the app. Pair
    /// with [`Self::overflow_x_scroll`], [`Self::overflow_y_scroll`], or
    /// [`Self::overflow_scroll`] to pick the axes. The content size is
    /// measured from the children unless `scroll_total`/`scroll_total_x`
    /// give it.
    pub fn track_scroll(mut self, handle: &ScrollHandle) -> Self {
        self.scroll_handle = Some(handle.clone());
        self.clips = true;
        self
    }

    /// Scroll horizontally with the [`Self::track_scroll`] handle.
    pub fn overflow_x_scroll(mut self) -> Self {
        self.scroll_axes.x = true;
        self.clips = true;
        self.base_style.layout.overflow.x = taffy::Overflow::Hidden;
        self
    }

    /// Scroll vertically with the [`Self::track_scroll`] handle.
    pub fn overflow_y_scroll(mut self) -> Self {
        self.scroll_axes.y = true;
        self.clips = true;
        self.base_style.layout.overflow.y = taffy::Overflow::Hidden;
        self
    }

    /// Scroll on both axes with the [`Self::track_scroll`] handle.
    pub fn overflow_scroll(self) -> Self {
        self.overflow_x_scroll().overflow_y_scroll()
    }

    pub fn hide_scrollbar(mut self) -> Self {
        self.hide_scrollbar = true;
        self
    }

    /// Show scrollbars only while the pointer is over the container, a
    /// thumb is held, or for [`SCROLLBAR_LINGER_MS`] after a tracked
    /// container scrolls.
    pub fn scrollbar_auto_hide(mut self) -> Self {
        self.scrollbar_auto_hide = true;
        self
    }

    pub fn focus_ring(mut self, target: FocusId) -> Self {
        self.focus_target = Some(target);
        self
    }

    pub fn focus_scope(mut self, scope: impl Into<FocusScopeId>) -> Self {
        self.focus_scope = Some(scope.into());
        self
    }

    /// Mark this element's focus scope modal: while it is painted, Tab
    /// cycles only within it and clicks outside it leave focus alone.
    pub fn trap_focus(mut self, trap: bool) -> Self {
        self.trap_focus = trap;
        self
    }

    pub fn tab_stop(mut self, tab_stop: impl Into<TabStop>) -> Self {
        self.tab_stop = Some(tab_stop.into());
        self
    }

    pub fn track_focus(self, target: FocusId) -> Self {
        self.focus_ring(target)
    }

    pub fn key_context(mut self, context: impl Into<KeyContext>) -> Self {
        self.key_context = Some(context.into());
        self
    }

    pub fn on_event_capture(mut self, kind: UiEventKind, result: UiEventResult) -> Self {
        self.event_bindings
            .push(UiEventBinding::new(kind, UiEventPhase::Capture).with_result(result));
        self
    }

    pub fn on_event(mut self, kind: UiEventKind, result: UiEventResult) -> Self {
        self.event_bindings
            .push(UiEventBinding::new(kind, UiEventPhase::Target).with_result(result));
        self
    }

    pub fn tooltip(mut self, text: impl Into<String>) -> Self {
        self.tooltip = Some(text.into());
        self
    }

    pub fn accessibility_id(mut self, id: impl Into<String>) -> Self {
        self.accessibility_id = Some(id.into());
        self
    }

    pub fn accessibility_role(mut self, role: AccessibilityRole) -> Self {
        self.accessibility_role = Some(role);
        self
    }

    pub fn accessibility_label(mut self, label: impl Into<String>) -> Self {
        self.accessibility_label = Some(label.into());
        self
    }

    pub fn accessibility_value(mut self, value: impl Into<String>) -> Self {
        self.accessibility_value = Some(value.into());
        self
    }

    pub fn accessibility_description(mut self, description: impl Into<String>) -> Self {
        self.accessibility_description = Some(description.into());
        self
    }

    pub fn accessibility_selected(mut self, selected: bool) -> Self {
        self.accessibility_selected = Some(selected);
        self
    }

    pub fn accessibility_toggled(mut self, toggled: bool) -> Self {
        self.accessibility_toggled = Some(toggled);
        self
    }

    pub fn accessibility_expanded(mut self, expanded: bool) -> Self {
        self.accessibility_expanded = Some(expanded);
        self
    }

    pub fn accessibility_disabled(mut self, disabled: bool) -> Self {
        self.accessibility_disabled = disabled;
        self
    }

    pub fn clip(mut self) -> Self {
        self.clips = true;
        self
    }

    /// Set a procedural GPU background effect (noise gradient, linear gradient).
    /// This replaces the solid `bg()` color for the background pass.
    pub fn bg_effect(mut self, effect: BackgroundEffect) -> Self {
        self.bg_effect = Some(effect);
        self
    }

    /// Apply a frosted-glass Gaussian blur backdrop to this div.
    /// Everything rendered behind this div will be blurred within its bounds.
    /// Typical radius: 8–20 pixels.
    pub fn blur(mut self, radius: f32) -> Self {
        self.blur_radius = Some(radius);
        self
    }

    // -- Internal: input registration --

    /// Bind the hit entry to semantic node `node` and register its handlers.
    /// A hit entry always comes with a semantic node (`hit_test` action).
    fn register_input(
        &mut self,
        node: usize,
        hit: Option<HitId>,
        bounds: Bounds,
        cx: &mut ElementContext,
    ) {
        if let Some(hit) = hit {
            cx.bind_hit(hit, node);
            cx.hit_table.set_identity(hit, self.hit_identity.take());
        }
        let click = self
            .on_click_handler
            .take()
            .or_else(|| self.on_click.take().map(ClickHandler::from_action));
        if let Some(click) = click {
            cx.handlers.on_click(node, click);
        }
        if let Some(start) = self.on_drag.take() {
            cx.handlers.on_drag(node, start);
        }
        if let Some(builder) = self.on_scroll.clone() {
            let max = (self.scroll_total_height > 0.0)
                .then(|| (self.scroll_total_height - bounds.height).max(0.0));
            cx.handlers.on_scroll(
                node,
                ScrollTarget {
                    builder,
                    offset: self.scroll_y,
                    max,
                },
            );
        }
        if let Some(builder) = self.on_scroll_x.clone() {
            let max = (self.scroll_total_width > 0.0)
                .then(|| (self.scroll_total_width - bounds.width).max(0.0));
            cx.handlers.on_scroll_x(
                node,
                ScrollTarget {
                    builder,
                    offset: self.scroll_x,
                    max,
                },
            );
        }
        if let Some(handle) = &self.scroll_handle {
            cx.handlers
                .on_scroll_handle(node, handle.clone(), self.scroll_axes);
        }
        for (binding, action) in self.key_bindings.drain(..) {
            cx.handlers.on_key(node, binding, action);
        }
    }

    // -- Internal: resolve style with overrides --

    /// The base style, copied only when a hover override applies.
    fn resolve_style(&self, hovered: bool) -> Cow<'_, ElementStyle> {
        match &self.hover_style {
            Some(ov) if hovered => {
                let mut resolved = self.base_style.clone();
                apply_override(&mut resolved, ov);
                Cow::Owned(resolved)
            }
            _ => Cow::Borrowed(&self.base_style),
        }
    }

    // -- Internal: scrolling --

    fn scrolls(&self) -> bool {
        self.on_scroll.is_some() || self.on_scroll_x.is_some() || self.scroll_handle.is_some()
    }

    /// Size of what the div scrolls over: `scroll_total`/`scroll_total_x`
    /// when given, else measured for a tracked container.
    fn scroll_content(&self, engine: &LayoutEngine, id: LayoutId) -> (f32, f32) {
        let measured = match self.scroll_handle {
            Some(_) => engine.scroll_content_size(id),
            None => (0.0, 0.0),
        };
        let given = |total: f32, measured: f32| if total > 0.0 { total } else { measured };
        (
            given(self.scroll_total_width, measured.0),
            given(self.scroll_total_height, measured.1),
        )
    }

    /// Lay out this frame's scrollbars and give the visible, interactive
    /// ones hit entries (under the current clip, above the children).
    fn prepaint_scrollbars(
        &self,
        bounds: Bounds,
        content: (f32, f32),
        scroll: (f32, f32),
        cx: &mut ElementContext,
    ) -> [Option<(Scrollbar, Option<HitId>)>; 2] {
        let axes = match self.scroll_handle {
            _ if self.hide_scrollbar => return [None, None],
            Some(_) => self.scroll_axes,
            None => ScrollAxes {
                x: self.scroll_total_width > 0.0,
                y: self.scroll_total_height > 0.0,
            },
        };
        let bars = scrollbars(bounds, content, scroll, axes);
        if bars.iter().all(Option::is_none) {
            return [None, None];
        }
        if self.scrollbar_auto_hide {
            let pointer_inside = cx
                .mouse_position
                .is_some_and(|(x, y)| bounds.contains(x, y) && cx.current_clip().contains(x, y));
            let visible = pointer_inside
                || self.scroll_handle.as_ref().is_some_and(|handle| {
                    handle.dragging().is_some() || handle.recently_scrolled(cx)
                });
            if !visible {
                return [None, None];
            }
        }
        bars.map(|bar| {
            bar.map(|bar| {
                let hit = self.scrollbar_sink(bar.axis).is_some().then(|| {
                    cx.insert_hit(
                        bar.hit,
                        HitFlags::DRAG | HitFlags::HOVER,
                        CursorHint::Default,
                    )
                });
                (bar, hit)
            })
        })
    }

    /// Where input on the scrollbar of `axis` goes, if anywhere.
    fn scrollbar_sink(&self, axis: Axis) -> Option<ScrollSink> {
        if let Some(handle) = &self.scroll_handle {
            return self
                .scroll_axes
                .has(axis)
                .then(|| ScrollSink::Handle(handle.clone()));
        }
        let builder = match axis {
            Axis::X => &self.on_scroll_x,
            Axis::Y => &self.on_scroll,
        };
        builder.clone().map(ScrollSink::Builder)
    }

    /// Author id of the div's accessibility node: its stable id or key
    /// when it has one, else a hash of its role and label. Equal fallbacks
    /// get `#2`, `#3`, ... in paint order, so ids do not move with layout.
    fn accessibility_key(&self, role: AccessibilityRole, label: Option<&str>) -> Cow<'_, str> {
        let stable = self
            .accessibility_id
            .as_deref()
            .or(self.semantic_id.as_ref().map(UiNodeId::as_str))
            .or(self.semantic_key.as_ref().map(UiKey::as_str))
            .or(self.test_id.as_ref().map(TestId::as_str));
        if let Some(key) = stable {
            return Cow::Borrowed(key);
        }
        let mut hasher = std::hash::DefaultHasher::new();
        (role as u8, label).hash(&mut hasher);
        Cow::Owned(format!("div:{:016x}", hasher.finish()))
    }
}

/// Div's prepaint state: its hit entry, when it responds to the pointer,
/// and its scroll offset and scrollbars.
pub struct DivPrepaintState {
    hit: Option<HitId>,
    translate: (f32, f32),
    scroll: (f32, f32),
    scrollbars: [Option<(Scrollbar, Option<HitId>)>; 2],
}

impl Element for Div {
    /// The div's layout node, for measuring scroll content.
    type LayoutState = LayoutId;
    type PrepaintState = DivPrepaintState;

    fn request_layout(
        &mut self,
        engine: &mut LayoutEngine,
        cx: &mut ElementContext,
    ) -> (LayoutId, Self::LayoutState) {
        // Layout children first, collecting their IDs on the engine's stack.
        let mark = engine.begin_children();
        for child in self.children.iter_mut() {
            let id = child.request_layout(engine, cx);
            engine.push_child(id);
        }
        let id = engine.finish_children(&self.base_style.layout, mark);
        (id, id)
    }

    fn prepaint(
        &mut self,
        bounds: Bounds,
        layout_id: &mut Self::LayoutState,
        engine: &LayoutEngine,
        cx: &mut ElementContext,
    ) -> DivPrepaintState {
        let translate = self
            .transitions
            .translate(self.semantic_key.as_ref(), self.translate, cx);
        let bounds = offset_bounds(bounds, translate);
        if let Some(key) = &self.semantic_key {
            cx.record_scroll_item(key, bounds);
        }
        let content = self.scroll_content(engine, *layout_id);
        let scroll = match &self.scroll_handle {
            Some(handle) => handle.begin_frame(bounds, content, self.scroll_axes, cx),
            None => (self.scroll_x, self.scroll_y),
        };
        let (child_dx, child_dy) = (translate.0 - scroll.0, translate.1 - scroll.1);
        let z = self.base_style.z_index;
        if z != 0 {
            cx.push_z_index(z);
        }

        let mut flags = HitFlags::NONE;
        if self.block_mouse {
            flags |= HitFlags::BLOCKS_MOUSE;
        }
        if self.hover_style.is_some() {
            flags |= HitFlags::HOVER;
        }
        if self.on_click.is_some() || self.on_click_handler.is_some() {
            flags |= HitFlags::CLICK;
        }
        if self.on_drag.is_some() {
            flags |= HitFlags::DRAG;
        }
        if self.scrolls() {
            flags |= HitFlags::SCROLL;
        }
        let hit = (!flags.is_empty() || self.hit_identity.is_some())
            .then(|| cx.insert_hit(bounds, flags, self.cursor));

        let clips = self.clips
            || self.base_style.layout.overflow.x != taffy::Overflow::Visible
            || self.base_style.layout.overflow.y != taffy::Overflow::Visible;
        if clips {
            cx.push_clip(bounds);
        }
        if let Some(handle) = &self.scroll_handle {
            cx.push_scroll_handle(handle);
        }

        if (child_dx, child_dy) != (0.0, 0.0) {
            for child in self.children.iter_mut() {
                child.prepaint_with_offset(engine, cx, child_dx, child_dy);
            }
        } else {
            for child in self.children.iter_mut() {
                child.prepaint(engine, cx);
            }
        }

        if let Some(handle) = &self.scroll_handle {
            cx.pop_scroll_handle();
            handle.end_frame(cx);
        }
        let scrollbars = self.prepaint_scrollbars(bounds, content, scroll, cx);
        if clips {
            cx.pop_clip();
        }
        if z != 0 {
            cx.pop_z_index();
        }

        DivPrepaintState {
            hit,
            translate,
            scroll,
            scrollbars,
        }
    }

    fn paint(
        &mut self,
        bounds: Bounds,
        _layout_state: &mut Self::LayoutState,
        prepaint_state: &mut DivPrepaintState,
        engine: &LayoutEngine,
        scene: &mut Scene,
        cx: &mut ElementContext,
    ) {
        let translate = prepaint_state.translate;
        let bounds = offset_bounds(bounds, translate);
        let scroll = prepaint_state.scroll;
        let (child_dx, child_dy) = (translate.0 - scroll.0, translate.1 - scroll.1);
        let hovered = prepaint_state.hit.is_some_and(|id| cx.is_hovered(id));
        let mut style = self.resolve_style(hovered);
        if !self.transitions.is_empty() {
            self.transitions
                .apply(self.semantic_key.as_ref(), style.to_mut(), cx);
        }
        let radii = style.corner_radii;
        let r = style.max_corner_radius();
        let z = style.z_index;
        let opacity = style.opacity;
        let background = style.background.map(|mut bg| {
            if opacity < 1.0 {
                bg.a = (bg.a as f32 * opacity) as u8;
            }
            bg
        });

        if z != 0 {
            scene.push_z_index(z);
        }

        if let Some(radius) = self.blur_radius {
            scene.blur_region(BlurRegionPrimitive {
                rect: bounds,
                blur_radius: radius,
                corner_radius: r,
            });
        }

        // Shadows
        for s in &style.shadows {
            scene.shadow(ShadowPrimitive {
                rect: bounds,
                blur_radius: s.blur_radius,
                corner_radius: s.corner_radius.max(r),
                offset: s.offset,
                color: s.color,
            });
        }

        // Background — effect quad takes priority over solid color.
        if let Some(effect) = self.bg_effect {
            let (effect_type, params, color_a, color_b) = match effect {
                BackgroundEffect::NoiseGradient {
                    scale,
                    color_a,
                    color_b,
                } => (EffectType::NoiseGradient, [scale, 0.0], color_a, color_b),
                BackgroundEffect::LinearGradient {
                    angle,
                    color_a,
                    color_b,
                } => (EffectType::LinearGradient, [angle, 0.0], color_a, color_b),
                BackgroundEffect::RadialGradient { color_a, color_b } => {
                    (EffectType::RadialGradient, [0.0, 0.0], color_a, color_b)
                }
                BackgroundEffect::Shimmer {
                    base,
                    highlight,
                    speed,
                } => (EffectType::Shimmer, [speed, 0.0], base, highlight),
                BackgroundEffect::Vignette { color, intensity } => (
                    EffectType::Vignette,
                    [intensity, 0.0],
                    color,
                    Color::TRANSPARENT,
                ),
                BackgroundEffect::ColorTint { color } => {
                    (EffectType::ColorTint, [0.0, 0.0], color, Color::TRANSPARENT)
                }
            };
            scene.effect_quad(EffectQuadPrimitive {
                rect: bounds,
                effect_type,
                color_a,
                color_b,
                params,
                corner_radius: r,
            });
        } else if let Some(bg) = background {
            scene.rounded_rect(RoundedRectPrimitive {
                rect: bounds,
                corner_radii: radii,
                color: bg,
            });
        }

        // Border
        if let Some(border) = style.border_color
            && style.border_widths != [0.0; 4]
        {
            scene.border(BorderPrimitive {
                rect: bounds,
                widths: style.border_widths,
                corner_radii: radii,
                color: border,
            });
        }

        let should_clip = self.clips
            || style.layout.overflow.x != taffy::Overflow::Visible
            || style.layout.overflow.y != taffy::Overflow::Visible;
        drop(style);

        // A clickable div with a stable id takes Tab focus without a
        // `focus_ring` (`SemanticFrame::focus_id`), so it gets the ring too.
        let clickable = self.on_click_handler.is_some()
            || self
                .on_click
                .as_ref()
                .is_some_and(|a| !a.is::<NoopAction>());
        let ring_target = self.focus_target.or_else(|| {
            let id = self
                .semantic_id
                .as_ref()
                .map(UiNodeId::as_str)
                .or(self.accessibility_id.as_deref())
                .or(self.test_id.as_ref().map(TestId::as_str));
            id.filter(|_| clickable || self.tab_stop.is_some())
                .map(FocusId::from_key)
        });
        if let Some(target) = ring_target
            && cx.is_focused(target)
        {
            let ring_inset = -2.0;
            let ring_bounds = Rect {
                x: bounds.x + ring_inset,
                y: bounds.y + ring_inset,
                width: bounds.width - ring_inset * 2.0,
                height: bounds.height - ring_inset * 2.0,
            };
            scene.border(BorderPrimitive {
                rect: ring_bounds,
                widths: [2.0; 4],
                corner_radii: radii.map(|c| c + 2.0),
                color: cx.theme.colors.focus_border,
            });
        }

        let click_action = self.on_click.clone();
        let has_click_action = click_action
            .as_ref()
            .is_some_and(|action| !action.is::<NoopAction>());
        let accessibility_label = self
            .accessibility_label
            .take()
            .or_else(|| self.tooltip.clone());
        let accessibility_role = self.accessibility_role.or_else(|| {
            (click_action.is_some() && accessibility_label.is_some())
                .then_some(AccessibilityRole::Button)
        });
        let semantic_role = self
            .semantic_role
            .or_else(|| accessibility_role.and_then(semantic_role_for))
            .or_else(|| self.scrolls().then_some(SemanticRole::ScrollArea));
        let suppress_descendant_accessibility_text =
            accessibility_role.is_some_and(role_labels_descendant_text);

        let mut semantic_actions = SemanticActions::default();
        if has_click_action || self.on_click_handler.is_some() {
            semantic_actions = semantic_actions.clickable().focusable();
        }
        if self.focus_target.is_some() {
            semantic_actions = semantic_actions.focusable();
        }
        if self.scrolls() {
            semantic_actions = semantic_actions.scrollable();
        }
        if self.tooltip.is_some() {
            semantic_actions = semantic_actions.tooltip();
        }
        if self.on_drag.is_some() {
            semantic_actions = semantic_actions.draggable();
        }
        if prepaint_state.hit.is_some() {
            semantic_actions = semantic_actions.hit_test();
        }

        let mut style_state = StyleState::empty();
        if hovered {
            style_state.insert(StyleState::HOVER);
        }
        if let Some(target) = ring_target
            && cx.is_focused(target)
        {
            style_state.insert(StyleState::FOCUS_VISIBLE);
        }
        if self.accessibility_disabled {
            style_state.insert(StyleState::DISABLED);
        }
        if self.accessibility_selected.unwrap_or(false) {
            style_state.insert(StyleState::SELECTED);
        }
        if self.accessibility_toggled.unwrap_or(false) {
            style_state.insert(StyleState::CHECKED);
        }
        if self.accessibility_expanded.unwrap_or(false) {
            style_state.insert(StyleState::EXPANDED);
        }

        // The accessibility node copies what it shares with the semantic
        // node; the semantic node then takes the div's strings, since paint
        // runs once per frame.
        let accessibility = accessibility_role
            .filter(|_| cx.accessibility_enabled())
            .map(|role| {
                let key = self.accessibility_key(role, accessibility_label.as_deref());
                let mut node = AccessibilityNode::new(key, role, bounds)
                    .disabled(self.accessibility_disabled)
                    .modal(self.trap_focus && self.focus_scope.is_some());
                if let Some(label) = &accessibility_label {
                    node = node.label(label.clone());
                }
                if let Some(value) = &self.accessibility_value {
                    node = node.value(value.clone());
                }
                if let Some(description) = &self.accessibility_description {
                    node = node.description(description.clone());
                }
                if let Some(selected) = self.accessibility_selected {
                    node = node.selected(selected);
                }
                if let Some(toggled) = self.accessibility_toggled {
                    node = node.toggled(toggled);
                }
                if let Some(expanded) = self.accessibility_expanded {
                    node = node.expanded(expanded);
                }
                node = self.accessibility_extra.apply(node);
                if !self.accessibility_disabled
                    && let Some(action) = click_action
                    && !action.is::<NoopAction>()
                {
                    node = node.action(AccessibilityAction::Click(action));
                } else if let Some(builder) = self.on_scroll.clone() {
                    node = node.action(AccessibilityAction::Scroll(builder));
                }
                node
            });

        let semantic_id = self
            .semantic_id
            .take()
            .or_else(|| self.accessibility_id.take().map(UiNodeId::from));
        let should_emit_semantic = semantic_id.is_some()
            || self.semantic_key.is_some()
            || self.test_id.is_some()
            || semantic_role.is_some()
            || !semantic_actions.is_empty()
            || self.focus_scope.is_some()
            || !self.key_bindings.is_empty()
            || self.tab_stop.is_some()
            || self.key_context.is_some()
            || !self.event_bindings.is_empty()
            || !style_state.is_empty()
            // Accessible divs need a semantic node so descendants nest under them.
            || accessibility_role.is_some();
        let semantic_parent = if should_emit_semantic {
            let mut node = SemanticNode::new(bounds);
            node.id = semantic_id;
            node.key = self.semantic_key.take();
            node.test_id = self.test_id.take();
            node.parent = cx.current_semantic_parent();
            node.role = semantic_role;
            node.label = accessibility_label;
            node.value = self.accessibility_value.take();
            node.description = self.accessibility_description.take();
            node.tooltip = self.tooltip.clone();
            node.actions = semantic_actions;
            node.state = SemanticNodeState {
                disabled: self.accessibility_disabled,
                selected: self.accessibility_selected,
                toggled: self.accessibility_toggled,
                expanded: self.accessibility_expanded,
                invalid: self.accessibility_extra.invalid(),
                required: self.accessibility_extra.required(),
                style_state,
            };
            node.focus = self.focus_target;
            node.focus_scope = self.focus_scope.take();
            node.modal = self.trap_focus;
            node.tab_stop = self.tab_stop;
            node.key_context = self.key_context.take();
            node.event_bindings = std::mem::take(&mut self.event_bindings);
            Some(cx.semantic.push(node))
        } else {
            None
        };

        if let Some(mut node) = accessibility {
            if let Some(focus) = semantic_parent.and_then(|index| cx.semantic.focus_id(index)) {
                node = node.focus(focus);
            }
            match semantic_parent {
                Some(index) => cx.push_accessibility_for_semantic(node, index),
                None => cx.push_accessibility(node),
            };
        }

        if let Some(node) = semantic_parent {
            self.register_input(node, prepaint_state.hit, bounds, cx);
        }

        if let Some(tip) = self.tooltip.take() {
            cx.tooltip_regions.push(TooltipRegion { bounds, text: tip });
        }

        if should_clip {
            if r > 0.0 {
                scene.clip_rounded(bounds, radii);
            } else {
                scene.clip(bounds);
            }
        }

        let pushed_text_color = if hovered {
            self.hover_style.as_ref().and_then(|ov| ov.text_color)
        } else {
            None
        };
        let pushed_icon_color = if hovered {
            self.hover_style.as_ref().and_then(|ov| ov.icon_color)
        } else {
            None
        };
        if let Some(tc) = pushed_text_color {
            cx.push_text_color(tc);
        }
        if let Some(ic) = pushed_icon_color {
            cx.push_icon_color(ic);
        }
        cx.push_accessibility_text_hidden(suppress_descendant_accessibility_text);
        if let Some(index) = semantic_parent {
            cx.push_semantic_parent(index);
            // Before the children, so each bar's node keeps its position
            // (and so its identity for drag capture) as children change.
            for (bar, hit) in prepaint_state.scrollbars.iter().flatten() {
                if let (Some(hit), Some(sink)) = (hit, self.scrollbar_sink(bar.axis)) {
                    let mut node = SemanticNode::new(bar.hit);
                    node.parent = Some(index);
                    node.actions = SemanticActions::default().draggable();
                    let node = cx.semantic.push(node);
                    cx.bind_hit(*hit, node);
                    cx.handlers.on_scrollbar(node, *bar, sink);
                }
            }
        }

        if (child_dx, child_dy) != (0.0, 0.0) {
            for child in self.children.iter_mut() {
                child.paint_with_offset(engine, scene, cx, child_dx, child_dy);
            }
        } else {
            for child in self.children.iter_mut() {
                child.paint(engine, scene, cx);
            }
        }

        if semantic_parent.is_some() {
            cx.pop_semantic_parent();
        }
        cx.pop_accessibility_text_hidden();
        if pushed_icon_color.is_some() {
            cx.pop_icon_color();
        }
        if pushed_text_color.is_some() {
            cx.pop_text_color();
        }

        let dragging = self.scroll_handle.as_ref().and_then(ScrollHandle::dragging);
        for (bar, hit) in prepaint_state.scrollbars.iter().flatten() {
            let hovered = hit.is_some_and(|hit| cx.is_hovered(hit));
            bar.paint(scene, cx.theme, hovered, dragging == Some(bar.axis));
        }

        if should_clip {
            scene.pop_clip();
        }

        // Debug wireframe: 1px outline around every div
        if cx.debug_wireframe {
            // Cycle colors by depth using bounds position as a hash
            let hash = ((bounds.x as u32).wrapping_mul(7) ^ (bounds.y as u32).wrapping_mul(13)) % 6;
            let wire_color = match hash {
                0 => Color::rgba(255, 80, 80, 120),  // red
                1 => Color::rgba(80, 255, 80, 120),  // green
                2 => Color::rgba(80, 80, 255, 120),  // blue
                3 => Color::rgba(255, 255, 80, 120), // yellow
                4 => Color::rgba(255, 80, 255, 120), // magenta
                _ => Color::rgba(80, 255, 255, 120), // cyan
            };
            scene.border(BorderPrimitive {
                rect: bounds,
                widths: [1.0; 4],
                corner_radii: radii,
                color: wire_color,
            });
        }

        if z != 0 {
            scene.pop_z_index();
        }
    }

    #[cfg(feature = "devtools")]
    fn inspect(&self) -> crate::inspector::InspectInfo {
        crate::inspector::InspectInfo {
            key: self.inspect_key(),
            z_index: self.base_style.z_index,
            blocks_mouse: self.block_mouse,
            style: Some(crate::inspector::StyleSummary::of(&self.base_style)),
        }
    }

    #[cfg(feature = "devtools")]
    fn inspect_style_mut(&mut self) -> Option<(UiKey, &mut ElementStyle)> {
        Some((self.inspect_key()?, &mut self.base_style))
    }
}

#[cfg(feature = "devtools")]
impl Div {
    /// The key devtools overrides are stored under: the sibling key, or the
    /// stable id when the div has no key.
    fn inspect_key(&self) -> Option<UiKey> {
        self.semantic_key.clone().or_else(|| {
            self.semantic_id
                .as_ref()
                .map(|id| UiKey::new(id.as_str()))
                .or_else(|| self.accessibility_id.as_deref().map(UiKey::from))
        })
    }
}

fn offset_bounds(bounds: Bounds, (dx, dy): (f32, f32)) -> Bounds {
    Bounds {
        x: bounds.x + dx,
        y: bounds.y + dy,
        ..bounds
    }
}

impl IntoAnyElement for Div {
    fn into_any(self) -> AnyElement {
        element_into_any(self)
    }
}
