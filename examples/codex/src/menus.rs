//! Every popover: composer pickers, tray pickers, thread header menus,
//! the review scope, panel tabs, the account menu, and the sidebar's
//! project menus. Each anchors to its trigger's frame from the last
//! frame's geometry, below it when it fits and above it otherwise, and
//! submenus open beside their row.

use quark::Rect;
use quark_app::ViewContext;
use quark_app::quark_ui::element::*;
use quark_app::quark_ui::style::Styled;

use crate::theme::{BODY, Pal, SMALL};
use crate::widgets::*;
use crate::{APPROVALS, Codex, MODELS, Menu, Msg, REASONING, Scope, Tab, composer, data, icons};

/// A menu under construction: rows and the height they add up to.
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
        self.h += if tall { 48.0 } else { 29.0 };
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

    fn at(self, (x, y): (f32, f32)) -> Div {
        menu_panel(self.p, x, y, self.w)
            .h(self.h)
            .children(self.rows)
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
    let below = r.y + r.height + 1.0;
    let y = if below + h <= win.1 - 4.0 {
        below
    } else {
        (r.y - h - 1.0).max(4.0)
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

pub fn view(app: &Codex, p: &Pal, vcx: &mut ViewContext) -> Option<AnyElement> {
    let win = app.size;
    if let Some(s) = composer_suggestions(app, p, vcx) {
        return Some(s);
    }
    let menu = app.menu?;
    let hi = app.menu_hi;
    let el = match menu {
        Menu::Approval => {
            let r = anchor(vcx, "pill.approval")?;
            let mut m = M::new(p, 453.0, hi);
            m.custom(
                hrow()
                    .h(28.0)
                    .px(8.0)
                    .gap(32.0)
                    .child(txt(
                        "How should Codex actions be approved?",
                        SMALL,
                        p.menu_header,
                    ))
                    .child(txt("Learn more", SMALL, p.menu_desc)),
                28.0,
            );
            for (i, (title, desc, icon)) in APPROVALS.iter().enumerate() {
                m.item(
                    Item {
                        icon: Some(icon),
                        title,
                        desc: Some(desc),
                        desc_below: true,
                        check: app.approval == i,
                        ..Item::default()
                    },
                    Msg::Approval(i),
                );
            }
            let h = m.h;
            m.at(place(r, 453.0, h, Align::Left, win))
        }
        Menu::Model | Menu::ModelList | Menu::Speed => {
            let r = anchor(vcx, "pill.model")?;
            let mut m = M::new(p, 210.0, hi);
            m.header("Reasoning");
            for (i, label) in REASONING.iter().enumerate() {
                m.item(
                    Item {
                        title: label,
                        check: app.reasoning == i,
                        ..Item::default()
                    },
                    Msg::Reasoning(i),
                );
            }
            m.sep();
            m.item(
                Item {
                    title: MODELS[app.model].0,
                    chevron: true,
                    hi: menu == Menu::ModelList,
                    ..Item::default()
                },
                Msg::Open(Menu::ModelList),
            );
            m.item(
                Item {
                    title: "Speed",
                    chevron: true,
                    hi: menu == Menu::Speed,
                    ..Item::default()
                },
                Msg::Open(Menu::Speed),
            );
            let h = m.h;
            let (x, y) = place(r, 210.0, h, Align::Right, win);
            let main = m.at((x, y));
            let sub = match menu {
                Menu::ModelList => {
                    let mut s = M::new(p, 201.0, None);
                    s.header("Model");
                    for (i, (name, _)) in MODELS.iter().enumerate() {
                        s.item(
                            Item {
                                title: name,
                                check: app.model == i,
                                ..Item::default()
                            },
                            Msg::Model(i),
                        );
                    }
                    let sh = s.h;
                    // Beside the model row, to the left when the right is full.
                    let row_y = y + 4.0 + 28.0 + 4.0 * 29.0 + 9.0;
                    Some(s.at(side(x, 210.0, 201.0, row_y - 32.0, sh, win)))
                }
                Menu::Speed => {
                    let mut s = M::new(p, 236.0, None);
                    s.header("Speed");
                    s.item(
                        Item {
                            title: "Standard",
                            desc: Some("Default speed"),
                            desc_below: true,
                            check: app.speed == 0,
                            ..Item::default()
                        },
                        Msg::Speed(0),
                    );
                    s.item(
                        Item {
                            title: "Fast",
                            desc: Some("1.5x speed, increased usage"),
                            desc_below: true,
                            check: app.speed == 1,
                            ..Item::default()
                        },
                        Msg::Speed(1),
                    );
                    let sh = s.h;
                    let row_y = y + 4.0 + 28.0 + 5.0 * 29.0 + 9.0;
                    Some(s.at(side(x, 210.0, 236.0, row_y - 32.0, sh, win)))
                }
                _ => None,
            };
            div()
                .children([main.into_any()])
                .children(sub.map(IntoAnyElement::into_any))
        }
        Menu::Add => {
            let r = anchor(vcx, composer::CARD_ID)?;
            add_menu(p, r, win, hi, "")
        }
        Menu::ChooseProject | Menu::NewProject | Menu::HeadlineProject => {
            let id = if menu == Menu::HeadlineProject {
                "headline.project"
            } else {
                "tray.project"
            };
            let r = anchor(vcx, id)?;
            let w = 262.0;
            let mut m = M::new(p, w, hi);
            if menu == Menu::HeadlineProject {
                m.custom(search_row(p, "Search projects"), 34.0);
                m.sep();
            }
            if app.project.is_some() {
                m.item(
                    Item {
                        icon: Some(icons::DOC),
                        title: data::DEMO_PROJECT,
                        check: true,
                        ..Item::default()
                    },
                    Msg::ChooseProject(app.project),
                );
            }
            m.item(
                Item {
                    icon: Some(icons::FOLDER),
                    title: "New project",
                    chevron: true,
                    hi: menu == Menu::NewProject,
                    ..Item::default()
                },
                Msg::Open(Menu::NewProject),
            );
            if app.project.is_some() {
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
            let (x, y) = place(r, w, h, Align::Left, win);
            let main = m.at((x, y)).into_any();
            let sub = (menu == Menu::NewProject).then(|| {
                let mut s = M::new(p, 220.0, None);
                s.item(
                    Item {
                        icon: Some(icons::COMPOSE),
                        title: "Start from scratch",
                        ..Item::default()
                    },
                    Msg::ChooseProject(Some(data::ProjectId(1))),
                );
                s.item(
                    Item {
                        icon: Some(icons::FOLDER_OPEN),
                        title: "Use an existing folder",
                        ..Item::default()
                    },
                    Msg::ChooseProject(Some(data::ProjectId(1))),
                );
                let sh = s.h;
                s.at(side(x, w, 220.0, y, sh, win)).into_any()
            });
            div().child(main).children(sub)
        }
        Menu::WorkLocation | Menu::Usage => {
            let r = anchor(vcx, "tray.location")?;
            let w = 185.0;
            let mut m = M::new(p, w, hi);
            m.header("Start in");
            m.item(
                Item {
                    icon: Some(icons::LAPTOP),
                    title: "Work locally",
                    check: true,
                    ..Item::default()
                },
                Msg::CloseMenus,
            );
            m.item(
                Item {
                    icon: Some(icons::WORKTREE),
                    title: "New worktree",
                    ..Item::default()
                },
                Msg::CloseMenus,
            );
            m.item(
                Item {
                    icon: Some(icons::CLOUD),
                    title: "Cloud",
                    ..Item::default()
                },
                Msg::CloseMenus,
            );
            m.sep();
            m.item(
                Item {
                    icon: Some(icons::GAUGE),
                    title: "Usage remaining",
                    chevron: menu == Menu::WorkLocation,
                    hi: menu == Menu::Usage,
                    ..Item::default()
                },
                Msg::Open(Menu::Usage),
            );
            if menu == Menu::Usage {
                m.custom(usage_rows(p), 76.0);
            }
            let h = m.h;
            m.at(place(r, w, h, Align::Left, win))
        }
        Menu::Branch => {
            let r = anchor(vcx, "tray.branch")?;
            let w = 297.0;
            let mut m = M::new(p, w, hi);
            m.custom(search_row(p, "Search branches"), 34.0);
            m.header("Branches");
            m.item(
                Item {
                    icon: Some(icons::BRANCH),
                    title: "main",
                    check: true,
                    ..Item::default()
                },
                Msg::CloseMenus,
            );
            m.custom(div().h(120.0), 120.0);
            m.sep();
            m.item(
                Item {
                    icon: Some(icons::PLUS),
                    title: "Create and checkout new branch...",
                    ..Item::default()
                },
                Msg::CloseMenus,
            );
            let h = m.h;
            m.at(place(r, w, h, Align::Left, win))
        }
        Menu::ChatActions => {
            let r = anchor(vcx, "header.actions")?;
            let mut m = M::new(p, 222.0, hi);
            let entries: [(&str, &str, Option<&str>, bool); 8] = [
                (icons::PIN, "Pin chat", Some("⌥⌘P"), false),
                (icons::PENCIL, "Rename chat", Some("⌥⌘R"), false),
                (icons::ARCHIVE, "Archive chat", Some("⇧⌘A"), false),
                (icons::CHAT_PLUS, "Open side chat", Some("⌥⌘S"), false),
                (icons::COPY, "Copy", None, true),
                (icons::FORK, "Fork", None, true),
                (icons::CLOCK, "Add scheduled task...", None, false),
                (icons::WINDOW_NEW, "Open in new window", None, false),
            ];
            for (i, (icon, title, keys, chevron)) in entries.into_iter().enumerate() {
                if i == 3 || i == 7 {
                    m.sep();
                }
                m.item(
                    Item {
                        icon: Some(icon),
                        title,
                        shortcut: keys,
                        chevron,
                        ..Item::default()
                    },
                    Msg::CloseMenus,
                );
            }
            let h = m.h;
            m.at(place(r, 222.0, h, Align::Left, win))
        }
        Menu::OpenIn => {
            let r = anchor(vcx, "header.openin")?;
            let mut m = M::new(p, 170.0, hi);
            for (icon, title) in [
                (icons::APP_FINDER, "Finder"),
                (icons::APP_TERMINAL, "Terminal"),
                (icons::APP_XCODE, "Xcode"),
            ] {
                m.item(
                    Item {
                        icon: Some(icon),
                        icon_colored: true,
                        title,
                        ..Item::default()
                    },
                    Msg::CloseMenus,
                );
            }
            let h = m.h;
            m.at(place(r, 170.0, h, Align::Left, win))
        }
        Menu::Summary => {
            let r = anchor(vcx, "header.summary")?;
            summary(p, r, win)
        }
        Menu::ReviewScope => {
            let r = anchor(vcx, "review.scope")?;
            let mut m = M::new(p, 180.0, hi);
            for (scope, chevron) in [
                (Some(Scope::Unstaged), false),
                (Some(Scope::Staged), false),
                (None, true),
                (Some(Scope::Branch), false),
                (Some(Scope::LastTurn), false),
            ] {
                let title = scope.map_or("Commit", Scope::label);
                m.item(
                    Item {
                        title,
                        chevron,
                        check: scope == Some(app.scope),
                        ..Item::default()
                    },
                    scope.map_or(Msg::CloseMenus, Msg::SetScope),
                );
            }
            let h = m.h;
            m.at(place(r, 180.0, h, Align::Left, win))
        }
        Menu::PanelTab => {
            let r = anchor(vcx, "panel.newtab")?;
            let mut m = M::new(p, 222.0, hi);
            let entries = [
                (
                    icons::REVIEW,
                    "Review",
                    Some("⌃⇧G"),
                    Msg::OpenTab(Tab::Review),
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
            m.at(place(r, 222.0, h, Align::Left, win))
        }
        Menu::Account => {
            let mut m = M::new(p, 281.0, hi);
            m.item(
                Item {
                    icon: Some(icons::USER),
                    title: data::ACCOUNT_EMAIL,
                    dim: true,
                    ..Item::default()
                },
                Msg::Noop,
            );
            m.item(
                Item {
                    icon: Some(icons::ORG),
                    title: "Personal account",
                    dim: true,
                    ..Item::default()
                },
                Msg::Noop,
            );
            m.sep();
            m.item(
                Item {
                    icon: Some(icons::USER),
                    title: "Profile",
                    ..Item::default()
                },
                Msg::Show(crate::Screen::Settings(crate::settings::Page::Profile)),
            );
            m.item(
                Item {
                    icon: Some(icons::SETTINGS),
                    title: "Settings",
                    shortcut: Some("⌘,"),
                    ..Item::default()
                },
                Msg::Show(crate::Screen::Settings(crate::settings::Page::General)),
            );
            m.sep();
            m.item(
                Item {
                    icon: Some(icons::GAUGE),
                    title: "Usage remaining",
                    chevron: true,
                    ..Item::default()
                },
                Msg::Noop,
            );
            m.item(
                Item {
                    icon: Some(icons::LOGOUT),
                    title: "Log out",
                    ..Item::default()
                },
                Msg::CloseMenus,
            );
            let h = m.h;
            m.at((9.0, win.1 - 52.0 - h))
        }
        Menu::ProjectActions(_) | Menu::SidebarOptions | Menu::ThreadContext(_) => {
            let mut m = M::new(p, 230.0, hi);
            let entries: &[(&str, &str)] = match menu {
                Menu::ProjectActions(_) => &[
                    (icons::PIN, "Pin project"),
                    (icons::REVEAL, "Reveal in Finder"),
                    (icons::WORKTREE, "Create permanent worktree"),
                    (icons::PENCIL, "Rename project"),
                    (icons::ARCHIVE, "Archive chats"),
                    (icons::TRASH, "Remove"),
                ],
                Menu::SidebarOptions => &[
                    (icons::ARCHIVE, "Archive all chats"),
                    (icons::LIST_FILTER, "Organize sidebar"),
                    (icons::SLIDERS, "Sort by"),
                ],
                _ => &[
                    (icons::PIN, "Pin chat"),
                    (icons::PENCIL, "Rename chat"),
                    (icons::ARCHIVE, "Archive chat"),
                    (icons::MARK_UNREAD, "Mark as unread"),
                    (icons::REVEAL, "Reveal in Finder"),
                    (icons::COPY, "Copy working directory"),
                    (icons::FORK, "Fork into local"),
                    (icons::WINDOW_NEW, "Open in new window"),
                ],
            };
            for (icon, title) in entries {
                m.item(
                    Item {
                        icon: Some(icon),
                        title,
                        chevron: menu == Menu::SidebarOptions && *title != "Archive all chats",
                        ..Item::default()
                    },
                    Msg::CloseMenus,
                );
            }
            let h = m.h;
            m.at((150.0, 200.0_f32.min(win.1 - h - 8.0)))
        }
    };
    Some(el.into_any())
}

/// A submenu beside a `parent_w`-wide menu at `x`: right of it when it
/// fits, else left.
fn side(x: f32, parent_w: f32, w: f32, y: f32, h: f32, win: (f32, f32)) -> (f32, f32) {
    let right = x + parent_w + 2.0;
    let sx = if right + w <= win.0 - 4.0 {
        right
    } else {
        x - w - 2.0
    };
    (sx.max(4.0), y.min(win.1 - h - 4.0).max(4.0))
}

fn search_row(p: &Pal, placeholder: &str) -> Div {
    hrow()
        .h(34.0)
        .px(9.0)
        .gap(8.0)
        .child(ico(icons::SEARCH, 14.0, p.menu_desc))
        .child(txt(placeholder, BODY, p.menu_desc))
}

fn usage_rows(p: &Pal) -> Div {
    div()
        .flex_col()
        .px(10.0)
        .gap(6.0)
        .pt(6.0)
        .child(
            hrow()
                .child(txt("Weekly", SMALL, p.menu_title))
                .child(div().flex_1())
                .child(txt("77%", SMALL, p.menu_title)),
        )
        .child(
            div()
                .w_full()
                .h(4.0)
                .rounded(2.0)
                .bg(p.menu_hi)
                .child(div().w(126.0).h(4.0).rounded(2.0).bg(p.accent)),
        )
        .child(
            hrow()
                .child(txt("Resets Oct 14", SMALL, p.menu_desc))
                .child(div().flex_1())
                .child(txt("Learn more", SMALL, p.menu_desc)),
        )
}

fn summary(p: &Pal, r: Rect, win: (f32, f32)) -> Div {
    let row = |icon: &'static str, label: &str, chevron: bool, dim: bool| {
        let c = if dim { p.menu_desc } else { p.menu_title };
        let mut d = hrow()
            .h(30.0)
            .px(16.0)
            .gap(10.0)
            .rounded(6.0)
            .hover_bg(p.menu_hi)
            .child(ico(icon, 16.0, c))
            .child(txt(label, BODY, c));
        if chevron {
            d = d.child(ico(icons::CHEVRON_DOWN, 12.0, p.menu_desc));
        }
        d
    };
    let w = 300.0;
    let h = 286.0;
    let (x, y) = place(r, w, h, Align::Right, win);
    menu_panel(p, x, y + 7.0, w)
        .h(h)
        .rounded(14.0)
        .py(10.0)
        .child(
            hrow()
                .h(32.0)
                .px(12.0)
                .child(txt("Environment", BODY, p.menu_header))
                .child(div().flex_1())
                .child(ico(icons::PLUS, 15.0, p.menu_header)),
        )
        .child(row(icons::REVIEW, "Changes", false, false).on_click(Msg::OpenTab(Tab::Review)))
        .child(row(icons::LAPTOP, "Local", true, false))
        .child(row(icons::BRANCH, "main", true, false))
        .child(row(icons::COMMIT, "Commit or push", false, true))
        .child(row(
            icons::LOGO_GITHUB,
            "Pull request status unavailable",
            false,
            true,
        ))
        .child(menu_sep(p).px(12.0))
        .child(
            hrow()
                .h(32.0)
                .px(16.0)
                .child(txt("Sources", BODY, p.menu_header)),
        )
        .child(
            hrow()
                .h(28.0)
                .px(16.0)
                .child(txt("No sources yet", BODY, p.menu_desc)),
        )
}

/// The "+" menu (and the `@` menu, which shares its first groups).
fn add_menu(p: &Pal, r: Rect, win: (f32, f32), hi: Option<usize>, query: &str) -> Div {
    let w = r.width;
    let mut m = M::new(p, w, hi.or(Some(0)));
    if query.is_empty() {
        m.header("Add");
        m.item(
            Item {
                icon: Some(icons::PAPERCLIP),
                title: "Files and folders",
                ..Item::default()
            },
            Msg::CloseMenus,
        );
        m.item(
            Item {
                icon: Some(icons::APP_TERMINAL),
                icon_colored: true,
                title: "Attach Codex Demo",
                ..Item::default()
            },
            Msg::CloseMenus,
        );
        m.item(
            Item {
                icon: Some(icons::TARGET),
                title: "Goal",
                desc: Some("Set a goal that Codex will keep working towards"),
                ..Item::default()
            },
            Msg::CloseMenus,
        );
        m.item(
            Item {
                icon: Some(icons::PLAN),
                title: "Plan mode",
                desc: Some("Turn plan mode on"),
                ..Item::default()
            },
            Msg::CloseMenus,
        );
        m.header("Plugins");
        for (icon, title, desc) in [
            (
                icons::PLUG_DOC,
                "Documents",
                "Create and edit document artifacts",
            ),
            (icons::PLUG_PDF, "PDF", "Read, create, and verify PDF files"),
            (
                icons::PLUG_SHEET,
                "Spreadsheets",
                "Create and edit spreadsheet files",
            ),
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
        m.item(
            Item {
                icon: Some(icons::CURSOR_CLICK),
                title: "Chess",
                desc: Some("Computer use"),
                ..Item::default()
            },
            Msg::CloseMenus,
        );
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
    let h = m.h.min(win.1 - (r.y + r.height) - 6.0).max(120.0);
    let (x, y) = (r.x, r.y + r.height + 4.0);
    menu_panel(p, x, y, w)
        .h(h)
        .overflow_hidden()
        .children(m.rows)
}

/// The slash and `@` suggestion lists the draft opens, attached under
/// the composer and as wide as it.
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
            for (i, (name, desc, icon)) in matches.into_iter().enumerate() {
                m.item(
                    Item {
                        icon: Some(icon),
                        title: name,
                        desc: Some(desc),
                        ..Item::default()
                    },
                    Msg::PickSuggestion(i),
                );
            }
            if m.items == 0 {
                m.custom(
                    hrow()
                        .h(29.0)
                        .px(8.0)
                        .child(txt("No commands", BODY, p.menu_desc)),
                    29.0,
                );
            }
            let h = m.h.min(win.1 - (r.y + r.height) - 6.0).max(60.0);
            menu_panel(p, r.x, r.y + r.height + 4.0, r.width)
                .h(h)
                .overflow_hidden()
                .children(m.rows)
        }
        composer::Suggest::At(query) => add_menu(p, r, win, Some(app.composer.hi), &query),
    };
    Some(el.into_any())
}
