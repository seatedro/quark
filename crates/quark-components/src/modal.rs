use quark::view;

use quark_ui::Action;
use quark_ui::design::{Ico, Rad, Shadow, Sp, Sz};
use quark_ui::element::*;
use quark_ui::style::Styled;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ModalAlign {
    Center,
    Top,
}

pub struct Modal {
    title: String,
    subtitle: String,
    icon: &'static str,
    max_width: f32,
    height: Option<f32>,
    /// Set by the caller; otherwise the theme's modal recipe or the default.
    gap: Option<f32>,
    padding: Option<f32>,
    align: ModalAlign,
    window_width: f32,
    window_height: f32,
    body: Vec<AnyElement>,
    footer: Vec<AnyElement>,
    on_dismiss: Action,
}

impl Modal {
    /// `on_dismiss` is emitted when the backdrop outside the panel is clicked.
    pub fn new(
        title: impl Into<String>,
        subtitle: impl Into<String>,
        icon: &'static str,
        max_width: f32,
        window_width: f32,
        window_height: f32,
        on_dismiss: impl Into<Action>,
    ) -> Self {
        Self {
            title: title.into(),
            subtitle: subtitle.into(),
            icon,
            max_width,
            height: None,
            gap: None,
            padding: None,
            align: ModalAlign::Center,
            window_width,
            window_height,
            body: Vec::new(),
            footer: Vec::new(),
            on_dismiss: on_dismiss.into(),
        }
    }

    pub fn height(mut self, h: f32) -> Self {
        self.height = Some(h);
        self
    }

    pub fn gap(mut self, gap: f32) -> Self {
        self.gap = Some(gap);
        self
    }

    pub fn padding(mut self, padding: f32) -> Self {
        self.padding = Some(padding);
        self
    }

    pub fn align(mut self, align: ModalAlign) -> Self {
        self.align = align;
        self
    }

    pub fn body_child(mut self, child: impl IntoAnyElement) -> Self {
        self.body.push(child.into_any());
        self
    }

    pub fn footer_child(mut self, child: impl IntoAnyElement) -> Self {
        self.footer.push(child.into_any());
        self
    }
}

impl RenderOnce for Modal {
    fn render(self, cx: &ElementContext) -> AnyElement {
        let tc = &cx.theme.colors;
        let scale = cx.theme.metrics.ui_scale();
        let recipe = cx.theme.components.modal;

        let panel_width = self
            .max_width
            .min(self.window_width - (Sz::MODAL_MARGIN * scale).round());
        // Unscaled: the panel's view scales its padding, gap, and radius
        // once. Scaling them here as well grew them by the square.
        let padding_x = self.padding.or(recipe.padding_x).unwrap_or(Sp::XXL);
        let padding_y = self.padding.or(recipe.padding_y).unwrap_or(Sp::XXL);
        let gap = self.gap.or(recipe.gap).unwrap_or(Sp::LG);
        let radius = recipe.radius.unwrap_or(Rad::XXXL);
        let shadows = crate::popover::Shadows::new(recipe.shadow, Shadow::MODAL, scale);
        let title_size = recipe.title_font_size.map(|s| s * scale);
        let subtitle_size = recipe.font_size.map(|s| s * scale);
        let max_h = self.window_height - (Sz::MODAL_MARGIN * scale).round() * 2.0;
        let accessibility_label = self.title.clone();

        let header = view! { scale,
            <div class="flex-col" gap={Sp::SM}>
                <div class="flex-row shrink-0 items-center" gap={Sp::SM}>
                    <icon svg={self.icon} size={Ico::LG} color={tc.accent} />
                    <text
                        class="text-lg font-semibold"
                        color={tc.text_strong}
                        @when {title_size.is_some()} { size={title_size.unwrap()} }
                    >
                        {&self.title}
                    </text>
                </div>
                if !self.subtitle.is_empty() {
                    <text
                        class="text-sm"
                        color={tc.text_muted}
                        @when {subtitle_size.is_some()} { size={subtitle_size.unwrap()} }
                    >
                        {&self.subtitle}
                    </text>
                }
            </div>
        };

        let panel = view! { scale,
            <div
                class="flex-col overflow-hidden"
                w={panel_width}
                px={padding_x}
                py={padding_y}
                gap={gap}
                bg={tc.elevated_surface}
                rounded={radius}
                border_b={tc.border}
                shadow_preset={shadows.layers()}
                on:click={quark_ui::element::NoopAction}
                id={format!("modal:{accessibility_label}")}
                test-id="modal"
                role="dialog"
                focus_scope={accessibility_label.clone()}
                trap_focus={true}
                accessibility_id={format!("modal:{accessibility_label}")}
                aria-label={accessibility_label}
                @when {self.height.is_some()} {
                    h={(self.height.unwrap() * scale).round().min(max_h)}
                }
            >
                {header}
                {...self.body}
                if !self.footer.is_empty() {
                    <spacer />
                    <div class="flex-row" gap={Sp::LG}>{...self.footer}</div>
                }
            </div>
        };

        view! { scale,
            <div
                class="absolute flex-col items-center"
                top={0.0}
                left={0.0}
                w={self.window_width}
                h={self.window_height}
                z_index={100}
                bg={tc.overlay_scrim}
                id="overlay.backdrop"
                test-id="modal-backdrop"
                on:click={self.on_dismiss}
                block_mouse
                hit_identity={HitIdentity::OverlayBackdrop}
                @when {self.align == ModalAlign::Center} { justify_center }
                @when {self.align == ModalAlign::Top} { pt={Sz::MODAL_TOP_OFFSET} }
            >
                {panel}
            </div>
        }
    }
}
