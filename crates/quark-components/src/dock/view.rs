//! The element a [`Dock`] builds: its regions and splits, the dividers
//! between groups, and each group's tabs, content, drop targets, and drop
//! preview.

use std::borrow::Cow;
use std::rc::Rc;

use accesskit::Role;
use quark::view;
use quark_ui::Action;
use quark_ui::FocusId;
use quark_ui::accessibility::{NumericActions, NumericValue, Orientation};
use quark_ui::element::{
    AnyElement, ClickEvent, DragHandler, DropTarget, DropTargetHit, DropTargetId, ElementHandle,
    IntoAnyElement, canvas, div,
};
use quark_ui::style::Styled;
use quark_ui::theme::Theme;

use super::drag::{DockRoot, DragContext, GroupHit, PaneDividerDrag, resize_cursor};
use super::tabs::{TabWidths, Title};
use super::{
    DOCK_ID, Dock, DockDestination, DockEvent, DockRegion, DockSplit, DockState, HostId, MIN_GROUP,
    MOVE_TAB_KEYS, PaneDividerEvent, PanelId,
};
use crate::pane_tree::{
    DropZone, PaneId, PaneNode, PaneSplit, Rect, TabGroup, child_sizes, divider_min, divider_span,
};
use crate::split::{Axis, DIVIDER_THICKNESS, NUDGE_STEP, NUDGE_STEP_LARGE, Split, SplitState};

/// What every group of one build shares: the theme, the frame's drop
/// targets for tab drags, and the app's titles and content.
struct Build<'t, F, C> {
    theme: &'t Theme,
    drag: Rc<DragContext>,
    title: F,
    content: C,
}

impl<'a> Dock<'a> {
    /// A dock filling `size` points. `on_event` wraps its events in the
    /// app's action type.
    pub fn new(
        state: &'a DockState,
        size: (f32, f32),
        on_event: impl Fn(DockEvent) -> Action + 'static,
    ) -> Self {
        Self {
            state,
            host: HostId::MAIN,
            handle: None,
            size,
            map: Rc::new(on_event),
            toggle_keys: Vec::new(),
            move_keys: Some((MOVE_TAB_KEYS.0.into(), MOVE_TAB_KEYS.1.into())),
            always_tabs: [false; 4],
            tab_width: 120.0,
            grips: false,
            fit: None,
            close_on_hover: false,
            indicator: false,
        }
    }

    /// Give every tab strip a grip at its end that drags the whole group:
    /// into another group, beside one, or (for an app that follows drags
    /// across windows) out into a window of its own. Off by default.
    pub fn group_grips(mut self, grips: bool) -> Self {
        self.grips = grips;
        self
    }

    /// The scope of the [`DropTargetId`]s a dock built with `handle` (or
    /// none) publishes: one per tab group body and one per tab strip, so
    /// drags that leave a window ([`quark_ui::element::DragSession`]) find
    /// the dock's groups in every window. [`Self::drop_destination`] reads
    /// a hit back.
    pub fn drop_scope(handle: Option<ElementHandle>) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::hash::DefaultHasher::new();
        (DOCK_ID, handle).hash(&mut hasher);
        hasher.finish()
    }

    /// The group a drop target of the dock names, whatever its scope.
    pub fn drop_target_pane(id: DropTargetId) -> PaneId {
        PaneId((id.key >> 1) as u32)
    }

    /// Where a drop on `hit`, one of the dock's targets, lands: its tab
    /// strip's slot, or the zone of its group's body. `None` when the group
    /// is gone.
    pub fn drop_destination(state: &DockState, hit: &DropTargetHit) -> Option<DockDestination> {
        let pane = Self::drop_target_pane(hit.id);
        let group = state.group(pane)?;
        let zone = if hit.id.key & 1 == 1 {
            DropZone::Tabs(hit.slot.unwrap_or(group.panels.len()))
        } else {
            // Docks paint untransformed, so the bounds are the body's size.
            DropZone::in_body(
                hit.bounds.width,
                hit.bounds.height,
                hit.local.0,
                hit.local.1,
            )
        };
        Some(DockDestination {
            host: state.host_of(pane)?,
            pane,
            zone,
        })
    }

    /// Name the dock's root element by `handle`. A tab drag finds the dock
    /// in the frame's geometry to map the pointer onto its groups, by the
    /// handle or else by the stable id `"dock"`, so a window with more than
    /// one dock gives each a handle.
    pub fn handle(mut self, handle: ElementHandle) -> Self {
        self.handle = Some(handle);
        self
    }

    /// Build the floating host `host` instead of the main host: its tree
    /// fills the dock, and every group shows its tabs so they can be
    /// dragged. A host that no longer exists builds an empty dock.
    pub fn host(mut self, host: HostId) -> Self {
        self.host = host;
        self
    }

    /// Stable id of `panel`'s tab, wherever it is docked: for finding it
    /// in the frame's geometry (anchoring a menu to it) or in the
    /// accessibility tree.
    pub fn tab_id(panel: PanelId) -> String {
        format!("dock:tab:{}", panel.0)
    }

    /// Toggle a side region with `binding` (keymap format, `"mod+b"`) from
    /// anywhere inside the dock, or anywhere at all when nothing is
    /// focused.
    pub fn toggle_key(mut self, region: DockRegion, binding: impl Into<String>) -> Self {
        self.toggle_keys.push((region, binding.into()));
        self
    }

    /// Keys (keymap format) that move the focused tab into the previous and
    /// next group that takes it, in tree order ([`DockState::move_target`]);
    /// `None` binds none. Defaults to Mod+Shift+Page Up and Page Down.
    pub fn move_tab_keys(mut self, keys: Option<(&str, &str)>) -> Self {
        self.move_keys = keys
            .map(|(previous, next)| (Cow::Owned(previous.to_owned()), Cow::Owned(next.to_owned())));
        self
    }

    /// Show the region's tab strip even for a single panel.
    pub fn always_show_tabs(mut self, region: DockRegion) -> Self {
        self.always_tabs[region.index()] = true;
        self
    }

    /// Width of every tab, shrunk so all tabs fit the strip. Tabs share one
    /// width so a drag maps to a slot without measuring titles. With
    /// [`Self::fit_tabs`], the most any one tab takes.
    pub fn tab_width(mut self, width: f32) -> Self {
        self.tab_width = width;
        self
    }

    /// Size each tab to its title rather than giving every tab the same
    /// width: `label_width` is a panel title's width in points at the
    /// theme's small font size (the app measures it with its text system;
    /// the dock does not), and each tab adds its padding and close button,
    /// up to [`Self::tab_width`]. Longer titles end in an ellipsis, and the
    /// tabs shrink together when the strip is too narrow for all of them.
    /// Off by default.
    pub fn fit_tabs(mut self, label_width: &'a dyn Fn(PanelId) -> f32) -> Self {
        self.fit = Some(label_width);
        self
    }

    /// Show an inactive tab's close button only while the pointer is over
    /// the tab; the active tab always shows it. The button stays in the
    /// accessibility tree either way. Off by default.
    pub fn tab_close_on_hover(mut self, on: bool) -> Self {
        self.close_on_hover = on;
        self
    }

    /// Mark the active tab with a 2-point accent bar along its bottom edge.
    /// Off by default.
    pub fn tab_indicator(mut self, on: bool) -> Self {
        self.indicator = on;
        self
    }

    /// Focus target of a group's active tab. Arrow keys move it.
    pub fn tab_focus(pane: PaneId) -> FocusId {
        FocusId::new(
            FocusId::from_key("dock:tab")
                .0
                .wrapping_add(u64::from(pane.0) + 1),
        )
    }

    fn strip_height(theme: &Theme) -> f32 {
        (theme.metrics.ui_font_size * 2.5).round()
    }

    /// A group shows tabs with more than one panel, when its region asks,
    /// when its region is split, or in a floating host, so every group's
    /// tabs can be dragged.
    fn shows_tabs(&self, region: DockRegion, group: &TabGroup) -> bool {
        self.host != HostId::MAIN
            || group.panels.len() > 1
            || self.always_tabs[region.index()]
            || matches!(self.state.root(region), PaneNode::Split(_))
    }

    /// What assistive tech calls a region's tab lists and dividers.
    fn area_label(&self, region: DockRegion) -> &'a str {
        if self.host == HostId::MAIN {
            self.state.label(region)
        } else {
            self.state.host_label(self.host)
        }
    }

    /// Each shown region's rect inside the dock.
    fn region_rects(&self) -> [Rect; 4] {
        let state = self.state;
        let (width, height) = self.size;
        let columns = state.columns.resolve(width);
        let columns = columns.as_slice();
        let rows = state.rows.resolve(height);
        let rows = rows.as_slice();
        let gap = |split: &SplitState, divider: usize| {
            if split.divider_shown(divider) {
                DIVIDER_THICKNESS
            } else {
                0.0
            }
        };
        let middle_x = columns[0] + gap(&state.columns, 0);
        let right_x = middle_x + columns[1] + gap(&state.columns, 1);
        let bottom_y = rows[0] + gap(&state.rows, 0);
        [
            Rect::new(0.0, 0.0, columns[0], height),
            Rect::new(right_x, 0.0, columns[2], height),
            Rect::new(middle_x, bottom_y, columns[1], rows[1]),
            Rect::new(middle_x, 0.0, columns[1], rows[0]),
        ]
    }

    /// `title` names each panel's tab (a `&'static str` or a `String`);
    /// `content` builds an active panel's content for the size it gets.
    pub fn build<T: Title>(
        self,
        theme: &Theme,
        title: impl Fn(PanelId) -> T,
        content: impl FnMut(PanelId, (f32, f32)) -> AnyElement,
    ) -> AnyElement {
        if self.host != HostId::MAIN {
            return self.build_floating(theme, title, content);
        }
        let state = self.state;
        let (width, height) = self.size;
        let rects = self.region_rects();

        let strip = Self::strip_height(theme);
        let mut hits = Vec::new();
        let mut groups = Vec::new();
        for region in DockRegion::ALL {
            if region != DockRegion::Center && !state.is_visible(region) {
                continue;
            }
            groups.clear();
            state
                .root(region)
                .layout(rects[region.index()], &mut groups);
            for &(pane, rect) in &groups {
                let Some(group) = state.group(pane) else {
                    continue;
                };
                let tabs = self.shows_tabs(region, group);
                hits.push(GroupHit {
                    pane,
                    region,
                    rect,
                    strip: if tabs { strip } else { 0.0 },
                    tabs: self.tab_widths(theme, rect.width, group),
                });
            }
        }
        let drag = DragContext::new(
            hits,
            state.policies,
            self.handle.map_or(DockRoot::Id, DockRoot::Handle),
            HostId::MAIN,
        );
        let mut cx = Build {
            theme,
            drag: Rc::new(drag),
            title,
            content,
        };
        let mut region = |r: DockRegion| {
            let rect = rects[r.index()];
            self.region(&mut cx, r, (rect.width, rect.height))
        };
        let left = region(DockRegion::Left);
        let right = region(DockRegion::Right);
        let center = region(DockRegion::Center);
        let bottom = region(DockRegion::Bottom);

        let rows_map = self.map.clone();
        let middle = Split::new("dock:rows", &state.rows, height, move |e| {
            rows_map(DockEvent::Split(DockSplit::Rows, e))
        })
        .child(center)
        .child(bottom)
        .build(theme);
        let columns_map = self.map.clone();
        let body = Split::new("dock:columns", &state.columns, width, move |e| {
            columns_map(DockEvent::Split(DockSplit::Columns, e))
        })
        .child(left)
        .child(middle)
        .child(right)
        .build(theme);

        view! {
            <div
                w={width}
                h={height}
                bg={theme.colors.background}
                @when {let Some(handle) = self.handle} { element_handle={handle} }
                @when {self.handle.is_none()} { id={DOCK_ID} }
                @for (r, binding) in &self.toggle_keys {
                    on_key={(binding.clone(), (self.map)(DockEvent::Toggle(*r)))}
                }
            >
                {body}
            </div>
        }
    }

    /// A floating host: its one tree filling the dock.
    fn build_floating<T: Title>(
        self,
        theme: &Theme,
        title: impl Fn(PanelId) -> T,
        content: impl FnMut(PanelId, (f32, f32)) -> AnyElement,
    ) -> AnyElement {
        let (width, height) = self.size;
        let strip = Self::strip_height(theme);
        let root = self.state.host_root(self.host);
        let mut groups = Vec::new();
        if let Some(root) = root {
            root.layout(Rect::new(0.0, 0.0, width, height), &mut groups);
        }
        let hits = groups
            .into_iter()
            .filter_map(|(pane, rect)| {
                let group = self.state.group(pane)?;
                Some(GroupHit {
                    pane,
                    region: DockRegion::Center,
                    rect,
                    strip,
                    tabs: self.tab_widths(theme, rect.width, group),
                })
            })
            .collect();
        let drag = DragContext::new(
            hits,
            [self.state.area_policy(self.host, DockRegion::Center); 4],
            self.handle.map_or(DockRoot::Id, DockRoot::Handle),
            self.host,
        );
        let mut cx = Build {
            theme,
            drag: Rc::new(drag),
            title,
            content,
        };
        let body = root.map(|root| self.node(&mut cx, DockRegion::Center, root, (width, height)));
        view! {
            <div
                w={width}
                h={height}
                bg={theme.colors.background}
                @when {let Some(handle) = self.handle} { element_handle={handle} }
                @when {self.handle.is_none()} { id={DOCK_ID} }
            >
                if let Some(body) = body {
                    {body}
                }
            </div>
        }
    }

    fn region<T: Title>(
        &self,
        cx: &mut Build<'_, impl Fn(PanelId) -> T, impl FnMut(PanelId, (f32, f32)) -> AnyElement>,
        region: DockRegion,
        size: (f32, f32),
    ) -> AnyElement {
        if region != DockRegion::Center && !self.state.is_visible(region) {
            return view! { <div w={size.0} h={size.1} bg={cx.theme.colors.surface} /> };
        }
        self.node(cx, region, self.state.root(region), size)
    }

    fn node<T: Title>(
        &self,
        cx: &mut Build<'_, impl Fn(PanelId) -> T, impl FnMut(PanelId, (f32, f32)) -> AnyElement>,
        region: DockRegion,
        node: &PaneNode,
        (width, height): (f32, f32),
    ) -> AnyElement {
        let split = match node {
            PaneNode::Tabs(group) => return self.group(cx, region, group, (width, height)),
            PaneNode::Split(split) => split,
        };
        let horizontal = split.axis == Axis::Horizontal;
        let extent = if horizontal { width } else { height };
        let sizes = child_sizes(&split.weights, extent);
        view! {
            <div
                @when {horizontal} { class="flex-row" }
                @when {!horizontal} { class="flex-col" }
                w={width}
                h={height}
            >
                for (i, (child, &size)) in split.children.iter().zip(&sizes).enumerate() {
                    if i > 0 {
                        {self.pane_divider(cx.theme, region, split, i - 1, &sizes, extent)}
                    }
                    <div
                        class="flex-none overflow-clip"
                        w={if horizontal { size } else { width }}
                        h={if horizontal { height } else { size }}
                    >
                        {self.node(
                            cx,
                            region,
                            child,
                            if horizontal {
                                (size, height)
                            } else {
                                (width, size)
                            },
                        )}
                    </div>
                }
            </div>
        }
    }

    /// Focus target of divider `divider` of the split `split` inside a
    /// region; arrow keys move it. Stays with the divider while it is
    /// resized; a split that gains or loses groups renumbers its dividers.
    pub fn divider_focus(split: PaneId, divider: usize) -> FocusId {
        let key = (u64::from(split.0) << 16) | (divider as u64 & 0xffff);
        FocusId::new(FocusId::from_key("dock:divider").0.wrapping_add(key + 1))
    }

    /// The divider's value for assistive tech is its position in points
    /// from the split's start, with the range [`DockState`] lets it move
    /// in; its text is that position, in points like a region divider's.
    fn pane_divider(
        &self,
        theme: &Theme,
        region: DockRegion,
        split: &PaneSplit,
        divider: usize,
        sizes: &[f32],
        extent: f32,
    ) -> AnyElement {
        let colors = &theme.colors;
        let horizontal = split.axis == Axis::Horizontal;
        let cursor = resize_cursor(split.axis);
        let (back, forward) = if horizontal {
            ("left", "right")
        } else {
            ("up", "down")
        };
        let map = self.map.clone();
        let id = split.id;
        let event = move |event| {
            (map)(DockEvent::PaneDivider {
                split: id,
                divider,
                event,
            })
        };
        let nudge = |delta: f32| event(PaneDividerEvent::Nudge { delta, extent });
        let to = |position: f32| event(PaneDividerEvent::SetPosition { position, extent });
        let set_position = event.clone();
        let numeric_actions = NumericActions::new(move |position| {
            set_position(PaneDividerEvent::SetPosition {
                position: position as f32,
                extent,
            })
        })
        .steps(nudge(-NUDGE_STEP), nudge(NUDGE_STEP));
        let avail: f32 = sizes.iter().sum();
        let (at, lo, hi) = divider_span(sizes, divider_min(MIN_GROUP, avail, sizes.len()), divider);
        let (drag_map, axis) = (self.map.clone(), split.axis);
        // The same wide invisible grip as a `Split` divider.
        let grip = 8.0;
        let offset = -(grip - DIVIDER_THICKNESS) / 2.0;
        view! {
            <div
                class="flex-none relative"
                bg={colors.border_variant}
                @when {horizontal} { w={DIVIDER_THICKNESS} class="h-full" }
                @when {!horizontal} { h={DIVIDER_THICKNESS} class="w-full" }
            >
                <div
                    class="absolute"
                    z_index={1}
                    accessibility_id={format!("dock:split:{}:{divider}", id.0)}
                    accessibility_role={Role::Splitter}
                    role="separator"
                    aria-label={quark_ui::i18n::tr_args(
                        "quark-resize-named",
                        [("name", self.area_label(region).into())],
                    )}
                    aria-valuetext={format!("{at:.0}")}
                    accessibility_numeric={NumericValue {
                        value: f64::from(at),
                        min: f64::from(lo),
                        max: f64::from(hi),
                        step: Some(f64::from(NUDGE_STEP)),
                    }}
                    accessibility_numeric_actions={numeric_actions}
                    // A line between side by side groups stands upright.
                    accessibility_orientation={if horizontal {
                        Orientation::Vertical
                    } else {
                        Orientation::Horizontal
                    }}
                    focus_ring={Self::divider_focus(id, divider)}
                    on_key={(back, nudge(-NUDGE_STEP))}
                    on_key={(forward, nudge(NUDGE_STEP))}
                    on_key={(format!("shift+{back}"), nudge(-NUDGE_STEP_LARGE))}
                    on_key={(format!("shift+{forward}"), nudge(NUDGE_STEP_LARGE))}
                    // Home and End: the ends of the range it may move in.
                    on_key={("home", to(lo))}
                    on_key={("end", to(hi))}
                    test_id="dock-pane-divider"
                    cursor={cursor}
                    hover_bg={colors.accent}
                    on:drag={move |press: ClickEvent| {
                        let map = drag_map.clone();
                        let drag = PaneDividerDrag::new(map, id, divider, axis, &press, extent);
                        Box::new(drag) as Box<dyn DragHandler>
                    }}
                    @when {horizontal} { class="top-0 bottom-0" left={offset} w={grip} }
                    @when {!horizontal} { class="left-0 right-0" top={offset} h={grip} }
                />
            </div>
        }
    }

    fn group<T: Title>(
        &self,
        cx: &mut Build<'_, impl Fn(PanelId) -> T, impl FnMut(PanelId, (f32, f32)) -> AnyElement>,
        region: DockRegion,
        group: &TabGroup,
        (width, height): (f32, f32),
    ) -> AnyElement {
        let theme = cx.theme;
        let colors = &theme.colors;
        let show_tabs = self.shows_tabs(region, group);
        let strip_height = if show_tabs {
            Self::strip_height(theme)
        } else {
            0.0
        };
        let body_height = (height - strip_height).max(0.0);
        let tabs = self.tab_widths(theme, width, group);
        // Where a dragged tab would land in this group.
        let preview = self
            .state
            .shown_target()
            .filter(|target| target.pane == group.id)
            .map(|target| match target.zone {
                DropZone::Tabs(i) => {
                    let x = (tabs.left(i) - 1.0).max(0.0);
                    (x, 0.0, 2.0, strip_height)
                }
                zone => {
                    let (x, y, w, h) = zone.preview(width, body_height);
                    (x, y + strip_height, w, h)
                }
            });
        let targets = self.drop_targets(group, (width, height), strip_height, tabs);
        view! {
            <div class="relative flex-col" w={width} h={height} bg={colors.surface}>
                <div class="absolute" left={0.0} top={0.0} w={width} h={height}>{targets}</div>
                if show_tabs && !group.panels.is_empty() {
                    {self.tab_strip(
                        theme,
                        region,
                        group,
                        (width, strip_height),
                        &cx.drag,
                        &cx.title
                    )}
                }
                if let Some(active) = group.active_panel() {
                    <div
                        w={width}
                        h={body_height}
                        class="overflow-clip"
                        accessibility_id={pane_panel_id(group.id)}
                        accessibility_role={Role::TabPanel}
                        role="tabpanel"
                        aria-label={quark::intern((cx.title)(active).as_ref())}
                    >
                        {(cx.content)(active, (width, body_height))}
                    </div>
                }
                if let Some((x, y, w, h)) = preview {
                    <div
                        class="absolute"
                        z_index={10}
                        left={x}
                        top={y}
                        w={w}
                        h={h}
                        bg={colors.accent.with_alpha(if w > 2.0 { 56 } else { 255 })}
                        test_id="dock-drop-preview"
                    />
                }
            </div>
        }
    }

    /// Publish the group's drop targets: its tab strip, a slot per tab and
    /// one for the rest of the strip, and its body.
    fn drop_targets(
        &self,
        group: &TabGroup,
        (width, height): (f32, f32),
        strip: f32,
        tabs: TabWidths,
    ) -> AnyElement {
        let scope = Self::drop_scope(self.handle);
        let key = u64::from(group.id.0) << 1;
        let revision = self.state.layout_revision();
        let count = group.panels.len();
        canvas(move |b, _scene, cx| {
            let rect = |x, y, width, height| quark::Rect {
                x,
                y,
                width,
                height,
            };
            if strip > 0.0 {
                let end = tabs.left(count);
                let slots: Vec<quark::Rect> = (0..count)
                    .map(|i| rect(b.x + tabs.left(i), b.y, tabs.width(i), strip))
                    .chain([rect(b.x + end, b.y, (b.width - end).max(0.0), strip)])
                    .collect();
                cx.add_drop_target(DropTarget {
                    id: DropTargetId {
                        scope,
                        key: key | 1,
                    },
                    revision,
                    layout: rect(b.x, b.y, b.width, strip),
                    slots: &slots,
                });
            }
            cx.add_drop_target(DropTarget {
                id: DropTargetId { scope, key },
                revision,
                layout: rect(b.x, b.y + strip, b.width, (b.height - strip).max(0.0)),
                slots: &[],
            });
        })
        .w(width)
        .h(height)
        .into_any()
    }
}

/// Accessibility id of a group's panel area, shared across frames.
fn pane_panel_id(pane: PaneId) -> std::sync::Arc<str> {
    use std::fmt::Write;
    thread_local! {
        static BUF: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
    }
    BUF.with(|buf| {
        let mut buf = buf.borrow_mut();
        buf.clear();
        let _ = write!(buf, "dock:pane:{}:panel", pane.0);
        quark::intern(&buf)
    })
}
