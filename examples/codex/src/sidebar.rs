//! The inset sidebar card: the "Codex ▾" mode switcher with the activity
//! bell and search, New chat and "Your dot", the Projects tree, and the
//! flat Recents list; or, with the bell on, the activity list. Geometry
//! follows the update captures' accessibility frames (card-local here:
//! the card starts at window x 52, y 44).

use accesskit::Role;
use quark_app::quark_ui::element::*;
use quark_app::quark_ui::style::Styled;
use quark_app::quark_ui::theme::Color;

use crate::data::{Status, Thread};
use crate::icons;
use crate::theme::{BODY, Pal, SMALL};
use crate::widgets::*;
use crate::{Codex, Menu, Msg, Screen};

const ROW_W: f32 = 272.0;

fn nav_row(p: &Pal, icon: Div, label: &str, msg: Msg) -> Div {
    hrow()
        .w(ROW_W)
        .h(30.0)
        .flex_shrink_0()
        .pl(8.0)
        .gap(8.0)
        .rounded(8.0)
        .hover_bg(p.row_hover)
        .accessibility_role(Role::Button)
        .accessibility_label(label.to_owned())
        .on_click(msg)
        .child(icon)
        .child(txt(label, BODY, p.sidebar_text))
}

/// A chat row; `indent` puts the title under a project's name.
fn thread_row(app: &Codex, p: &Pal, t: &Thread, indent: bool) -> Div {
    let selected = app.screen == Screen::Thread(t.id);
    let mut row = hrow()
        .w(ROW_W)
        .h(30.0)
        .flex_shrink_0()
        .pl(if indent { 32.0 } else { 8.0 })
        .pr(10.0)
        .gap(6.0)
        .rounded(8.0)
        .when(selected, |d| d.bg(p.row_selected))
        .hover_bg(if selected {
            p.row_selected
        } else {
            p.row_hover
        })
        .accessibility_role(Role::ListItem)
        .accessibility_label(t.title.clone())
        .accessibility_selected(selected)
        .on_click(Msg::Select(t.id))
        .child(clipped(
            t.title.clone(),
            p,
            if selected { p.row_selected } else { p.sidebar },
        ));
    match t.status {
        Status::Running => row = row.child(ico(icons::LOADER, 13.0, p.sidebar_muted)),
        Status::Awaiting => {
            row = row
                .child(
                    hrow()
                        .h(22.0)
                        .px(9.0)
                        .rounded(11.0)
                        .bg(Color::rgba(0x1f, 0x4a, 0x31, 255))
                        .max_w(116.0)
                        .overflow_hidden()
                        .child(
                            txt(
                                "Awaiting approval",
                                SMALL,
                                Color::rgba(0x4c, 0xd2, 0x86, 255),
                            )
                            .truncate(),
                        ),
                )
                .child(div().w(6.0))
                .child(ico(icons::LOADER, 13.0, p.sidebar_muted))
        }
        Status::Error => row = row.child(ico(icons::ALERT, 15.0, p.error)),
        Status::Idle => {}
    }
    row
}

/// A title clipped at the row's edge under a short fade, as the app does
/// (no ellipsis). The fade is painted in `bg`, the row's fill.
fn clipped(title: String, p: &Pal, bg: Color) -> Div {
    div()
        .flex_1()
        .min_w(0.0)
        .overflow_hidden()
        .relative()
        .child(txt(title, BODY, p.sidebar_text))
        .child(
            div()
                .absolute()
                .right(0.0)
                .top(0.0)
                .w(22.0)
                .h(18.0)
                .bg_effect(linear_gradient(0.0, bg.with_alpha(0), bg)),
        )
}

fn header(p: &Pal, label: &str) -> Div {
    hrow()
        .w(ROW_W - 8.0)
        .h(26.0)
        .flex_shrink_0()
        .pl(8.0)
        .child(txt(label, BODY, p.sidebar_muted))
}

pub fn view(app: &Codex, p: &Pal, w: f32, h: f32) -> AnyElement {
    let switcher = hrow()
        .absolute()
        .left(10.0)
        .top(10.0)
        .h(32.0)
        .px(6.0)
        .gap(6.0)
        .rounded(8.0)
        .hover_bg(p.row_hover)
        .when(app.menu == Some(Menu::Mode), |d| d.bg(p.row_selected))
        .id("sidebar.mode")
        .accessibility_role(Role::Button)
        .accessibility_label("Switch mode, current mode: Codex")
        .on_click(Msg::Open(Menu::Mode))
        .child(text("Codex").size(18.0).semibold().color(p.text).no_wrap())
        .child(ico(icons::CHEVRON_DOWN, 13.0, p.sidebar_muted));
    let mut bell = icon_button(
        p,
        icons::BELL,
        28.0,
        16.0,
        if app.activity {
            p.accent
        } else {
            p.sidebar_muted
        },
        "View activity",
        Msg::ToggleActivity,
    );
    if app.activity {
        bell = bell.bg(p.badge);
    }
    let tools = hrow()
        .absolute()
        .left(216.0)
        .top(12.0)
        .gap(4.0)
        .child(bell)
        .child(icon_button(
            p,
            icons::SEARCH,
            28.0,
            15.0,
            p.sidebar_muted,
            "Search",
            Msg::Palette(true),
        ));

    let dot = div()
        .w(16.0)
        .h(16.0)
        .items_center()
        .justify_center()
        .child(ico(icons::PET, 15.0, p.accent));
    let nav = div()
        .absolute()
        .left(8.0)
        .top(52.0)
        .flex_col()
        .gap(1.0)
        .child(nav_row(
            p,
            div().child(ico(icons::COMPOSE, 16.0, p.sidebar_text)),
            "New chat",
            Msg::NewChat,
        ))
        .child(
            nav_row(p, dot, "Your dot", Msg::Noop)
                .pr(6.0)
                .child(div().flex_1())
                .child(
                    hrow()
                        .w(24.0)
                        .h(20.0)
                        .justify_center()
                        .rounded(10.0)
                        .bg(p.badge)
                        .child(txt("1", SMALL, p.badge_text)),
                ),
        );

    let mut list = div().flex_col().w(ROW_W).flex_shrink_0();
    if app.activity {
        list = list.child(
            header(p, "Priority")
                .child(div().flex_1())
                .child(ico(icons::ELLIPSIS, 15.0, p.sidebar_muted))
                .child(div().w(12.0))
                .child(ico(icons::CLEAR_ALL, 15.0, p.sidebar_muted))
                .child(div().w(10.0)),
        );
        list = list.child(div().h(6.0));
        for (i, t) in app.data.threads.iter().take(2).enumerate() {
            let preview = if i == 0 {
                "The note wasn’t written because .codex is protected…"
            } else {
                "codex-demo"
            };
            let selected = app.screen == Screen::Thread(t.id);
            list =
                list.child(
                    div()
                        .w(ROW_W)
                        .flex_col()
                        .px(8.0)
                        .py(6.0)
                        .gap(2.0)
                        .rounded(8.0)
                        .when(selected, |d| d.bg(p.row_selected))
                        .on_click(Msg::Select(t.id))
                        .child(
                            hrow()
                                .gap(4.0)
                                .child(
                                    div().flex_1().min_w(0.0).overflow_hidden().child(
                                        txt(t.title.clone(), BODY, p.sidebar_text).truncate(),
                                    ),
                                )
                                .child(txt(crate::data::DEMO_PROJECT, BODY, p.sidebar_muted)),
                        )
                        .child(
                            text(preview)
                                .size(SMALL)
                                .color(p.sidebar_muted)
                                .line_height(1.3)
                                .wrap_width(190.0),
                        ),
                );
            if i == 0 {
                list = list
                    .child(div().h(18.0))
                    .child(header(p, "Today"))
                    .child(div().h(4.0));
            }
        }
    } else {
        list = list.child(header(p, "Projects")).child(div().h(6.0));
        for project in &app.data.projects {
            let threads: Vec<&Thread> = app.data.threads_in(project.id).collect();
            let selected = app.screen == Screen::Home && app.project == Some(project.id);
            list = list.child(
                hrow()
                    .w(ROW_W)
                    .h(30.0)
                    .flex_shrink_0()
                    .pl(8.0)
                    .gap(8.0)
                    .rounded(8.0)
                    .when(selected, |d| d.bg(p.row_selected))
                    .hover_bg(if selected {
                        p.row_selected
                    } else {
                        p.row_hover
                    })
                    .accessibility_role(Role::Button)
                    .accessibility_label(project.name)
                    .on_click(Msg::ChooseProject(Some(project.id)))
                    .child(ico(icons::FOLDER_OPEN, 16.0, p.sidebar_text))
                    .child(txt(project.name, BODY, p.sidebar_text)),
            );
            list = list.child(div().h(1.0));
            for t in &threads {
                list = list.child(thread_row(app, p, t, true)).child(div().h(1.0));
            }
        }
        list = list
            .child(div().h(24.0))
            .child(header(p, "Recents"))
            .child(div().h(6.0));
        for t in &app.data.threads {
            list = list.child(thread_row(app, p, t, false)).child(div().h(1.0));
        }
    }
    let list = div()
        .absolute()
        .left(8.0)
        .top(127.0)
        .w(ROW_W)
        .h((h - 127.0).max(0.0))
        .flex_col()
        .overflow_hidden()
        .accessibility_role(Role::List)
        .accessibility_label(if app.activity { "Activity" } else { "Chats" })
        .child(list);

    div()
        .absolute()
        .left(0.0)
        .top(0.0)
        .w(w)
        .h(h)
        .bg(p.sidebar)
        .border_r(p.frame_border.lerp(p.text, 0.06))
        .accessibility_role(Role::Navigation)
        .accessibility_label("Sidebar")
        .child(switcher)
        .child(tools)
        .child(nav)
        .child(list)
        .into_any()
}
