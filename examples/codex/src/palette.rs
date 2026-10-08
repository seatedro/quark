//! The search palette (Search in the sidebar, Cmd+K): chats with Cmd+1..9
//! shortcuts, then suggested commands; typing filters the chats.

use accesskit::Role;
use quark_app::ViewContext;
use quark_app::quark_ui::element::*;
use quark_app::quark_ui::style::Styled;
use quark_app::quark_ui::text_input::TextField;

use crate::data::{Data, ThreadId};
use crate::theme::{BODY, Pal, SMALL};
use crate::widgets::*;
use crate::{Codex, Msg, PALETTE_FOCUS, icons};

pub struct State {
    pub field: TextField,
}

impl Default for State {
    fn default() -> Self {
        Self::new()
    }
}

impl State {
    pub fn new() -> Self {
        Self {
            field: TextField::new(""),
        }
    }

    pub fn query(&self) -> String {
        self.field.text().trim().to_lowercase()
    }

    /// Chats matching the query, newest first, at most nine.
    pub fn matches<'a>(&self, data: &'a Data) -> Vec<&'a crate::data::Thread> {
        let q = self.query();
        data.threads
            .iter()
            .filter(|t| t.title.to_lowercase().contains(&q))
            .take(9)
            .collect()
    }

    pub fn first_match(&self, data: &Data) -> Option<ThreadId> {
        self.matches(data).first().map(|t| t.id)
    }
}

pub fn view(app: &Codex, p: &Pal, vcx: &mut ViewContext) -> AnyElement {
    let Some(state) = &app.palette else {
        return div().into_any();
    };
    let w = 518.0_f32.min(app.size.0 - 24.0);
    let x = ((app.size.0 - w) / 2.0).round();
    let input = text_input("Search chats or run a command", "")
        .placeholder("Search chats or run a command")
        .focus_target(PALETTE_FOCUS)
        .focused(vcx.is_focused(PALETTE_FOCUS))
        .field(&state.field)
        .bare()
        .w(w - 28.0)
        .h(20.0);
    let mut panel = menu_panel(p, x, 118.0, w)
        .rounded(14.0)
        .p(4.0)
        .z_index(70)
        .accessibility_role(Role::Dialog)
        .accessibility_label("Search")
        .child(hrow().h(40.0).px(10.0).child(input))
        .child(menu_header(p, "Chats"));
    let matches = state.matches(&app.data);
    if matches.is_empty() {
        panel = panel.child(
            hrow()
                .h(31.0)
                .px(8.0)
                .child(txt("No matches", BODY, p.menu_desc)),
        );
    }
    for (i, t) in matches.iter().enumerate() {
        let project = app.data.project(t.project).map_or("", |p| p.name);
        panel = panel.child(
            hrow()
                .h(31.0)
                .pl(36.0)
                .pr(8.0)
                .gap(10.0)
                .rounded(7.0)
                .when(i == 0, |d| d.bg(p.menu_hi))
                .hover_bg(p.menu_hi)
                .accessibility_role(Role::ListBoxOption)
                .accessibility_label(t.title.clone())
                .on_click(Msg::Select(t.id))
                .child(
                    div()
                        .flex_1()
                        .min_w(0.0)
                        .overflow_hidden()
                        .child(txt(t.title.clone(), BODY, p.menu_title).truncate()),
                )
                .child(txt(project, SMALL, p.menu_desc))
                .child(kbd(p, &format!("⌘{}", i + 1))),
        );
    }
    panel = panel.child(menu_header(p, "Suggested"));
    for ((label, keys), icon) in
        crate::data::SUGGESTED
            .iter()
            .zip([icons::COMPOSE, icons::FOLDER_OPEN, icons::SETTINGS])
    {
        let msg = match *label {
            "New chat" => Msg::NewChat,
            "Settings" => Msg::Show(crate::Screen::Settings(crate::settings::Page::General)),
            _ => Msg::Palette(false),
        };
        panel = panel.child(
            hrow()
                .h(31.0)
                .px(8.0)
                .gap(10.0)
                .rounded(7.0)
                .hover_bg(p.menu_hi)
                .accessibility_role(Role::ListBoxOption)
                .accessibility_label(label.to_string())
                .on_click(msg)
                .child(ico(icon, 16.0, p.menu_title))
                .child(txt(*label, BODY, p.menu_title))
                .child(div().flex_1())
                .child(kbd(p, keys)),
        );
    }
    div()
        .absolute()
        .left(0.0)
        .top(0.0)
        .w(app.size.0)
        .h(app.size.1)
        .z_index(65)
        .child(
            div()
                .absolute()
                .left(0.0)
                .top(0.0)
                .w(app.size.0)
                .h(app.size.1)
                .on_click(Msg::Palette(false)),
        )
        .child(panel)
        .into_any()
}
