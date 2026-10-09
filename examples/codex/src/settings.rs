//! Settings (u50, u51): the section list in the sidebar card ("Settings",
//! search, Personal / Integrations / Coding), and the pages in the main
//! card. General and Appearance follow the update captures; Keyboard
//! shortcuts and the rest show their headline rows (26.623 content).

use accesskit::Role;
use quark_app::ViewContext;
use quark_app::quark_ui::element::*;
use quark_app::quark_ui::style::Styled;
use quark_app::quark_ui::theme::{Color, ThemeMode};

use crate::theme::{BODY, Pal, SMALL};
use crate::widgets::*;
use crate::{Codex, Frame, Msg, Screen, ThemeChoice, icons};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    General,
    Notifications,
    Import,
    Profile,
    Appearance,
    Account,
    ArchivedChats,
    Configuration,
    Personalization,
    Pets,
    KeyboardShortcuts,
    Usage,
    DataControls,
    Plugins,
    ComputerUse,
    Appshots,
    Browser,
    Hooks,
    Connections,
    CodeReview,
    Git,
    Worktrees,
}

type Entry = (Page, &'static str, &'static str);

const SECTIONS: &[(&str, &[Entry])] = &[
    (
        "Personal",
        &[
            (Page::General, "General", icons::SETTINGS),
            (Page::Notifications, "Notifications", icons::BELL),
            (Page::Import, "Import", icons::DOWNLOAD),
            (Page::Profile, "Profile", icons::USER),
            (Page::Appearance, "Appearance", icons::SUN),
            (Page::Account, "Account", icons::USER),
            (Page::ArchivedChats, "Archived chats", icons::ARCHIVE),
            (Page::Configuration, "Configuration", icons::SHIELD_TERM),
            (Page::Personalization, "Personalization", icons::SMILE),
            (Page::Pets, "Mini & Pets", icons::PET),
            (
                Page::KeyboardShortcuts,
                "Keyboard shortcuts",
                icons::KEYBOARD,
            ),
            (Page::Usage, "Usage & billing", icons::GAUGE),
            (Page::DataControls, "Data controls", icons::SHIELD_ALERT),
        ],
    ),
    (
        "Integrations",
        &[
            (Page::Plugins, "Plugins", icons::AT),
            (Page::ComputerUse, "Computer use", icons::CURSOR_CLICK),
            (Page::Appshots, "Appshots", icons::CAMERA),
            (Page::Browser, "Browser", icons::BROWSER),
        ],
    ),
    (
        "Coding",
        &[
            (Page::Hooks, "Hooks", icons::ANCHOR),
            (Page::Connections, "Connections", icons::GLOBE),
            (Page::CodeReview, "Code Review", icons::PR),
            (Page::Git, "Git", icons::BRANCH),
            (Page::Worktrees, "Worktrees", icons::WORKTREE),
        ],
    ),
];

pub fn title(page: Page) -> &'static str {
    SECTIONS
        .iter()
        .flat_map(|(_, pages)| pages.iter())
        .find(|(p, _, _)| *p == page)
        .map_or("Settings", |(_, t, _)| t)
}

pub fn view(app: &mut Codex, page: Page, p: &Pal, f: &Frame, vcx: &mut ViewContext) -> AnyElement {
    let h = f.card_h();
    let local = f.main_x - f.left;
    let mut root = div().absolute().left(0.0).top(0.0).w(f.w).h(h);
    if f.sidebar_w > 0.0 {
        root = root.child(nav(page, p, f.sidebar_w, h));
    }
    let content_w = (f.main_w - 40.0).clamp(0.0, 600.0);
    let body = page_body(app, page, p, content_w);
    let _ = vcx;
    root.child(
        div()
            .absolute()
            .left(local)
            .top(0.0)
            .w(f.main_w)
            .h(h)
            .flex_col()
            .pl(20.0)
            .overflow_y_scroll()
            .track_scroll(&app.settings_handle)
            .scrollbar_auto_hide()
            .child(
                div()
                    .w(content_w)
                    .flex_col()
                    .flex_shrink_0()
                    .pt(74.0)
                    .pb(60.0)
                    .child(
                        text(title(page))
                            .size(26.0)
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

fn nav(page: Page, p: &Pal, w: f32, h: f32) -> Div {
    let mut list = div().flex_col().w(272.0);
    for (i, (section, pages)) in SECTIONS.iter().enumerate() {
        list = list.child(
            hrow()
                .h(if i == 0 { 30.0 } else { 46.0 })
                .flex_shrink_0()
                .pt(if i == 0 { 0.0 } else { 16.0 })
                .pl(8.0)
                .child(txt(*section, BODY, p.sidebar_muted)),
        );
        for (pg, label, icon) in pages.iter() {
            let selected = *pg == page;
            let mut row = hrow()
                .h(30.0)
                .flex_shrink_0()
                .pl(8.0)
                .pr(10.0)
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
                .child(ico(icon, 15.0, p.sidebar_text))
                .child(txt(*label, BODY, p.sidebar_text));
            if *pg == Page::Profile {
                row = row.child(div().flex_1()).child(ico(
                    icons::OPEN_EXTERNAL,
                    13.0,
                    p.sidebar_muted,
                ));
            }
            list = list.child(row).child(div().h(1.0));
        }
    }
    div()
        .absolute()
        .left(0.0)
        .top(0.0)
        .w(w)
        .h(h)
        .bg(p.sidebar)
        .border_r(p.frame_border.lerp(p.text, 0.06))
        .accessibility_role(Role::Navigation)
        .accessibility_label("Settings")
        .child(
            div().absolute().left(16.0).top(14.0).child(
                text("Settings")
                    .size(18.0)
                    .semibold()
                    .color(p.text)
                    .no_wrap(),
            ),
        )
        .child(
            hrow()
                .absolute()
                .left(8.0)
                .top(52.0)
                .w(272.0)
                .h(36.0)
                .pl(13.0)
                .gap(9.0)
                .rounded(18.0)
                .bg(p.rail_tile.lerp(p.sidebar, 0.45))
                .child(ico(icons::SEARCH, 15.0, p.sidebar_muted))
                .child(txt("Search", BODY, p.sidebar_muted)),
        )
        .child(
            div()
                .absolute()
                .left(8.0)
                .top(104.0)
                .h(h - 104.0)
                .overflow_hidden()
                .child(list),
        )
}

fn section_title(p: &Pal, title: &str) -> Div {
    div().pb(12.0).child(
        text(title.to_owned())
            .size(BODY)
            .medium()
            .color(p.text)
            .no_wrap(),
    )
}

/// A grouped card of rows separated by inset hairlines.
fn group(p: &Pal, rows: Vec<Div>) -> Div {
    let border = settings_border(p);
    let mut card = div()
        .w_full()
        .flex_col()
        .bg(settings_card(p))
        .border(border)
        .rounded(12.0);
    let n = rows.len();
    for (i, row) in rows.into_iter().enumerate() {
        card = card.child(row);
        if i + 1 < n {
            card = card.child(
                div()
                    .w_full()
                    .px(16.0)
                    .child(div().w_full().h(1.0).bg(border)),
            );
        }
    }
    card
}

fn settings_card(p: &Pal) -> Color {
    if p.mode == ThemeMode::Dark {
        Color::rgba(0x23, 0x23, 0x23, 255)
    } else {
        p.settings_card
    }
}

fn settings_border(p: &Pal) -> Color {
    if p.mode == ThemeMode::Dark {
        Color::rgba(0x35, 0x35, 0x35, 255)
    } else {
        p.settings_border
    }
}

fn setting_row(
    p: &Pal,
    title: &str,
    desc: Option<&str>,
    control: impl IntoAnyElement,
    w: f32,
) -> Div {
    setting_row_wrap(p, title, desc, control, (w - 220.0).max(160.0))
}

/// [`setting_row`] with the description wrapped at `wrap`.
fn setting_row_wrap(
    p: &Pal,
    title: &str,
    desc: Option<&str>,
    control: impl IntoAnyElement,
    wrap: f32,
) -> Div {
    let mut text_col = div()
        .flex_col()
        .flex_1()
        .min_w(0.0)
        .gap(3.0)
        .child(txt(title, BODY, p.text));
    if let Some(desc) = desc {
        text_col = text_col.child(
            text(desc.to_owned())
                .size(SMALL)
                .color(p.muted)
                .line_height(1.25)
                .wrap_width(wrap),
        );
    }
    hrow()
        .w_full()
        .min_h(44.0)
        .px(16.0)
        .py(11.0)
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
        .h(28.0)
        .px(10.0)
        .gap(8.0)
        .rounded(9.0)
        .border(settings_border(p).lerp(p.text, 0.08));
    if let Some(icon) = icon {
        d = d.child(svg_icon(icon, 14.0));
    }
    d.child(txt(label, SMALL, p.text))
        .child(ico(icons::CHEVRON_DOWN, 11.0, p.muted))
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
    let change = hrow()
        .h(28.0)
        .px(10.0)
        .rounded(8.0)
        .bg(p.rail_tile.lerp(settings_card(p), 0.3))
        .child(txt("Change", SMALL, p.text));
    div()
        .flex_col()
        .w(w)
        .child(section_title(p, "Permissions"))
        .child(group(
            p,
            vec![
                setting_row(
                    p,
                    "Default permissions",
                    Some("By default, ChatGPT can read and edit files in its workspace. It can ask for additional access when needed"),
                    div().opacity(0.6).child(toggle(p, true)),
                    w,
                ),
                setting_row(
                    p,
                    "Full access",
                    Some("When ChatGPT runs with full access, it can edit any file on your computer and run commands with network, without your approval. This significantly increases the risk of data loss, leaks, or unexpected behavior. Learn more about elevated risks."),
                    toggle_for(app, p, "full-access"),
                    w,
                ),
            ],
        ))
        .child(div().h(44.0))
        .child(section_title(p, "General"))
        .child(group(
            p,
            vec![
                setting_row_wrap(
                    p,
                    "Projectless task folder",
                    Some("The location where tasks started outside of projects store their data by default."),
                    hrow().gap(12.0).child(text("/Users/rohit/…uments/Codex").size(12.5).mono().color(p.muted).no_wrap()).child(change),
                    250.0,
                ),
                setting_row(p, "Default file open destination", Some("Where files and folders open by default"), dropdown(p, "Default app", Some(icons::APP_FINDER)), w),
                setting_row(p, "Language", Some("Language for the app UI"), dropdown(p, "Auto detect", None), w),
                setting_row(p, "Show in menu bar", Some("Keep ChatGPT in the menu bar when its window is closed"), toggle_for(app, p, "menu-bar"), w),
            ],
        ))
}

fn swatch(p: &Pal, dot: Color, label: &str) -> Div {
    hrow()
        .h(30.0)
        .pl(9.0)
        .pr(10.0)
        .gap(8.0)
        .rounded(15.0)
        .border(settings_border(p).lerp(p.text, 0.08))
        .child(
            div()
                .w(14.0)
                .h(14.0)
                .rounded(7.0)
                .bg(dot)
                .border(p.muted.with_alpha(90)),
        )
        .child(
            text(label.to_owned())
                .size(12.5)
                .mono()
                .color(p.text)
                .no_wrap(),
        )
}

fn appearance(app: &Codex, p: &Pal, w: f32) -> Div {
    let preview = |choice: ThemeChoice, left: Color, right: Color| {
        let selected = app.theme_choice == choice;
        let mut tile = hrow()
            .w(80.0)
            .h(58.0)
            .rounded(8.0)
            .overflow_hidden()
            .bg(left)
            .accessibility_role(Role::RadioButton)
            .accessibility_label(format!("{choice:?}"))
            .accessibility_selected(selected)
            .on_click(Msg::SetTheme(choice))
            .child(
                div()
                    .w(40.0)
                    .h(58.0)
                    .bg(left)
                    .flex_col()
                    .p(8.0)
                    .gap(4.0)
                    .child(div().w(20.0).h(3.0).bg(p.muted))
                    .child(div().w(28.0).h(3.0).bg(p.muted)),
            )
            .child(
                div()
                    .w(40.0)
                    .h(58.0)
                    .bg(right)
                    .flex_col()
                    .p(8.0)
                    .gap(4.0)
                    .child(div().w(24.0).h(3.0).bg(p.accent))
                    .child(div().w(18.0).h(3.0).bg(p.accent)),
            );
        tile = if selected {
            tile.border(p.accent).border_w(2.0)
        } else {
            tile.border(settings_border(p))
        };
        tile
    };
    let white = Color::rgba(0xf4, 0xf4, 0xf4, 255);
    let black = Color::rgba(0x1c, 0x1c, 0x1c, 255);
    let (bg_hex, fg_hex, bg, fg) = if p.mode == ThemeMode::Dark {
        (
            "#181818",
            "#FFFFFF",
            Color::rgba(0x18, 0x18, 0x18, 255),
            Color::rgba(255, 255, 255, 255),
        )
    } else {
        (
            "#FFFFFF",
            "#1A1C1F",
            Color::rgba(255, 255, 255, 255),
            Color::rgba(0x1a, 0x1c, 0x1f, 255),
        )
    };
    let theme_picker = hrow()
        .gap(14.0)
        .child(ico(icons::DOWNLOAD, 15.0, p.muted))
        .child(ico(icons::COPY, 15.0, p.muted))
        .child(
            hrow()
                .h(30.0)
                .pl(6.0)
                .pr(10.0)
                .gap(8.0)
                .rounded(15.0)
                .bg(p.bg)
                .w(176.0)
                .child(
                    div()
                        .w(20.0)
                        .h(20.0)
                        .rounded(5.0)
                        .bg(p.badge)
                        .items_center()
                        .justify_center()
                        .child(
                            text("Aa")
                                .size(10.0)
                                .semibold()
                                .color(p.badge_text)
                                .no_wrap(),
                        ),
                )
                .child(txt("ChatGPT", SMALL, p.text))
                .child(div().flex_1())
                .child(ico(icons::CHEVRON_DOWN, 11.0, p.muted)),
        );
    div()
        .flex_col()
        .w(w)
        .child(section_title(p, "Visual style"))
        .child(group(
            p,
            vec![
                setting_row(
                    p,
                    "Mode",
                    None,
                    hrow()
                        .gap(16.0)
                        .child(preview(ThemeChoice::System, white, black))
                        .child(preview(ThemeChoice::Light, white, white))
                        .child(preview(ThemeChoice::Dark, black, black)),
                    w,
                )
                .h(76.0),
            ],
        ))
        .child(div().h(16.0))
        .child(group(
            p,
            vec![
                setting_row(p, "Theme", None, theme_picker, w),
                setting_row(
                    p,
                    "Accent",
                    None,
                    swatch(p, Color::rgba(255, 255, 255, 255), "White"),
                    w,
                ),
                setting_row(p, "Background", None, swatch(p, bg, bg_hex), w),
                setting_row(p, "Foreground", None, swatch(p, fg, fg_hex), w),
                setting_row(p, "Font", None, dropdown(p, "System", None), w),
            ],
        ))
        .child(div().h(50.0))
        .child(
            hrow()
                .gap(6.0)
                .child(txt("Advanced", BODY, p.muted))
                .child(ico(icons::CHEVRON_RIGHT, 12.0, p.muted)),
        )
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
        ("Changes", "⌃⇧G"),
    ];
    group(
        p,
        rows.iter()
            .map(|(t, k)| setting_row(p, t, None, kbd(p, k), w))
            .collect(),
    )
}
