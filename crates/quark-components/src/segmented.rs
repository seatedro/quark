//! A segmented control: a row of mutually exclusive choices.
//!
//! Like a radio group it is one Tab stop, on the selected segment, and Left
//! and Right move the selection (wrapping), emitting the new segment's
//! action. The app moves focus to [`segmented_focus_id`] with it.

use quark::{SemanticRole, TabStop};
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
        let base = FocusId::from_key(&self.id);
        let len = self.items.len();
        let selected = self.items.iter().position(|item| item.selected);
        let focused = (0..len).find(|&i| cx.focus == Some(list_nav::item_focus(base, i)));
        let current = focused.or(selected);
        let tab_stop = selected.or((len > 0).then_some(0));

        let mut row = div()
            .flex_row()
            .flex_shrink_0()
            .items_center()
            .overflow_hidden()
            .id(self.id.clone())
            .test_id("segmented-control")
            .semantic_role(SemanticRole::RadioGroup)
            .accessibility_role(accesskit::Role::RadioGroup)
            .accessibility_id(self.id.clone())
            .bg(tc.element_background)
            .rounded(Rad::XL * scale)
            .p(Sp::XXS * scale)
            .gap(Sp::XXS * scale);
        for (key, delta) in [("arrowright", 1), ("arrowleft", -1)] {
            if let Some(to) = list_nav::step(len, current, delta, true, |_| false) {
                row = row.on_key(key, self.items[to].action.clone());
            }
        }

        for (i, item) in self.items.into_iter().enumerate() {
            let mut segment = div()
                .flex_1()
                .items_center()
                .justify_center()
                .id(format!("segmented:{:?}:{}", item.action, item.label))
                .key(item.label.clone())
                .test_id("segmented-item")
                .semantic_role(SemanticRole::RadioButton)
                .px(Sp::MD * scale)
                .py(Sp::XXS * scale)
                .rounded(Rad::LG * scale)
                .focus_ring(list_nav::item_focus(base, i))
                .accessibility_role(accesskit::Role::RadioButton)
                .accessibility_id(format!("segmented:{:?}:{}", item.action, item.label))
                .accessibility_label(item.label.clone())
                .accessibility_selected(item.selected)
                .accessibility_toggled(item.selected)
                .cursor(CursorHint::Pointer)
                .on_click(item.action);
            if item.selected {
                segment = segment.bg(tc.ghost_element_hover);
            } else {
                segment = segment
                    .hover_bg(tc.ghost_element_hover)
                    .hover_text_color(tc.text);
            }
            if tab_stop != Some(i) {
                segment = segment.tab_stop(TabStop::disabled(0));
            }
            if let Some(tip) = item.tooltip_text {
                segment = segment.tooltip(tip);
            }
            row = row.child(segment.child(text(item.label).text_sm().medium().color(
                if item.selected {
                    tc.text
                } else {
                    tc.text_muted
                },
            )));
        }
        row.into_any()
    }
}
