use quark::view;

use quark_ui::design::{Rad, Sp};
use quark_ui::element::*;
use quark_ui::style::Styled;
use quark_ui::theme::Theme;

pub fn kbd(label: impl Into<String>, theme: &Theme) -> AnyElement {
    let tc = &theme.colors;
    let scale = theme.metrics.ui_scale();
    let label = label.into();
    let recipe = theme.components.kbd;
    let px = recipe.padding_x.unwrap_or(Sp::SM);
    let py = recipe.padding_y.unwrap_or(Sp::XXS);
    let height = recipe.height.map(|h| (h * scale).round());
    let font_size = recipe.font_size.map(|s| s * scale);

    view! { scale,
        <div
            class="shrink-0 items-center justify-center"
            px={px}
            py={py}
            border={tc.border_variant}
            rounded={recipe.radius.unwrap_or(Rad::MD)}
            @when {height.is_some()} { h={height.unwrap()} pt={0.0} pb={0.0} }
        >
            <text
                class="text-xs font-mono text-center"
                color={tc.text_muted}
                @when {font_size.is_some()} { size={font_size.unwrap()} }
            >
                {&label}
            </text>
        </div>
    }
}
