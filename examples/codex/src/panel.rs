//! The side panel card: the Changes tab (scope pill, toolbar, find bar,
//! and the shared diff viewer of `diff::Changes`), the Terminal tab, and
//! the file viewer with its tree. Its tab strip lives in
//! the title bar (`tab_strip`). The full view hands it the whole window
//! and floats the composer over it (u29).

use accesskit::Role;
use quark::view;
use quark_app::ViewContext;
use quark_app::quark_ui::element::*;
use quark_app::quark_ui::style::Styled;
use quark_app::quark_ui::theme::Color;
use quark_components::split::{Axis, DIVIDER_THICKNESS, Pane, Split, SplitState};

use crate::data;
use crate::diff::DiffMsg;
use crate::theme::{BODY, CODE, Pal, SMALL};
use crate::widgets::*;
use crate::{Codex, Frame, Menu, Msg, SCOPES, Tab, composer, icons};

/// The panel card's width beside the thread (u25, u26), until the divider
/// moves it.
pub const PANEL_W: f32 = 319.0;
/// The narrowest the divider makes the panel, and the widest.
pub const PANEL_MIN: f32 = 240.0;
pub const PANEL_MAX: f32 = 900.0;
/// The narrowest the divider leaves the thread.
pub const THREAD_MIN: f32 = 320.0;
/// The divider's split: its focus and accessibility names.
const SPLIT_ID: &str = "codex.panel";
const ROW_H: f32 = 21.5;
/// The Changes tab's toolbar row, with the gap above it.
const TOOLBAR_H: f32 = 52.0;

/// The thread beside the panel: the thread takes what the panel leaves.
pub fn split_state() -> SplitState {
    SplitState::new(
        Axis::Horizontal,
        vec![
            Pane::flex("thread").min(THREAD_MIN),
            Pane::fixed("side panel", PANEL_W)
                .min(PANEL_MIN)
                .max(PANEL_MAX),
        ],
    )
}

pub fn width(app: &Codex, avail: f32) -> f32 {
    if !app.side_panel || !matches!(app.screen, crate::Screen::Thread(_)) {
        0.0
    } else if app.full_view {
        avail
    } else {
        app.panel_split.resolve(avail).as_slice()[1]
    }
}

/// The thread and the panel side by side, with the divider between them
/// that drags (or, focused, steps with the arrow keys) the panel's width.
pub fn beside_thread(
    app: &mut Codex,
    id: data::ThreadId,
    p: &Pal,
    f: &Frame,
    vcx: &mut ViewContext,
) -> AnyElement {
    let avail = f.main_w + DIVIDER_THICKNESS + f.panel_w;
    let thread = crate::thread::view(app, id, p, (0.0, f.main_w, f.card_h()), vcx);
    let panel = view(app, p, (0.0, f.panel_w, f.card_h()), vcx);
    // The divider draws in the theme's border color and lights up in its
    // accent while hovered: the panel's own edge line, and a faint tint
    // over the 8 points that take the pointer.
    let mut theme = vcx.theme.clone();
    theme.colors.border_variant = p.frame_border.lerp(p.text, 0.06);
    theme.colors.accent = p.text.with_alpha(28);
    let split = Split::new(SPLIT_ID, &app.panel_split, avail, |e| {
        Msg::PanelSplit(e).into()
    })
    .child(thread)
    .child(panel)
    .build(&theme);
    view! {
        <div class="absolute top-0" left={f.main_x - f.left} w={avail} h={f.card_h()}>
            <div w={avail} h={f.card_h()}>{split}</div>
        </div>
    }
}

pub fn view(
    app: &mut Codex,
    p: &Pal,
    (x, w, h): (f32, f32, f32),
    vcx: &mut ViewContext,
) -> AnyElement {
    let full = app.full_view;
    let body: AnyElement = match app.tab {
        Tab::Changes => changes(app, p, w, h, vcx),
        Tab::Terminal => {
            let term_h = (h - if full { 80.0 } else { 8.0 }).max(0.0);
            view! { <div class="pl-4 pt-2">{app.terminal.view(w - 16.0, term_h, vcx)}</div> }
        }
        Tab::File => file_viewer(app, p, w, h),
    };
    let composer = full.then(|| {
        let cw = 560.0_f32.min(w - 40.0);
        let composer = composer::compact(app, p, cw, vcx);
        view! {
            <div class="absolute" left={((w - cw) / 2.0).round()} top={h - 60.0}>{composer}</div>
        }
    });
    view! {
        <div
            class="absolute top-0"
            left={x}
            w={w}
            h={h}
            bg={p.bg}
            accessibility_role={Role::Complementary}
            aria-label="Side panel"
        >
            {body}
            {?composer}
        </div>
    }
}

/// The panel's tabs in the title bar, with "+", the full view toggle, and
/// the panel toggle. In the full view the thread is a tab too.
pub fn tab_strip(app: &Codex, p: &Pal, f: &Frame) -> AnyElement {
    let full = app.full_view;
    let left = if full { 232.0 } else { f.panel_x + 7.0 };
    let tab = |icon: AnyElement, label: String, active: bool, w: f32, msg: Msg| {
        view! { -> Div,
            <div
                class="flex-row items-center h-[30] pl-[10] pr-1.5 gap-2 rounded-[8]"
                w={w}
                hover_bg={p.rail_tile.with_alpha(120)}
                role="tab"
                aria-label={label.clone()}
                aria-selected={active}
                on:click={msg}
                @when {active} { bg={p.rail_tile} border={p.frame_border.lerp(p.text, 0.08)} }
            >
                {icon}
                <div class="flex-1 min-w-0 overflow-hidden">
                    <txt(label, SMALL, if active { p.text } else { p.muted }) class="truncate" />
                </div>
            </div>
        }
    };
    // Tabs share what the strip has: up to 236 points each in the full
    // view, the panel's width beside the thread.
    let tab_w = if full {
        (236.0_f32).min((f.w - 232.0 - 170.0 - 236.0) / app.tabs.len() as f32)
    } else {
        (f.panel_w - 118.0).max(80.0) / app.tabs.len() as f32
    };
    view! {
        <div>
            <div class="flex-row items-center absolute top-[7] h-[30] gap-1.5" left={left}>
                if full && let Some(t) = app.current_thread() {
                    <tab(
                        view! { <icon svg={icons::CHAT} size={14.0} color={p.muted} /> },
                        t.title.clone(),
                        false,
                        230.0,
                        Msg::FullView
                    )
                    >
                        <icon svg={icons::ELLIPSIS} size={13.0} color={p.muted} />
                    </tab>
                    <div class="w-0.5" />
                }
                for t in &app.tabs {
                    let (icon, label): (AnyElement, String) = match t {
                        Tab::Changes => (
                            view! { <icon svg={icons::REVIEW} size={14.0} color={p.text_soft} /> },
                            "Changes".into(),
                        ),
                        Tab::Terminal => (
                            view! { <icon svg={icons::TERMINAL} size={14.0} color={p.text_soft} /> },
                            data::DEMO_PROJECT.into(),
                        ),
                        Tab::File => (
                            view! { <js_badge() /> },
                            app.open_file.unwrap_or("cart.js").into(),
                        ),
                    };
                    <tab(icon, label, *t == app.tab, tab_w, Msg::ShowTab(*t))>
                        <icon_button(
                            p,
                            icons::CLOSE,
                            20.0,
                            11.0,
                            p.muted,
                            "Close tab",
                            Msg::CloseTab(*t)
                        )
                        />
                    </tab>
                }
                <div class="w-1.5" />
                <icon_button(
                    p,
                    icons::PLUS,
                    28.0,
                    16.0,
                    p.icon,
                    "New tab",
                    Msg::Open(Menu::PanelTab)
                )
                    id="panel.newtab"
                />
            </div>
            <div
                class="flex-row items-center absolute top-2 gap-1.5 w-[121] justify-end"
                left={f.w - 129.0}
            >
                if full {
                    <icon_button(
                        p,
                        icons::SUMMARY,
                        28.0,
                        16.0,
                        p.icon,
                        "Toggle summary",
                        Msg::Open(Menu::Summary)
                    )
                        id="header.summary"
                    />
                }
                <icon_button(
                    p,
                    if full { icons::COLLAPSE } else { icons::EXPAND },
                    28.0,
                    14.0,
                    p.icon,
                    if full {
                        "Exit full view"
                    } else {
                        "Enter full view"
                    },
                    Msg::FullView
                )
                    bg={if full {
                        p.rail_tile
                    }}
                />
                <icon_button(
                    p,
                    icons::PANEL_RIGHT,
                    28.0,
                    16.0,
                    p.text,
                    "Hide tabs",
                    Msg::ToggleSidePanel
                )
                    bg={p.rail_tile}
                />
            </div>
        </div>
    }
}

/// The Changes tab: scope pill, toolbar, the find bar while open, and
/// the shared diff viewer under them.
fn changes(app: &mut Codex, p: &Pal, w: f32, h: f32, vcx: &mut ViewContext) -> AnyElement {
    let full = app.full_view;
    let (adds, dels) = app.changes.stats();
    let wrap = app.changes.wraps();
    let find = app.changes.find_bar(p, w, vcx);
    let find_h = if find.is_some() {
        crate::diff::FIND_H
    } else {
        0.0
    };
    let diff = app
        .changes
        .review_view(w, (h - TOOLBAR_H - find_h).max(0.0), vcx);
    let tool = |svg: &'static str, label: &str, msg: Msg| {
        icon_button(p, svg, 28.0, 14.0, p.icon, label, msg)
    };
    view! {
        <div class="flex-col" w={w} h={h}>
            <div class="h-1" />
            <div class="flex-row items-center h-12 px-2" w={w} role="toolbar" aria-label="Changes">
                <div
                    class="flex-row items-center h-8 pl-[14] pr-[10] gap-1.5 rounded-[16]"
                    bg={p.tray}
                    id="changes.scope"
                    role="button"
                    aria-label={SCOPES[app.scope]}
                    on:click={Msg::Open(Menu::ChangesScope)}
                >
                    <txt(SCOPES[app.scope], SMALL, p.text) />
                    <icon svg={icons::CHEVRON_DOWN} size={11.0} color={p.text} />
                    <div class="w-1" />
                    <txt(format!("+{adds}"), SMALL, p.add_num) />
                    <txt(format!("-{dels}"), SMALL, p.del_num) />
                </div>
                <div class="flex-1" />
                <div class="flex-row items-center h-8 px-1 gap-0.5 rounded-[16]" bg={p.tray}>
                    <tool(icons::ELLIPSIS, "Options", Msg::Open(Menu::ChangesOptions))
                        id="changes.options"
                    />
                    <tool(icons::FILE_SEARCH, "Find in changes", Msg::Diff(DiffMsg::Find(true))) />
                    if full {
                        <tool(icons::REFRESH_CW, "Refresh", Msg::Noop) />
                        <tool(icons::WRAP, "Word wrap", Msg::Diff(DiffMsg::ToggleWrap))
                            aria-pressed={wrap}
                            bg={if wrap {
                                p.panel_tile
                            }}
                        />
                        <tool(
                            icons::LIST_FILTER,
                            "Expand all diffs",
                            Msg::Diff(DiffMsg::ExpandAll)
                        )
                        />
                        <div
                            class="w-7 h-7 items-center justify-center rounded-[7]"
                            role="button"
                            aria-label="Toggle split diff"
                            hover_bg={p.row_hover.with_alpha(110)}
                            on:click={Msg::Diff(DiffMsg::ToggleSplit)}
                        >
                            {split_glyph(p)}
                        </div>
                    }
                    <tool(icons::FILES, "Show files", Msg::Noop) />
                </div>
            </div>
            {?find}
            {diff}
        </div>
    }
}

/// The split-diff toolbar glyph: a red and a green half.
fn split_glyph(p: &Pal) -> AnyElement {
    view! {
        <div class="w-[14] h-3 rounded-[3] overflow-hidden flex-row" border={p.icon}>
            <div class="w-1.5 h-full" bg={p.del_bar} />
            <div class="w-1.5 h-full" bg={p.add_bar} />
        </div>
    }
}

/// A small JavaScript highlighter in Codex's token colors: comments,
/// keywords, called names, numbers, strings, and identifiers after `.`.
pub fn highlight(code: &str, p: &Pal) -> Div {
    view! { -> Div,
        <div class="flex-row items-center">
            for (token, color) in tokens(code, p) {
                <text size={CODE} color={color} class="font-mono whitespace-nowrap">
                    {token.replace(' ', "\u{a0}")}
                </text>
            }
        </div>
    }
}

pub fn tokens<'a>(code: &'a str, p: &Pal) -> Vec<(&'a str, Color)> {
    const KEYWORDS: &[&str] = &[
        "export", "return", "const", "let", "import", "from", "if", "else", "new",
    ];
    let mut out = Vec::new();
    let bytes = code.as_bytes();
    let mut i = 0;
    while i < code.len() {
        let rest = &code[i..];
        if rest.starts_with("//") {
            out.push((rest, p.syn_comment));
            break;
        }
        let c = bytes[i] as char;
        let len = if c.is_ascii_alphabetic() || c == '_' {
            rest.find(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
                .unwrap_or(rest.len())
        } else if c.is_ascii_digit() {
            rest.find(|ch: char| !ch.is_ascii_digit())
                .unwrap_or(rest.len())
        } else if c == '"' || c == '\'' {
            rest[1..].find(c).map_or(rest.len(), |e| e + 2)
        } else if c == ' ' {
            rest.find(|ch: char| ch != ' ').unwrap_or(rest.len())
        } else {
            rest.chars().next().map_or(1, char::len_utf8)
        };
        let token = &rest[..len];
        let after = rest[len..].trim_start();
        let before = code[..i].trim_end();
        let color = if c.is_ascii_digit() {
            p.syn_number
        } else if c == '"' || c == '\'' {
            p.syn_string
        } else if c.is_ascii_alphabetic() || c == '_' {
            if token == "function" {
                p.syn_function
            } else if KEYWORDS.contains(&token) {
                p.syn_keyword
            } else if after.starts_with('(') && !before.ends_with("function")
                || before.ends_with("function")
            {
                p.syn_function
            } else if before.ends_with('.') || before.ends_with("return") || before.ends_with("+") {
                p.syn_ident
            } else {
                p.code
            }
        } else if "=>*+-/?".contains(c) {
            if token == "=" && after.starts_with('>') || token == ">" && before.ends_with('=') {
                p.syn_function
            } else {
                p.syn_ident
            }
        } else {
            p.syn_punct
        };
        out.push((token, color));
        i += len;
    }
    out
}

/// The file tree beside the code: an opaque column of its own, so code
/// never shows through it, with the filter field and one row per entry.
fn file_tree(p: &Pal, w: f32, h: f32, selected: Option<&str>) -> AnyElement {
    view! {
        <div
            class="flex-col shrink-0 px-2 pt-2 overflow-hidden"
            w={w}
            h={h}
            bg={p.bg}
            border_l={p.hairline}
            role="tree"
            aria-label="Files"
        >
            <div
                class="flex-row items-center shrink-0 h-7 px-[9] gap-[7] rounded-[7] overflow-hidden"
                border={p.hairline}
            >
                <icon svg={icons::SEARCH} size={14.0} color={p.muted} />
                <div class="flex-1 min-w-0 overflow-hidden">
                    <txt("Filter files...", BODY, p.muted) class="truncate" />
                </div>
            </div>
            <div class="h-1.5 shrink-0" />
            for &(name, dir) in data::FILES {
                let sel = Some(name) == selected;
                <div
                    class="flex-row items-center shrink-0 h-7 px-[7] gap-[10] rounded-[6]"
                    bg={if sel {
                        p.panel_tile
                    }}
                    hover_bg={p.panel_tile}
                    role="treeitem"
                    aria-label={name.to_owned()}
                    aria-selected={sel}
                    on:click={if dir { Msg::Noop } else { Msg::OpenFile(name) }}
                >
                    <div class="w-[15] shrink-0 items-center">
                        if dir {
                            <icon svg={icons::CHEVRON_RIGHT} size={14.0} color={p.muted} />
                        } else if name.ends_with(".js") {
                            {js_badge()}
                        } else if name.ends_with(".json") {
                            <icon svg={icons::FILE_JSON} size={15.0} />
                        } else {
                            <icon svg={icons::FILE_MD} size={15.0} />
                        }
                    </div>
                    <div class="flex-1 min-w-0 overflow-hidden">
                        <txt(name, SMALL, p.text_soft) class="truncate" />
                    </div>
                </div>
            }
        </div>
    }
}

/// The marker strip, the line numbers right-aligned in it, and the gap
/// before the code (capture 40: numbers end at x 43, code starts at 60).
const MARKER_W: f32 = 4.0;
const NUMBERS_W: f32 = 39.0;
const GUTTER_GAP: f32 = 16.0;
/// The tree's widest; narrower panels give it 60% (capture 88).
const TREE_W: f32 = 249.0;
const HEADER_H: f32 = 38.0;

fn file_text(file: &str) -> &'static str {
    match file {
        "cart.js" => data::CART_JS,
        "cart.test.js" => {
            "import { test } from \"node:test\";\nimport assert from \"node:assert\";\nimport { subtotal, applyDiscount } from \"./cart.js\";\n\ntest(\"subtotal multiplies price by quantity\", () => {\n  assert.equal(subtotal([{ price: 2, qty: 3 }]), 6);\n});\n\ntest(\"applyDiscount takes a percentage off\", () => {\n  assert.equal(applyDiscount(100, 10), 90);\n});\n"
        }
        "package.json" => {
            "{\n  \"name\": \"codex-demo\",\n  \"version\": \"0.1.0\",\n  \"type\": \"module\",\n  \"scripts\": { \"test\": \"node --test\" }\n}\n"
        }
        _ => "# codex-demo\n\nA tiny cart module for trying Codex.\n",
    }
}

/// The Files tab: breadcrumbs, the open file, and the file tree. The code
/// scrolls down with its gutter and sideways on its own, clipped at the
/// tree, so line numbers stay in one fixed column at any width.
fn file_viewer(app: &Codex, p: &Pal, w: f32, h: f32) -> AnyElement {
    let file = app.open_file.unwrap_or("cart.js");
    let tree_w = TREE_W.min((w * 0.6).floor());
    let code_w = w - tree_w;
    let body_h = (h - HEADER_H).max(0.0);
    let lines: Vec<&str> = file_text(file).lines().chain(std::iter::once("")).collect();
    let changed = [3usize, 7];
    let marker = |n: usize| {
        if file == "cart.js" && changed.contains(&n) {
            Color::rgba(0xf0, 0x8a, 0x3c, 255)
        } else {
            Color::TRANSPARENT
        }
    };
    view! {
        <div class="flex-col" w={w} h={h}>
            <div
                class="flex-row items-center shrink-0 pl-4 pr-2 gap-1.5 overflow-hidden"
                w={w}
                h={HEADER_H}
                border_b={p.hairline}
            >
                // The project name gives way first, so the file's name stays.
                <div class="flex-row items-center flex-1 min-w-0 gap-1.5 overflow-hidden">
                    <div class="min-w-0 overflow-hidden">
                        <txt(data::DEMO_PROJECT, SMALL, p.muted) class="truncate" />
                    </div>
                    <icon svg={icons::CHEVRON_RIGHT} size={13.0} color={p.muted} />
                    <div class="shrink-0">
                        <txt(file, SMALL, p.text) />
                    </div>
                </div>
                <icon_button(p, icons::ELLIPSIS, 28.0, 15.0, p.icon, "More", Msg::Noop) />
                <div class="w-1.5 shrink-0" />
                <div
                    class="flex-row items-center shrink-0 h-7 px-[9] gap-2 rounded-[8]"
                    border={p.hairline}
                >
                    <icon svg={icons::APP_FINDER} size={15.0} />
                    <txt("Open", BODY, p.text) />
                    <icon svg={icons::CHEVRON_DOWN} size={13.0} color={p.muted} />
                </div>
                <div class="w-1.5 shrink-0" />
                <icon_button(p, icons::FILES, 28.0, 15.0, p.text, "Toggle file tree", Msg::Noop)
                    bg={p.panel_tile}
                />
            </div>
            <div class="flex-row shrink-0" w={w} h={body_h}>
                <div
                    class="shrink-0 overflow-y-scroll"
                    w={code_w}
                    h={body_h}
                    track_scroll={&app.file_scroll}
                    scrollbar_auto_hide
                    accessibility_role={Role::Document}
                    aria-label={file.to_owned()}
                >
                    <div class="flex-row items-start pt-1" w={code_w}>
                        <div class="flex-col shrink-0" w={MARKER_W + NUMBERS_W}>
                            for (i, _) in lines.iter().enumerate() {
                                <div class="flex-row items-center shrink-0" h={ROW_H}>
                                    <div
                                        class="shrink-0"
                                        w={MARKER_W}
                                        h={ROW_H}
                                        bg={marker(i + 1)}
                                    />
                                    <div
                                        class="flex-row items-center justify-end shrink-0"
                                        w={NUMBERS_W}
                                        test-id="file.line-number"
                                    >
                                        <text
                                            size={CODE}
                                            color={p.line_num}
                                            class="font-mono whitespace-nowrap"
                                        >
                                            {(i + 1).to_string()}
                                        </text>
                                    </div>
                                </div>
                            }
                        </div>
                        <div class="shrink-0" w={GUTTER_GAP} />
                        <div
                            class="flex-col overflow-x-scroll scrollbar-none"
                            w={(code_w - MARKER_W - NUMBERS_W - GUTTER_GAP).max(0.0)}
                            track_scroll={&app.file_scroll_x}
                            test-id="file.code"
                        >
                            for line in &lines {
                                <div class="flex-row items-center shrink-0" h={ROW_H}>
                                    {highlight(line, p)}
                                </div>
                            }
                        </div>
                    </div>
                </div>
                {file_tree(p, tree_w, body_h, Some(file))}
            </div>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::tokens;
    use crate::theme::DARK as P;

    // Catches the highlighter losing text or coloring a line differently
    // from the app: keywords red, `function` and called names purple,
    // members orange, comments grey.
    #[test]
    fn javascript_line_tokens_take_codex_colors() {
        let line = "export function subtotal(items) { return items.reduce(x); } // done";
        let toks = tokens(line, &P);
        assert_eq!(toks.iter().map(|t| t.0).collect::<String>(), line);
        let color = |word: &str| toks.iter().find(|t| t.0 == word).map(|t| t.1);
        assert_eq!(color("export"), Some(P.syn_keyword));
        assert_eq!(color("function"), Some(P.syn_function));
        assert_eq!(color("subtotal"), Some(P.syn_function));
        assert_eq!(color("reduce"), Some(P.syn_function));
        assert_eq!(color("items"), Some(P.code));
        assert_eq!(color("// done"), Some(P.syn_comment));
    }
}
