//! The new-chat screen (u01, u73): the agent glyph, the headline with the
//! project as a dotted-underlined picker, and the composer with its tray
//! pinned to the bottom of the main card. Coordinates are card-local.

use accesskit::Role;
use quark_app::ViewContext;
use quark_app::quark_ui::element::*;
use quark_app::quark_ui::style::Styled;

use crate::theme::{HEADING, Pal};
use crate::widgets::*;
use crate::{Codex, Menu, Msg, composer, icons};

/// The composer column's widest.
pub const MAX_COLUMN: f32 = 768.0;

pub fn view(
    app: &mut Codex,
    p: &Pal,
    (x, w, h): (f32, f32, f32),
    vcx: &mut ViewContext,
) -> AnyElement {
    let column = (w - 32.0).clamp(0.0, MAX_COLUMN);
    let cx = ((w - column) / 2.0).round();
    let (card, card_h) = composer::card(app, p, column, "Do anything", vcx);
    let card_top = h - 16.0 - card_h;
    let tray_w = column - 26.0;
    let headline = match app.project.and_then(|id| app.data.project(id)) {
        Some(project) => hrow()
            .child(txt("What should we build in ", HEADING, p.text))
            .child(
                div()
                    .relative()
                    .id("headline.project")
                    .accessibility_role(Role::Button)
                    .accessibility_label(format!("{}?", project.name))
                    .on_click(Msg::Open(Menu::ProjectPicker))
                    .child(txt(format!("{}?", project.name), HEADING, p.text))
                    .child(dotted(p, 31.0)),
            ),
        None => hrow().child(txt("What should we work on?", HEADING, p.text)),
    };
    // The glyph and headline sit a little above the middle of the space
    // over the composer (u73: glyph top 266, headline 342 of 738).
    let mid = (h - card_h - 16.0) * 0.5 - 66.0;
    div()
        .absolute()
        .left(x)
        .top(0.0)
        .w(w)
        .h(h)
        .child(
            div()
                .absolute()
                .left(0.0)
                .top(mid.max(8.0))
                .w(w)
                .flex_col()
                .items_center()
                .child(svg_icon(icons::MASCOT, 48.0).color(p.muted))
                .child(div().h(28.0))
                .child(headline),
        )
        .child(
            div()
                .absolute()
                .left(cx + 13.0)
                .top(card_top - 38.0)
                .child(composer::tray(app, p, tray_w)),
        )
        .child(div().absolute().left(cx).top(card_top).child(card))
        .into_any()
}

/// A dotted underline across its parent at `y` (the "?" excluded).
fn dotted(p: &Pal, y: f32) -> Div {
    let mut row = hrow()
        .absolute()
        .left(0.0)
        .right(14.0)
        .top(y)
        .h(1.0)
        .gap(2.0)
        .overflow_hidden();
    for _ in 0..80 {
        row = row.child(div().w(2.0).h(1.0).flex_shrink_0().bg(p.muted));
    }
    row
}
