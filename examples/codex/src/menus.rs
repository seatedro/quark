//! Every popover: permissions, the model and effort card, "+", the mode
//! switcher, the profile menu, chat actions, the summary, the approval
//! split button, the Changes scope and options, new tab, and the tray's
//! pickers; plus the slash and `@` lists the draft opens. Each anchors to
//! its trigger's frame from the last frame's geometry, below it when it
//! fits and above it otherwise.

use quark::Rect;
use quark_app::ViewContext;
use quark_app::quark_ui::element::*;
use quark_app::quark_ui::style::Styled;
use quark_app::quark_ui::theme::Color;

use crate::theme::{BODY, Pal, SMALL};
use crate::widgets::*;
use crate::{APPROVALS, Codex, EFFORTS, MODEL, Menu, Msg, SCOPES, Tab, composer, data, icons};

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

fn simple(
    m: &mut M,
    entries: &[(Option<&'static str>, &str, Option<&str>, bool, bool)],
    msg: impl Fn(usize) -> Msg,
) {
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
                hrow()
                    .h(28.0)
                    .px(8.0)
                    .child(txt(
                        "How should ChatGPT actions be approved?",
                        SMALL,
                        p.menu_header,
                    ))
                    .child(div().flex_1())
                    .child(txt("Learn more", SMALL, p.menu_desc)),
                28.0,
            );
            for (i, (title, desc, icon)) in APPROVALS.iter().enumerate() {
                let row = menu_item(
                    p,
                    Item {
                        icon: Some(icon),
                        title,
                        desc: Some(desc),
                        desc_below: true,
                        check: app.approval == i,
                        hi: hi == Some(i),
                        // Full access reads in orange.
                        tint: (i == 2).then_some(p.orange),
                        ..Item::default()
                    },
                    Msg::Approval(i),
                )
                .h(43.0);
                m.custom(row, 43.0);
            }
            let h = m.h;
            m.at(place(r, w, h, Align::Left, win))
        }
        Menu::Model => {
            let r = anchor(vcx, "pill.model")?;
            let w = 256.0;
            let h = 98.0;
            let stops = EFFORTS.len();
            let mut slider = hrow()
                .w(w - 24.0)
                .h(26.0)
                .rounded(13.0)
                .bg(p.menu_hi)
                .px(2.0)
                .justify_between()
                .relative();
            for i in 0..stops {
                let knob = i == app.effort;
                slider = slider.child(
                    div()
                        .w(if knob { 24.0 } else { 24.0 })
                        .h(24.0)
                        .items_center()
                        .justify_center()
                        .accessibility_role(accesskit::Role::RadioButton)
                        .accessibility_label(EFFORTS[i])
                        .accessibility_selected(knob)
                        .on_click(Msg::Effort(i))
                        .child(if knob {
                            div()
                                .w(24.0)
                                .h(24.0)
                                .rounded(12.0)
                                .bg(Color::rgba(255, 255, 255, 255))
                        } else {
                            div().w(4.0).h(4.0).rounded(2.0).bg(p.muted)
                        }),
                );
            }
            let (x, y) = place(r, w, h, Align::Right, win);
            menu_panel(p, x + 54.0, y, w)
                .h(h)
                .rounded(12.0)
                .px(12.0)
                .pt(8.0)
                .child(
                    hrow()
                        .w(w - 24.0)
                        .h(40.0)
                        .child(ico(icons::BOLT, 15.0, p.muted))
                        .child(
                            div()
                                .flex_1()
                                .flex_col()
                                .items_center()
                                .child(
                                    text(EFFORTS[app.effort])
                                        .size(BODY)
                                        .medium()
                                        .color(p.accent)
                                        .no_wrap(),
                                )
                                .child(
                                    hrow().gap(2.0).child(txt(MODEL, 12.0, p.muted)).child(ico(
                                        icons::CHEVRON_RIGHT,
                                        10.0,
                                        p.muted,
                                    )),
                                ),
                        )
                        .child(ico(icons::ROTATE, 14.0, p.muted)),
                )
                .child(div().h(8.0))
                .child(slider)
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
            m.at(place(r, w, h, Align::Left, win)).py(6.0).rounded(12.0)
        }
        Menu::Profile => {
            let w = 228.0;
            let mut m = M::new(p, w, hi);
            m.custom(
                hrow()
                    .h(40.0)
                    .px(8.0)
                    .gap(10.0)
                    .child(
                        div()
                            .w(18.0)
                            .h(18.0)
                            .rounded(9.0)
                            .bg(p.avatar)
                            .items_center()
                            .justify_center()
                            .child(txt("SE", 7.0, Color::rgba(255, 255, 255, 255))),
                    )
                    .child(
                        div()
                            .flex_col()
                            .child(txt(data::ACCOUNT_NAME, BODY, p.menu_title))
                            .child(txt(data::ACCOUNT_PLAN, SMALL, p.menu_desc)),
                    ),
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
            m.at((49.0, win.1 - 6.0 - h)).rounded(12.0)
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
    Some(el.into_any())
}

fn search_row(p: &Pal, placeholder: &str) -> Div {
    hrow()
        .h(34.0)
        .px(9.0)
        .gap(8.0)
        .child(ico(icons::SEARCH, 14.0, p.menu_desc))
        .child(txt(placeholder, BODY, p.menu_desc))
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
    menu_panel(p, x, y + 4.0, w)
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
        .child(row(icons::REVIEW, "Changes", false, false).on_click(Msg::OpenTab(Tab::Changes)))
        .child(row(icons::LAPTOP, "Local", true, false))
        .child(row(icons::BRANCH, "main", true, false))
        .child(row(icons::COMMIT, "Commit or push", false, true))
        .child(row(
            icons::PR,
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

/// The "+" menu (u74) and the `@` menu, which shares its first groups;
/// it opens above the composer, as wide as it.
fn add_menu(p: &Pal, r: Rect, _win: (f32, f32), hi: Option<usize>, query: &str) -> Div {
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
    menu_panel(p, r.x, y, w)
        .h(h)
        .overflow_hidden()
        .children(m.rows)
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
                    hrow()
                        .h(29.0)
                        .px(8.0)
                        .child(txt("No commands", BODY, p.menu_desc)),
                    29.0,
                );
            }
            let h = m.h.min(320.0).min(r.y - 50.0).max(60.0);
            menu_panel(p, r.x, r.y - h - 8.0, r.width)
                .h(h)
                .overflow_hidden()
                .children(m.rows)
        }
        composer::Suggest::At(query) => add_menu(p, r, win, Some(app.composer.hi), &query),
    };
    Some(el.into_any())
}
