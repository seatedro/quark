//! Small building blocks every surface shares: text and icon shorthands,
//! icon buttons, and the one menu style all Codex popovers use.

use accesskit::Role;
use quark_app::quark_ui::Action;
use quark_app::quark_ui::element::*;
use quark_app::quark_ui::style::Styled;
use quark_app::quark_ui::theme::Color;

use crate::icons;
use crate::theme::{BODY, Pal, SMALL};

/// One line of text that never wraps.
pub fn txt(s: impl Into<String>, size: f32, color: Color) -> TextElement {
    text(s).size(size).color(color).no_wrap().line_height(1.25)
}

pub fn ico(svg: &'static str, size: f32, color: Color) -> SvgIcon {
    svg_icon(svg, size).color(color)
}

/// A horizontal row with its children centered vertically.
pub fn hrow() -> Div {
    div().flex_row().items_center()
}

/// A square ghost button holding one icon, with a hover fill.
pub fn icon_button(
    p: &Pal,
    svg: &'static str,
    box_size: f32,
    icon_size: f32,
    color: Color,
    label: &str,
    action: impl Into<Action>,
) -> Div {
    div()
        .w(box_size)
        .h(box_size)
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .rounded(7.0)
        .hover_bg(p.row_hover.with_alpha(110))
        .accessibility_role(Role::Button)
        .accessibility_label(label.to_owned())
        .on_click(action)
        .child(ico(svg, icon_size, color))
}

/// A menu surface at `(x, y)` in window coordinates: translucent fill
/// over a backdrop blur, a hairline border, and a soft shadow.
pub fn menu_panel(p: &Pal, x: f32, y: f32, w: f32) -> Div {
    div()
        .absolute()
        .left(x)
        .top(y)
        .w(w)
        .flex_col()
        .p(4.0)
        .bg(p.menu)
        .border(p.menu_border)
        .rounded(10.0)
        .shadow(16.0, 6.0, p.shadow)
        .blur(18.0)
        .z_index(60)
        .accessibility_role(Role::Menu)
}

pub fn menu_header(p: &Pal, label: &str) -> Div {
    hrow()
        .h(28.0)
        .px(8.0)
        .flex_shrink_0()
        .child(txt(label, SMALL, p.menu_header))
}

pub fn menu_sep(p: &Pal) -> Div {
    div()
        .w_full()
        .h(9.0)
        .flex_shrink_0()
        .justify_center()
        .px(8.0)
        .child(div().w_full().h(1.0).bg(p.menu_border))
}

/// What one menu row shows.
#[derive(Default)]
pub struct Item<'a> {
    pub icon: Option<&'static str>,
    pub icon_colored: bool,
    pub title: &'a str,
    pub desc: Option<&'a str>,
    /// Description under the title (two-line rows) instead of beside it.
    pub desc_below: bool,
    pub check: bool,
    pub chevron: bool,
    pub shortcut: Option<&'a str>,
    pub hi: bool,
    pub dim: bool,
    /// Title, description, and icon in this color (Full access's orange).
    pub tint: Option<Color>,
}

pub fn menu_item(p: &Pal, item: Item, action: impl Into<Action>) -> Div {
    let tall = item.desc_below && item.desc.is_some();
    let title_color = if let Some(t) = item.tint {
        t
    } else if item.dim {
        p.menu_desc
    } else if item.hi {
        p.text
    } else {
        p.menu_title
    };
    let mut label = div().flex_1().min_w(0.0).overflow_hidden();
    if tall {
        label = label
            .flex_col()
            .child(txt(item.title, BODY, title_color))
            .child(
                txt(
                    item.desc.unwrap_or(""),
                    SMALL,
                    item.tint.unwrap_or(p.menu_desc),
                )
                .truncate(),
            );
    } else {
        label = label
            .flex_row()
            .items_center()
            .gap(7.0)
            .child(txt(item.title, BODY, title_color));
        if let Some(desc) = item.desc {
            label = label.child(txt(desc, SMALL, p.menu_desc).truncate());
        }
    }
    let mut row = hrow()
        .w_full()
        .h(if tall { 48.0 } else { 29.0 })
        .flex_shrink_0()
        .px(8.0)
        .gap(9.0)
        .rounded(6.0)
        .hover_bg(p.menu_hi)
        .accessibility_role(Role::MenuItem)
        .accessibility_label(item.title.to_owned())
        .on_click(action);
    if item.hi {
        row = row.bg(p.menu_hi);
    }
    if let Some(svg) = item.icon {
        let icon = if item.icon_colored {
            svg_icon(svg, 16.0)
        } else {
            ico(
                svg,
                16.0,
                item.tint
                    .unwrap_or(if item.dim { p.menu_desc } else { p.menu_title }),
            )
        };
        row = row.child(
            div()
                .self_start()
                .pt(if tall { 7.0 } else { 6.5 })
                .child(icon),
        );
    }
    row = row.child(label);
    if let Some(keys) = item.shortcut {
        row = row.child(txt(keys, SMALL, p.menu_desc));
    }
    if item.check {
        row = row.child(ico(icons::CHECK, 15.0, p.menu_check));
    }
    if item.chevron {
        row = row.child(ico(icons::CHEVRON_RIGHT, 15.0, p.faint));
    }
    row
}

/// A rounded key-chord chip ("⌘1").
pub fn kbd(p: &Pal, keys: &str) -> Div {
    hrow()
        .h(18.0)
        .px(5.0)
        .rounded(5.0)
        .bg(p.menu_hi)
        .child(txt(keys, 12.0, p.kbd))
}

/// The yellow JS file badge.
pub fn js_badge() -> Div {
    div()
        .w(14.0)
        .h(14.0)
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .rounded(3.0)
        .bg(Color::rgba(0x4a, 0x41, 0x1e, 255))
        .child(
            text("JS")
                .size(7.0)
                .bold()
                .color(Color::rgba(0xf0, 0xcd, 0x4b, 255))
                .no_wrap(),
        )
}

/// A transparent full-window layer under an open menu: a click outside
/// the menu lands here and closes it.
pub fn click_catcher(w: f32, h: f32, action: impl Into<Action>) -> Div {
    div()
        .absolute()
        .left(0.0)
        .top(0.0)
        .w(w)
        .h(h)
        .z_index(55)
        .on_click(action)
}

/// A toggle switch.
pub fn toggle(p: &Pal, on: bool) -> Div {
    let track = if on { p.toggle_on } else { p.menu_hi };
    let knob = div()
        .w(16.0)
        .h(16.0)
        .rounded(8.0)
        .bg(Color::rgba(255, 255, 255, 255));
    div()
        .w(32.0)
        .h(20.0)
        .flex_shrink_0()
        .rounded(10.0)
        .bg(track)
        .flex_row()
        .items_center()
        .px(2.0)
        .when(on, |d| d.justify_end())
        .child(knob)
}

/// Text and icon colors for everything under a div: an inherited color
/// wins over a child's own (how Full access turns orange).
pub trait TintAll {
    fn text_color_all(self, color: Color) -> Self;
}

impl TintAll for Div {
    fn text_color_all(mut self, color: Color) -> Self {
        let style = self.element_style_mut();
        style.text_color = Some(color);
        style.icon_color = Some(color);
        self
    }
}
