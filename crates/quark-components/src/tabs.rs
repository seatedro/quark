//! A row of tabs ([`TabBar`]) and a segmented switcher ([`SegmentedTabs`])
//! over the same [`TabItem`]s. The app owns which tab is active and the
//! list itself: tabs only emit their actions.
//!
//! A `TabBar` tab given [`TabItem::on_close`] shows a close button named
//! "Close <label>", and closes on a middle click or Delete while it has
//! focus. Each emits the close action once, without selecting the tab.
//! `SegmentedTabs` ignores `on_close`.

use quark::view;

use quark_ui::design::{Rad, Shadow, Sp, Sz};
use quark_ui::element::{
    AnyElement, ElementContext, IntoAnyElement, RenderOnce, div, svg_icon, text,
};
use quark_ui::icons::lucide;
use quark_ui::style::Styled;
use quark_ui::theme::Color;
use quark_ui::{Action, FocusId};

pub struct TabItem {
    label: String,
    action: Action,
    active: bool,
    count: Option<String>,
    icon: Option<&'static str>,
    on_close: Option<Action>,
    id: Option<String>,
}

impl TabItem {
    pub fn new(label: impl Into<String>, action: impl Into<Action>) -> Self {
        Self {
            label: label.into(),
            action: action.into(),
            active: false,
            count: None,
            icon: None,
            on_close: None,
            id: None,
        }
    }

    /// Make the tab closable in a [`TabBar`]: `action` is emitted by its
    /// close button, a middle click, or Delete while it has focus. The app
    /// removes the tab and picks the next active one.
    pub fn on_close(mut self, action: impl Into<Action>) -> Self {
        self.on_close = Some(action.into());
        self
    }

    /// A stable identity for the tab, unique in its bar, so its focus and
    /// accessibility node survive relabeling and reordering. Without one,
    /// the tab is known by its action and label.
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Focus target of the [`TabBar`] tab given [`Self::id`] `id`, to move
    /// focus to it, say onto the next tab after closing the focused one.
    pub fn focus_id(id: &str) -> FocusId {
        FocusId::from_key(&Self::key_for(id))
    }

    /// Focus target of the close button of the tab given [`Self::id`] `id`.
    pub fn close_focus_id(id: &str) -> FocusId {
        FocusId::from_key(&format!("{}:close", Self::key_for(id)))
    }

    fn key_for(id: &str) -> String {
        format!("tab:{id}")
    }

    /// The tab's node identity in a [`TabBar`].
    fn key(&self) -> String {
        match &self.id {
            Some(id) => Self::key_for(id),
            None => format!("tab:{:?}:{}", self.action, self.label),
        }
    }

    pub fn active(mut self, a: bool) -> Self {
        self.active = a;
        self
    }

    pub fn count(mut self, c: impl Into<String>) -> Self {
        self.count = Some(c.into());
        self
    }

    pub fn icon(mut self, svg: &'static str) -> Self {
        self.icon = Some(svg);
        self
    }
}

pub struct TabBar {
    items: Vec<TabItem>,
    fill: bool,
    label: Option<String>,
}

pub fn tab_bar(items: Vec<TabItem>) -> TabBar {
    TabBar {
        items,
        fill: false,
        label: None,
    }
}

impl TabBar {
    pub fn fill(mut self) -> Self {
        self.fill = true;
        self
    }

    /// The tab list's accessible name, which tells it apart from other tab
    /// lists around it (a dock's, say).
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }
}

impl RenderOnce for TabBar {
    fn render(self, cx: &ElementContext) -> AnyElement {
        let tc = &cx.theme.colors;
        let fill = self.fill;

        view! {
            <div
                class="flex-row items-end"
                border_b={tc.border_variant}
                id="tab-bar"
                test-id="tab-bar"
                role="tablist"
                accessibility_id={"tab-bar"}
                @when {let Some(label) = self.label} { aria-label={label} }
            >
                for item in self.items {
                    {Self::tab(item, fill, cx)}
                }
            </div>
        }
    }
}

impl TabBar {
    fn tab(item: TabItem, fill: bool, cx: &ElementContext) -> AnyElement {
        let tc = &cx.theme.colors;
        let m = &cx.theme.metrics;
        let icon_size = m.ui_small_font_size;
        let key = item.key();
        let close_label = item.on_close.as_ref().map(|_| {
            quark_ui::i18n::tr_args(
                "quark-close-named",
                [("name", quark_ui::i18n::Arg::Text(&item.label))],
            )
        });
        view! {
            <div
                class="flex-col items-center"
                id={key.clone()}
                key={key.clone()}
                test-id="tab"
                role="tab"
                on:click={item.action.clone()}
                accessibility_id={key.clone()}
                aria-label={item.label.clone()}
                aria-selected={item.active}
                // An explicit target, so a press focuses the tab (as a
                // Dock's does) and Delete then closes it.
                focus_ring={FocusId::from_key(&key)}
                @when {let Some(close) = &item.on_close} {
                    // Mac keyboards label Backspace "Delete".
                    on:middle_click={close.clone()}
                    on_key={("delete", close.clone())}
                    on_key={("backspace", close.clone())}
                }
                @when {!item.active} { hover_bg={tc.ghost_element_hover} }
                @when {fill} { flex_1 }
            >
                <div
                    class="flex-row items-center"
                    gap={m.spacing_xs}
                    px={m.spacing_md}
                    py={m.spacing_sm}
                >
                    if let Some(svg) = item.icon {
                        <icon
                            svg={svg}
                            size={icon_size}
                            color={if item.active {
                                tc.accent
                            } else {
                                tc.text_muted
                            }}
                        />
                    }
                    <text
                        class="text-sm"
                        color={if item.active {
                            tc.text_strong
                        } else {
                            tc.text_muted
                        }}
                        @when {item.active} { medium }
                    >
                        {item.label}
                    </text>
                    if let Some(count_text) = item.count {
                        <div
                            px={m.spacing_xs}
                            py={Sz::TAB_BADGE_PY}
                            bg={tc.element_background}
                            rounded={Rad::XL}
                        >
                            <text class="text-xs" color={tc.text_muted}>{count_text}</text>
                        </div>
                    }
                    if let (Some(close), Some(label)) = (item.on_close, close_label) {
                        // Its own node, so assistive tech and Tab reach it
                        // apart from the tab, and a click on it is not a
                        // click on the tab.
                        <div
                            class="flex-none items-center justify-center"
                            rounded={m.control_radius * 0.5}
                            p={2.0}
                            hover_bg={tc.ghost_element_hover}
                            id={format!("{key}:close")}
                            accessibility_id={format!("{key}:close")}
                            test-id="tab-close"
                            accessibility_role={accesskit::Role::Button}
                            aria-label={label}
                            on:click={close}
                        >
                            <icon svg={lucide::X} size={icon_size} color={tc.text_muted} />
                        </div>
                    }
                </div>
                <div
                    w_full
                    h={Sz::TAB_INDICATOR_H}
                    bg={if item.active {
                        tc.accent
                    } else {
                        Color::TRANSPARENT
                    }}
                />
            </div>
        }
    }
}

pub struct SegmentedTabs {
    items: Vec<TabItem>,
}

pub fn segmented_tabs(items: Vec<TabItem>) -> SegmentedTabs {
    SegmentedTabs { items }
}

impl RenderOnce for SegmentedTabs {
    fn render(self, cx: &ElementContext) -> AnyElement {
        let tc = &cx.theme.colors;
        let m = &cx.theme.metrics;
        let scale = m.ui_scale();
        let seg_gap = (Sp::XXS * scale).round();
        let inner_radius = m.control_radius - seg_gap;

        view! {
            <div
                class="flex-row items-center"
                id="segmented-tabs"
                test-id="segmented-tabs"
                role="tablist"
                accessibility_id={"segmented-tabs"}
                gap={seg_gap}
                p={seg_gap}
                bg={tc.element_background}
                rounded={m.control_radius}
            >
                for item in self.items {
                    <div
                        class="flex-row flex-1 items-center justify-center"
                        id={format!("segmented-tab:{:?}:{}", item.action, item.label)}
                        key={item.label.clone()}
                        test-id="segmented-tab"
                        role="tab"
                        px={m.spacing_md}
                        py={m.spacing_xs}
                        rounded={inner_radius}
                        on:click={item.action.clone()}
                        accessibility_id={format!("segmented-tab:{:?}:{}", item.action, item.label)}
                        aria-label={item.label.clone()}
                        aria-selected={item.active}
                        @when {item.active} { bg={tc.surface} shadow_preset={Shadow::SUBTLE} }
                        @when {!item.active} { hover_bg={tc.ghost_element_hover} }
                    >
                        <text
                            class="text-sm font-medium"
                            color={if item.active {
                                tc.text_strong
                            } else {
                                tc.text_muted
                            }}
                        >
                            {item.label}
                        </text>
                    </div>
                }
            </div>
        }
    }
}
