use quark::view;

use quark_ui::design::{Shadow, Sp};
use quark_ui::element::*;
use quark_ui::style::Styled;
use quark_ui::theme::{Theme, scaled_or};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TooltipSide {
    Top,
    Bottom,
    Left,
    Right,
}

pub fn tooltip_layer(
    content: &str,
    x: f32,
    y: f32,
    side: TooltipSide,
    theme: &Theme,
) -> AnyElement {
    let tc = &theme.colors;
    let m = &theme.metrics;
    let scale = m.ui_scale();
    let recipe = theme.components.tooltip;
    let px = scaled_or(recipe.padding_x, scale, m.spacing_sm);
    let py = scaled_or(recipe.padding_y, scale, m.spacing_xs);
    let radius = scaled_or(recipe.radius, scale, m.control_radius - Sp::XXS);
    let font = recipe
        .font_size
        .map_or(m.ui_small_font_size - 1.0, |s| s * scale);

    let (offset_x, offset_y) = match side {
        TooltipSide::Top => (0.0, -(m.spacing_sm + Sp::XS)),
        TooltipSide::Bottom => (0.0, m.spacing_sm + Sp::XS),
        TooltipSide::Left => (-(m.spacing_sm + Sp::XS), 0.0),
        TooltipSide::Right => (m.spacing_sm + Sp::XS, 0.0),
    };

    let shadows = crate::popover::Shadows::new(recipe.shadow, Shadow::TOOLTIP, scale);
    view! {
        <div class="absolute"
             left={x + offset_x} top={y + offset_y}
             z_index={500}
             px={px} py={py}
             bg={tc.elevated_surface}
             border={tc.border}
             rounded={radius}
             shadow_preset={shadows.layers()}>
            <text size={font} color={tc.text}>{content}</text>
        </div>
    }
}

pub struct TooltipState {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub side: TooltipSide,
    pub visible: bool,
    pub show_at_ms: u64,
}

impl Default for TooltipState {
    fn default() -> Self {
        Self {
            text: String::new(),
            x: 0.0,
            y: 0.0,
            side: TooltipSide::Bottom,
            visible: false,
            show_at_ms: 0,
        }
    }
}

impl TooltipState {
    pub fn show(
        &mut self,
        text: impl Into<String>,
        x: f32,
        y: f32,
        side: TooltipSide,
        delay_ms: u64,
        now_ms: u64,
    ) {
        self.text = text.into();
        self.x = x;
        self.y = y;
        self.side = side;
        self.show_at_ms = now_ms + delay_ms;
        self.visible = false;
    }

    pub fn hide(&mut self) {
        self.visible = false;
        self.text.clear();
    }

    pub fn tick(&mut self, now_ms: u64) {
        if !self.text.is_empty() && !self.visible && now_ms >= self.show_at_ms {
            self.visible = true;
        }
    }

    pub fn render(&self, theme: &Theme) -> Option<AnyElement> {
        if self.visible && !self.text.is_empty() {
            Some(tooltip_layer(&self.text, self.x, self.y, self.side, theme))
        } else {
            None
        }
    }
}
