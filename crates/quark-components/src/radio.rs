//! A radio group: one choice among a few options, all visible.
//!
//! The group is a single Tab stop, on the chosen option (or the first
//! enabled one while nothing is chosen). The arrow keys move the choice to
//! the next or previous enabled option, wrapping at the ends. The group
//! emits the app's action for the new choice; the app moves focus with it,
//! to [`radio_focus_id`], so screen readers follow.

use quark::TabStop;
use quark_ui::element::{AnyElement, CursorHint, ElementContext, IntoAnyElement, RenderOnce, div};
use quark_ui::element::text;
use quark_ui::style::Styled;
use quark_ui::theme::Color;
use quark_ui::{Action, FocusId};

use crate::list_nav;

/// One option of a [`radio_group`].
#[derive(Debug, Clone, PartialEq)]
pub struct RadioOption {
    pub label: String,
    pub disabled: bool,
}

impl RadioOption {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            disabled: false,
        }
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

/// Focus target of option `index` of the radio group `id`.
pub fn radio_focus_id(id: &str, index: usize) -> FocusId {
    list_nav::item_focus(FocusId::from_key(id), index)
}

/// A radio group named `label` choosing among `options`, with `selected`
/// chosen. `on_select(i)` is the app's action for choosing option `i`.
/// `id` must be unique in the window.
pub fn radio_group(
    id: &str,
    label: impl Into<String>,
    options: Vec<RadioOption>,
    selected: Option<usize>,
    on_select: impl Fn(usize) -> Action + 'static,
) -> RadioGroup {
    RadioGroup {
        id: id.to_owned(),
        label: label.into(),
        options,
        selected,
        on_select: Box::new(on_select),
        horizontal: false,
    }
}

pub struct RadioGroup {
    id: String,
    label: String,
    options: Vec<RadioOption>,
    selected: Option<usize>,
    on_select: Box<dyn Fn(usize) -> Action>,
    horizontal: bool,
}

impl RadioGroup {
    /// Lay the options out in a row instead of a column.
    pub fn horizontal(mut self, horizontal: bool) -> Self {
        self.horizontal = horizontal;
        self
    }
}

impl RenderOnce for RadioGroup {
    fn render(self, cx: &ElementContext) -> AnyElement {
        let tc = &cx.theme.colors;
        let m = &cx.theme.metrics;
        let options = &self.options;
        let disabled = |i: usize| options[i].disabled;
        let base = FocusId::from_key(&self.id);
        // The arrows move from the focused option, or from the chosen one
        // when focus is elsewhere in the group's subtree.
        let focused = (0..options.len()).find(|&i| cx.focus == Some(list_nav::item_focus(base, i)));
        let current = focused.or(self.selected);
        let tab_stop = self
            .selected
            .filter(|&i| i < options.len() && !disabled(i))
            .or_else(|| list_nav::step(options.len(), None, 1, false, disabled));

        let mut group = div()
            .flex_col()
            .gap(m.spacing_sm)
            .accessibility_id(self.id.clone())
            .test_id("radio-group")
            .accessibility_role(accesskit::Role::RadioGroup)
            .accessibility_label(self.label.clone());
        if self.horizontal {
            group = group.flex_row().gap(m.spacing_lg);
        }
        if let Some(next) = list_nav::step(options.len(), current, 1, true, disabled) {
            let action = (self.on_select)(next);
            group = group
                .on_key("arrowdown", action.clone())
                .on_key("arrowright", action);
        }
        if let Some(previous) = list_nav::step(options.len(), current, -1, true, disabled) {
            let action = (self.on_select)(previous);
            group = group
                .on_key("arrowup", action.clone())
                .on_key("arrowleft", action);
        }

        let size = (m.ui_font_size * 1.125).round();
        let dot = (size * 0.45).round();
        for (i, option) in options.iter().enumerate() {
            let checked = self.selected == Some(i);
            let (ring, fill) = match (option.disabled, checked) {
                (true, _) => (tc.border_variant, tc.text_muted),
                (false, true) => (tc.accent, tc.accent),
                (false, false) => (tc.border, Color::TRANSPARENT),
            };
            let circle = div()
                .flex_shrink_0()
                .items_center()
                .justify_center()
                .w(size)
                .h(size)
                .rounded(size / 2.0)
                .border(ring)
                .when(checked, |c| {
                    c.child(div().w(dot).h(dot).rounded(dot / 2.0).bg(fill))
                });
            let mut item = div()
                .flex_row()
                .items_center()
                .gap(m.spacing_sm)
                .rounded(m.control_radius)
                .accessibility_id(format!("{}-{i}", self.id))
                .test_id("radio")
                .accessibility_role(accesskit::Role::RadioButton)
                .accessibility_label(option.label.clone())
                .accessibility_toggled(checked)
                .accessibility_disabled(option.disabled)
                .child(circle)
                .child(text(option.label.clone()).text_sm().color(if option.disabled {
                    tc.text_muted
                } else {
                    tc.text
                }));
            if !option.disabled {
                item = item
                    .focus_ring(list_nav::item_focus(base, i))
                    .cursor(CursorHint::Pointer)
                    .on_click((self.on_select)(i));
                if tab_stop != Some(i) {
                    item = item.tab_stop(TabStop::disabled(0));
                }
            }
            group = group.child(item);
        }
        group.into_any()
    }
}
