//! Shimmering placeholders shown where content is still loading, so the
//! wait previews the shape of what will appear. The shimmer runs on the
//! GPU through quark's shimmer effect, with no per-frame CPU work.

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
    view! { scale,
        <div w={width} h={height}
             rounded={Rad::SM}
             bg={Color::TRANSPARENT}
             bg_effect={shimmer(tc.element_background, tc.ghost_element_hover, 1.0)} />
    }
}

/// A column of `count` shimmering lines of varied widths, as a placeholder
/// for a list or a paragraph. Lines wider than the column are clipped by
/// it.
pub fn skeleton_lines(count: usize, theme: &Theme) -> AnyElement {
    let scale = theme.metrics.ui_scale();
    let lines: Vec<AnyElement> = LINE_WIDTHS
        .iter()
        .cycle()
        .take(count)
        .map(|w| skeleton(*w, LINE_H, theme))
        .collect();
    view! { scale,
        <div class="flex-col w-full" p={Sp::MD} gap={Sp::MD}>
            {...lines}
        </div>
    }
}
