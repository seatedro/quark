//! A segmented control: a row of mutually exclusive choices.
//!
//! Like a radio group it is one Tab stop, on the selected segment, and Left
//! and Right move the selection (wrapping), emitting the new segment's
//! action. The app moves focus to [`segmented_focus_id`] with it.

use quark::{TabStop, view};
use quark_ui::Action;
use quark_ui::FocusId;
use quark_ui::design::{Rad, Sp};
use quark_ui::element::*;
use quark_ui::style::Styled;

use crate::list_nav;

pub struct SegmentedItem {
    pub label: String,
    pub action: Action,
    pub selected: bool,
    pub tooltip_text: Option<String>,
}

impl SegmentedItem {
    pub fn new(label: impl Into<String>, action: impl Into<Action>, selected: bool) -> Self {
        Self {
            label: label.into(),
            action: action.into(),
            selected,
            tooltip_text: None,
        }
    }

    pub fn tooltip(mut self, text: impl Into<String>) -> Self {
        self.tooltip_text = Some(text.into());
        self
    }
}

/// How much faster a segment's side padding gives way than its label, so
/// labels truncate only once the padding is gone.
const PAD_SHRINK: f32 = 1000.0;

/// The id a [`SegmentedControl`] uses until [`SegmentedControl::id`] sets
/// one.
const DEFAULT_ID: &str = "segmented-control";

/// Focus target of segment `index` of the segmented control `id`.
pub fn segmented_focus_id(id: &str, index: usize) -> FocusId {
    list_nav::item_focus(FocusId::from_key(id), index)
}

pub struct SegmentedControl {
    id: String,
    items: Vec<SegmentedItem>,
}

impl SegmentedControl {
    pub fn new(items: Vec<SegmentedItem>) -> Self {
        Self {
            id: DEFAULT_ID.to_owned(),
            items,
        }
    }

    /// A window-unique id; needed when a window shows more than one.
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }
}

impl RenderOnce for SegmentedControl {
    fn render(self, cx: &ElementContext) -> AnyElement {
        let tc = &cx.theme.colors;
        let scale = cx.theme.metrics.ui_scale();
        // Rounded so the inset is even on every side.
        let recipe = cx.theme.components.segmented;
        let scaled = |v: Option<f32>, default: f32| (v.unwrap_or(default) * scale).round();
        let inset = scaled(recipe.gap, Sp::XXS);
        let radius = scaled(recipe.radius, Rad::XL);
        let pad_x = scaled(recipe.padding_x, Sp::MD);
        let pad_y = scaled(recipe.padding_y, Sp::XXS);
        let height = recipe.height.map(|h| (h * scale).round());
        let font_size = recipe.font_size.map(|s| s * scale);
        let base = FocusId::from_key(&self.id);
        let len = self.items.len();
        let selected = self.items.iter().position(|item| item.selected);
        let focused = (0..len).find(|&i| cx.focus == Some(list_nav::item_focus(base, i)));
        let current = focused.or(selected);
        let tab_stop = selected.or((len > 0).then_some(0));

        view! {
            <div
                class="flex-row min-w-0 items-center overflow-hidden"
                id={self.id.clone()}
                test_id="segmented-control"
                role="radiogroup"
                accessibility_role={accesskit::Role::RadioGroup}
                accessibility_id={self.id.clone()}
                bg={tc.element_background}
                border={tc.border_variant}
                rounded={radius}
                p={inset}
                gap={inset}
                @when {let Some(to) = list_nav::step(len, current, 1, true, |_| false)} {
                    on_key={("arrowright", self.items[to].action.clone())}
                }
                @when {let Some(to) = list_nav::step(len, current, -1, true, |_| false)} {
                    on_key={("arrowleft", self.items[to].action.clone())}
                }
            >
                for (i, item) in self.items.into_iter().enumerate() {
                    // Segments keep their natural widths and give way in a
                    // narrow container: first their side padding, then their
                    // labels, which truncate rather than spill past it.
                    <div
                        class="flex-auto min-w-0 flex-row items-center justify-center"
                        id={format!("segmented:{:?}:{}", item.action, item.label)}
                        key={item.label.clone()}
                        test_id="segmented-item"
                        role="radio"
                        py={pad_y}
                        rounded={(radius - inset).max(0.0)}
                        focus_ring={list_nav::item_focus(base, i)}
                        accessibility_role={accesskit::Role::RadioButton}
                        accessibility_id={format!("segmented:{:?}:{}", item.action, item.label)}
                        aria-label={item.label.clone()}
                        aria-selected={item.selected}
                        aria-checked={item.selected}
                        class="cursor-pointer"
                        on:click={item.action}
                        @when {height.is_some()} { h={height.unwrap()} pt={0.0} pb={0.0} }
                        @when {item.selected} { bg={tc.element_hover} }
                        // One `hover` for both: each `hover_*` helper
                        // replaces the last one's style.
                        @when {!item.selected} {
                            hover={|s| s.bg(tc.ghost_element_hover).text_color(tc.text)}
                        }
                        @when {tab_stop != Some(i)} { tab_stop={TabStop::disabled(0)} }
                        @when {let Some(tip) = item.tooltip_text} { tooltip={tip} }
                    >
                        <div w={pad_x} flex_shrink_val={PAD_SHRINK} />
                        <text
                            class="text-sm font-medium truncate"
                            @when {font_size.is_some()} { size={font_size.unwrap()} }
                            color={if item.selected {
                                tc.text
                            } else {
                                tc.text_muted
                            }}
                        >
                            {item.label}
                        </text>
                        <div w={pad_x} flex_shrink_val={PAD_SHRINK} />
                    </div>
                }
            </div>
        }
    }
}
