//! The new-chat screen: the headline (with a project picker inside a
//! project), the large composer over its context tray, the connector
//! suggestion cards, and the rate-limit banner.

use accesskit::Role;
use quark_app::ViewContext;
use quark_app::quark_ui::element::*;
use quark_app::quark_ui::style::Styled;
use quark_app::quark_ui::theme::Color;

use crate::theme::{BODY, HEADING, Pal};
use crate::widgets::*;
use crate::{Codex, HEADER_H, Menu, Msg, composer, icons};

/// The composer column's widest.
pub const MAX_COLUMN: f32 = 768.0;

pub fn view(
    app: &mut Codex,
    p: &Pal,
    (x, w, h): (f32, f32, f32),
    vcx: &mut ViewContext,
) -> AnyElement {
    let column = (w - 38.0).clamp(0.0, MAX_COLUMN);
    let cx = (w - column) / 2.0;
    let (card, card_h) = composer::card(app, p, column, "Do anything", vcx);
    let tray = composer::tray(app, p, column - 2.0);
    // Headline, composer, and tray sit just above the middle (captures 04
    // and 13 put the composer's top at 335 of 738).
    let block_h = 34.0 + 43.0 + card_h + 42.0;
    let top = ((h - block_h) / 2.0 - 2.0).max(HEADER_H);
    let headline = match app.project.and_then(|id| app.data.project(id)) {
        Some(project) => hrow()
            .child(txt("What should we build in ", HEADING, p.text))
            .child(
                div()
                    .id("headline.project")
                    .rounded(8.0)
                    .hover_bg(p.menu_hi)
                    .accessibility_role(Role::Button)
                    .accessibility_label(format!("Project: {}", project.name))
                    .on_click(Msg::Open(Menu::HeadlineProject))
                    .child(txt(project.name, HEADING, p.text)),
            )
            .child(txt("?", HEADING, p.text)),
        None => hrow().child(txt("What should we work on?", HEADING, p.text)),
    };
    let mut pane = div()
        .absolute()
        .left(x)
        .top(0.0)
        .w(w)
        .h(h)
        .child(div().absolute().left(w - 36.0).top(9.0).child(icon_button(
            p,
            icons::PANEL_RIGHT,
            28.0,
            17.0,
            p.icon,
            "Toggle side panel",
            Msg::ToggleSidePanel,
        )))
        .child(
            div()
                .absolute()
                .left(0.0)
                .top(top)
                .w(w)
                .h(34.0)
                .flex_row()
                .justify_center()
                .items_center()
                .child(headline),
        )
        .child(
            div()
                .absolute()
                .left(cx + 1.0)
                .top(top + 34.0 + 43.0 + card_h - 6.0)
                .child(tray),
        )
        .child(div().absolute().left(cx).top(top + 34.0 + 43.0).child(card));
    let cards_top = top + 34.0 + 43.0 + card_h + 76.0;
    if cards_top + 115.0 < h {
        pane = pane.child(connectors(p, w, cards_top));
    }
    if app.banner {
        pane = pane.child(banner(p, cx, column, h - 95.0));
    }
    pane.into_any()
}

fn connectors(p: &Pal, w: f32, top: f32) -> Div {
    let card = |logo: &'static str, title: &str, desc: &str, done: bool| {
        let (tc, dc, lc) = if done {
            (p.card_title_dim, p.card_desc_dim, p.card_title_dim)
        } else {
            (p.text, p.card_desc, p.text)
        };
        let mut head = hrow()
            .w_full()
            .child(ico(logo, 16.0, lc))
            .child(div().flex_1());
        if done {
            head = head.child(
                div()
                    .w(16.0)
                    .h(16.0)
                    .rounded(8.0)
                    .bg(Color::rgba(0x1e, 0xa4, 0x4a, 255))
                    .items_center()
                    .justify_center()
                    .child(ico(icons::CHECK, 11.0, Color::rgba(0x10, 0x10, 0x10, 255))),
            );
        }
        div()
            .w(185.0)
            .h(115.0)
            .flex_col()
            .px(13.0)
            .pt(14.0)
            .border(p.card_border)
            .rounded(12.0)
            .hover_bg(p.tray)
            .accessibility_role(Role::Button)
            .accessibility_label(format!("{title} {desc}"))
            .child(head)
            .child(div().h(12.0))
            .child(txt(title, BODY, tc))
            .child(div().h(4.0))
            .child(
                text(desc)
                    .size(BODY)
                    .color(dc)
                    .line_height(1.3)
                    .wrap_width(159.0),
            )
    };
    let row_w = 3.0 * 185.0 + 2.0 * 12.0;
    div()
        .absolute()
        .left((w - row_w) / 2.0)
        .top(top)
        .flex_row()
        .gap(12.0)
        .child(card(
            icons::LOGO_SLACK,
            "Connect messaging",
            "Catch up on engineering threads",
            true,
        ))
        .child(card(
            icons::LOGO_GITHUB,
            "Connect GitHub",
            "Review PRs, code, and CI checks",
            true,
        ))
        .child(card(
            icons::LOGO_LINEAR,
            "Connect Linear",
            "Track bugs and implementation work",
            false,
        ))
}

fn banner(p: &Pal, x: f32, w: f32, top: f32) -> Div {
    hrow()
        .absolute()
        .left(x + 1.0)
        .top(top)
        .w(w - 2.0)
        .h(70.0)
        .px(18.0)
        .gap(12.0)
        .bg(p.banner)
        .rounded(14.0)
        .border(p.hairline)
        .accessibility_role(Role::Alert)
        .child(svg_icon(icons::BADGE_RESET, 26.0).color(p.text_soft))
        .child(
            div()
                .flex_col()
                .flex_1()
                .gap(3.0)
                .child(
                    text("You have a new rate limit reset available")
                        .size(BODY)
                        .medium()
                        .color(p.text)
                        .no_wrap(),
                )
                .child(txt(
                    "You were granted a rate limit reset that will expire in 30 days.",
                    BODY,
                    p.card_desc.lerp(p.text, 0.4),
                )),
        )
        .child(
            hrow()
                .h(29.0)
                .px(12.0)
                .rounded(15.0)
                .bg(p.send_active)
                .accessibility_role(Role::Button)
                .accessibility_label("See resets")
                .child(txt("See resets", 13.5, p.send_active_glyph)),
        )
        .child(icon_button(
            p,
            icons::CLOSE,
            24.0,
            13.0,
            p.muted,
            "Dismiss",
            Msg::DismissBanner,
        ))
}
