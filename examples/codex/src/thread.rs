//! A thread: the header (project, title, chat actions, Open in, summary,
//! side panel), the transcript, and the follow-up composer pinned to the
//! bottom.

use accesskit::Role;
use quark_app::ViewContext;
use quark_app::quark_ui::element::*;
use quark_app::quark_ui::style::Styled;
use quark_app::quark_ui::theme::{Color, ThemeMode};

use crate::data::{Item, ThreadId};
use crate::theme::{BODY, CODE, Pal, SMALL};
use crate::widgets::*;
use crate::{Codex, HEADER_H, Menu, Msg, composer, icons};

/// Turn column's widest on wide windows.
pub const MAX_COLUMN: f32 = 800.0;

pub fn view(
    app: &mut Codex,
    id: ThreadId,
    p: &Pal,
    (x, w, h): (f32, f32, f32),
    vcx: &mut ViewContext,
) -> AnyElement {
    let Some(thread) = app.data.thread(id) else {
        return div().into_any();
    };
    let title = thread.title.clone();
    let items = thread.items.clone();
    let expanded = app.panel_expanded && app.side_panel;
    let column = (w - 32.0).clamp(0.0, MAX_COLUMN);
    let cx = (w - column) / 2.0;

    let mut pane = div().absolute().left(x).top(0.0).w(w).h(h);
    if expanded {
        // The side panel covers the thread; only the composer stays, over it.
        return pane.into_any();
    }
    let (card, card_h) = composer::card(app, p, column, "Ask for follow-up changes", vcx);
    let card_top = h - 16.0 - card_h;
    let list_top = HEADER_H;
    let list_h = (card_top - 8.0 - list_top).max(0.0);
    let now = app.now_ms;
    let transcript = transcript(&items, p, column, now, app.size.0);
    if app.stick_bottom {
        // A pending request: the handle clamps it to the content once laid out.
        app.thread_scroll.set_offset(0.0, f32::MAX / 4.0);
        app.stick_bottom = false;
    }
    pane = pane
        .child(header(app, p, &title, w))
        .child(
            div()
                .absolute()
                .left(0.0)
                .top(list_top)
                .w(w)
                .h(list_h)
                .flex_col()
                .items_center()
                .overflow_y_scroll()
                .track_scroll(&app.thread_scroll)
                .scrollbar_auto_hide()
                .accessibility_role(Role::Log)
                .accessibility_label("Transcript")
                .child(transcript),
        )
        .child(div().absolute().left(cx).top(card_top).child(card));
    let _ = vcx;
    pane.into_any()
}

fn header(app: &Codex, p: &Pal, title: &str, w: f32) -> Div {
    let compact = w < 560.0;
    let open_in = hrow()
        .h(28.0)
        .rounded(8.0)
        .border(p.hairline)
        .bg(if p.mode == ThemeMode::Dark {
            p.bubble
        } else {
            p.bg
        })
        .child(
            hrow()
                .h(28.0)
                .pl(9.0)
                .pr(if compact { 4.0 } else { 9.0 })
                .gap(8.0)
                .id("header.openin")
                .accessibility_role(Role::Button)
                .accessibility_label("Open in")
                .child(svg_icon(icons::APP_FINDER, 15.0))
                .when(!compact, |d| d.child(txt("Open in", BODY, p.text))),
        )
        .child(
            hrow()
                .h(28.0)
                .pr(6.0)
                .accessibility_role(Role::Button)
                .accessibility_label("Secondary action")
                .on_click(Msg::Open(Menu::OpenIn))
                .child(ico(icons::CHEVRON_DOWN, 14.0, p.muted)),
        );
    let panel_on = app.side_panel;
    let mut toggle = icon_button(
        p,
        icons::PANEL_RIGHT,
        28.0,
        17.0,
        p.icon,
        "Toggle side panel",
        Msg::ToggleSidePanel,
    );
    if panel_on {
        toggle = toggle.bg(p.panel_tile);
    }
    let left = if app.sidebar_shown() { 16.0 } else { 220.0 };
    let title_w = (w - left - 26.0 - 40.0 - 200.0).max(40.0);
    hrow()
        .absolute()
        .left(0.0)
        .top(9.0)
        .w(w)
        .h(28.0)
        .pl(left)
        .pr(8.0)
        .child(
            div()
                .id("header.project")
                .accessibility_role(Role::Button)
                .accessibility_label("Project: codex-demo")
                .child(ico(icons::DOC, 16.0, p.text)),
        )
        .child(div().w(10.0))
        .child(
            div().max_w(title_w).min_w(0.0).overflow_hidden().child(
                text(title.to_owned())
                    .size(BODY)
                    .medium()
                    .color(p.text)
                    .no_wrap()
                    .truncate(),
            ),
        )
        .child(div().w(7.0))
        .child(
            icon_button(
                p,
                icons::ELLIPSIS,
                29.0,
                17.0,
                p.icon,
                "Chat actions",
                Msg::Open(Menu::ChatActions),
            )
            .id("header.actions"),
        )
        .child(div().flex_1())
        .when(!panel_on || !app.sidebar_shown(), |d| {
            d.child(open_in).child(div().w(6.0))
        })
        .when(!panel_on, |d| {
            d.child(
                icon_button(
                    p,
                    icons::SUMMARY,
                    28.0,
                    17.0,
                    p.icon,
                    "Toggle summary",
                    Msg::Open(Menu::Summary),
                )
                .id("header.summary"),
            )
            .child(div().w(6.0))
        })
        .when(!panel_on, |d| d.child(toggle))
}

/// The turns, top to bottom, in a `column`-wide column.
pub fn transcript(items: &[Item], p: &Pal, column: f32, now_ms: u64, window_w: f32) -> Div {
    let mut col = div().w(column).flex_col().flex_shrink_0().pt(33.0).pb(24.0);
    let last_user = items.iter().rposition(|i| matches!(i, Item::User { .. }));
    for (i, item) in items.iter().enumerate() {
        let next = items.get(i + 1);
        let el: Div = match item {
            Item::User { text: body, time } => {
                let show_actions = Some(i) == last_user;
                let narrow = window_w < 560.0;
                let mut actions = hrow().h(30.0).pt(4.0).gap(2.0).justify_end().w_full();
                if show_actions {
                    if narrow {
                        actions = actions
                            .child(txt(*time, SMALL, p.muted))
                            .child(div().w(6.0));
                    }
                    actions = actions
                        .child(icon_button(
                            p,
                            icons::COPY,
                            26.0,
                            15.0,
                            p.icon,
                            "Copy message",
                            Msg::Noop,
                        ))
                        .child(icon_button(
                            p,
                            icons::PENCIL,
                            26.0,
                            15.0,
                            p.icon,
                            "Edit message",
                            Msg::Noop,
                        ));
                }
                div()
                    .w_full()
                    .flex_col()
                    .items_end()
                    .child(
                        div()
                            .max_w((column * 0.77).floor())
                            .px(12.0)
                            .py(10.0)
                            .bg(p.bubble)
                            .rounded(14.0)
                            .accessibility_role(Role::Button)
                            .accessibility_label("Edit user message")
                            .child(text(body.clone()).size(BODY).color(p.text).line_height(1.5)),
                    )
                    .child(actions)
                    .child(div().h(16.0))
            }
            Item::Thinking => div()
                .w_full()
                .h(40.0)
                .pt(9.0)
                .child(shimmer("Thinking", p, now_ms)),
            Item::Error(msg) => {
                let after = if matches!(next, Some(Item::ModelChanged { .. })) {
                    20.0
                } else {
                    34.0
                };
                div()
                    .w_full()
                    .flex_col()
                    .child(
                        hrow()
                            .w_full()
                            .px(13.0)
                            .py(10.0)
                            .gap(12.0)
                            .bg(p.notice)
                            .border(p.notice_border)
                            .rounded(11.0)
                            .accessibility_role(Role::Alert)
                            .child(div().self_start().pt(1.0).child(ico(
                                icons::ALERT,
                                17.0,
                                p.text,
                            )))
                            .child(div().flex_1().min_w(0.0).child(
                                text(msg.clone()).size(BODY).color(p.text).line_height(1.5),
                            )),
                    )
                    .child(div().h(after))
            }
            Item::ModelChanged { from, to } => {
                let line = || div().flex_1().h(1.0).bg(p.hairline);
                div()
                    .w_full()
                    .flex_col()
                    .child(
                        hrow()
                            .w_full()
                            .h(22.0)
                            .gap(8.0)
                            .accessibility_role(Role::Note)
                            .child(line())
                            .child(ico(icons::CUBE, 14.0, p.divider_text))
                            .child(txt(
                                format!("Model changed from {from} to {to}."),
                                BODY,
                                p.divider_text,
                            ))
                            .child(ico(icons::INFO, 12.0, p.divider_text))
                            .child(line()),
                    )
                    .child(div().h(23.0))
            }
            Item::Assistant(body) => div()
                .w_full()
                .flex_col()
                .child(prose(body, p))
                .child(div().h(14.0)),
            Item::Tool { verb, detail } => hrow()
                .w_full()
                .h(26.0)
                .gap(6.0)
                .child(txt(*verb, SMALL, p.muted))
                .child(txt(detail.clone(), SMALL, p.faint).truncate()),
            Item::Command {
                command,
                output,
                exit,
                secs,
            } => command_card(p, command, output, *exit, *secs),
            Item::Worked(time) => {
                let line = || div().flex_1().h(1.0).bg(p.hairline);
                div()
                    .w_full()
                    .flex_col()
                    .child(div().h(10.0))
                    .child(
                        hrow()
                            .w_full()
                            .h(22.0)
                            .gap(8.0)
                            .child(txt(format!("Worked for {time}"), SMALL, p.muted))
                            .child(ico(icons::CHEVRON_DOWN, 12.0, p.muted))
                            .child(line()),
                    )
                    .child(div().h(14.0))
            }
            Item::FilesChanged(files) => files_card(p, files),
            Item::Approval { command, reason } => approval_card(p, command, reason),
        };
        col = col.child(el);
    }
    col
}

/// "Thinking" with a light band sweeping across it: each letter's color
/// is mixed toward the highlight by its distance from the band.
pub fn shimmer(label: &str, p: &Pal, now_ms: u64) -> Div {
    let base = p.faint;
    let high = p.text;
    let n = label.chars().count() as f32;
    // The band crosses the word and a gap as wide again every 1.6 s.
    let phase = (now_ms % 1600) as f32 / 1600.0;
    let center = phase * (n * 2.0) - n * 0.5;
    let mut row = hrow()
        .accessibility_role(Role::Status)
        .accessibility_label(label.to_owned());
    for (i, ch) in label.chars().enumerate() {
        let d = ((i as f32 - center).abs() / 2.2).min(1.0);
        let color = base.lerp(high, 1.0 - d);
        row = row.child(txt(ch.to_string(), BODY, color));
    }
    row
}

/// Assistant prose with `code` spans set in mono chips.
fn prose(body: &str, p: &Pal) -> Div {
    let mut flow = div().w_full().flex_row().flex_wrap().items_center();
    for (i, part) in body.split('`').enumerate() {
        if i % 2 == 1 {
            flow = flow.child(
                div().px(4.0).rounded(4.0).bg(p.bubble).child(
                    text(part.to_owned())
                        .size(CODE + 0.5)
                        .mono()
                        .color(p.text_soft)
                        .no_wrap(),
                ),
            );
        } else {
            for word in part.split_inclusive(' ') {
                flow = flow.child(
                    text(word.replace(' ', "\u{a0}"))
                        .size(BODY)
                        .color(p.text_soft)
                        .no_wrap()
                        .line_height(1.55),
                );
            }
        }
    }
    flow
}

fn command_card(p: &Pal, command: &str, output: &str, exit: i32, secs: u32) -> Div {
    let mut out = div().flex_col().px(12.0).pb(10.0).gap(2.0);
    for line in output.lines() {
        let color = if line.starts_with('✖') {
            p.error
        } else if line.starts_with('✔') {
            p.success
        } else {
            p.muted
        };
        out = out.child(
            text(line.to_owned())
                .size(CODE)
                .mono()
                .color(color)
                .no_wrap(),
        );
    }
    div()
        .w_full()
        .flex_col()
        .child(
            div()
                .w_full()
                .flex_col()
                .bg(p.tray)
                .border(p.hairline)
                .rounded(10.0)
                .overflow_hidden()
                .accessibility_role(Role::Group)
                .accessibility_label(format!("Ran {command}"))
                .child(
                    hrow()
                        .h(34.0)
                        .px(12.0)
                        .gap(8.0)
                        .child(ico(icons::TERMINAL, 14.0, p.muted))
                        .child(txt("Ran", SMALL, p.muted))
                        .child(
                            text(command.to_owned())
                                .size(CODE)
                                .mono()
                                .color(p.text_soft)
                                .no_wrap(),
                        )
                        .child(div().flex_1())
                        .child(txt(
                            if exit == 0 {
                                format!("{secs}s")
                            } else {
                                format!("exit {exit} · {secs}s")
                            },
                            SMALL,
                            if exit == 0 { p.faint } else { p.error },
                        )),
                )
                .child(out),
        )
        .child(div().h(12.0))
}

fn files_card(p: &Pal, files: &[(String, u32, u32)]) -> Div {
    let (adds, dels) = files.iter().fold((0, 0), |(a, d), f| (a + f.1, d + f.2));
    let stat = |a: u32, d: u32| {
        hrow()
            .gap(4.0)
            .child(txt(format!("+{a}"), SMALL, p.success))
            .child(txt(format!("-{d}"), SMALL, p.error))
    };
    let mut card = div()
        .w_full()
        .flex_col()
        .bg(p.tray)
        .border(p.hairline)
        .rounded(12.0)
        .accessibility_role(Role::Group)
        .accessibility_label(format!("{} files changed", files.len()))
        .child(
            hrow()
                .h(38.0)
                .px(14.0)
                .gap(6.0)
                .child(txt(
                    format!(
                        "{} file{} changed",
                        files.len(),
                        if files.len() == 1 { "" } else { "s" }
                    ),
                    SMALL,
                    p.text_soft,
                ))
                .child(stat(adds, dels))
                .child(div().flex_1())
                .child(txt("Undo", SMALL, p.text_soft)),
        );
    for (name, a, d) in files {
        card = card.child(div().w_full().h(1.0).bg(p.hairline)).child(
            hrow()
                .h(36.0)
                .px(14.0)
                .gap(6.0)
                .on_click(Msg::OpenFile("cart.js"))
                .child(txt(name.clone(), SMALL, p.text_soft))
                .child(stat(*a, *d)),
        );
    }
    div().w_full().flex_col().child(card).child(div().h(14.0))
}

fn approval_card(p: &Pal, command: &str, reason: &str) -> Div {
    let button = |label: &str, primary: bool| {
        hrow()
            .h(28.0)
            .px(12.0)
            .rounded(14.0)
            .bg(if primary { p.send_active } else { p.menu_hi })
            .accessibility_role(Role::Button)
            .accessibility_label(label.to_owned())
            .child(txt(
                label,
                SMALL,
                if primary {
                    p.send_active_glyph
                } else {
                    p.text_soft
                },
            ))
    };
    div()
        .w_full()
        .flex_col()
        .p(14.0)
        .gap(10.0)
        .bg(p.notice)
        .border(p.menu_border)
        .rounded(14.0)
        .accessibility_role(Role::AlertDialog)
        .accessibility_label("Approve command")
        .child(
            hrow()
                .gap(8.0)
                .child(ico(icons::SHIELD_TERM, 16.0, p.text))
                .child(
                    text("Allow Codex to run this command?")
                        .size(BODY)
                        .medium()
                        .color(p.text)
                        .no_wrap(),
                ),
        )
        .child(txt(reason, SMALL, p.muted))
        .child(
            div().w_full().px(10.0).py(8.0).rounded(8.0).bg(p.bg).child(
                text(command.to_owned())
                    .size(CODE)
                    .mono()
                    .color(p.text_soft)
                    .no_wrap(),
            ),
        )
        .child(
            hrow()
                .gap(8.0)
                .child(div().flex_1())
                .child(button("Always allow", false))
                .child(button("Deny", false))
                .child(button("Allow", true)),
        )
}

pub fn white() -> Color {
    Color::rgba(255, 255, 255, 255)
}
