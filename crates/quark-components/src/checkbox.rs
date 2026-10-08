use quark::{Props, view};

use quark_ui::Action;
use quark_ui::animation::{Curve, Motion, Prop};
use quark_ui::design::{Shadow, Sp, Sz};
use quark_ui::element::{
    AnyElement, ElementContext, IntoAnyElement, RenderOnce, div, svg_icon, text,
};
use quark_ui::icons::lucide;
use quark_ui::style::Styled;
use quark_ui::theme::Color;

/// A checkbox: `checkbox(true).label("Wrap")`, or in `view!`
/// `<Checkbox checked={wrap} label="Wrap" on:toggle={Msg::ToggleWrap} />`.
#[derive(Props)]
pub struct Checkbox {
    checked: bool,
    #[prop(optional, into)]
    label: Option<String>,
    #[prop(optional, into)]
    on_toggle: Option<Action>,
    #[prop(default)]
    disabled: bool,
}

pub fn checkbox(checked: bool) -> Checkbox {
    Checkbox::builder().checked(checked).build()
}

impl Checkbox {
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn on_toggle(mut self, action: impl Into<Action>) -> Self {
        self.on_toggle = Some(action.into());
        self
    }

    pub fn disabled(mut self, d: bool) -> Self {
        self.disabled = d;
        self
    }
}

impl RenderOnce for Checkbox {
    fn render(self, cx: &ElementContext) -> AnyElement {
        let tc = &cx.theme.colors;
        let m = &cx.theme.metrics;
        let scale = m.ui_scale();
        let size = (m.ui_font_size * 1.125).round();
        let icon_size = size - Sp::XS * scale;
        let radius = (m.control_radius * 0.5).max(Sz::CHECKBOX_RAD_MIN * scale);

        let (box_bg, box_border, check_color) = if self.disabled {
            (tc.element_background, tc.border_variant, tc.text_muted)
        } else if self.checked {
            (tc.accent, tc.accent, Color::rgba(255, 255, 255, 255))
        } else {
            (Color::TRANSPARENT, tc.border, tc.icon)
        };

        let can_hover = !self.disabled && !self.checked;
        let check_box = view! {
            <div class="shrink-0 items-center justify-center"
                 w={size} h={size}
                 bg={box_bg} border={box_border} rounded={radius}
                 @when {can_hover} { hover_bg={tc.ghost_element_hover} }>
                if self.checked {
                    <icon svg={lucide::CHECK} size={icon_size} color={check_color} />
                }
            </div>
        };

        let label_text = self.label;
        let accessibility_label = label_text.clone().unwrap_or_else(|| "Checkbox".to_owned());
        let click_action = self.on_toggle.filter(|_| !self.disabled);
        let accessibility_id = format!("checkbox:{:?}:{accessibility_label}", click_action);
        let label_color = if self.disabled {
            tc.text_muted
        } else {
            tc.text
        };

        view! {
            <div class="flex-row items-center" gap={m.spacing_sm}
                 id={accessibility_id.clone()}
                 key={accessibility_label.clone()}
                 test-id="checkbox"
                 role="checkbox"
                 accessibility_id={accessibility_id}
                 aria-label={accessibility_label}
                 aria-checked={self.checked}
                 aria-disabled={self.disabled}
                 @when {click_action.is_some()} { on:click={click_action.unwrap()} }>
                {check_box}
                if let Some(label_text) = label_text {
                    <text class="text-sm" color={label_color}>{label_text}</text>
                }
            </div>
        }
    }
}

/// An on/off switch. Space or Enter flips it when focused, and screen
/// readers hear a switch with its on or off state. The thumb slides between
/// the ends.
#[derive(Props)]
pub struct Switch {
    on: bool,
    #[prop(optional, into)]
    label: Option<String>,
    #[prop(optional, into)]
    on_toggle: Option<Action>,
    #[prop(default)]
    disabled: bool,
}

pub fn switch(on: bool) -> Switch {
    Switch::builder().on(on).build()
}

/// The switch's earlier name.
pub type Toggle = Switch;

/// [`switch`] by its earlier name.
pub fn toggle(on: bool) -> Switch {
    switch(on)
}

impl Switch {
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn on_toggle(mut self, action: impl Into<Action>) -> Self {
        self.on_toggle = Some(action.into());
        self
    }

    pub fn disabled(mut self, d: bool) -> Self {
        self.disabled = d;
        self
    }
}

impl RenderOnce for Switch {
    fn render(self, cx: &ElementContext) -> AnyElement {
        let tc = &cx.theme.colors;
        let m = &cx.theme.metrics;
        let scale = m.ui_scale();
        let thumb_inset = Sp::XXS * scale;

        let track_w = (m.ui_font_size * 2.25).round();
        let track_h = (m.ui_font_size * 1.25).round();
        let thumb_size = track_h - Sp::XS * scale;
        let travel = track_w - thumb_size - 2.0 * thumb_inset;

        let (track_bg, thumb_bg) = if self.disabled {
            (tc.element_background, tc.text_muted)
        } else if self.on {
            (tc.accent, Color::rgba(255, 255, 255, 255))
        } else {
            (tc.element_background, tc.icon)
        };
        let hover_bg = if self.on {
            tc.accent_strong
        } else {
            tc.element_hover
        };

        let label_text = self.label;
        let accessibility_label = label_text.clone().unwrap_or_else(|| "Switch".to_owned());
        let click_action = self.on_toggle.filter(|_| !self.disabled);
        let accessibility_id = format!("switch:{:?}:{accessibility_label}", click_action);

        let label_color = if self.disabled {
            tc.text_muted
        } else {
            tc.text
        };
        // Keyed by the switch, so the slide animates across frames.
        let thumb_key = format!("{accessibility_id}:thumb");
        view! {
            <div class="flex-row items-center" gap={m.spacing_sm} rounded={track_h / 2.0}
                 id={accessibility_id.clone()} key={accessibility_label.clone()} test_id="switch"
                 role="switch" accessibility_role={accesskit::Role::Switch}
                 accessibility_id={accessibility_id} aria-label={accessibility_label}
                 aria-checked={self.on} aria-disabled={self.disabled}
                 @when {let Some(action) = click_action} { on:click={action} }>
                <div class="shrink-0" w={track_w} h={track_h} bg={track_bg} rounded={track_h / 2.0}
                     @when {!self.disabled} { hover_bg={hover_bg} }>
                    <div class="absolute" top={thumb_inset} left={thumb_inset} w={thumb_size}
                         h={thumb_size} rounded={thumb_size / 2.0} bg={thumb_bg}
                         shadow_preset={Shadow::SUBTLE} key={thumb_key}
                         translate={(if self.on { travel } else { 0.0 }, 0.0)}
                         transition={(Prop::Transform, Motion::tween(140, Curve::EaseOutCubic))} />
                </div>
                if let Some(label) = label_text {
                    <text class="text-sm" color={label_color}>{label}</text>
                }
            </div>
        }
    }
}
