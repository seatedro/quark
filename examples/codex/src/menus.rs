//! Every popover: permissions, the model and effort card, "+", the mode
//! switcher, the profile menu, chat actions, the summary, the approval
//! split button, the Changes scope and options, new tab, and the tray's
//! pickers; plus the slash and `@` lists the draft opens. Each anchors to
//! its trigger's frame from the last frame's geometry, below it when it
//! fits and above it otherwise.

use quark::{Rect, view};
use quark_app::ViewContext;
use quark_app::quark_ui::element::*;
use quark_app::quark_ui::style::Styled;
use quark_app::quark_ui::theme::Color;

use crate::theme::{BODY, Pal, SMALL};
use crate::widgets::*;
use crate::{APPROVALS, Codex, EFFORTS, MODEL, Menu, Msg, SCOPES, Tab, composer, data, icons};

/// A menu under construction: rows and the height they add up to.
// view!: placement needs the menu's height before it is drawn, so rows
// are pushed here with their heights; each row's markup is built by view!.
struct M<'a> {
    p: &'a Pal,
    w: f32,
    h: f32,
    rows: Vec<AnyElement>,
    items: usize,
    hi: Option<usize>,
}

impl<'a> M<'a> {
    fn new(p: &'a Pal, w: f32, hi: Option<usize>) -> Self {
        Self {
            p,
            w,
            h: 8.0 + 2.0,
            rows: Vec::new(),
            items: 0,
            hi,
        }
    }

    fn header(&mut self, label: &str) -> &mut Self {
        self.rows.push(menu_header(self.p, label).into_any());
        self.h += 28.0;
        self
    }

    fn item(&mut self, mut item: Item, msg: Msg) -> &mut Self {
        let tall = item.desc_below && item.desc.is_some();
        item.hi = item.hi || self.hi == Some(self.items);
        self.items += 1;
        self.rows.push(menu_item(self.p, item, msg).into_any());
        self.h += if tall { 43.0 } else { 29.0 };
        self
    }

    fn sep(&mut self) -> &mut Self {
        self.rows.push(menu_sep(self.p).into_any());
        self.h += 9.0;
        self
    }

    fn custom(&mut self, el: impl IntoAnyElement, h: f32) -> &mut Self {
        self.rows.push(el.into_any());
        self.h += h;
        self
    }

    /// The menu at `(x, y)`, for callers that restyle the panel.
    fn panel(self, (x, y): (f32, f32)) -> Div {
        view! { -> Div,
            <menu_panel(self.p, x, y, self.w) h={self.h}>{...self.rows}</menu_panel>
        }
    }

    fn at(self, at: (f32, f32)) -> AnyElement {
        self.panel(at).into_any()
    }
}

#[derive(Clone, Copy)]
enum Align {
    Left,
    Right,
}

/// Where a `w` x `h` menu goes for a trigger at `r` in a `win` window.
fn place(r: Rect, w: f32, h: f32, align: Align, win: (f32, f32)) -> (f32, f32) {
    let x = match align {
        Align::Left => r.x,
        Align::Right => r.x + r.width - w,
    }
    .clamp(8.0, (win.0 - w - 8.0).max(8.0));
    let below = r.y + r.height + 4.0;
    let y = if below + h <= win.1 - 4.0 {
        below
    } else {
        (r.y - h - 4.0).max(4.0)
    };
    (x, y)
}

fn anchor(vcx: &mut ViewContext, id: &str) -> Option<Rect> {
    match vcx.geometry().by_id(id) {
        Ok(g) => Some(g.bounds),
        Err(_) => {
            // Opened before its trigger was ever drawn: try next frame.
            vcx.frame.request_frame();
            None
        }
    }
}

/// A plain row: icon, title, shortcut, checked, submenu. An empty title
/// is a separator.
type Entry<'a> = (Option<&'static str>, &'a str, Option<&'a str>, bool, bool);

fn simple(m: &mut M, entries: &[Entry], msg: impl Fn(usize) -> Msg) {
    for (i, (icon, title, keys, check, chevron)) in entries.iter().enumerate() {
        if title.is_empty() {
            m.sep();
            continue;
        }
        m.item(
            Item {
                icon: *icon,
                title,
                shortcut: *keys,
                check: *check,
                chevron: *chevron,
                ..Item::default()
            },
            msg(i),
        );
    }
}

pub fn view(app: &Codex, p: &Pal, vcx: &mut ViewContext) -> Option<AnyElement> {
    let win = app.size;
    if let Some(s) = composer_suggestions(app, p, vcx) {
        return Some(s);
    }
    let menu = app.menu?;
    let hi = app.menu_hi;
    let el = match menu {
        Menu::Permissions => {
            let r = anchor(vcx, "pill.permissions")?;
            let w = 419.0;
            let mut m = M::new(p, w, hi);
            m.custom(
                view! {
                    <div class="flex-row items-center h-7 px-2">
                        <txt("How should ChatGPT actions be approved?", SMALL, p.menu_header) />
                        <div class="flex-1" />
                        <txt("Learn more", SMALL, p.menu_desc) />
                    </div>
                },
                28.0,
            );
            for (i, (title, desc, icon)) in APPROVALS.iter().enumerate() {
                let item = Item {
                    icon: Some(icon),
                    title,
                    desc: Some(desc),
                    desc_below: true,
                    check: app.approval == i,
                    hi: hi == Some(i),
                    // Full access reads in orange.
                    tint: (i == 2).then_some(p.orange),
                    ..Item::default()
                };
                m.custom(
                    view! { <menu_item(p, item, Msg::Approval(i)) class="h-[43]" /> },
                    43.0,
                );
            }
            let h = m.h;
            m.at(place(r, w, h, Align::Left, win))
        }
        Menu::Model => {
            let r = anchor(vcx, "pill.model")?;
            let w = 256.0;
            let h = 98.0;
            let (x, y) = place(r, w, h, Align::Right, win);
            view! {
                <menu_panel(p, x + 54.0, y, w) h={h} class="rounded-[12] px-3 pt-2">
                    <div class="flex-row items-center h-10" w={w - 24.0}>
                        <icon svg={icons::BOLT} size={15.0} color={p.muted} />
                        <div class="flex-1 flex-col items-center">
                            <text size={BODY} color={p.accent} class="font-medium whitespace-nowrap">
                                {EFFORTS[app.effort]}
                            </text>
                            <div class="flex-row items-center gap-0.5">
                                <txt(MODEL, 12.0, p.muted) />
                                <icon svg={icons::CHEVRON_RIGHT} size={10.0} color={p.muted} />
                            </div>
                        </div>
                        <icon svg={icons::ROTATE} size={14.0} color={p.muted} />
                    </div>
                    <div class="h-2" />
                    <div class="flex-row items-center h-[26] rounded-[13] px-0.5 justify-between relative"
                         w={w - 24.0} bg={p.menu_hi}>
                        for (i, effort) in EFFORTS.iter().enumerate() {
                            let knob = i == app.effort;
                            <div class="w-6 h-6 items-center justify-center" role="radio"
                                 aria-label={*effort} aria-selected={knob} on:click={Msg::Effort(i)}>
                                if knob {
                                    <div class="w-6 h-6 rounded-[12] bg-white" />
                                } else {
                                    <div class="w-1 h-1 rounded-[2]" bg={p.muted} />
                                }
                            </div>
                        }
                    </div>
                </menu_panel>
            }
        }
        Menu::Add => {
            let r = anchor(vcx, composer::CARD_ID)?;
            add_menu(p, r, win, hi, "")
        }
        Menu::Mode => {
            let r = anchor(vcx, "sidebar.mode")?;
            let w = 241.0;
            let mut m = M::new(p, w, hi);
            m.item(
                Item {
                    title: "ChatGPT",
                    desc: Some("Create, learn, and explore"),
                    desc_below: true,
                    ..Item::default()
                },
                Msg::CloseMenus,
            );
            m.item(
                Item {
                    title: "Codex",
                    desc: Some("Build, debug, and ship"),
                    desc_below: true,
                    check: true,
                    ..Item::default()
                },
                Msg::CloseMenus,
            );
            m.h += 14.0;
            let h = m.h;
            view! { <{m.panel(place(r, w, h, Align::Left, win))} class="py-1.5 rounded-[12]" /> }
        }
        Menu::Profile => {
            let w = 228.0;
            let mut m = M::new(p, w, hi);
            m.custom(
                view! {
                    <div class="flex-row items-center h-10 px-2 gap-[10]">
                        <div class="w-[18] h-[18] rounded-[9] items-center justify-center"
                             bg={p.avatar}>
                            <txt("SE", 7.0, Color::rgba(255, 255, 255, 255)) />
                        </div>
                        <div class="flex-col">
                            <txt(data::ACCOUNT_NAME, BODY, p.menu_title) />
                            <txt(data::ACCOUNT_PLAN, SMALL, p.menu_desc) />
                        </div>
                    </div>
                },
                40.0,
            );
            m.sep();
            simple(
                &mut m,
                &[
                    (Some(icons::GAUGE), "Usage", Some("76% left"), false, false),
                    (Some(icons::PET), "Show Pet", Some("⌥Space"), false, false),
                    (Some(icons::SETTINGS), "Settings", Some("⌘,"), false, false),
                    (None, "", None, false, false),
                    (Some(icons::HELP), "Help", None, false, true),
                    (Some(icons::LOGOUT), "Log out", None, false, false),
                ],
                |i| match i {
                    2 => Msg::Show(crate::Screen::Settings(crate::settings::Page::General)),
                    _ => Msg::CloseMenus,
                },
            );
            let h = m.h;
            view! { <{m.panel((49.0, win.1 - 6.0 - h))} class="rounded-[12]" /> }
        }
        Menu::ChatActions => {
            let r = anchor(vcx, "header.actions")?;
            let mut m = M::new(p, 222.0, hi);
            simple(
                &mut m,
                &[
                    (Some(icons::PIN), "Pin chat", Some("⌥⌘P"), false, false),
                    (
                        Some(icons::PENCIL),
                        "Rename chat",
                        Some("⌥⌘R"),
                        false,
                        false,
                    ),
                    (
                        Some(icons::ARCHIVE),
                        "Archive chat",
                        Some("⇧⌘A"),
                        false,
                        false,
                    ),
                    (None, "", None, false, false),
                    (
                        Some(icons::CHAT_PLUS),
                        "Open side chat",
                        Some("⌥⌘S"),
                        false,
                        false,
                    ),
                    (Some(icons::COPY), "Copy", None, false, true),
                    (Some(icons::FORK), "Fork", None, false, true),
                    (
                        Some(icons::CLOCK),
                        "Add scheduled task...",
                        None,
                        false,
                        false,
                    ),
                    (None, "", None, false, false),
                    (
                        Some(icons::WINDOW_NEW),
                        "Open in new window",
                        None,
                        false,
                        false,
                    ),
                ],
                |_| Msg::CloseMenus,
            );
            let h = m.h;
            m.at(place(r, 222.0, h, Align::Right, win))
        }
        Menu::Summary => {
            let r = anchor(vcx, "header.summary")?;
            summary(p, r, win)
        }
        Menu::ApprovalOptions => {
            let r = anchor(vcx, "approval.options")?;
            let w = 194.0;
            let mut m = M::new(p, w, hi);
            m.item(
                Item {
                    title: "Allow once",
                    ..Item::default()
                },
                Msg::Decide(true),
            );
            m.item(
                Item {
                    title: "Allow similar commands",
                    shortcut: Some("ⓘ"),
                    ..Item::default()
                },
                Msg::Decide(true),
            );
            let h = m.h;
            m.at(place(r, w, h, Align::Right, win))
        }
        Menu::ChangesScope => {
            let r = anchor(vcx, "changes.scope")?;
            let w = 200.0;
            let mut m = M::new(p, w, hi);
            for (i, scope) in SCOPES.iter().enumerate() {
                if i == 1 || i == 4 {
                    m.sep();
                }
                m.item(
                    Item {
                        title: scope,
                        check: app.scope == i,
                        chevron: i == 4,
                        ..Item::default()
                    },
                    Msg::SetScope(i),
                );
            }
            let h = m.h;
            m.at(place(r, w, h, Align::Left, win))
        }
        Menu::ChangesOptions => {
            let r = anchor(vcx, "changes.options")?;
            let w = 220.0;
            let mut m = M::new(p, w, hi);
            simple(
                &mut m,
                &[
                    (Some(icons::REFRESH_CW), "Refresh", None, false, false),
                    (Some(icons::WRAP), "Word wrap", None, false, false),
                    (
                        Some(icons::SPLIT_DIFF),
                        "Switch to Auto diff",
                        None,
                        false,
                        false,
                    ),
                    (
                        Some(icons::LIST_FILTER),
                        "Expand all diffs",
                        None,
                        false,
                        false,
                    ),
                    (None, "", None, false, false),
                    (Some(icons::DOC), "Load full files", None, true, false),
                    (Some(icons::BROWSER), "Rich preview", None, false, false),
                    (Some(icons::REVIEW), "Word diffs", None, true, false),
                    (Some(icons::INFO), "Hide white space", None, false, false),
                    (Some(icons::CUBE), "Hide imports", None, false, false),
                ],
                |_| Msg::CloseMenus,
            );
            m.item(
                Item {
                    icon: Some(icons::COPY),
                    title: "Copy git apply command",
                    dim: true,
                    ..Item::default()
                },
                Msg::CloseMenus,
            );
            let h = m.h;
            m.at(place(r, w, h, Align::Left, win))
        }
        Menu::PanelTab => {
            let r = anchor(vcx, "panel.newtab")?;
            let mut m = M::new(p, 222.0, hi);
            let entries = [
                (
                    icons::REVIEW,
                    "Changes",
                    Some("⌃⇧G"),
                    Msg::OpenTab(Tab::Changes),
                ),
                (
                    icons::TERMINAL,
                    "Terminal",
                    None,
                    Msg::OpenTab(Tab::Terminal),
                ),
                (icons::GLOBE, "Browser", Some("⌘T"), Msg::CloseMenus),
                (icons::FILES, "Files", Some("⌘P"), Msg::OpenFile("cart.js")),
                (icons::SIDE_CHAT, "Side chat", Some("⌥⌘S"), Msg::CloseMenus),
            ];
            for (icon, title, keys, msg) in entries {
                m.item(
                    Item {
                        icon: Some(icon),
                        title,
                        shortcut: keys,
                        ..Item::default()
                    },
                    msg,
                );
            }
            let h = m.h;
            m.at(place(r, 222.0, h, Align::Right, win))
        }
        Menu::WorkIn | Menu::ProjectPicker => {
            let id = if menu == Menu::WorkIn {
                "tray.location"
            } else {
                "tray.project"
            };
            let r = anchor(vcx, id)?;
            let w = 240.0;
            let mut m = M::new(p, w, hi);
            if menu == Menu::WorkIn {
                m.header("Work in");
                simple(
                    &mut m,
                    &[
                        (Some(icons::LAPTOP), "This computer", None, true, false),
                        (Some(icons::WORKTREE), "New worktree", None, false, false),
                        (Some(icons::CLOUD), "Cloud", None, false, false),
                    ],
                    |_| Msg::CloseMenus,
                );
            } else {
                m.custom(search_row(p, "Search projects"), 34.0);
                m.sep();
                m.item(
                    Item {
                        icon: Some(icons::FOLDER),
                        title: data::DEMO_PROJECT,
                        check: app.project.is_some(),
                        ..Item::default()
                    },
                    Msg::ChooseProject(Some(data::ProjectId(1))),
                );
                m.item(
                    Item {
                        icon: Some(icons::FOLDER_OPEN),
                        title: "New project",
                        chevron: true,
                        ..Item::default()
                    },
                    Msg::CloseMenus,
                );
                m.item(
                    Item {
                        icon: Some(icons::CHAT),
                        title: "Don't work in a project",
                        ..Item::default()
                    },
                    Msg::ChooseProject(None),
                );
            }
            let h = m.h;
            m.at(place(r, w, h, Align::Left, win))
        }
    };
    Some(el)
}

fn search_row(p: &Pal, placeholder: &str) -> AnyElement {
    view! {
        <div class="flex-row items-center h-[34] px-[9] gap-2">
            <icon svg={icons::SEARCH} size={14.0} color={p.menu_desc} />
            <txt(placeholder, BODY, p.menu_desc) />
        </div>
    }
}

fn summary(p: &Pal, r: Rect, win: (f32, f32)) -> AnyElement {
    let row = |icon: &'static str, label: &str, chevron: bool, dim: bool| {
        let c = if dim { p.menu_desc } else { p.menu_title };
        view! { -> Div,
            <div class="flex-row items-center h-[30] px-4 gap-[10] rounded-[6]" hover_bg={p.menu_hi}>
                <icon svg={icon} size={16.0} color={c} />
                <txt(label, BODY, c) />
                if chevron {
                    <icon svg={icons::CHEVRON_DOWN} size={12.0} color={p.menu_desc} />
                }
            </div>
        }
    };
    let w = 300.0;
    let h = 286.0;
    let (x, y) = place(r, w, h, Align::Right, win);
    view! {
        <menu_panel(p, x, y + 4.0, w) h={h} class="rounded-[14] py-[10]">
            <div class="flex-row items-center h-8 px-3">
                <txt("Environment", BODY, p.menu_header) />
                <div class="flex-1" />
                <icon svg={icons::PLUS} size={15.0} color={p.menu_header} />
            </div>
            <row(icons::REVIEW, "Changes", false, false) on:click={Msg::OpenTab(Tab::Changes)} />
            <row(icons::LAPTOP, "Local", true, false) />
            <row(icons::BRANCH, "main", true, false) />
            <row(icons::COMMIT, "Commit or push", false, true) />
            <row(icons::PR, "Pull request status unavailable", false, true) />
            <menu_sep(p) class="px-3" />
            <div class="flex-row items-center h-8 px-4">
                <txt("Sources", BODY, p.menu_header) />
            </div>
            <div class="flex-row items-center h-7 px-4">
                <txt("No sources yet", BODY, p.menu_desc) />
            </div>
        </menu_panel>
    }
}

/// The "+" menu (u74) and the `@` menu, which shares its first groups;
/// it opens above the composer, as wide as it.
fn add_menu(p: &Pal, r: Rect, _win: (f32, f32), hi: Option<usize>, query: &str) -> AnyElement {
    let w = r.width;
    let mut m = M::new(p, w, hi.or(Some(0)));
    if query.is_empty() {
        m.header("Add");
        let items: [(&str, bool, &str, Option<&str>); 7] = [
            (icons::PAPERCLIP, false, "Files and folders", None),
            (icons::GLOBE, false, "Attach Safari", None),
            (
                icons::FOLDER,
                false,
                "Work in a project",
                Some("Choose project for new chats"),
            ),
            (
                icons::TARGET,
                false,
                "Goal",
                Some("Set a goal to keep pursuing"),
            ),
            (icons::BULB, false, "Plan mode", Some("Turn plan mode on")),
            (icons::RECORD, false, "Record a skill", None),
            (icons::SKETCH, false, "Sketch", Some("Draw a sketch")),
        ];
        for (icon, colored, title, desc) in items {
            m.item(
                Item {
                    icon: Some(icon),
                    icon_colored: colored,
                    title,
                    desc,
                    ..Item::default()
                },
                Msg::CloseMenus,
            );
        }
        m.header("Plugins");
        for (icon, title, desc) in [
            (
                icons::PLUG_SHEET,
                "Tailscale",
                "Securely connect your devices",
            ),
            (icons::PLUG_DOC, "Exa", "Web search for AI agents"),
            (
                icons::PLUG_SLIDES,
                "Presentations",
                "Create and edit presentations",
            ),
        ] {
            m.item(
                Item {
                    icon: Some(icon),
                    icon_colored: true,
                    title,
                    desc: Some(desc),
                    ..Item::default()
                },
                Msg::CloseMenus,
            );
        }
    } else {
        m.header("Files");
        for (i, file) in composer::FILES
            .iter()
            .filter(|f| f.contains(query))
            .enumerate()
        {
            m.item(
                Item {
                    icon: Some(icons::DOC),
                    title: file,
                    desc: Some(data::DEMO_PROJECT),
                    ..Item::default()
                },
                Msg::PickSuggestion(i),
            );
        }
    }
    // Taller lists scroll inside a 320-point menu (u74).
    let h = m.h.min(320.0).min(r.y - 50.0).max(80.0);
    let y = r.y - h - 6.0;
    view! {
        <menu_panel(p, r.x, y, w) h={h} class="overflow-hidden">{...m.rows}</menu_panel>
    }
}

/// The slash and `@` lists the draft opens, above the composer and as
/// wide as it.
fn composer_suggestions(app: &Codex, p: &Pal, vcx: &mut ViewContext) -> Option<AnyElement> {
    let s = app.composer.suggest()?;
    if app.menu.is_some() {
        return None;
    }
    let r = anchor(vcx, composer::CARD_ID)?;
    let win = app.size;
    let el = match s {
        composer::Suggest::Slash(filter) => {
            let matches = composer::State::slash_matches(&filter);
            let mut m = M::new(
                p,
                r.width,
                Some(app.composer.hi.min(matches.len().saturating_sub(1))),
            );
            for (i, (name, desc, icon, sub)) in matches.into_iter().enumerate() {
                m.item(
                    Item {
                        icon: Some(icon),
                        title: name,
                        desc: (!desc.is_empty()).then_some(*desc),
                        chevron: *sub,
                        ..Item::default()
                    },
                    Msg::PickSuggestion(i),
                );
            }
            if m.items == 0 {
                m.custom(
                    view! {
                        <div class="flex-row items-center h-[29] px-2">
                            <txt("No commands", BODY, p.menu_desc) />
                        </div>
                    },
                    29.0,
                );
            }
            let h = m.h.min(320.0).min(r.y - 50.0).max(60.0);
            view! {
                <menu_panel(p, r.x, r.y - h - 8.0, r.width) h={h} class="overflow-hidden">
                    {...m.rows}
                </menu_panel>
            }
        }
        composer::Suggest::At(query) => add_menu(p, r, win, Some(app.composer.hi), &query),
    };
    Some(el)
}
