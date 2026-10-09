//! Small building blocks every surface shares: text and icon shorthands,
//! icon buttons, and the one menu style all Codex popovers use.

use accesskit::Role;
use quark::view;
use quark_app::quark_ui::Action;
use quark_app::quark_ui::element::*;
use quark_app::quark_ui::style::Styled;
use quark_app::quark_ui::theme::Color;

use crate::icons;
use crate::theme::{BODY, Pal, SMALL};

/// One line of text that never wraps.
pub fn txt(s: impl Into<String>, size: f32, color: Color) -> TextElement {
    view! { -> TextElement,
        <text size={size} color={color} class="whitespace-nowrap leading-tight">{s}</text>
    }
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
    view! { -> Div,
        <div w={box_size} h={box_size} class="shrink-0 items-center justify-center rounded-[7]"
             hover_bg={p.row_hover.with_alpha(110)} role="button" aria-label={label.to_owned()}
             on:click={action}>
            <icon svg={svg} size={icon_size} color={color} />
        </div>
    }
}

/// A menu surface at `(x, y)` in window coordinates: translucent fill
/// over a backdrop blur, a hairline border, and a soft shadow.
pub fn menu_panel(p: &Pal, x: f32, y: f32, w: f32) -> Div {
    // The platform role alone, not `role="menu"` (which pins the semantic
    // role too), so a caller's `accessibility_role` (the palette's Dialog)
    // still sets both.
    view! { -> Div,
        <div class="absolute flex-col p-1 rounded-[10] blur-[18] z-60" left={x} top={y} w={w}
             bg={p.menu} border={p.menu_border} shadow={(16.0, 6.0, p.shadow)}
             accessibility_role={Role::Menu} />
    }
}

pub fn menu_header(p: &Pal, label: &str) -> Div {
    view! { -> Div,
        <div class="flex-row items-center h-7 px-2 shrink-0">
            <txt(label, SMALL, p.menu_header) />
        </div>
    }
}

pub fn menu_sep(p: &Pal) -> Div {
    view! { -> Div,
        <div class="w-full h-[9] shrink-0 justify-center px-2">
            <div class="w-full h-px" bg={p.menu_border} />
        </div>
    }
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
    let icon_color = item
        .tint
        .unwrap_or(if item.dim { p.menu_desc } else { p.menu_title });
    view! { -> Div,
        <div class="flex-row items-center w-full shrink-0 px-2 gap-[9] rounded-[6]"
             h={if tall { 48.0 } else { 29.0 }} hover_bg={p.menu_hi} role="menuitem"
             aria-label={item.title.to_owned()} on:click={action} bg={if item.hi { p.menu_hi }}>
            if let Some(svg) = item.icon {
                <div class="self-start" pt={if tall { 7.0 } else { 6.5 }}>
                    if item.icon_colored {
                        <icon svg={svg} size={16.0} />
                    } else {
                        <icon svg={svg} size={16.0} color={icon_color} />
                    }
                </div>
            }
            if tall {
                <div class="flex-1 min-w-0 overflow-hidden flex-col">
                    <txt(item.title, BODY, title_color) />
                    <txt(item.desc.unwrap_or(""), SMALL, item.tint.unwrap_or(p.menu_desc))
                         class="truncate" />
                </div>
            } else {
                <div class="flex-1 min-w-0 overflow-hidden flex-row items-center gap-[7]">
                    <txt(item.title, BODY, title_color) />
                    if let Some(desc) = item.desc {
                        <txt(desc, SMALL, p.menu_desc) class="truncate" />
                    }
                </div>
            }
            if let Some(keys) = item.shortcut {
                <txt(keys, SMALL, p.menu_desc) />
            }
            if item.check {
                <icon svg={icons::CHECK} size={15.0} color={p.menu_check} />
            }
            if item.chevron {
                <icon svg={icons::CHEVRON_RIGHT} size={15.0} color={p.faint} />
            }
        </div>
    }
}

/// A rounded key-chord chip ("⌘1").
pub fn kbd(p: &Pal, keys: &str) -> Div {
    view! { -> Div,
        <div class="flex-row items-center h-[18] px-[5] rounded-[5]" bg={p.menu_hi}>
            <txt(keys, 12.0, p.kbd) />
        </div>
    }
}

/// The yellow JS file badge.
pub fn js_badge() -> Div {
    view! { -> Div,
        <div class="w-[14] h-[14] shrink-0 items-center justify-center rounded-[3] bg-[#4a411e]">
            <text size={7.0} class="font-bold text-[#f0cd4b] whitespace-nowrap">"JS"</text>
        </div>
    }
}

/// A transparent full-window layer under an open menu: a click outside
/// the menu lands here and closes it.
pub fn click_catcher(w: f32, h: f32, action: impl Into<Action>) -> Div {
    view! { -> Div,
        <div class="absolute left-0 top-0 z-55" w={w} h={h} on:click={action} />
    }
}

/// A toggle switch.
pub fn toggle(p: &Pal, on: bool) -> Div {
    view! { -> Div,
        <div class="w-8 h-5 shrink-0 rounded-[10] flex-row items-center px-0.5"
             bg={if on { p.toggle_on } else { p.menu_hi }} @when {on} { class="justify-end" }>
            <div class="w-4 h-4 rounded-[8] bg-white" />
        </div>
    }
}

/// Text and icon colors for everything under a div: an inherited color
/// wins over a child's own (how Full access turns orange).
pub trait TintAll {
    fn text_color_all(self, color: Color) -> Self;
}

impl TintAll for Div {
    fn text_color_all(mut self, color: Color) -> Self {
        // view!: Div has no setter for the inherited text and icon colors.
        let style = self.element_style_mut();
        style.text_color = Some(color);
        style.icon_color = Some(color);
        self
    }
}
