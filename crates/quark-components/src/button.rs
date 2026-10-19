use quark::{Props, view};

use crate::Child;
use quark_ui::design::{Alpha, Ico, Rad, Sp};
use quark_ui::element::CursorHint;
use quark_ui::element::*;
use quark_ui::style::{StyleOverride, Styled};
use quark_ui::theme::{Color, ControlMetrics};
use quark_ui::{Action, FocusId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonStyle {
    Filled,
    Subtle,
    Ghost,
    Danger,
    Outline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonSize {
    Default,
    Compact,
}

/// A button. Build it with [`Button::new`] and its builder methods, or in
/// `view!` from its props:
/// `<Button on:click={msg} icon={lucide::SEND} variant={ButtonStyle::Filled}>"Send"</Button>`.
#[derive(Props)]
pub struct Button {
    #[prop(optional)]
    icon: Option<&'static str>,
    #[prop(optional, into)]
    label: Option<String>,
    #[prop(into)]
    on_click: Action,
    #[prop(default = ButtonStyle::Ghost)]
    variant: ButtonStyle,
    #[prop(default = ButtonSize::Default)]
    size: ButtonSize,
    #[prop(default)]
    active: bool,
    #[prop(default)]
    disabled: bool,
    #[prop(optional, into)]
    tooltip: Option<std::sync::Arc<str>>,
    #[prop(optional)]
    fixed_size: Option<f32>,
    /// Sizes for this button over the theme's
    /// [`ComponentMetrics::button`](quark_ui::theme::ComponentMetrics)
    /// recipe, field by field.
    #[prop(optional)]
    metrics: Option<ControlMetrics>,
    /// The button's focus target; see [`Button::focus_id`].
    #[prop(optional)]
    focus_id: Option<FocusId>,
    /// Content after the icon and label. When it is all text and there is
    /// no label or tooltip, it is also the button's accessible name.
    #[prop(default)]
    children: Vec<Child>,
}

impl Button {
    pub fn new(action: impl Into<Action>) -> Self {
        Self::builder().on_click(action).build()
    }

    pub fn icon(mut self, icon: &'static str) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn style(mut self, style: ButtonStyle) -> Self {
        self.variant = style;
        self
    }

    pub fn size(mut self, size: ButtonSize) -> Self {
        self.size = size;
        self
    }

    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn tooltip(mut self, text: impl Into<std::sync::Arc<str>>) -> Self {
        self.tooltip = Some(text.into());
        self
    }

    pub fn fixed_size(mut self, size: f32) -> Self {
        self.fixed_size = Some(size);
        self
    }

    pub fn metrics(mut self, metrics: ControlMetrics) -> Self {
        self.metrics = Some(metrics);
        self
    }

    /// Lets the app move focus to the button.
    pub fn focus_id(mut self, id: FocusId) -> Self {
        self.focus_id = Some(id);
        self
    }
}

impl RenderOnce for Button {
    fn render(self, cx: &ElementContext) -> AnyElement {
        let theme = cx.theme;
        let tc = &theme.colors;
        let scale = theme.metrics.ui_scale();

        let (icon_size, unscaled_px, unscaled_py, recipe) = match self.size {
            ButtonSize::Default => (Ico::BUTTON_DEFAULT, Sp::MD, Sp::XS, theme.components.button),
            ButtonSize::Compact => (
                Ico::BUTTON_COMPACT,
                Sp::SM,
                Sp::XXS,
                theme.components.button_compact,
            ),
        };
        let m = self.metrics.unwrap_or_default().or(recipe);
        let icon_size = m.icon_size.unwrap_or(icon_size);
        let unscaled_px = m.padding_x.unwrap_or(unscaled_px);
        let unscaled_py = m.padding_y.unwrap_or(unscaled_py);
        let gap = m.gap.unwrap_or(Sp::SM);

        let (bg, hover_bg, icon_color, text_color) = match self.variant {
            ButtonStyle::Filled => (tc.accent, tc.accent_strong, tc.on_accent, tc.on_accent),
            ButtonStyle::Subtle => (
                tc.element_background,
                tc.element_hover,
                tc.text_muted,
                tc.text,
            ),
            ButtonStyle::Ghost => {
                if self.active {
                    (
                        tc.ghost_element_active,
                        tc.ghost_element_hover,
                        tc.text,
                        tc.text,
                    )
                } else {
                    (
                        Color::TRANSPARENT,
                        tc.ghost_element_hover,
                        tc.text_muted,
                        tc.text_muted,
                    )
                }
            }
            ButtonStyle::Danger => (
                tc.status_error.with_alpha(Alpha::TINT),
                tc.status_error.with_alpha(Alpha::DIM),
                tc.status_error,
                tc.status_error,
            ),
            ButtonStyle::Outline => (
                Color::TRANSPARENT,
                tc.ghost_element_hover,
                tc.text_muted,
                tc.text,
            ),
        };
        // (rest, hover) border colors.
        let border =
            (self.variant == ButtonStyle::Outline).then_some((tc.border_variant, tc.border));
        // The border takes layout space, so take it out of the padding.
        let border_inset = if border.is_some() { 1.0 / scale } else { 0.0 };

        let disabled = self.disabled;
        let (bg, icon_color, text_color) = if disabled {
            (
                bg,
                icon_color.with_alpha(Alpha::MUTED),
                text_color.with_alpha(Alpha::MUTED),
            )
        } else {
            (bg, icon_color, text_color)
        };

        let icon_only = self.icon.is_some() && self.label.is_none();
        let actual_px = if icon_only { unscaled_py } else { unscaled_px };
        // A fixed size is the caller's own square: the theme's recipe
        // height does not resize it, this button's own metrics do.
        let own_height = self.metrics.and_then(|m| m.height);
        let fixed = self.fixed_size.map(|s| own_height.unwrap_or(s));
        let icon = self.icon;
        let action = if disabled {
            quark_ui::element::NoopAction.into()
        } else {
            self.on_click
        };
        let tooltip_text = self.tooltip;
        let focus_id = self.focus_id.filter(|_| !disabled);
        let cursor = if disabled {
            CursorHint::Default
        } else {
            CursorHint::Pointer
        };

        let label_text = self.label;
        let children = self.children;
        // A button hides its descendants' text from screen readers, so
        // text children have to become its name explicitly.
        let accessibility_label = label_text
            .clone()
            .or_else(|| tooltip_text.as_deref().map(str::to_owned))
            .or_else(|| Child::text_of(&children))
            .unwrap_or_default();
        let accessibility_id = format!("button:{action:?}:{accessibility_label}");
        let hover_icon = tc.text;
        // One `hover` for fill and border: each `hover_*` helper replaces
        // the hover style the last one set.
        let hover = move |s: StyleOverride| {
            let s = if icon_only {
                s.icon_color(hover_icon)
            } else {
                s.bg(hover_bg)
            };
            match border {
                Some((_, strong)) => s.border_color(strong),
                None => s,
            }
        };

        view! { scale,
            <div
                class="shrink-0"
                bg={bg}
                cursor={cursor}
                id={accessibility_id.clone()}
                test-id="button"
                role="button"
                aria-label={accessibility_label}
                accessibility_id={accessibility_id}
                aria-selected={self.active}
                aria-disabled={disabled}
                @when {!disabled} { on:click={action} }
                @when {let Some(target) = focus_id} { focus_ring={target} }
                @when {let Some((rest, _)) = border} { border={rest} }
                @when {let Some(fixed) = fixed} {
                    items_center
                    justify_center
                    w={fixed}
                    h={fixed}
                    rounded={m.radius.unwrap_or(Rad::SM)}
                }
                @when {fixed.is_none()} {
                    class="flex-row items-center"
                    gap={gap}
                    px={actual_px - border_inset}
                    py={unscaled_py - border_inset}
                    rounded={m.radius.unwrap_or(Rad::XL)}
                }
                @when {let (None, Some(height)) = (fixed, m.height)} {
                    h={height}
                    pt={0.0}
                    pb={0.0}
                }
                @when {!disabled} { hover={hover} }
                tooltip={?tooltip_text.clone()}
            >
                if icon.is_some() {
                    <icon svg={icon.unwrap()} size={icon_size} color={icon_color} />
                }
                if let Some(label) = label_text {
                    <text
                        class="font-medium"
                        color={text_color}
                        @when {self.size == ButtonSize::Default} { class="text-sm" }
                        @when {self.size == ButtonSize::Compact} { class="text-xs" }
                        size={?m.font_size}
                    >
                        {label}
                    </text>
                }
                {...children.into_iter().map(Child::into_any)}
            </div>
        }
    }
}
