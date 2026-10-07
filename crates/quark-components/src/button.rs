use quark::{Props, view};

use quark_ui::Action;
use quark_ui::design::{Alpha, Ico, Rad, Sp};
use quark_ui::element::CursorHint;
use quark_ui::element::*;
use quark_ui::style::Styled;
use quark_ui::theme::Color;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonStyle {
    Filled,
    Subtle,
    Ghost,
    Danger,
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
    /// Content after the icon and label; text here names the button for
    /// screen readers when it has no label or tooltip.
    #[prop(default)]
    children: Vec<AnyElement>,
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
}

impl RenderOnce for Button {
    fn render(self, cx: &ElementContext) -> AnyElement {
        let theme = cx.theme;
        let tc = &theme.colors;
        let scale = theme.metrics.ui_scale();

        let (icon_size, unscaled_px, unscaled_py) = match self.size {
            ButtonSize::Default => (Ico::BUTTON_DEFAULT, Sp::MD, Sp::XS),
            ButtonSize::Compact => (Ico::BUTTON_COMPACT, Sp::SM, Sp::XXS),
        };

        let (bg, hover_bg, icon_color, text_color) = match self.variant {
            ButtonStyle::Filled => (tc.accent, tc.accent_strong, tc.text_strong, tc.text_strong),
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
        };

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
        let fixed = self.fixed_size.map(|s| (s * scale).round());
        let icon = self.icon;
        let action = if disabled {
            quark_ui::element::NoopAction.into()
        } else {
            self.on_click
        };
        let tooltip_text = self.tooltip;
        let cursor = if disabled {
            CursorHint::Default
        } else {
            CursorHint::Pointer
        };

        let label_text = self.label;
        let children = self.children;
        // Without a label or tooltip, text children name the button (a
        // button's descendant text is its accessible name); with neither,
        // the name is empty, as before children existed.
        let accessibility_label = label_text
            .clone()
            .or_else(|| tooltip_text.as_deref().map(str::to_owned))
            .or_else(|| children.is_empty().then(String::new));
        let accessibility_id = format!(
            "button:{action:?}:{}",
            accessibility_label.as_deref().unwrap_or_default()
        );

        let label_el = label_text.map(|label| {
            let mut txt = text(label).medium().color(text_color);
            match self.size {
                ButtonSize::Default => txt = txt.text_sm(),
                ButtonSize::Compact => txt = txt.text_xs(),
            }
            txt
        });

        view! { scale,
            <div class="shrink-0" bg={bg}
                 cursor={cursor}
                 id={accessibility_id.clone()}
                 test-id="button"
                 role="button"
                 @when {accessibility_label.is_some()} {
                     aria-label={accessibility_label.unwrap_or_default()}
                 }
                 accessibility_id={accessibility_id}
                 aria-selected={self.active}
                 aria-disabled={disabled}
                 @when { !disabled } { on:click={action} }
                 @when { fixed.is_some() } {
                     items_center justify_center
                     w={fixed.unwrap()} h={fixed.unwrap()}
                     rounded={Rad::SM}
                 }
                 @when { fixed.is_none() } {
                     class="flex-row items-center"
                     gap={Sp::SM} px={actual_px} py={unscaled_py}
                     rounded={Rad::XL}
                 }
                 @when { !disabled && icon_only } { hover_icon_color={tc.text} }
                 @when { !disabled && !icon_only } { hover_bg={hover_bg} }
                 @when { tooltip_text.is_some() } {
                     tooltip={tooltip_text.clone().unwrap_or_default()}
                 }>
                if icon.is_some() {
                    <icon svg={icon.unwrap()} size={icon_size} color={icon_color} />
                }
                {?label_el}
                {...children}
            </div>
        }
    }
}
