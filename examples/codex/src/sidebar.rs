//! The sidebar: top navigation, the Projects list (five threads each,
//! then "Show more"), and the account footer with its Update pill.
//! Geometry follows the capture's accessibility frames: 280-point rows at
//! x 10, a 31-point pitch, icons at x 18 and labels at x 42.

use accesskit::Role;
use quark_app::quark_ui::element::*;
use quark_app::quark_ui::style::Styled;
use quark_app::quark_ui::theme::Color;

use crate::data::{Status, Thread};
use crate::icons;
use crate::theme::{BODY, Pal, SMALL, TINY};
use crate::widgets::*;
use crate::{Codex, Menu, Msg, SIDEBAR_W, Screen};

const ROW_W: f32 = 280.0;
const FOOTER_TOP: f32 = 680.0;
/// Threads a project shows before "Show more".
const PER_PROJECT: usize = 5;

fn nav_row(p: &Pal, svg: &'static str, label: &str, msg: Msg, h: f32) -> Div {
    hrow()
        .w(ROW_W)
        .h(h)
        .pl(8.0)
        .gap(8.0)
        .rounded(8.0)
        .hover_bg(p.row_hover)
        .accessibility_role(Role::Button)
        .accessibility_label(label.to_owned())
        .on_click(msg)
        .child(ico(svg, 16.0, p.sidebar_text))
        .child(txt(label, BODY, p.sidebar_text))
}

fn thread_row(app: &Codex, p: &Pal, t: &Thread) -> Div {
    let selected = app.screen == Screen::Thread(t.id);
    let status = if t.status == Status::Running {
        Some(
            div()
                .w(16.0)
                .h(16.0)
                .child(ico(icons::LOADER, 14.0, p.sidebar_text)),
        )
    } else if t.status == Status::Error {
        Some(div().child(ico(icons::ALERT, 16.0, p.error)))
    } else {
        None
    };
    let lead = div().w(16.0).h(16.0).flex_shrink_0();
    let lead = match (t.status, status) {
        (Status::Error, Some(s)) => lead.child(s),
        _ => lead,
    };
    // Cloud glyph, then the age right-aligned in a 28-point box.
    let mut right = hrow().w(48.0).flex_shrink_0().justify_end();
    if t.status == Status::Running {
        right = right.child(ico(icons::LOADER, 14.0, p.sidebar_text));
    } else {
        if t.cloud {
            right = right.child(ico(icons::CLOUD, 15.0, p.sidebar_meta));
        }
        right = right.child(div().w(28.0).flex_row().justify_end().child(txt(
            t.age,
            SMALL,
            p.sidebar_meta,
        )));
    }
    hrow()
        .w(ROW_W)
        .h(30.0)
        .flex_shrink_0()
        .pl(8.0)
        .pr(8.0)
        .gap(8.0)
        .rounded(8.0)
        .when(selected, |d| d.bg(p.row_selected))
        .hover_bg(if selected {
            p.row_selected
        } else {
            p.row_hover
        })
        .id(format!("thread:{}", t.id.0))
        .accessibility_role(Role::ListItem)
        .accessibility_label(t.title.clone())
        .accessibility_selected(selected)
        .on_click(Msg::Select(t.id))
        .child(lead)
        .child(
            div()
                .flex_1()
                .min_w(0.0)
                .overflow_hidden()
                .child(txt(t.title.clone(), BODY, p.sidebar_text).truncate()),
        )
        .child(right)
}

fn project_header(p: &Pal, name: &str, id: crate::data::ProjectId) -> Div {
    let hidden = Color::TRANSPARENT;
    hrow()
        .w(ROW_W)
        .h(30.0)
        .flex_shrink_0()
        .pl(6.0)
        .pr(4.0)
        .gap(8.0)
        .rounded(8.0)
        .hover_icon_color(p.sidebar_muted)
        .accessibility_role(Role::Button)
        .accessibility_label(name.to_owned())
        .on_click(Msg::ToggleProject(id))
        .child(div().child(ico(icons::PROJECT, 16.0, p.sidebar_text)))
        .child(txt(name, BODY, p.sidebar_text))
        .child(div().flex_1())
        .child(icon_button(
            p,
            icons::ELLIPSIS,
            24.0,
            14.0,
            hidden,
            &format!("Project actions for {name}"),
            Msg::Open(Menu::ProjectActions(id)),
        ))
        .child(icon_button(
            p,
            icons::COMPOSE,
            24.0,
            14.0,
            hidden,
            &format!("Start new chat in {name}"),
            Msg::ChooseProject(Some(id)),
        ))
}

fn section_header(p: &Pal, label: &str) -> Div {
    let hidden = Color::TRANSPARENT;
    hrow()
        .w(ROW_W)
        .h(25.0)
        .flex_shrink_0()
        .pl(8.0)
        .pr(2.0)
        .hover_icon_color(p.sidebar_muted)
        .child(txt(label, SMALL, p.sidebar_muted))
        .child(div().flex_1())
        .child(icon_button(
            p,
            icons::COLLAPSE_ALL,
            24.0,
            14.0,
            hidden,
            "Collapse all",
            Msg::Noop,
        ))
        .child(icon_button(
            p,
            icons::ELLIPSIS,
            24.0,
            14.0,
            hidden,
            "Project sidebar options",
            Msg::Open(Menu::SidebarOptions),
        ))
        .child(icon_button(
            p,
            icons::FOLDER,
            24.0,
            14.0,
            hidden,
            "Add new project",
            Msg::Noop,
        ))
}

pub fn view(app: &Codex, p: &Pal, h: f32) -> AnyElement {
    let nav = div()
        .absolute()
        .left(10.0)
        .top(46.0)
        .flex_col()
        .gap(1.0)
        .child(nav_row(p, icons::COMPOSE, "New chat", Msg::NewChat, 29.0))
        .child(nav_row(
            p,
            icons::SEARCH,
            "Search",
            Msg::Palette(true),
            29.0,
        ))
        .child(div().h(1.0))
        .child(nav_row(p, icons::CLOCK, "Scheduled", Msg::Noop, 30.0))
        .child(nav_row(p, icons::AT, "Plugins", Msg::Noop, 30.0));

    let mut list = div().flex_col().w(ROW_W).flex_shrink_0();
    for project in &app.data.projects {
        let threads: Vec<&Thread> = app.data.threads_in(project.id).collect();
        // The demo project shows only once a scene or the user opened it.
        if project.id.0 == 1 && app.project != Some(project.id) && threads.is_empty() {
            continue;
        }
        list = list.child(project_header(p, project.name, project.id));
        list = list.child(div().h(2.0));
        if !app.collapsed.contains(&project.id) {
            if threads.is_empty() {
                list = list.child(hrow().h(30.0).pl(32.0).child(txt(
                    "No chats",
                    BODY,
                    p.sidebar_muted,
                )));
            }
            for t in threads.iter().take(PER_PROJECT) {
                list = list.child(thread_row(app, p, t)).child(div().h(1.0));
            }
            if threads.len() > PER_PROJECT {
                list = list.child(hrow().h(24.0).pl(23.0).child(txt(
                    "Show more",
                    BODY,
                    p.sidebar_muted,
                )));
            }
        }
        list = list.child(div().h(8.0));
    }
    let list_top = 184.0;
    let list_h = (FOOTER_TOP - list_top).max(0.0);
    let projects = div()
        .absolute()
        .left(10.0)
        .top(list_top)
        .w(ROW_W)
        .h(list_h)
        .flex_col()
        .overflow_hidden()
        .accessibility_role(Role::List)
        .accessibility_label("Projects")
        .child(section_header(p, "Projects"))
        .child(div().h(4.0).flex_shrink_0())
        .child(list);
    // The list fades out over its last rows.
    let fade = div()
        .absolute()
        .left(0.0)
        .top(FOOTER_TOP - 36.0)
        .w(SIDEBAR_W)
        .h(36.0)
        .bg_effect(linear_gradient(
            std::f32::consts::FRAC_PI_2,
            p.sidebar.with_alpha(0),
            p.sidebar.with_alpha(235),
        ));

    let footer = hrow()
        .absolute()
        .left(8.0)
        .top(h - 50.0)
        .w(284.0)
        .h(40.0)
        .child(
            hrow()
                .w(220.0)
                .h(40.0)
                .pl(8.0)
                .gap(12.0)
                .rounded(8.0)
                .hover_bg(p.row_hover)
                .accessibility_role(Role::Button)
                .accessibility_label("Open settings")
                .on_click(Msg::Open(Menu::Account))
                .child(
                    div()
                        .w(28.0)
                        .h(28.0)
                        .rounded(14.0)
                        .bg(p.avatar)
                        .items_center()
                        .justify_center()
                        .child(txt("SE", TINY, Color::rgba(255, 255, 255, 255))),
                )
                .child(
                    div()
                        .flex_col()
                        .child(txt(crate::data::ACCOUNT_NAME, BODY, p.text))
                        .child(txt(crate::data::ACCOUNT_PLAN, 12.0, p.sidebar_meta)),
                ),
        )
        .child(div().flex_1())
        .child(update_pill(p));

    div()
        .absolute()
        .left(0.0)
        .top(0.0)
        .w(SIDEBAR_W)
        .h(h)
        .bg(p.sidebar)
        .accessibility_role(Role::Navigation)
        .accessibility_label("Sidebar")
        .child(nav)
        .child(projects)
        .child(fade)
        .child(footer)
        .into_any()
}

pub fn update_pill(p: &Pal) -> Div {
    hrow()
        .h(20.0)
        .px(10.0)
        .rounded(10.0)
        .bg(p.accent)
        .accessibility_role(Role::Button)
        .accessibility_label("Update")
        .child(
            text("Update")
                .size(TINY)
                .medium()
                .color(Color::rgba(255, 255, 255, 255))
                .no_wrap(),
        )
}
