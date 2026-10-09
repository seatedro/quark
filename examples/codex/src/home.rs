//! The new-chat screen (u01, u73): the agent glyph, the headline with the
//! project as a dotted-underlined picker, and the composer with its tray
//! pinned to the bottom of the main card. Coordinates are card-local.

use quark::view;
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
    let project = app.project.and_then(|id| app.data.project(id));
    // The glyph and headline sit a little above the middle of the space
    // over the composer (u73: glyph top 266, headline 342 of 738).
    let mid = (h - card_h - 16.0) * 0.5 - 66.0;
    view! {
        <div class="absolute top-0" left={x} w={w} h={h}>
            <div class="absolute left-0 flex-col items-center" top={mid.max(8.0)} w={w}>
                <icon svg={icons::MASCOT} size={48.0} color={p.muted} />
                <div class="h-7" />
                <div class="flex-row items-center">
                    if let Some(project) = project {
                        <txt("What should we build in ", HEADING, p.text) />
                        <div
                            id="headline.project"
                            role="button"
                            aria-label={format!("{}?", project.name)}
                            on:click={Msg::Open(Menu::ProjectPicker)}
                        >
                            <txt(format!("{}?", project.name), HEADING, p.text)
                                underline_style={dotted(p.muted, 3.0).offset(5.0)}
                            />
                        </div>
                    } else {
                        <txt("What should we work on?", HEADING, p.text) />
                    }
                </div>
            </div>
            <div class="absolute" left={cx + 13.0} top={card_top - 38.0}>
                {composer::tray(app, p, tray_w)}
            </div>
            <div class="absolute" left={cx} top={card_top}>{card}</div>
        </div>
    }
}
