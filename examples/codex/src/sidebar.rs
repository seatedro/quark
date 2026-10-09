//! The inset sidebar card: the "Codex ▾" mode switcher with the activity
//! bell and search, New chat and "Your dot", the Projects tree, and the
//! flat Recents list; or, with the bell on, the activity list. Geometry
//! follows the update captures' accessibility frames (card-local here:
//! the card starts at window x 52, y 44).

use accesskit::Role;
use quark::{FadeEdge, view};
use quark_app::quark_ui::element::*;
use quark_app::quark_ui::style::Styled;
use quark_app::quark_ui::theme::Color;

use crate::data::{Status, Thread};
use crate::icons;
use crate::theme::{BODY, Pal, SMALL};
use crate::widgets::*;
use crate::{Codex, Menu, Msg, Screen};

const ROW_W: f32 = 272.0;

fn nav_row(p: &Pal, icon: AnyElement, label: &str, msg: Msg) -> Div {
    view! { -> Div,
        <div class="flex-row items-center shrink-0 h-[30] pl-2 gap-2 rounded-[8]" w={ROW_W}
             hover_bg={p.row_hover} role="button" aria-label={label.to_owned()} on:click={msg}>
            {icon}
            <txt(label, BODY, p.sidebar_text) />
        </div>
    }
}

/// Rows reveal their actions while hovered or holding focus.
const ROW: GroupId = GroupId::new("sidebar.thread");

/// A chat row; `indent` puts the title under a project's name. The title
/// fades out at the row's edge instead of ending in an ellipsis, as the
/// app does. Hovering or focusing the row swaps its status for Pin and
/// Archive, in the same trailing space so nothing moves.
fn thread_row(app: &Codex, p: &Pal, t: &Thread, indent: bool) -> AnyElement {
    let selected = app.screen == Screen::Thread(t.id);
    let fill = if selected { p.row_selected } else { p.row_hover };
    view! {
        <div class="relative flex-row items-center shrink-0 h-[30] pr-[10] gap-1.5 rounded-[8]"
             w={ROW_W} pl={if indent { 32.0 } else { 8.0 }} bg={if selected { p.row_selected }}
             hover_bg={fill} role="listitem" aria-label={t.title.clone()} aria-selected={selected}
             on:click={Msg::Select(t.id)} interaction_group={ROW}>
            <div class="flex-1 min-w-0 overflow-hidden">
                <txt(t.title.clone(), BODY, p.sidebar_text) overflow={TextOverflow::Fade(22.0)} />
            </div>
            <div class="flex-row items-center gap-1.5" show_when={GroupCondition::Idle(ROW)}>
                match t.status {
                    Status::Running => <icon svg={icons::LOADER} size={13.0} color={p.sidebar_muted} />
                    Status::Awaiting => {
                        <div class="flex-row items-center h-[22] px-[9] rounded-[11] bg-[#1f4a31]
                                    max-w-[116] overflow-hidden">
                            <txt("Awaiting approval", SMALL, Color::rgba(0x4c, 0xd2, 0x86, 255))
                                 class="truncate" />
                        </div>
                        <div class="w-1.5" />
                        <icon svg={icons::LOADER} size={13.0} color={p.sidebar_muted} />
                    }
                    Status::Error => <icon svg={icons::ALERT} size={15.0} color={p.error} />
                    Status::Idle => {}
                }
            </div>
            <div class="absolute right-0 top-0 h-full flex-row"
                 show_when={GroupCondition::HoveredOrFocusWithin(ROW)}
                 fade_edge={(FadeEdge::Left, 22.0)}>
                <div class="flex-row items-center h-full pl-[22] pr-1 gap-0.5" bg={fill}
                     rounded_corners={[0.0, 8.0, 8.0, 0.0]}>
                    <icon_button(p, icons::PIN, 24.0, 14.0, p.sidebar_muted, "Pin chat", Msg::Noop) />
                    <icon_button(p, icons::ARCHIVE, 24.0, 14.0, p.sidebar_muted, "Archive chat",
                                 Msg::Noop) />
                </div>
            </div>
        </div>
    }
}

fn header(p: &Pal, label: &str) -> Div {
    view! { -> Div,
        <div class="flex-row items-center h-[26] shrink-0 pl-2" w={ROW_W - 8.0}>
            <txt(label, BODY, p.sidebar_muted) />
        </div>
    }
}

pub fn view(app: &Codex, p: &Pal, w: f32, h: f32) -> AnyElement {
    let bell = if app.activity {
        p.accent
    } else {
        p.sidebar_muted
    };
    view! {
        <div class="absolute left-0 top-0" w={w} h={h} bg={p.sidebar}
             border_r={p.frame_border.lerp(p.text, 0.06)}
             accessibility_role={Role::Navigation} aria-label="Sidebar">
            <div class="flex-row items-center absolute left-[10] top-[10] h-8 px-1.5 gap-1.5
                        rounded-[8]"
                 hover_bg={p.row_hover} bg={if app.menu == Some(Menu::Mode) { p.row_selected }}
                 id="sidebar.mode" role="button" aria-label="Switch mode, current mode: Codex"
                 on:click={Msg::Open(Menu::Mode)}>
                <text size={18.0} color={p.text} class="font-semibold whitespace-nowrap">"Codex"</text>
                <icon svg={icons::CHEVRON_DOWN} size={13.0} color={p.sidebar_muted} />
            </div>
            <div class="flex-row items-center absolute left-[216] top-3 gap-1">
                <icon_button(p, icons::BELL, 28.0, 16.0, bell, "View activity",
                             Msg::ToggleActivity) bg={if app.activity { p.badge }} />
                <icon_button(p, icons::SEARCH, 28.0, 15.0, p.sidebar_muted, "Search",
                             Msg::Palette(true)) />
            </div>
            <div class="absolute left-2 top-[52] flex-col gap-px">
                <nav_row(p, view! {
                    <div><icon svg={icons::COMPOSE} size={16.0} color={p.sidebar_text} /></div>
                }, "New chat", Msg::NewChat) />
                <nav_row(p, view! {
                    <div class="w-4 h-4 items-center justify-center">
                        <icon svg={icons::PET} size={15.0} color={p.accent} />
                    </div>
                }, "Your dot", Msg::Noop) class="pr-1.5">
                    <div class="flex-1" />
                    <div class="flex-row items-center w-6 h-5 justify-center rounded-[10]"
                         bg={p.badge}>
                        <txt("1", SMALL, p.badge_text) />
                    </div>
                </nav_row>
            </div>
            <div class="absolute left-2 top-[127] flex-col overflow-hidden" w={ROW_W}
                 h={(h - 127.0).max(0.0)} role="list"
                 aria-label={if app.activity { "Activity" } else { "Chats" }}>
                <div class="flex-col shrink-0" w={ROW_W}>
                    if app.activity {
                        <header(p, "Priority")>
                            <div class="flex-1" />
                            <icon svg={icons::ELLIPSIS} size={15.0} color={p.sidebar_muted} />
                            <div class="w-3" />
                            <icon svg={icons::CLEAR_ALL} size={15.0} color={p.sidebar_muted} />
                            <div class="w-[10]" />
                        </header>
                        <div class="h-1.5" />
                        for (i, t) in app.data.threads.iter().take(2).enumerate() {
                            let preview = if i == 0 {
                                "The note wasn’t written because .codex is protected…"
                            } else {
                                "codex-demo"
                            };
                            let selected = app.screen == Screen::Thread(t.id);
                            <div class="flex-col px-2 py-1.5 gap-0.5 rounded-[8]" w={ROW_W}
                                 bg={if selected { p.row_selected }} on:click={Msg::Select(t.id)}>
                                <div class="flex-row items-center gap-1">
                                    <div class="flex-1 min-w-0 overflow-hidden">
                                        <txt(t.title.clone(), BODY, p.sidebar_text) class="truncate" />
                                    </div>
                                    <txt(crate::data::DEMO_PROJECT, BODY, p.sidebar_muted) />
                                </div>
                                <text size={SMALL} color={p.sidebar_muted} line_height={1.3}
                                      wrap_width={190.0}>{preview}</text>
                            </div>
                            if i == 0 {
                                <div class="h-[18]" />
                                <header(p, "Today") />
                                <div class="h-1" />
                            }
                        }
                    } else {
                        <header(p, "Projects") />
                        <div class="h-1.5" />
                        for project in &app.data.projects {
                            let selected = app.screen == Screen::Home && app.project == Some(project.id);
                            <div class="flex-row items-center shrink-0 h-[30] pl-2 gap-2 rounded-[8]"
                                 w={ROW_W} bg={if selected { p.row_selected }}
                                 hover_bg={if selected { p.row_selected } else { p.row_hover }}
                                 role="button" aria-label={project.name}
                                 on:click={Msg::ChooseProject(Some(project.id))}>
                                <icon svg={icons::FOLDER_OPEN} size={16.0} color={p.sidebar_text} />
                                <txt(project.name, BODY, p.sidebar_text) />
                            </div>
                            <div class="h-px" />
                            for t in app.data.threads_in(project.id) {
                                {thread_row(app, p, t, true)}
                                <div class="h-px" />
                            }
                        }
                        <div class="h-6" />
                        <header(p, "Recents") />
                        <div class="h-1.5" />
                        for t in &app.data.threads {
                            {thread_row(app, p, t, false)}
                            <div class="h-px" />
                        }
                    }
                </div>
            </div>
        </div>
    }
}
