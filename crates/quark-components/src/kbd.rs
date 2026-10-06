use quark::view;

use quark_ui::design::{Alpha, Rad, Sp};
use quark_ui::element::*;
use quark_ui::style::Styled;
use quark_ui::theme::Theme;

pub fn kbd(label: impl Into<String>, theme: &Theme) -> AnyElement {
    let tc = &theme.colors;
    let scale = theme.metrics.ui_scale();
    let label = label.into();

    view! { scale,
        <div class="shrink-0 items-center justify-center"
             px={Sp::SM} py={Sp::XXS}
             bg={tc.element_background}
             border={tc.border_variant}
             rounded={Rad::MD}
             shadow={(1.0, 1.0, tc.border_variant.with_alpha(Alpha::MEDIUM))}>
            <text class="text-xs mono text-center" color={tc.text}>{&label}</text>
        </div>
    }
}
