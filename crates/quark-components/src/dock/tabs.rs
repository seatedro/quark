//! Tab strips: their widths, their cached build, and the picture of a
//! dragged tab.

use std::borrow::Cow;
use std::collections::HashMap;
use std::rc::Rc;

use accesskit::Role;
use quark::{TabStop, view};
use quark_ui::Action;
use quark_ui::design::{Shadow, Sz};
use quark_ui::element::{
    AnyElement, CacheKey, ClickEvent, CursorHint, DragHandler, DragPreview, IntoAnyElement, cached,
    div, svg_icon, text,
};
use quark_ui::icons::lucide;
use quark_ui::style::Styled;
use quark_ui::theme::{Color, Theme, ThemeColors, ThemeMetrics};

use super::drag::{DragContext, GroupDrag, Origin, PayloadDrag, TabDrag};
use super::{Dock, DockEvent, DockRegion, EventMap, HostId, PanelId};
use crate::pane_tree::{PaneId, TabGroup};

/// Strips with more tabs than this share one width even when the dock
/// fits tabs to their titles, so the widths live inline (no allocation).
const MAX_FITTED_TABS: usize = 24;

/// The widths of a strip's tabs, left to right: one shared width, or each
/// tab's own when the dock fits tabs to their titles.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct TabWidths {
    count: usize,
    shared: f32,
    own: Option<[f32; MAX_FITTED_TABS]>,
}

impl TabWidths {
    fn shared(width: f32, count: usize) -> Self {
        Self {
            count,
            shared: width,
            own: None,
        }
    }

    pub(super) fn width(&self, index: usize) -> f32 {
        self.own.map_or(self.shared, |own| own[index])
    }

    /// The left edge of tab `index` (the right edge of the last tab for
    /// `count`), from the strip's left edge.
    pub(super) fn left(&self, index: usize) -> f32 {
        let index = index.min(self.count);
        match &self.own {
            Some(own) => own[..index].iter().sum(),
            None => self.shared * index as f32,
        }
    }

    /// The tab under `x` from the strip's left edge, or `count` past the
    /// last tab.
    pub(super) fn index_at(&self, x: f32) -> usize {
        match &self.own {
            None => ((x / self.shared).floor().max(0.0) as usize).min(self.count),
            Some(own) => {
                let mut right = 0.0;
                for (index, width) in own[..self.count].iter().enumerate() {
                    right += width;
                    if x < right {
                        return index;
                    }
                }
                self.count
            }
        }
    }

    pub(super) fn hash_into(&self, hasher: &mut impl std::hash::Hasher) {
        use std::hash::Hash;
        self.count.hash(hasher);
        for index in 0..self.count {
            self.width(index).to_bits().hash(hasher);
        }
    }
}

/// Width of a group's grip at the end of its tab strip.
const GRIP_WIDTH: f32 = 24.0;

impl Dock<'_> {
    /// The part of a strip `width` wide its tabs share.
    fn tabs_width(&self, width: f32) -> f32 {
        if self.grips {
            (width - GRIP_WIDTH).max(0.0)
        } else {
            width
        }
    }

    fn tab_width_for(&self, width: f32, count: usize) -> f32 {
        self.tab_width.min(width / count.max(1) as f32).floor()
    }

    /// The widths of `group`'s tabs in a strip `width` wide.
    pub(super) fn tab_widths(&self, theme: &Theme, width: f32, group: &TabGroup) -> TabWidths {
        let width = self.tabs_width(width);
        let count = group.panels.len();
        let shared = TabWidths::shared(self.tab_width_for(width, count), count);
        let Some(label_width) = self.fit else {
            return shared;
        };
        if count == 0 || count > MAX_FITTED_TABS {
            return shared;
        }
        let m = &theme.metrics;
        // Around the title: padding on both sides, the gap, the close
        // button (an icon with 2 points of padding), and the divider on the
        // tab's right, as `StripView::tab` lays them out; plus a point, so a
        // title that just fits is not cut short by rounding.
        let chrome = m.spacing_sm * 2.0 + m.spacing_xs + m.ui_small_font_size + 4.0 + 2.0;
        let min = chrome + m.ui_small_font_size * 2.0;
        let max = self.tab_width.max(min);
        let mut own = [0.0; MAX_FITTED_TABS];
        for (slot, panel) in own.iter_mut().zip(&group.panels) {
            *slot = (label_width(*panel) + chrome).ceil().clamp(min, max);
        }
        let total: f32 = own[..count].iter().sum();
        if total > width {
            let shrink = width / total;
            for slot in &mut own[..count] {
                *slot = (*slot * shrink).floor();
            }
        }
        TabWidths {
            count,
            shared: shared.shared,
            own: Some(own),
        }
    }

    pub(super) fn tab_strip<T: Title>(
        &self,
        theme: &Theme,
        region: DockRegion,
        group: &TabGroup,
        (width, height): (f32, f32),
        drag: &Rc<DragContext>,
        title: &impl Fn(PanelId) -> T,
    ) -> AnyElement {
        use std::hash::{Hash, Hasher};
        let tabs = self.tab_widths(theme, width, group);
        // Everything the strip reads besides the theme (which the element
        // cache compares itself) and the event map (which must be pure).
        let mut hasher = std::hash::DefaultHasher::new();
        (
            self.state.layout_revision(),
            self.host,
            region,
            group.id,
            group.active,
        )
            .hash(&mut hasher);
        group.panels.hash(&mut hasher);
        for panel in &group.panels {
            self.state.confined.contains(panel).hash(&mut hasher);
            title(*panel).as_ref().hash(&mut hasher);
        }
        let policies = self.state.policies.map(|p| (p.can_leave, p.accepts));
        (policies, &self.move_keys, self.grips, drag.hash).hash(&mut hasher);
        (self.close_on_hover, self.indicator).hash(&mut hasher);
        tabs.hash_into(&mut hasher);
        (height.to_bits(), width.to_bits()).hash(&mut hasher);
        let hash = hasher.finish();
        let data = self
            .state
            .strips
            .get(self.host, group.id, hash, || StripData {
                pane: group.id,
                region,
                panels: group.panels.clone(),
                active: group.active,
                titles: group.panels.iter().map(|&p| title(p).into()).collect(),
                confined: group
                    .panels
                    .iter()
                    .map(|p| self.state.confined.contains(p))
                    .collect(),
                moves: group
                    .active_panel()
                    .and_then(|panel| {
                        let (previous, next) = self.move_keys.as_ref()?;
                        let moves = [(previous, false), (next, true)]
                            .into_iter()
                            .filter_map(|(key, forward)| {
                                let destination = self.state.move_target(panel, forward)?;
                                let event = DockEvent::MoveTab {
                                    panel,
                                    destination,
                                    index: None,
                                };
                                Some((key.clone(), (self.map)(event)))
                            })
                            .collect();
                        Some(moves)
                    })
                    .unwrap_or_default(),
                label: self.state.group_label(group.id),
                tabs,
                height,
                grips: self.grips,
                close_on_hover: self.close_on_hover,
                indicator: self.indicator,
            });
        let strip = StripView {
            data,
            map: self.map.clone(),
            drag: drag.clone(),
            colors: theme.colors,
            metrics: theme.metrics,
        };
        let mut key = std::hash::DefaultHasher::new();
        ("dock:strip", self.handle, self.host, group.id).hash(&mut key);
        cached(CacheKey(key.finish()), hash, move || strip.build())
            .w(width)
            .h(height)
            .flex_shrink_0()
            .into_any()
    }
}

/// A panel title [`Dock::build`] takes: read for hashing every frame, and
/// owned only when a tab strip is rebuilt, so `&'static str` titles cost
/// no allocation in a steady frame.
pub trait Title: AsRef<str> + Into<String> {}

impl<T: AsRef<str> + Into<String>> Title for T {}

/// Everything a tab strip shows and does, gathered when it changes and
/// shared with its cached build closure, so a steady frame replays the
/// strip from the element cache without building its tabs.
#[derive(Debug)]
struct StripData {
    pane: PaneId,
    region: DockRegion,
    panels: Vec<PanelId>,
    active: usize,
    titles: Vec<String>,
    confined: Vec<bool>,
    /// The active tab's move keys, bound only toward a group that takes it,
    /// so a refused move leaves the key to the app.
    moves: Vec<(Cow<'static, str>, Action)>,
    label: String,
    tabs: TabWidths,
    height: f32,
    grips: bool,
    close_on_hover: bool,
    indicator: bool,
}

/// The last [`StripData`] built per tab group, with the inputs hash it was
/// built for. Lives in [`DockState`] (the one thing a [`Dock`] borrows from
/// frame to frame); it is a cache, so it is neither compared nor cloned.
#[derive(Default)]
pub(crate) struct StripMemo(std::cell::RefCell<HashMap<(HostId, PaneId), BuiltStrip>>);

/// A strip's data and the inputs hash it was built for.
type BuiltStrip = (u64, Rc<StripData>);

impl StripMemo {
    fn get(
        &self,
        host: HostId,
        pane: PaneId,
        hash: u64,
        build: impl FnOnce() -> StripData,
    ) -> Rc<StripData> {
        let mut strips = self.0.borrow_mut();
        if let Some((built, data)) = strips.get(&(host, pane))
            && *built == hash
        {
            return data.clone();
        }
        // Groups come and go with the layout; drop the ones gone before
        // the map outgrows any layout's group count by much.
        if strips.len() > 64 {
            strips.clear();
        }
        let data = Rc::new(build());
        strips.insert((host, pane), (hash, data.clone()));
        data
    }
}

impl Clone for StripMemo {
    fn clone(&self) -> Self {
        Self::default()
    }
}

impl PartialEq for StripMemo {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl std::fmt::Debug for StripMemo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("StripMemo")
    }
}

/// A tab strip's build closure: its data plus what it needs from the
/// frame that built it.
struct StripView {
    data: Rc<StripData>,
    map: EventMap,
    drag: Rc<DragContext>,
    colors: ThemeColors,
    metrics: ThemeMetrics,
}

impl StripView {
    fn build(&self) -> AnyElement {
        let data = &*self.data;
        let colors = &self.colors;
        let grip = data.grips.then(|| self.grip());
        view! {
            <div
                class="flex-row w-full"
                h={data.height}
                class="flex-none overflow-clip"
                border_b={colors.border_variant}
                accessibility_id={format!("dock:pane:{}:tabs", data.pane.0)}
                accessibility_role={Role::TabList}
                role="tablist"
                aria-label={data.label.clone()}
                test_id="dock-tabs"
            >
                for index in 0..data.panels.len() {
                    {self.tab(index)}
                }
                <div class="flex-1 h-full" bg={Color::TRANSPARENT} />
                if let Some(grip) = grip {
                    {grip}
                }
            </div>
        }
    }

    /// The grip that drags the whole group.
    fn grip(&self) -> AnyElement {
        let data = &*self.data;
        let colors = &self.colors;
        let m = &self.metrics;
        let map = self.map.clone();
        let ctx = self.drag.clone();
        let (pane, region, height) = (data.pane, data.region, data.height);
        let name = data.titles.get(data.active).cloned().unwrap_or_default();
        // The picture under the pointer: the active tab, and how many more.
        let label = match data.panels.len() {
            0 | 1 => name.clone(),
            n => format!("{name} +{}", n - 1),
        };
        view! {
            <div
                class="flex-none items-center justify-center"
                w={GRIP_WIDTH}
                class="h-full"
                // Pointer only: the keyboard moves groups through the app's
                // menu ("Move group to new window").
                id={format!("dock:grip:{}", pane.0)}
                test_id="dock-group-grip"
                cursor={CursorHint::Grab}
                hover_bg={colors.ghost_element_hover}
                on:drag={move |press: ClickEvent| {
                    let preview = tab_preview(PanelId(u64::MAX), &label, (120.0, height));
                    let drag = PayloadDrag::new(map.clone(), ctx.clone(), &press, preview);
                    Box::new(GroupDrag::new(drag, pane, region)) as Box<dyn DragHandler>
                }}
            >
                <icon
                    svg={lucide::GRIP_VERTICAL}
                    size={m.ui_small_font_size}
                    color={colors.text_muted}
                />
            </div>
        }
    }

    /// The close button of `panel`'s tab, drawn clear while `hidden`.
    fn close_button(&self, panel: PanelId, name: &str, hidden: bool, close: Action) -> AnyElement {
        let colors = &self.colors;
        let m = &self.metrics;
        let label = quark_ui::i18n::tr_args(
            "quark-close-named",
            [("name", quark_ui::i18n::Arg::Text(name))],
        );
        let color = if hidden {
            Color::TRANSPARENT
        } else {
            colors.text_muted
        };
        view! {
            <div
                class="flex-none items-center justify-center"
                rounded={m.control_radius * 0.5}
                p={2.0}
                hover_bg={colors.ghost_element_hover}
                accessibility_id={format!("dock:close:{}", panel.0)}
                accessibility_role={Role::Button}
                aria-label={label}
                on:click={close}
            >
                <icon svg={lucide::X} size={m.ui_small_font_size} color={color} />
            </div>
        }
    }

    fn tab(&self, index: usize) -> AnyElement {
        let data = &*self.data;
        let colors = &self.colors;
        let m = &self.metrics;
        let (pane, region) = (data.pane, data.region);
        let (tab_width, tab_height) = (data.tabs.width(index), data.height);
        let panel = data.panels[index];
        let count = data.panels.len();
        let selected = index == data.active;
        let name = data.titles[index].clone();
        let map = self.map.clone();
        let ctx = self.drag.clone();
        let confined = data.confined[index];
        let from = Origin {
            region,
            pane,
            index,
            count,
        };
        let close = (self.map)(DockEvent::Close { pane, index });
        // Roving focus: only the active tab is a focus target, so selecting
        // a neighbor by arrow key moves focus with it.
        let prev = index.checked_sub(1).unwrap_or(count - 1);
        let next = (index + 1) % count;
        let select = |index| (self.map)(DockEvent::Select { pane, index });
        // Only the focusable active tab takes the move keys.
        let moves: &[(Cow<'static, str>, Action)] = if selected { &data.moves } else { &[] };
        let label_color = if selected {
            colors.text_strong
        } else {
            colors.text_muted
        };
        let drag_title = name.clone();
        // An inactive tab's close button can wait for the pointer: drawn
        // clear, and in the muted color while the tab is hovered.
        let hidden_close = data.close_on_hover && !selected;
        let close_button = self.close_button(panel, &name, hidden_close, close.clone());
        let (hover_fill, hover_icon) = (colors.ghost_element_hover, colors.text_muted);
        view! {
            <div
                class="flex-row flex-none items-center"
                gap={m.spacing_xs}
                px={m.spacing_sm}
                w={tab_width}
                class="h-full"
                border_r={colors.border_variant}
                accessibility_id={Dock::tab_id(panel)}
                accessibility_role={Role::Tab}
                role="tab"
                aria-label={name.clone()}
                aria-selected={selected}
                test_id="dock-tab"
                // For assistive tech and Enter: a pointer press starts the
                // drag below, which selects the tab itself.
                on:click={select(index)}
                cursor={CursorHint::Default}
                on:middle_click={close.clone()}
                on:drag={move |press: ClickEvent| {
                    let preview = tab_preview(panel, &drag_title, (tab_width, tab_height));
                    let drag = PayloadDrag::new(map.clone(), ctx.clone(), &press, preview);
                    Box::new(TabDrag::new(drag, panel, confined, from)) as Box<dyn DragHandler>
                }}
                // The strip clips to its height and regions to their
                // edges, so the ring is drawn inside the tab.
                focus_ring_offset={-Sz::FOCUS_RING_W}
                @when {selected} {
                    bg={colors.background}
                    focus_ring={Dock::tab_focus(pane)}
                    on_key={("left", select(prev))}
                    on_key={("right", select(next))}
                    on_key={("home", select(0))}
                    on_key={("end", select(count - 1))}
                    // Mac keyboards label Backspace "Delete".
                    on_key={("delete", close.clone())}
                    on_key={("backspace", close.clone())}
                }
                @for (key, action) in moves { on_key={(key.clone(), action.clone())} }
                // A click makes an inactive tab focusable without making it
                // a Tab stop: a press focuses it, and the selection then
                // hands focus to the group (`DockOutcome::focus`).
                @when {!selected && !hidden_close} {
                    hover_bg={colors.ghost_element_hover}
                    tab_stop={TabStop::disabled(0)}
                }
                @when {hidden_close} {
                    hover={move |s| s.bg(hover_fill).icon_color(hover_icon)}
                    tab_stop={TabStop::disabled(0)}
                }
            >
                // Let a long title shrink and truncate instead of pushing the
                // close button out of the tab.
                <div class="flex-1 min-w-0 overflow-clip">
                    <text class="text-sm" color={label_color} class="truncate">{name}</text>
                </div>
                {close_button}
                if data.indicator && selected {
                    <div
                        class="absolute"
                        left={0.0}
                        bottom={0.0}
                        w={tab_width}
                        h={2.0}
                        bg={colors.accent}
                        test_id="dock-tab-indicator"
                    />
                }
            </div>
        }
    }
}

/// The picture of a dragged tab: its title on a raised tab of its size,
/// which the drag replaces with the tab's measured size.
fn tab_preview(panel: PanelId, title: &str, size: (f32, f32)) -> DragPreview {
    use std::hash::{Hash, Hasher};
    let mut key = std::hash::DefaultHasher::new();
    (panel, title).hash(&mut key);
    let title = title.to_owned();
    DragPreview::new(key.finish(), size, move |theme, (width, height)| {
        let colors = &theme.colors;
        let m = &theme.metrics;
        view! {
            <div
                class="flex-row items-center"
                w={width}
                h={height}
                px={m.spacing_sm}
                bg={colors.elevated_surface}
                rounded={m.control_radius}
                shadow_preset={Shadow::POPOVER}
            >
                <div class="flex-1 min-w-0 overflow-clip">
                    <text class="text-sm truncate" color={colors.text_strong}>{title.clone()}</text>
                </div>
            </div>
        }
    })
    .hotspot(size.0 / 2.0, size.1 / 2.0)
}
