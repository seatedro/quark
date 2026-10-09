//! Settings (u50, u51): the section list in the sidebar card ("Settings",
//! search, Personal / Integrations / Coding), and the pages in the main
//! card. General and Appearance follow the update captures; Keyboard
//! shortcuts and the rest show their headline rows (26.623 content).

use accesskit::Role;
use quark::view;
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
    let content_w = (f.main_w - 40.0).clamp(0.0, 600.0);
    let _ = vcx;
    view! {
        <div class="absolute left-0 top-0" w={f.w} h={h}>
            if f.sidebar_w > 0.0 {
                {nav(page, p, f.sidebar_w, h)}
            }
            <div
                class="absolute top-0 flex-col pl-5 overflow-y-scroll"
                left={local}
                w={f.main_w}
                h={h}
                track_scroll={&app.settings_handle}
                scrollbar_auto_hide
            >
                <div class="flex-col shrink-0 pt-[74] pb-[60]" w={content_w}>
                    <text size={26.0} color={p.text} class="font-medium whitespace-nowrap">
                        {title(page)}
                    </text>
                    <div class="h-10" />
                    {page_body(app, page, p, content_w)}
                </div>
            </div>
        </div>
    }
}

fn nav(page: Page, p: &Pal, w: f32, h: f32) -> AnyElement {
    view! {
        <div
            class="absolute left-0 top-0"
            w={w}
            h={h}
            bg={p.sidebar}
            border_r={p.frame_border.lerp(p.text, 0.06)}
            accessibility_role={Role::Navigation}
            aria-label="Settings"
        >
            <div class="absolute left-4 top-[14]">
                <text size={18.0} color={p.text} class="font-semibold whitespace-nowrap">
                    "Settings"
                </text>
            </div>
            <div
                class="flex-row items-center absolute left-2 top-[52] w-[272] h-9 pl-[13] gap-[9]
                        rounded-[18]"
                bg={p.rail_tile.lerp(p.sidebar, 0.45)}
            >
                <icon svg={icons::SEARCH} size={15.0} color={p.sidebar_muted} />
                <txt("Search", BODY, p.sidebar_muted) />
            </div>
            <div class="absolute left-2 top-[104] overflow-hidden" h={h - 104.0}>
                <div class="flex-col w-[272]">
                    for (i, (section, pages)) in SECTIONS.iter().enumerate() {
                        <div
                            class="flex-row items-center shrink-0 pl-2"
                            h={if i == 0 { 30.0 } else { 46.0 }}
                            pt={if i == 0 { 0.0 } else { 16.0 }}
                        >
                            <txt(*section, BODY, p.sidebar_muted) />
                        </div>
                        for (pg, label, icon) in pages.iter() {
                            let selected = *pg == page;
                            <div
                                class="flex-row items-center h-[30] shrink-0 pl-2 pr-[10] gap-2 rounded-[8]"
                                bg={if selected {
                                    p.row_selected
                                }}
                                hover_bg={if selected {
                                    p.row_selected
                                } else {
                                    p.row_hover
                                }}
                                role="button"
                                aria-label={label.to_string()}
                                aria-selected={selected}
                                on:click={Msg::Show(Screen::Settings(*pg))}
                            >
                                <icon svg={icon} size={15.0} color={p.sidebar_text} />
                                <txt(*label, BODY, p.sidebar_text) />
                                if *pg == Page::Profile {
                                    <div class="flex-1" />
                                    <icon
                                        svg={icons::OPEN_EXTERNAL}
                                        size={13.0}
                                        color={p.sidebar_muted}
                                    />
                                }
                            </div>
                            <div class="h-px" />
                        }
                    }
                </div>
            </div>
        </div>
    }
}

fn section_title(p: &Pal, title: &str) -> AnyElement {
    view! {
        <div class="pb-3">
            <text size={BODY} color={p.text} class="font-medium whitespace-nowrap">
                {title.to_owned()}
            </text>
        </div>
    }
}

/// A grouped card of rows separated by inset hairlines.
fn group<E: IntoAnyElement>(p: &Pal, rows: Vec<E>) -> AnyElement {
    let border = settings_border(p);
    let n = rows.len();
    view! {
        <div class="w-full flex-col rounded-[12]" bg={settings_card(p)} border={border}>
            for (i, row) in rows.into_iter().enumerate() {
                {row}
                if i + 1 < n {
                    <div class="w-full px-4">
                        <div class="w-full h-px" bg={border} />
                    </div>
                }
            }
        </div>
    }
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
    view! { -> Div,
        <div class="flex-row items-center w-full min-h-11 px-4 py-[11] gap-4">
            <div class="flex-col flex-1 min-w-0 gap-[3]">
                <txt(title, BODY, p.text) />
                if let Some(desc) = desc {
                    <text size={SMALL} color={p.muted} class="leading-tight" wrap_width={wrap}>
                        {desc.to_owned()}
                    </text>
                }
            </div>
            {control}
        </div>
    }
}

fn toggle_for(app: &Codex, p: &Pal, key: &'static str) -> AnyElement {
    view! {
        <div
            role="switch"
            aria-checked={app.toggles.contains(&key)}
            aria-label={key}
            on:click={Msg::SetToggle(key)}
        >
            {toggle(p, app.toggles.contains(&key))}
        </div>
    }
}

fn dropdown(p: &Pal, label: &str, icon: Option<&'static str>) -> AnyElement {
    view! {
        <div
            class="flex-row items-center h-7 px-[10] gap-2 rounded-[9]"
            border={settings_border(p).lerp(p.text, 0.08)}
        >
            if let Some(icon) = icon {
                <icon svg={icon} size={14.0} />
            }
            <txt(label, SMALL, p.text) />
            <icon svg={icons::CHEVRON_DOWN} size={11.0} color={p.muted} />
        </div>
    }
}

fn page_body(app: &Codex, page: Page, p: &Pal, w: f32) -> AnyElement {
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
                    .map(|(t, d)| setting_row(p, t, Some(d), view! { <div /> }, w))
                    .collect(),
            )
        }
    }
}

fn general(app: &Codex, p: &Pal, w: f32) -> AnyElement {
    view! {
        <div class="flex-col" w={w}>
            {section_title(p, "Permissions")}
            {group(
                p,
                vec![
                    setting_row(
                        p,
                        "Default permissions",
                        Some(
                            "By default, ChatGPT can read and edit files in its workspace. It can ask for additional access when needed"
                        ),
                        view! { <div class="opacity-60">{toggle(p, true)}</div> },
                        w,
                    ),
                    setting_row(
                        p,
                        "Full access",
                        Some(
                            "When ChatGPT runs with full access, it can edit any file on your computer and run commands with network, without your approval. This significantly increases the risk of data loss, leaks, or unexpected behavior. Learn more about elevated risks."
                        ),
                        toggle_for(app, p, "full-access"),
                        w,
                    ),
                ],
            )}
            <div class="h-11" />
            {section_title(p, "General")}
            {group(
                p,
                vec![
                    setting_row_wrap(
                        p,
                        "Projectless task folder",
                        Some(
                            "The location where tasks started outside of projects store their data by default."
                        ),
                        view! {
                            <div class="flex-row items-center gap-3">
                                <text size={12.5} color={p.muted} class="font-mono whitespace-nowrap">
                                    "/Users/rohit/…uments/Codex"
                                </text>
                                <div class="flex-row items-center h-7 px-[10] rounded-[8]"
                                     bg={p.rail_tile.lerp(settings_card(p), 0.3)}>
                                    <txt("Change", SMALL, p.text) />
                                </div>
                            </div>
                        },
                        250.0,
                    ),
                    setting_row(
                        p,
                        "Default file open destination",
                        Some("Where files and folders open by default"),
                        dropdown(p, "Default app", Some(icons::APP_FINDER)),
                        w
                    ),
                    setting_row(
                        p,
                        "Language",
                        Some("Language for the app UI"),
                        dropdown(p, "Auto detect", None),
                        w
                    ),
                    setting_row(
                        p,
                        "Show in menu bar",
                        Some("Keep ChatGPT in the menu bar when its window is closed"),
                        toggle_for(app, p, "menu-bar"),
                        w
                    ),
                ],
            )}
        </div>
    }
}

fn swatch(p: &Pal, dot: Color, label: &str) -> AnyElement {
    view! {
        <div
            class="flex-row items-center h-[30] pl-[9] pr-[10] gap-2 rounded-[15]"
            border={settings_border(p).lerp(p.text, 0.08)}
        >
            <div class="w-[14] h-[14] rounded-[7]" bg={dot} border={p.muted.with_alpha(90)} />
            <text size={12.5} color={p.text} class="font-mono whitespace-nowrap">
                {label.to_owned()}
            </text>
        </div>
    }
}

fn appearance(app: &Codex, p: &Pal, w: f32) -> AnyElement {
    let preview = |choice: ThemeChoice, left: Color, right: Color| {
        let selected = app.theme_choice == choice;
        view! {
            <div
                class="flex-row items-center w-20 h-[58] rounded-[8] overflow-hidden"
                bg={left}
                role="radio"
                aria-label={format!("{choice:?}")}
                aria-selected={selected}
                on:click={Msg::SetTheme(choice)}
                border={if selected {
                    p.accent
                } else {
                    settings_border(p)
                }}
                @when {selected} { class="border-2" }
            >
                <div class="w-10 h-[58] flex-col p-2 gap-1" bg={left}>
                    <div class="w-5 h-[3]" bg={p.muted} />
                    <div class="w-7 h-[3]" bg={p.muted} />
                </div>
                <div class="w-10 h-[58] flex-col p-2 gap-1" bg={right}>
                    <div class="w-6 h-[3]" bg={p.accent} />
                    <div class="w-[18] h-[3]" bg={p.accent} />
                </div>
            </div>
        }
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
    let theme_picker = view! {
        <div class="flex-row items-center gap-[14]">
            <icon svg={icons::DOWNLOAD} size={15.0} color={p.muted} />
            <icon svg={icons::COPY} size={15.0} color={p.muted} />
            <div
                class="flex-row items-center h-[30] pl-1.5 pr-[10] gap-2 rounded-[15] w-44"
                bg={p.bg}
            >
                <div class="w-5 h-5 rounded-[5] items-center justify-center" bg={p.badge}>
                    <text size={10.0} color={p.badge_text} class="font-semibold whitespace-nowrap">
                        "Aa"
                    </text>
                </div>
                <txt("ChatGPT", SMALL, p.text) />
                <div class="flex-1" />
                <icon svg={icons::CHEVRON_DOWN} size={11.0} color={p.muted} />
            </div>
        </div>
    };
    view! {
        <div class="flex-col" w={w}>
            {section_title(p, "Visual style")}
            {group(
                p,
                vec![view! {
                    <setting_row(p, "Mode", None, view! {
                        <div class="flex-row items-center gap-4">
                            {preview(ThemeChoice::System, white, black)}
                            {preview(ThemeChoice::Light, white, white)}
                            {preview(ThemeChoice::Dark, black, black)}
                        </div>
                    }, w) class="h-[76]" />
                }],
            )}
            <div class="h-4" />
            {group(
                p,
                vec![
                    setting_row(p, "Theme", None, theme_picker, w),
                    setting_row(
                        p,
                        "Accent",
                        None,
                        swatch(p, Color::rgba(255, 255, 255, 255), "White"),
                        w
                    ),
                    setting_row(p, "Background", None, swatch(p, bg, bg_hex), w),
                    setting_row(p, "Foreground", None, swatch(p, fg, fg_hex), w),
                    setting_row(p, "Font", None, dropdown(p, "System", None), w),
                ],
            )}
            <div class="h-[50]" />
            <div class="flex-row items-center gap-1.5">
                <txt("Advanced", BODY, p.muted) />
                <icon svg={icons::CHEVRON_RIGHT} size={12.0} color={p.muted} />
            </div>
        </div>
    }
}

fn shortcuts(p: &Pal, w: f32) -> AnyElement {
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
