//! The search palette (Search in the sidebar, Cmd+K): chats with Cmd+1..9
//! shortcuts, then suggested commands; typing filters the chats.

use accesskit::Role;
use quark::view;
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
        return view! { <div /> };
    };
    let w = 518.0_f32.min(app.size.0 - 24.0);
    let x = ((app.size.0 - w) / 2.0).round();
    let matches = state.matches(&app.data);
    let suggested =
        crate::data::SUGGESTED
            .iter()
            .zip([icons::COMPOSE, icons::FOLDER_OPEN, icons::SETTINGS]);
    view! {
        <div class="absolute left-0 top-0 z-65" w={app.size.0} h={app.size.1}>
            <div class="absolute left-0 top-0" w={app.size.0} h={app.size.1}
                 on:click={Msg::Palette(false)} block_mouse />
            <menu_panel(p, x, 118.0, w) class="rounded-[14] p-1 z-70"
                        accessibility_role={Role::Dialog} aria-label="Search">
                <div class="flex-row items-center h-10 px-[10]">
                    <text_input("Search chats or run a command", "")
                        placeholder="Search chats or run a command" focus_target={PALETTE_FOCUS}
                        focused={vcx.is_focused(PALETTE_FOCUS)} field={&state.field} bare
                        w={w - 28.0} class="h-5" />
                </div>
                {menu_header(p, "Chats")}
                if matches.is_empty() {
                    <div class="flex-row items-center h-[31] px-2">
                        <txt("No matches", BODY, p.menu_desc) />
                    </div>
                }
                for (i, t) in matches.iter().enumerate() {
                    let project = t
                        .project
                        .and_then(|id| app.data.project(id))
                        .map_or("", |p| p.name);
                    <div class="flex-row items-center h-[31] pl-9 pr-2 gap-[10] rounded-[7]"
                         bg={if i == 0 { p.menu_hi }} hover_bg={p.menu_hi} role="option"
                         aria-label={t.title.clone()} on:click={Msg::Select(t.id)}>
                        <div class="flex-1 min-w-0 overflow-hidden">
                            <txt(t.title.clone(), BODY, p.menu_title) class="truncate" />
                        </div>
                        <txt(project, SMALL, p.menu_desc) />
                        {kbd(p, &format!("⌘{}", i + 1))}
                    </div>
                }
                {menu_header(p, "Suggested")}
                for ((label, keys), icon) in suggested {
                    let msg = match *label {
                        "New chat" => Msg::NewChat,
                        "Settings" => Msg::Show(crate::Screen::Settings(crate::settings::Page::General)),
                        _ => Msg::Palette(false),
                    };
                    <div class="flex-row items-center h-[31] px-2 gap-[10] rounded-[7]"
                         hover_bg={p.menu_hi} role="option" aria-label={label.to_string()}
                         on:click={msg}>
                        <icon svg={icon} size={16.0} color={p.menu_title} />
                        <txt(*label, BODY, p.menu_title) />
                        <div class="flex-1" />
                        {kbd(p, keys)}
                    </div>
                }
            </menu_panel>
        </div>
    }
}
