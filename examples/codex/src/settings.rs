//! Settings: the section list in the sidebar's place ("Back to app",
//! search, Personal / Integrations / Coding / Archived), and the pages.
//! General and Appearance are built out; the others show their headline
//! rows.

use accesskit::Role;
use quark_app::ViewContext;
use quark_app::quark_ui::element::*;
use quark_app::quark_ui::style::Styled;
use quark_app::quark_ui::theme::Color;

use crate::theme::{BODY, CODE, Pal, SMALL};
use crate::widgets::*;
use crate::{Codex, Msg, SIDEBAR_W, Screen, ThemeChoice, icons, panel, sidebar};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    General,
    Import,
    Profile,
    Appearance,
    Configuration,
    Personalization,
    Pets,
    KeyboardShortcuts,
    Appshots,
    McpServers,
    Browser,
    ComputerUse,
    Hooks,
    Connections,
    Git,
    Environments,
    Worktrees,
    ArchivedChats,
}

const SECTIONS: &[(&str, &[(Page, &str, &str)])] = &[
    (
        "Personal",
        &[
            (Page::General, "General", icons::SETTINGS),
            (Page::Import, "Import", icons::DOWNLOAD),
            (Page::Profile, "Profile", icons::USER),
            (Page::Appearance, "Appearance", icons::SUN),
            (Page::Configuration, "Configuration", icons::SHIELD_TERM),
            (Page::Personalization, "Personalization", icons::SMILE),
            (Page::Pets, "Pets", icons::PAW),
            (
                Page::KeyboardShortcuts,
                "Keyboard shortcuts",
                icons::KEYBOARD,
            ),
        ],
    ),
    (
        "Integrations",
        &[
            (Page::Appshots, "Appshots", icons::CAMERA),
            (Page::McpServers, "MCP servers", icons::MCP),
            (Page::Browser, "Browser", icons::BROWSER),
            (Page::ComputerUse, "Computer use", icons::CURSOR_CLICK),
        ],
    ),
    (
        "Coding",
        &[
            (Page::Hooks, "Hooks", icons::ANCHOR),
            (Page::Connections, "Connections", icons::GLOBE),
            (Page::Git, "Git", icons::BRANCH),
            (Page::Environments, "Environments", icons::MONITOR),
            (Page::Worktrees, "Worktrees", icons::WORKTREE),
        ],
    ),
    (
        "Archived",
        &[(Page::ArchivedChats, "Archived chats", icons::ARCHIVE)],
    ),
];

pub fn title(page: Page) -> &'static str {
    SECTIONS
        .iter()
        .flat_map(|(_, pages)| pages.iter())
        .find(|(p, _, _)| *p == page)
        .map_or("Settings", |(_, t, _)| t)
}

pub fn view(app: &mut Codex, page: Page, p: &Pal, vcx: &mut ViewContext) -> AnyElement {
    let (w, h) = app.size;
    let nav_shown = w >= crate::SIDEBAR_BREAKPOINT;
    let nav_w = if nav_shown { SIDEBAR_W } else { 0.0 };
    let mut root = div().absolute().left(0.0).top(0.0).w(w).h(h);
    if nav_shown {
        root = root.child(nav(page, p, h));
    }
    let content_w = (w - nav_w - 40.0).clamp(0.0, 760.0);
    let body = page_body(app, page, p, content_w);
    let _ = vcx;
    root.child(
        div()
            .absolute()
            .left(nav_w)
            .top(0.0)
            .w(w - nav_w)
            .h(h)
            .flex_col()
            .items_center()
            .overflow_y_scroll()
            .track_scroll(&app.settings_handle)
            .scrollbar_auto_hide()
            .child(
                div()
                    .w(content_w)
                    .flex_col()
                    .flex_shrink_0()
                    .pt(62.0)
                    .pb(60.0)
                    .child(
                        text(title(page))
                            .size(20.0)
                            .medium()
                            .color(p.text)
                            .no_wrap(),
                    )
                    .child(div().h(40.0))
                    .child(body),
            ),
    )
    .into_any()
}

fn nav(page: Page, p: &Pal, h: f32) -> Div {
    let mut list = div().flex_col().w(284.0);
    for (section, pages) in SECTIONS {
        list = list.child(hrow().h(37.0).pt(14.0).pl(8.0).child(txt(
            *section,
            BODY,
            p.sidebar_muted,
        )));
        for (pg, label, icon) in pages.iter() {
            let selected = *pg == page;
            list = list.child(
                hrow()
                    .h(29.0)
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
                    .accessibility_label(label.to_string())
                    .accessibility_selected(selected)
                    .on_click(Msg::Show(Screen::Settings(*pg)))
                    .child(ico(icon, 17.0, p.sidebar_text))
                    .child(txt(*label, BODY, p.sidebar_text)),
            );
            list = list.child(div().h(1.0));
        }
    }
    div()
        .absolute()
        .left(0.0)
        .top(0.0)
        .w(SIDEBAR_W)
        .h(h)
        .bg(p.sidebar)
        .accessibility_role(Role::Navigation)
        .accessibility_label("Settings")
        .child(
            hrow()
                .absolute()
                .left(8.0)
                .top(46.0)
                .w(284.0)
                .h(31.0)
                .pl(8.0)
                .gap(8.0)
                .rounded(8.0)
                .hover_bg(p.row_hover)
                .accessibility_role(Role::Link)
                .accessibility_label("Back to app")
                .on_click(Msg::NewChat)
                .child(ico(icons::ARROW_LEFT, 16.0, p.sidebar_muted))
                .child(txt("Back to app", BODY, p.sidebar_muted)),
        )
        .child(
            hrow()
                .absolute()
                .left(8.0)
                .top(85.0)
                .w(284.0)
                .h(28.0)
                .pl(9.0)
                .gap(8.0)
                .rounded(8.0)
                .border(p.sidebar_text.with_alpha(40))
                .child(ico(icons::SEARCH, 15.0, p.sidebar_muted))
                .child(txt("Search settings…", BODY, p.sidebar_muted)),
        )
        .child(
            div()
                .absolute()
                .left(8.0)
                .top(113.0)
                .h(h - 113.0 - 40.0)
                .overflow_hidden()
                .child(list),
        )
        .child(
            div()
                .absolute()
                .left(SIDEBAR_W - 65.0)
                .top(h - 26.0)
                .child(sidebar::update_pill(p)),
        )
}

fn section_title(p: &Pal, title: &str, sub: Option<&str>) -> Div {
    let mut d = div().flex_col().gap(7.0).pb(16.0).child(
        text(title.to_owned())
            .size(BODY)
            .medium()
            .color(p.text)
            .no_wrap(),
    );
    if let Some(sub) = sub {
        d = d.child(txt(sub, BODY, p.muted));
    }
    d
}

/// A grouped card of rows separated by hairlines.
fn group(p: &Pal, rows: Vec<Div>) -> Div {
    let mut card = div()
        .w_full()
        .flex_col()
        .bg(p.settings_card)
        .border(p.settings_border)
        .rounded(10.0);
    let n = rows.len();
    for (i, row) in rows.into_iter().enumerate() {
        card = card.child(row);
        if i + 1 < n {
            card = card.child(div().w_full().h(1.0).bg(p.settings_border));
        }
    }
    card
}

fn setting_row(
    p: &Pal,
    title: &str,
    desc: Option<&str>,
    control: impl IntoAnyElement,
    w: f32,
) -> Div {
    let mut text_col = div()
        .flex_col()
        .flex_1()
        .min_w(0.0)
        .gap(5.0)
        .child(txt(title, BODY, p.text));
    if let Some(desc) = desc {
        text_col = text_col.child(
            text(desc.to_owned())
                .size(SMALL)
                .color(p.muted)
                .line_height(1.45)
                .wrap_width(w - 140.0),
        );
    }
    hrow()
        .w_full()
        .px(13.0)
        .py(12.0)
        .gap(16.0)
        .child(text_col)
        .child(control.into_any())
}

fn toggle_for(app: &Codex, p: &Pal, key: &'static str) -> Div {
    div()
        .accessibility_role(Role::Switch)
        .accessibility_toggled(app.toggles.contains(&key))
        .accessibility_label(key)
        .on_click(Msg::SetToggle(key))
        .child(toggle(p, app.toggles.contains(&key)))
}

fn dropdown(p: &Pal, label: &str, icon: Option<&'static str>) -> Div {
    let mut d = hrow()
        .h(30.0)
        .px(10.0)
        .gap(8.0)
        .rounded(8.0)
        .bg(p.menu_hi.with_alpha(160))
        .border(p.settings_border);
    if let Some(icon) = icon {
        d = d.child(svg_icon(icon, 15.0));
    }
    d.child(txt(label, BODY, p.text))
        .child(div().w(70.0))
        .child(ico(icons::CHEVRON_DOWN, 13.0, p.muted))
}

fn page_body(app: &Codex, page: Page, p: &Pal, w: f32) -> Div {
    match page {
        Page::General => general(app, p, w),
        Page::Appearance => appearance(app, p, w),
        Page::KeyboardShortcuts => shortcuts(p, w),
        _ => {
            let rows = match page {
                Page::Hooks => vec![(
                    "No hooks",
                    "Hooks run commands before and after Codex acts.",
                )],
                Page::Worktrees => vec![(
                    "No worktrees",
                    "Worktrees Codex creates for chats show here.",
                )],
                Page::ArchivedChats => vec![(
                    "No archived chats",
                    "Archive a chat from its menu to keep it here.",
                )],
                Page::McpServers => vec![(
                    "Add server",
                    "Connect Codex to tools over the Model Context Protocol.",
                )],
                Page::Git => vec![
                    ("Branch prefix", "Prefix for branches Codex creates."),
                    (
                        "Commit instructions",
                        "Guidance Codex follows when writing commit messages.",
                    ),
                ],
                _ => vec![("Coming soon", "This page has no settings in the demo.")],
            };
            group(
                p,
                rows.into_iter()
                    .map(|(t, d)| setting_row(p, t, Some(d), div(), w))
                    .collect(),
            )
        }
    }
}

fn general(app: &Codex, p: &Pal, w: f32) -> Div {
    let mode_card = |icon: &'static str, title: &str, desc: &str, on: bool| {
        let radio = if on {
            div()
                .w(18.0)
                .h(18.0)
                .rounded(9.0)
                .bg(p.accent)
                .items_center()
                .justify_center()
                .child(
                    div()
                        .w(6.0)
                        .h(6.0)
                        .rounded(3.0)
                        .bg(Color::rgba(255, 255, 255, 255)),
                )
        } else {
            div().w(18.0).h(18.0).rounded(9.0).border(p.muted)
        };
        hrow()
            .flex_1()
            .h(64.0)
            .px(14.0)
            .gap(12.0)
            .rounded(10.0)
            .when(on, |d| d.bg(p.composer))
            .border(if on { p.composer } else { p.settings_border })
            .accessibility_role(Role::RadioButton)
            .accessibility_label(title.to_owned())
            .child(ico(icon, 18.0, p.text))
            .child(
                div()
                    .flex_col()
                    .flex_1()
                    .min_w(0.0)
                    .gap(5.0)
                    .child(txt(title, SMALL, p.text))
                    .child(txt(desc, SMALL, p.muted).truncate()),
            )
            .child(radio)
    };
    div()
        .flex_col()
        .w(w)
        .child(section_title(p, "Work mode", Some("Choose how much technical detail Codex shows")))
        .child(
            hrow()
                .w(w)
                .gap(11.0)
                .child(mode_card(icons::TERMINAL, "For coding", "More technical responses and control", true))
                .child(mode_card(icons::CHAT, "For everyday work", "Same power, less technical detail", false)),
        )
        .child(div().h(34.0))
        .child(section_title(p, "Permissions", None))
        .child(group(
            p,
            vec![
                setting_row(
                    p,
                    "Default permissions",
                    Some("By default, Codex can read and edit files in its workspace. It can ask for additional access when needed"),
                    div().opacity(0.55).child(toggle(p, true)),
                    w,
                ),
                setting_row(
                    p,
                    "Auto-review",
                    Some("Codex can read and edit files in its workspace. Codex automatically reviews requests for additional access. Auto-review can make mistakes. Learn more about elevated risks."),
                    toggle_for(app, p, "auto-review"),
                    w,
                ),
                setting_row(
                    p,
                    "Full access",
                    Some("When Codex runs with full access, it can edit any file on your computer and run commands with network, without your approval. This significantly increases the risk of data loss, leaks, or unexpected behavior. Learn more about elevated risks."),
                    toggle_for(app, p, "full-access"),
                    w,
                ),
            ],
        ))
        .child(div().h(34.0))
        .child(section_title(p, "General", None))
        .child(group(
            p,
            vec![
                setting_row(p, "Default file open destination", Some("Where files and folders open by default"), dropdown(p, "Default app", Some(icons::APP_FINDER)), w),
                setting_row(p, "Speed", Some("Fast runs 1.5x faster with increased usage"), dropdown(p, "Standard", None), w),
                setting_row(p, "Composer send shortcut", Some("Enter sends; Shift+Enter adds a line"), dropdown(p, "Enter", None), w),
                setting_row(p, "Notifications", Some("Notify when a chat finishes or needs approval"), toggle_for(app, p, "notifications"), w),
            ],
        ))
}

fn swatch(p: &Pal, color: Color, hexs: &str, filled: bool) -> Div {
    let fg = if filled {
        Color::rgba(255, 255, 255, 255)
    } else {
        p.text
    };
    hrow()
        .w(136.0)
        .h(28.0)
        .px(9.0)
        .gap(8.0)
        .rounded(7.0)
        .bg(color)
        .border(p.settings_border)
        .child(
            div()
                .w(14.0)
                .h(14.0)
                .rounded(7.0)
                .border(fg.with_alpha(160)),
        )
        .child(text(hexs.to_owned()).size(CODE).mono().color(fg).no_wrap())
}

fn appearance(app: &Codex, p: &Pal, w: f32) -> Div {
    let preview = |label: &str, choice: ThemeChoice, light: bool| {
        let selected = app.theme_choice == choice;
        let (bg, card, line) = if light {
            (
                Color::rgba(0xee, 0xee, 0xee, 255),
                Color::rgba(255, 255, 255, 255),
                Color::rgba(0xd8, 0xd8, 0xd8, 255),
            )
        } else {
            (
                Color::rgba(0x5c, 0x5c, 0x5c, 255),
                Color::rgba(0xf6, 0xf6, 0xf6, 255),
                Color::rgba(0xd8, 0xd8, 0xd8, 255),
            )
        };
        let tile_w = ((w - 24.0) / 3.0).floor();
        let mut tile = div()
            .w(tile_w)
            .h(145.0)
            .rounded(10.0)
            .bg(bg)
            .overflow_hidden()
            .flex_col()
            .items_center()
            .pt(30.0)
            .child(div().w(tile_w * 0.36).h(6.0).rounded(3.0).bg(line))
            .child(div().h(6.0))
            .child(div().w(tile_w * 0.6).h(4.0).rounded(2.0).bg(line))
            .child(div().h(8.0))
            .child(
                div()
                    .w(tile_w * 0.82)
                    .h(110.0)
                    .rounded(8.0)
                    .bg(card)
                    .flex_col()
                    .p(10.0)
                    .gap(8.0)
                    .child(div().w(tile_w * 0.3).h(5.0).rounded(2.0).bg(line))
                    .child(div().w_full().h(1.0).bg(line))
                    .child(div().w(tile_w * 0.36).h(5.0).rounded(2.0).bg(line)),
            );
        if selected {
            tile = tile.border(p.accent).border_w(2.0);
        }
        div()
            .flex_col()
            .items_center()
            .gap(10.0)
            .accessibility_role(Role::RadioButton)
            .accessibility_label(label.to_owned())
            .accessibility_selected(selected)
            .on_click(Msg::SetTheme(choice))
            .child(tile)
            .child(txt(label, SMALL, if selected { p.text } else { p.muted }))
    };
    let diff_line = |n: u32, code: &str, kind: i8, half: f32| {
        let (bg, num) = match kind {
            -1 => (Some(p.del_code), p.del_num),
            1 => (Some(p.add_code), p.add_num),
            _ => (None, p.line_num),
        };
        let mut row = hrow().w(half).h(21.5);
        if let Some(bg) = bg {
            row = row.bg(bg);
        }
        row.child(
            hrow()
                .w(46.0)
                .justify_end()
                .pr(8.0)
                .child(text(n.to_string()).size(CODE).mono().color(num).no_wrap()),
        )
        .child(div().pl(14.0).child(panel::highlight(code, p)))
    };
    let half = (w / 2.0).floor();
    let old = [
        "const themePreview: ThemeConfig = {",
        "  surface: \"sidebar\",",
        "  accent: \"#2563eb\",",
        "  contrast: 42,",
        "};",
    ];
    let new = [
        "const themePreview: ThemeConfig = {",
        "  surface: \"sidebar-elevated\",",
        "  accent: \"#0ea5e9\",",
        "  contrast: 68,",
        "};",
    ];
    let mut diff = div()
        .w(w)
        .flex_col()
        .py(4.0)
        .bg(p.settings_card)
        .border(p.settings_border)
        .rounded(10.0)
        .overflow_hidden();
    for i in 0..5 {
        let kind = if (1..4).contains(&i) { 1 } else { 0 };
        diff = diff.child(
            hrow()
                .child(diff_line(i as u32 + 1, old[i], -kind, half))
                .child(diff_line(i as u32 + 1, new[i], kind, half)),
        );
    }
    let mode = if p.mode == quark_app::quark_ui::theme::ThemeMode::Dark {
        "Dark theme"
    } else {
        "Light theme"
    };
    let (bgc, fgc, bgs, fgs) = if p.mode == quark_app::quark_ui::theme::ThemeMode::Dark {
        (
            Color::rgba(0x18, 0x18, 0x18, 255),
            Color::rgba(255, 255, 255, 255),
            "#181818",
            "#FFFFFF",
        )
    } else {
        (
            Color::rgba(255, 255, 255, 255),
            Color::rgba(0x1a, 0x1c, 0x1f, 255),
            "#FFFFFF",
            "#1A1C1F",
        )
    };
    let field = |value: &str| {
        hrow()
            .w(136.0)
            .h(28.0)
            .px(9.0)
            .rounded(7.0)
            .border(p.settings_border)
            .overflow_hidden()
            .child(txt(value, SMALL, p.muted))
    };
    div()
        .flex_col()
        .w(w)
        .child(
            hrow()
                .w(w)
                .gap(12.0)
                .child(preview("System", ThemeChoice::System, false))
                .child(preview("Light", ThemeChoice::Light, true))
                .child(preview("Dark", ThemeChoice::Dark, false)),
        )
        .child(div().h(24.0))
        .child(diff)
        .child(div().h(20.0))
        .child(group(
            p,
            vec![
                hrow()
                    .w_full()
                    .h(40.0)
                    .px(16.0)
                    .gap(20.0)
                    .child(txt(mode, BODY, p.text))
                    .child(div().flex_1())
                    .child(txt("Import", BODY, p.muted))
                    .child(txt("Copy theme", BODY, p.muted))
                    .child(dropdown(p, "Codex", None)),
                setting_row(p, "Accent", None, swatch(p, p.accent, "#339CFF", true), w),
                setting_row(p, "Background", None, swatch(p, bgc, bgs, false), w),
                setting_row(p, "Foreground", None, swatch(p, fgc, fgs, fgc.r < 128), w),
                setting_row(p, "UI font", None, field("-apple-system, BlinkMac"), w),
                setting_row(p, "Code font", None, field("ui-monospace, \"SFMono"), w),
                setting_row(
                    p,
                    "Translucent sidebar",
                    None,
                    toggle_for(app, p, "translucent"),
                    w,
                ),
            ],
        ))
}

fn shortcuts(p: &Pal, w: f32) -> Div {
    let rows = [
        ("New chat", "⌘N"),
        ("Search chats", "⌘K"),
        ("Settings", "⌘,"),
        ("Toggle sidebar", "⌘B"),
        ("Toggle side panel", "⌘J"),
        ("Pin chat", "⌥⌘P"),
        ("Rename chat", "⌥⌘R"),
        ("Archive chat", "⇧⌘A"),
        ("Open side chat", "⌥⌘S"),
        ("Review", "⌃⇧G"),
    ];
    group(
        p,
        rows.iter()
            .map(|(t, k)| setting_row(p, t, None, kbd(p, k), w))
            .collect(),
    )
}
