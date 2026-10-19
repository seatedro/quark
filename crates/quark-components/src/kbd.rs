use quark::view;

use quark_ui::design::{Ico, Rad, Sp};
use quark_ui::element::*;
use quark_ui::style::Styled;
use quark_ui::theme::Theme;

pub fn kbd(label: impl Into<String>, theme: &Theme) -> AnyElement {
    keycap(label_text(label.into(), theme), theme)
}

pub fn kbd_icon(svg: &'static str, theme: &Theme) -> AnyElement {
    let scale = theme.metrics.ui_scale();
    let size = theme.components.kbd.icon_size.unwrap_or(Ico::XS) * scale;
    // Keeps the height of a text keycap.
    let content = div()
        .flex_row()
        .items_center()
        .child(label_text("\u{200B}".to_owned(), theme))
        .child(svg_icon(svg, size).color(theme.colors.text_muted))
        .into_any();
    keycap(content, theme)
}

fn label_text(label: String, theme: &Theme) -> AnyElement {
    let tc = &theme.colors;
    let scale = theme.metrics.ui_scale();
    view! { scale,
        <text
            class="text-xs font-mono text-center"
            color={tc.text_muted}
            size={?theme.components.kbd.font_size}
        >
            {&label}
        </text>
    }
}

fn keycap(content: AnyElement, theme: &Theme) -> AnyElement {
    let tc = &theme.colors;
    let scale = theme.metrics.ui_scale();
    let recipe = theme.components.kbd;
    let px = recipe.padding_x.unwrap_or(Sp::SM);
    let py = match recipe.height {
        Some(_) => 0.0,
        None => recipe.padding_y.unwrap_or(Sp::XXS),
    };

    view! { scale,
        <div
            class="shrink-0 items-center justify-center"
            px={px}
            py={py}
            border={tc.border_variant}
            rounded={recipe.radius.unwrap_or(Rad::MD)}
            h={?recipe.height}
        >
            {content}
        </div>
    }
}
