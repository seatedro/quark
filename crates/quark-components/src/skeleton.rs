//! Shimmering placeholders shown where content is still loading, so the
//! wait previews the shape of what will appear. The shimmer runs on the
//! GPU through quark's shimmer effect, with no per-frame CPU work. Under
//! [`Theme::reduced_motion`] the placeholders are flat and still, so nothing
//! asks for frames.

use quark::view;

use quark_ui::design::{Rad, Sp};
use quark_ui::element::*;
use quark_ui::style::Styled;
use quark_ui::theme::{Color, Theme};

/// Line widths of [`skeleton_lines`], in design pixels, cycled. Varied so
/// the placeholder reads as text rather than a ladder; fixed so it does not
/// change between frames.
const LINE_WIDTHS: [f32; 14] = [
    148.0, 92.0, 176.0, 120.0, 64.0, 210.0, 136.0, 104.0, 180.0, 88.0, 156.0, 128.0, 72.0, 200.0,
];

/// Height of one [`skeleton_lines`] line, in design pixels.
const LINE_H: f32 = 12.0;

/// One shimmering block of `width` by `height` design pixels.
pub fn skeleton(width: f32, height: f32, theme: &Theme) -> AnyElement {
    let tc = &theme.colors;
    let scale = theme.metrics.ui_scale();
    let radius = theme.components.skeleton.radius.unwrap_or(Rad::SM);
    let still = theme.reduced_motion;
    view! { scale,
        <div w={width} h={height}
             rounded={radius}
             @when {still} { bg={tc.element_background} }
             @when {!still} {
                 bg={Color::TRANSPARENT}
                 bg_effect={shimmer(tc.element_background, tc.ghost_element_hover, 1.0)}
             } />
    }
}

/// A column of `count` shimmering lines of varied widths, as a placeholder
/// for a list or a paragraph. Lines wider than the column are clipped by
/// it.
pub fn skeleton_lines(count: usize, theme: &Theme) -> AnyElement {
    let scale = theme.metrics.ui_scale();
    let recipe = theme.components.skeleton;
    let line_h = recipe.height.map_or(LINE_H, |h| (h * scale).round());
    let lines: Vec<AnyElement> = LINE_WIDTHS
        .iter()
        .cycle()
        .take(count)
        .map(|w| skeleton(*w, line_h, theme))
        .collect();
    // Unscaled: the view scales padding and gap.
    view! { scale,
        <div class="flex-col w-full"
             px={recipe.padding_x.unwrap_or(Sp::MD)}
             py={recipe.padding_y.unwrap_or(Sp::MD)}
             gap={recipe.gap.unwrap_or(Sp::MD)}>
            {...lines}
        </div>
    }
}
