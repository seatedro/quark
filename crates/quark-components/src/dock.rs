//! Docked regions of tabbed panels around a center, built on [`Split`].
//!
//! A [`DockState`] has four regions: left, right, and bottom docks that can
//! be resized and hidden, and a center that fills the rest. Each region
//! holds an ordered list of app-defined [`PanelId`]s with one active. With
//! more than one panel (or when asked), a region shows a tab strip: click a
//! tab to select it, drag it along the strip to reorder, close it with its
//! close button, or move between tabs with the arrow keys.
//!
//! The dock knows nothing about what panels are: the app gives each one a
//! title and builds the active ones' content. Like [`Split`], it emits
//! [`DockEvent`]s through a caller supplied mapping, and the app passes them
//! back to [`DockState::apply`]. [`DockState::snapshot`] persists sizes,
//! visibility, and tab order.

use std::rc::Rc;

use accesskit::Role;
use quark::SemanticRole;
use quark_ui::element::{
    AnyElement, ClickEvent, DragHandler, DragReleaseResult, IntoAnyElement, div, svg_icon, text,
};
use quark_ui::icons::lucide;
use quark_ui::style::Styled;
use quark_ui::theme::{Color, Theme};
use quark_ui::{Action, FocusId};
use serde::{Deserialize, Serialize};

use crate::split::{Axis, Pane, Split, SplitEvent, SplitSnapshot, SplitState};

/// An app-chosen panel identity. Persisted, so keep values stable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PanelId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DockRegion {
    Left,
    Right,
    Bottom,
    Center,
}

impl DockRegion {
    pub const ALL: [DockRegion; 4] = [Self::Left, Self::Right, Self::Bottom, Self::Center];

    fn index(self) -> usize {
        self as usize
    }

    fn name(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Right => "right",
            Self::Bottom => "bottom",
            Self::Center => "center",
        }
    }

    /// Which split holds the region, and its pane there.
    fn place(self) -> (DockSplit, usize) {
        match self {
            Self::Left => (DockSplit::Columns, 0),
            Self::Right => (DockSplit::Columns, 2),
            Self::Center => (DockSplit::Rows, 0),
            Self::Bottom => (DockSplit::Rows, 1),
        }
    }
}

/// The dock's two splits: left | middle | right, and inside the middle,
/// center over bottom.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DockSplit {
    Columns,
    Rows,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DockEvent {
    Split(DockSplit, SplitEvent),
    Select {
        region: DockRegion,
        index: usize,
    },
    Close {
        region: DockRegion,
        index: usize,
    },
    Move {
        region: DockRegion,
        from: usize,
        to: usize,
    },
    /// Hide or show a side region.
    Toggle(DockRegion),
}

/// Sizes of the three side regions and the center's minimum. Labels name
/// the regions for assistive tech.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DockLayout {
    pub left: Pane,
    pub right: Pane,
    pub bottom: Pane,
    pub center_label: &'static str,
    pub center_min_width: f32,
    pub center_min_height: f32,
}

impl Default for DockLayout {
    fn default() -> Self {
        Self {
            left: Pane::fixed("Left dock", 260.0).min(160.0).max(520.0),
            right: Pane::fixed("Right dock", 420.0).min(240.0).max(900.0),
            bottom: Pane::fixed("Bottom dock", 240.0).min(100.0).max(700.0),
            center_label: "Center",
            center_min_width: 320.0,
            center_min_height: 160.0,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RegionSnapshot {
    pub panels: Vec<PanelId>,
    pub active: usize,
}

/// The persisted part of a [`DockState`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DockSnapshot {
    pub columns: SplitSnapshot,
    pub rows: SplitSnapshot,
    pub left: RegionSnapshot,
    pub right: RegionSnapshot,
    pub bottom: RegionSnapshot,
    pub center: RegionSnapshot,
}

#[derive(Debug, Clone, Default, PartialEq)]
struct Region {
    panels: Vec<PanelId>,
    active: usize,
}

/// Panels, sizes, and visibility of a dock, owned by the app.
#[derive(Debug, Clone, PartialEq)]
pub struct DockState {
    columns: SplitState,
    rows: SplitState,
    regions: [Region; 4],
    labels: [&'static str; 4],
}

impl DockState {
    pub fn new(layout: DockLayout) -> Self {
        let columns = SplitState::new(
            Axis::Horizontal,
            vec![
                layout.left.collapsible(),
                Pane::flex(layout.center_label).min(layout.center_min_width),
                layout.right.collapsible(),
            ],
        )
        .hide_collapsed_dividers(true);
        let rows = SplitState::new(
            Axis::Vertical,
            vec![
                Pane::flex(layout.center_label).min(layout.center_min_height),
                layout.bottom.collapsible(),
            ],
        )
        .hide_collapsed_dividers(true);
        let mut state = Self {
            columns,
            rows,
            regions: Default::default(),
            labels: [
                layout.left.label,
                layout.right.label,
                layout.bottom.label,
                layout.center_label,
            ],
        };
        // Empty side regions take no space until a panel arrives.
        for region in [DockRegion::Left, DockRegion::Right, DockRegion::Bottom] {
            state.set_hidden(region, true);
        }
        state
    }

    fn split(&self, which: DockSplit) -> &SplitState {
        match which {
            DockSplit::Columns => &self.columns,
            DockSplit::Rows => &self.rows,
        }
    }

    fn split_mut(&mut self, which: DockSplit) -> &mut SplitState {
        match which {
            DockSplit::Columns => &mut self.columns,
            DockSplit::Rows => &mut self.rows,
        }
    }

    fn set_hidden(&mut self, region: DockRegion, hidden: bool) {
        if region != DockRegion::Center {
            let (split, pane) = region.place();
            self.split_mut(split).set_collapsed(pane, hidden);
        }
    }

    pub fn label(&self, region: DockRegion) -> &'static str {
        self.labels[region.index()]
    }

    pub fn panels(&self, region: DockRegion) -> &[PanelId] {
        &self.regions[region.index()].panels
    }

    pub fn active(&self, region: DockRegion) -> Option<PanelId> {
        let r = &self.regions[region.index()];
        r.panels.get(r.active).copied()
    }

    /// Shown with its content: has panels, and is not hidden.
    pub fn is_visible(&self, region: DockRegion) -> bool {
        if self.regions[region.index()].panels.is_empty() {
            return false;
        }
        let (split, pane) = region.place();
        !self.split(split).is_collapsed(pane)
    }

    /// Size of a side region while shown, kept while hidden.
    pub fn size(&self, region: DockRegion) -> f32 {
        let (split, pane) = region.place();
        self.split(split).size(pane)
    }

    /// Show or hide a side region that has panels.
    pub fn set_visible(&mut self, region: DockRegion, visible: bool) {
        if !self.regions[region.index()].panels.is_empty() {
            self.set_hidden(region, !visible);
        }
    }

    pub fn toggle(&mut self, region: DockRegion) {
        self.set_visible(region, !self.is_visible(region));
    }

    /// Add a panel at the end of a region, or find it there, and make it
    /// active. Shows the region.
    pub fn open(&mut self, region: DockRegion, panel: PanelId) {
        let r = &mut self.regions[region.index()];
        r.active = match r.panels.iter().position(|p| *p == panel) {
            Some(i) => i,
            None => {
                r.panels.push(panel);
                r.panels.len() - 1
            }
        };
        self.set_hidden(region, false);
    }

    pub fn select(&mut self, region: DockRegion, index: usize) {
        let r = &mut self.regions[region.index()];
        if index < r.panels.len() {
            r.active = index;
        }
    }

    /// Remove a panel. The neighbor after it becomes active (or before it,
    /// at the end); a side region left empty hides.
    pub fn close(&mut self, region: DockRegion, index: usize) -> Option<PanelId> {
        let r = &mut self.regions[region.index()];
        if index >= r.panels.len() {
            return None;
        }
        let removed = r.panels.remove(index);
        if index < r.active || r.active >= r.panels.len() {
            r.active = r.active.saturating_sub(1);
        }
        if r.panels.is_empty() {
            self.set_hidden(region, true);
        }
        Some(removed)
    }

    /// Move the panel at `from` to `to`, keeping the same panel active.
    pub fn move_panel(&mut self, region: DockRegion, from: usize, to: usize) {
        let r = &mut self.regions[region.index()];
        let len = r.panels.len();
        if from >= len || to >= len || from == to {
            return;
        }
        let active = r.panels[r.active];
        let panel = r.panels.remove(from);
        r.panels.insert(to, panel);
        r.active = r.panels.iter().position(|p| *p == active).unwrap_or(0);
    }

    /// Apply an event from the dock's element. `now_ms` dates divider
    /// presses for double click. Returns true when the change is settled
    /// and worth persisting.
    pub fn apply(&mut self, event: DockEvent, now_ms: u64) -> bool {
        match event {
            DockEvent::Split(which, event) => self.split_mut(which).apply(event, now_ms),
            DockEvent::Select { region, index } => {
                self.select(region, index);
                true
            }
            DockEvent::Close { region, index } => self.close(region, index).is_some(),
            DockEvent::Move { region, from, to } => {
                self.move_panel(region, from, to);
                true
            }
            DockEvent::Toggle(region) => {
                self.toggle(region);
                true
            }
        }
    }

    pub fn snapshot(&self) -> DockSnapshot {
        let region = |r: DockRegion| {
            let r = &self.regions[r.index()];
            RegionSnapshot {
                panels: r.panels.clone(),
                active: r.active,
            }
        };
        DockSnapshot {
            columns: self.columns.snapshot(),
            rows: self.rows.snapshot(),
            left: region(DockRegion::Left),
            right: region(DockRegion::Right),
            bottom: region(DockRegion::Bottom),
            center: region(DockRegion::Center),
        }
    }

    /// Restore a snapshot. Panels `keep` rejects (ones the app no longer
    /// has) are dropped; regions left empty hide.
    pub fn restore(&mut self, snapshot: &DockSnapshot, keep: impl Fn(PanelId) -> bool) {
        self.columns.restore(&snapshot.columns);
        self.rows.restore(&snapshot.rows);
        let saved = [
            &snapshot.left,
            &snapshot.right,
            &snapshot.bottom,
            &snapshot.center,
        ];
        for region in DockRegion::ALL {
            let s = saved[region.index()];
            let active = s.panels.get(s.active).copied();
            let mut panels: Vec<PanelId> = Vec::with_capacity(s.panels.len());
            for &p in &s.panels {
                if keep(p) && !panels.contains(&p) {
                    panels.push(p);
                }
            }
            let r = &mut self.regions[region.index()];
            r.active = active
                .and_then(|a| panels.iter().position(|p| *p == a))
                .unwrap_or(0);
            r.panels = panels;
            if r.panels.is_empty() {
                self.set_hidden(region, true);
            }
        }
    }
}

type EventMap = Rc<dyn Fn(DockEvent) -> Action>;

/// Builds the element for a [`DockState`].
pub struct Dock<'a> {
    state: &'a DockState,
    size: (f32, f32),
    map: EventMap,
    toggle_keys: Vec<(DockRegion, String)>,
    always_tabs: [bool; 4],
    tab_width: f32,
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
            size,
            map: Rc::new(on_event),
            toggle_keys: Vec::new(),
            always_tabs: [false; 4],
            tab_width: 120.0,
        }
    }

    /// Toggle a side region with `binding` (keymap format, `"mod+b"`) from
    /// anywhere inside the dock, or anywhere at all when nothing is
    /// focused.
    pub fn toggle_key(mut self, region: DockRegion, binding: impl Into<String>) -> Self {
        self.toggle_keys.push((region, binding.into()));
        self
    }

    /// Show the region's tab strip even for a single panel.
    pub fn always_show_tabs(mut self, region: DockRegion) -> Self {
        self.always_tabs[region.index()] = true;
        self
    }

    /// Width of every tab, shrunk so all tabs fit the strip. Tabs share one
    /// width so a drag maps to a slot without measuring titles.
    pub fn tab_width(mut self, width: f32) -> Self {
        self.tab_width = width;
        self
    }

    /// Focus target of a region's active tab. Arrow keys move it.
    pub fn tab_focus(region: DockRegion) -> FocusId {
        FocusId::from_key(match region {
            DockRegion::Left => "dock:tab:left",
            DockRegion::Right => "dock:tab:right",
            DockRegion::Bottom => "dock:tab:bottom",
            DockRegion::Center => "dock:tab:center",
        })
    }

    /// `title` names each panel's tab; `content` builds an active panel's
    /// content for the size it gets.
    pub fn build(
        self,
        theme: &Theme,
        title: impl Fn(PanelId) -> String,
        mut content: impl FnMut(PanelId, (f32, f32)) -> AnyElement,
    ) -> AnyElement {
        let state = self.state;
        let (width, height) = self.size;
        let columns = state.columns.resolve(width);
        let middle_width = columns.as_slice()[1];
        let rows = state.rows.resolve(height);

        let mut region =
            |r: DockRegion, size: (f32, f32)| self.region(theme, r, size, &title, &mut content);
        let left = region(DockRegion::Left, (columns.as_slice()[0], height));
        let right = region(DockRegion::Right, (columns.as_slice()[2], height));
        let center = region(DockRegion::Center, (middle_width, rows.as_slice()[0]));
        let bottom = region(DockRegion::Bottom, (middle_width, rows.as_slice()[1]));

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

        let mut root = div().w(width).h(height).bg(theme.colors.background);
        for (r, binding) in self.toggle_keys {
            root = root.on_key(binding, (self.map)(DockEvent::Toggle(r)));
        }
        root.child(body).into_any()
    }

    fn region(
        &self,
        theme: &Theme,
        region: DockRegion,
        (width, height): (f32, f32),
        title: &impl Fn(PanelId) -> String,
        content: &mut impl FnMut(PanelId, (f32, f32)) -> AnyElement,
    ) -> AnyElement {
        let state = self.state;
        let colors = &theme.colors;
        let mut root = div().flex_col().w(width).h(height).bg(colors.surface);
        if !state.is_visible(region) && region != DockRegion::Center {
            return root.into_any();
        }
        let panels = state.panels(region);
        let Some(active) = state.active(region) else {
            return root.into_any();
        };
        let active_index = state.regions[region.index()].active;
        let show_tabs = panels.len() > 1 || self.always_tabs[region.index()];
        let mut body_height = height;
        if show_tabs {
            let strip_height = (theme.metrics.ui_font_size * 2.5).round();
            body_height = (height - strip_height).max(0.0);
            root = root.child(self.tab_strip(
                theme,
                region,
                active_index,
                (width, strip_height),
                title,
            ));
        }
        let name = title(active);
        root.child(
            div()
                .w(width)
                .h(body_height)
                .clip()
                .accessibility_id(format!("dock:{}:panel", region.name()))
                .accessibility_role(Role::TabPanel)
                .semantic_role(SemanticRole::TabPanel)
                .accessibility_label(name)
                .child(content(active, (width, body_height))),
        )
        .into_any()
    }

    fn tab_strip(
        &self,
        theme: &Theme,
        region: DockRegion,
        active: usize,
        (width, height): (f32, f32),
        title: &impl Fn(PanelId) -> String,
    ) -> AnyElement {
        let colors = &theme.colors;
        let m = &theme.metrics;
        let panels = self.state.panels(region);
        let count = panels.len();
        let tab_width = self.tab_width.min(width / count as f32).floor();
        let mut strip = div()
            .flex_row()
            .w_full()
            .h(height)
            .flex_none()
            .clip()
            .border_b(colors.border_variant)
            .accessibility_id(format!("dock:{}:tabs", region.name()))
            .accessibility_role(Role::TabList)
            .semantic_role(SemanticRole::TabList)
            .accessibility_label(self.state.label(region))
            .test_id("dock-tabs");
        for (index, &panel) in panels.iter().enumerate() {
            let selected = index == active;
            let name = title(panel);
            let map = self.map.clone();
            let mut tab = div()
                .flex_row()
                .flex_none()
                .items_center()
                .gap(m.spacing_xs)
                .px(m.spacing_sm)
                .w(tab_width)
                .h_full()
                .border_r(colors.border_variant)
                .accessibility_id(format!("dock:{}:tab:{}", region.name(), panel.0))
                .accessibility_role(Role::Tab)
                .semantic_role(SemanticRole::Tab)
                .accessibility_label(name.clone())
                .accessibility_selected(selected)
                .test_id("dock-tab")
                .on_drag(move |press: ClickEvent| {
                    Box::new(TabDrag {
                        map: map.clone(),
                        region,
                        start: index,
                        current: index,
                        count,
                        origin: press.x,
                        tab_width,
                    }) as Box<dyn DragHandler>
                });
            if selected {
                // Roving focus: only the active tab is a focus target, so
                // selecting a neighbor by arrow key moves focus with it.
                let prev = index.checked_sub(1).unwrap_or(count - 1);
                let next = (index + 1) % count;
                tab = tab
                    .bg(colors.background)
                    .focus_ring(Self::tab_focus(region))
                    .on_key(
                        "left",
                        (self.map)(DockEvent::Select {
                            region,
                            index: prev,
                        }),
                    )
                    .on_key(
                        "right",
                        (self.map)(DockEvent::Select {
                            region,
                            index: next,
                        }),
                    )
                    .on_key("home", (self.map)(DockEvent::Select { region, index: 0 }))
                    .on_key(
                        "end",
                        (self.map)(DockEvent::Select {
                            region,
                            index: count - 1,
                        }),
                    )
                    .on_key("delete", (self.map)(DockEvent::Close { region, index }));
            } else {
                tab = tab.hover_bg(colors.ghost_element_hover);
            }
            let label_color = if selected {
                colors.text_strong
            } else {
                colors.text_muted
            };
            let close = div()
                .flex_none()
                .items_center()
                .justify_center()
                .rounded(m.control_radius * 0.5)
                .p(2.0)
                .hover_bg(colors.ghost_element_hover)
                .accessibility_id(format!("dock:{}:close:{}", region.name(), panel.0))
                .accessibility_role(Role::Button)
                .accessibility_label(format!("Close {name}"))
                .on_click((self.map)(DockEvent::Close { region, index }))
                .child(svg_icon(lucide::X, m.ui_small_font_size).color(colors.text_muted));
            tab = tab
                .child(
                    div()
                        .flex_1()
                        // Let a long title shrink and truncate instead of
                        // pushing the close button out of the tab.
                        .min_w(0.0)
                        .clip()
                        .child(text(name).text_sm().color(label_color).truncate()),
                )
                .child(close);
            strip = strip.child(tab);
        }
        strip
            .child(div().flex_1().h_full().bg(Color::TRANSPARENT))
            .into_any()
    }
}

/// Selects a tab on press, then moves it one slot per tab width dragged.
struct TabDrag {
    map: EventMap,
    region: DockRegion,
    start: usize,
    current: usize,
    count: usize,
    origin: f32,
    tab_width: f32,
}

impl DragHandler for TabDrag {
    fn on_press(&mut self) -> Vec<Action> {
        vec![(self.map)(DockEvent::Select {
            region: self.region,
            index: self.start,
        })]
    }

    fn on_move(&mut self, x: f32, _y: f32) -> Vec<Action> {
        let slots = ((x - self.origin) / self.tab_width).round() as isize;
        let target = (self.start as isize + slots).clamp(0, self.count as isize - 1) as usize;
        if target == self.current {
            return Vec::new();
        }
        let from = std::mem::replace(&mut self.current, target);
        vec![(self.map)(DockEvent::Move {
            region: self.region,
            from,
            to: target,
        })]
    }

    fn on_release(&mut self) -> DragReleaseResult {
        DragReleaseResult::empty()
    }
}
