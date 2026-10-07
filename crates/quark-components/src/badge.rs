use quark::{Props, view};

use quark_ui::design::{Alpha, Rad, Sp, Sz};
use quark_ui::element::{
    AnyElement, ElementContext, IntoAnyElement, RenderOnce, div, svg_icon, text,
};
use quark_ui::style::Styled;
use quark_ui::theme::{Color, ThemeColors};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BadgeVariant {
    Default,
    Info,
    Success,
    Warning,
    Error,
    Accent,
}

/// A small status pill: `badge("New").success()`, or in `view!`
/// `<Badge label="New" variant={BadgeVariant::Success} />`.
#[derive(Props)]
pub struct Badge {
    #[prop(into)]
    label: String,
    #[prop(default = BadgeVariant::Default)]
    variant: BadgeVariant,
    #[prop(optional)]
    icon: Option<&'static str>,
}

pub fn badge(label: impl Into<String>) -> Badge {
    Badge::builder().label(label).build()
}

impl Badge {
    pub fn variant(mut self, v: BadgeVariant) -> Self {
        self.variant = v;
        self
    }

    pub fn icon(mut self, svg: &'static str) -> Self {
        self.icon = Some(svg);
        self
    }

    pub fn success(self) -> Self {
        self.variant(BadgeVariant::Success)
    }

    pub fn error(self) -> Self {
        self.variant(BadgeVariant::Error)
    }

    pub fn warning(self) -> Self {
        self.variant(BadgeVariant::Warning)
    }

    pub fn info(self) -> Self {
        self.variant(BadgeVariant::Info)
    }

    pub fn accent(self) -> Self {
        self.variant(BadgeVariant::Accent)
    }
}

fn variant_colors(variant: BadgeVariant, tc: &ThemeColors) -> (Color, Color) {
    match variant {
        BadgeVariant::Default => (tc.element_background, tc.text_muted),
        BadgeVariant::Info => (tc.status_info.with_alpha(Alpha::WHISPER), tc.status_info),
        BadgeVariant::Success => (tc.line_add, tc.line_add_text),
        BadgeVariant::Warning => (tc.line_modified, tc.status_warning),
        BadgeVariant::Error => (tc.line_del, tc.line_del_text),
        BadgeVariant::Accent => (tc.accent.with_alpha(Alpha::WHISPER), tc.accent),
    }
}

impl RenderOnce for Badge {
    fn render(self, cx: &ElementContext) -> AnyElement {
        let tc = &cx.theme.colors;
        let m = &cx.theme.metrics;
        let scale = m.ui_scale();
        let (bg, fg) = variant_colors(self.variant, tc);
        let icon_size = (m.ui_small_font_size - Sp::XXS * scale).max(Sz::ICON_MIN * scale);

        view! { scale,
            <div class="flex-row shrink-0 items-center"
                 gap={m.spacing_xs} px={m.spacing_sm}
                 py={Sp::XXS} bg={bg}
                 rounded={Rad::PILL}>
                if let Some(svg) = self.icon {
                    <icon svg={svg} size={icon_size} color={fg} />
                }
                <text class="text-xs font-medium" color={fg}>{self.label}</text>
            </div>
        }
    }
}
