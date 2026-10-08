//! The side panel card: the Changes tab (scope pill, options, split diff
//! of cart.js with "1 unmodified line" folds and word highlights), the
//! Terminal tab, and the file viewer with its tree. Its tab strip lives in
//! the title bar (`tab_strip`). The full view hands it the whole window
//! and floats the composer over it (u29).

use accesskit::Role;
use quark_app::ViewContext;
use quark_app::quark_ui::element::*;
use quark_app::quark_ui::style::Styled;
use quark_app::quark_ui::theme::Color;

use crate::data::{self, DiffLine};
use crate::theme::{BODY, CODE, Pal, SMALL};
use crate::widgets::*;
use crate::{Codex, Frame, Menu, Msg, SCOPES, Tab, composer, icons};

/// The panel card's width beside the thread (u25, u26).
pub const PANEL_W: f32 = 319.0;
const ROW_H: f32 = 21.5;
const GUTTER: f32 = 46.0;

pub fn width(app: &Codex, avail: f32) -> f32 {
    if !app.side_panel || !matches!(app.screen, crate::Screen::Thread(_)) {
        0.0
    } else if app.full_view {
        avail
    } else {
        PANEL_W.min(avail * 0.6)
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
        Tab::Changes => changes(app, p, w, h),
        Tab::Terminal => div()
            .pl(16.0)
            .pt(8.0)
            .child(
                app.terminal
                    .view(w - 16.0, (h - if full { 80.0 } else { 8.0 }).max(0.0), vcx),
            )
            .into_any(),
        Tab::File => file_viewer(app, p, w, h),
    };
    let mut pane = div()
        .absolute()
        .left(x)
        .top(0.0)
        .w(w)
        .h(h)
        .bg(p.bg)
        .when(!full, |d| d.border_l(p.frame_border.lerp(p.text, 0.06)))
        .accessibility_role(Role::Complementary)
        .accessibility_label("Side panel")
        .child(body);
    if full {
        let cw = 560.0_f32.min(w - 40.0);
        let composer = composer::compact(app, p, cw, vcx);
        pane = pane.child(
            div()
                .absolute()
                .left(((w - cw) / 2.0).round())
                .top(h - 60.0)
                .child(composer),
        );
    }
    pane.into_any()
}

/// The panel's tabs in the title bar, with "+", the full view toggle, and
/// the panel toggle. In the full view the thread is a tab too.
pub fn tab_strip(app: &Codex, p: &Pal, f: &Frame) -> Div {
    let full = app.full_view;
    let left = if full { 232.0 } else { f.panel_x + 7.0 };
    let mut strip = hrow().absolute().left(left).top(7.0).h(30.0).gap(6.0);
    let tab =
        |icon: AnyElement, label: String, active: bool, w: f32, msg: Msg, close: Option<Msg>| {
            let mut t =
                hrow()
                    .w(w)
                    .h(30.0)
                    .pl(10.0)
                    .pr(6.0)
                    .gap(8.0)
                    .rounded(8.0)
                    .hover_bg(p.rail_tile.with_alpha(120))
                    .accessibility_role(Role::Tab)
                    .accessibility_label(label.clone())
                    .accessibility_selected(active)
                    .on_click(msg)
                    .child(icon)
                    .child(div().flex_1().min_w(0.0).overflow_hidden().child(
                        txt(label, SMALL, if active { p.text } else { p.muted }).truncate(),
                    ));
            if active {
                t = t.bg(p.rail_tile).border(p.frame_border.lerp(p.text, 0.08));
            }
            if let Some(close) = close {
                t = t.child(icon_button(
                    p,
                    icons::CLOSE,
                    20.0,
                    11.0,
                    p.muted,
                    "Close tab",
                    close,
                ));
            }
            t
        };
    if full && let Some(t) = app.current_thread() {
        strip = strip
            .child(
                tab(
                    ico(icons::CHAT, 14.0, p.muted).into_any(),
                    t.title.clone(),
                    false,
                    230.0,
                    Msg::FullView,
                    None,
                )
                .child(ico(icons::ELLIPSIS, 13.0, p.muted)),
            )
            .child(div().w(2.0));
    }
    for t in &app.tabs {
        let (icon, label): (AnyElement, String) = match t {
            Tab::Changes => (
                ico(icons::REVIEW, 14.0, p.text_soft).into_any(),
                "Changes".into(),
            ),
            Tab::Terminal => (
                ico(icons::TERMINAL, 14.0, p.text_soft).into_any(),
                data::DEMO_PROJECT.into(),
            ),
            Tab::File => (
                js_badge().into_any(),
                app.open_file.unwrap_or("cart.js").into(),
            ),
        };
        let w = if full {
            236.0
        } else {
            (f.panel_w - 118.0).max(80.0) / app.tabs.len() as f32
        };
        strip = strip.child(tab(
            icon,
            label,
            *t == app.tab,
            w,
            Msg::ShowTab(*t),
            Some(Msg::CloseTab(*t)),
        ));
    }
    strip = strip.child(div().w(6.0)).child(
        icon_button(
            p,
            icons::PLUS,
            28.0,
            16.0,
            p.icon,
            "New tab",
            Msg::Open(Menu::PanelTab),
        )
        .id("panel.newtab"),
    );
    let mut right = hrow()
        .absolute()
        .left(f.w - 129.0)
        .top(8.0)
        .gap(6.0)
        .w(121.0)
        .justify_end();
    if full {
        right = right.child(
            icon_button(
                p,
                icons::SUMMARY,
                28.0,
                16.0,
                p.icon,
                "Toggle summary",
                Msg::Open(Menu::Summary),
            )
            .id("header.summary"),
        );
    }
    let full_toggle = icon_button(
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
        Msg::FullView,
    );
    right = right
        .child(if full {
            full_toggle.bg(p.rail_tile)
        } else {
            full_toggle
        })
        .child(
            icon_button(
                p,
                icons::PANEL_RIGHT,
                28.0,
                16.0,
                p.text,
                "Hide tabs",
                Msg::ToggleSidePanel,
            )
            .bg(p.rail_tile),
        );
    div().child(strip).child(right)
}

/// The Changes tab.
fn changes(app: &Codex, p: &Pal, w: f32, h: f32) -> AnyElement {
    let full = app.full_view;
    let scope = hrow()
        .h(32.0)
        .pl(14.0)
        .pr(10.0)
        .gap(6.0)
        .rounded(16.0)
        .bg(p.tray)
        .id("changes.scope")
        .accessibility_role(Role::Button)
        .accessibility_label(SCOPES[app.scope])
        .on_click(Msg::Open(Menu::ChangesScope))
        .child(txt(SCOPES[app.scope], SMALL, p.text))
        .child(ico(icons::CHEVRON_DOWN, 11.0, p.text))
        .child(div().w(4.0))
        .child(txt("+2", SMALL, p.add_num))
        .child(txt("-2", SMALL, p.del_num));
    let tool = |svg: &'static str, label: &str, msg: Msg| {
        icon_button(p, svg, 28.0, 14.0, p.icon, label, msg)
    };
    let mut tools = hrow()
        .h(32.0)
        .px(4.0)
        .gap(2.0)
        .rounded(16.0)
        .bg(p.tray)
        .child(
            tool(icons::ELLIPSIS, "Options", Msg::Open(Menu::ChangesOptions)).id("changes.options"),
        )
        .child(tool(icons::FILE_SEARCH, "Jump to file", Msg::Noop));
    if full {
        tools = tools
            .child(tool(icons::REFRESH_CW, "Refresh", Msg::Noop))
            .child(tool(icons::WRAP, "Word wrap", Msg::Noop))
            .child(tool(icons::LIST_FILTER, "Expand all diffs", Msg::Noop))
            .child(
                div()
                    .w(28.0)
                    .h(28.0)
                    .items_center()
                    .justify_center()
                    .child(split_glyph(p)),
            );
    }
    tools = tools.child(tool(icons::FILES, "Show files", Msg::Noop));
    let toolbar = hrow()
        .w(w)
        .h(48.0)
        .px(8.0)
        .child(scope)
        .child(div().flex_1())
        .child(tools);
    let header = hrow()
        .w(w)
        .h(36.0)
        .pl(16.0)
        .pr(10.0)
        .gap(9.0)
        .border_b(p.hairline)
        .border_t(p.hairline)
        .accessibility_role(Role::Heading)
        .accessibility_label("cart.js")
        .child(js_badge())
        .child(txt("cart.js", BODY, p.text_soft))
        .child(div().flex_1())
        .child(txt("+2", BODY, p.add_num))
        .child(txt("-2", BODY, p.del_num))
        .child(div().w(2.0))
        .child(icon_button(
            p,
            icons::OPEN_EXTERNAL,
            24.0,
            13.0,
            p.icon,
            "Open in",
            Msg::OpenFile("cart.js"),
        ))
        .child(icon_button(
            p,
            icons::ELLIPSIS,
            24.0,
            13.0,
            p.icon,
            "File options",
            Msg::Noop,
        ));
    div()
        .w(w)
        .h(h)
        .flex_col()
        .child(div().h(4.0))
        .child(toolbar)
        .child(header)
        .child(split_diff(p, w))
        .into_any()
}

/// The split-diff toolbar glyph: a red and a green half.
fn split_glyph(p: &Pal) -> Div {
    div()
        .w(14.0)
        .h(12.0)
        .rounded(3.0)
        .overflow_hidden()
        .flex_row()
        .border(p.icon)
        .child(div().w(6.0).h_full().bg(p.del_bar))
        .child(div().w(6.0).h_full().bg(p.add_bar))
}

/// The striped bar a deleted row carries; added rows get a solid one.
fn bar(p: &Pal, kind: i8) -> Div {
    let mut b = div()
        .absolute()
        .left(0.0)
        .top(0.0)
        .w(4.0)
        .h(ROW_H)
        .flex_col();
    if kind < 0 {
        for i in 0..6 {
            b = b.child(div().w(4.0).h(ROW_H / 6.0).bg(if i % 2 == 0 {
                p.del_bar
            } else {
                p.del_code
            }));
        }
    } else {
        b = b.bg(p.add_bar);
    }
    b
}

/// One side of a diff row: gutter, number, highlighted code; added lines
/// get word highlights on what the change added.
fn diff_cell(
    p: &Pal,
    w: f32,
    num: Option<u32>,
    kind: i8,
    code: &str,
    words: &[&str],
    with_bar: bool,
) -> Div {
    let (bg, num_color) = match kind {
        -1 => (Some(p.del_code), p.del_num),
        1 => (Some(p.add_code), p.add_num),
        _ => (None, p.line_num),
    };
    let mut gutter = hrow().w(GUTTER).h(ROW_H).justify_end().pr(10.0).relative();
    if with_bar && kind != 0 {
        gutter = gutter.child(bar(p, kind));
    }
    if let Some(n) = num {
        gutter = gutter.child(
            text(n.to_string())
                .size(CODE)
                .mono()
                .color(num_color)
                .no_wrap(),
        );
    }
    let mut code_box = hrow()
        .w((w - GUTTER).max(0.0))
        .h(ROW_H)
        .pl(12.0)
        .overflow_hidden();
    if let Some(bg) = bg {
        code_box = code_box.bg(bg);
        gutter = gutter.bg(bg);
    }
    hrow()
        .w(w)
        .h(ROW_H)
        .child(gutter)
        .child(code_box.child(highlight_words(code, words, p)))
}

/// A unified diff row `w` wide, for the inline diff card.
pub fn diff_row(p: &Pal, w: f32, line: DiffLine, with_bar: bool) -> Div {
    match line {
        DiffLine::Context { new, .. } => diff_cell(
            p,
            w,
            Some(new),
            0,
            data::line_of(data::CART_JS, new),
            &[],
            with_bar,
        ),
        DiffLine::Del { old } => diff_cell(
            p,
            w,
            Some(old),
            -1,
            data::line_of(data::CART_JS_OLD, old),
            &[],
            with_bar,
        ),
        DiffLine::Add { new } => diff_cell(
            p,
            w,
            Some(new),
            1,
            data::line_of(data::CART_JS, new),
            data::added_words(new),
            with_bar,
        ),
        DiffLine::Fold(n) => fold_bar(p, w, n),
    }
}

fn fold_bar(p: &Pal, w: f32, n: u32) -> Div {
    hrow().w(w).h(33.0).child(div().w(GUTTER).h(33.0)).child(
        hrow().flex_1().h(33.0).pl(8.0).bg(p.fold).child(txt(
            format!("{n} unmodified line{}", if n == 1 { "" } else { "s" }),
            SMALL,
            p.muted,
        )),
    )
}

fn split_diff(p: &Pal, w: f32) -> Div {
    let half = (w / 2.0).floor();
    let mut col = div().flex_col().w(w).pt(2.0);
    let lines = data::cart_diff(true);
    let mut i = 0;
    while i < lines.len() {
        let (left, right) = match lines[i] {
            DiffLine::Fold(n) => {
                i += 1;
                (
                    fold_bar(p, half, n),
                    hrow()
                        .w(half)
                        .h(33.0)
                        .pl(4.0)
                        .child(div().w(half - 4.0).h(33.0).bg(p.fold)),
                )
            }
            DiffLine::Context { old, new } => {
                i += 1;
                (
                    diff_cell(
                        p,
                        half,
                        Some(old),
                        0,
                        data::line_of(data::CART_JS_OLD, old),
                        &[],
                        true,
                    ),
                    diff_cell(
                        p,
                        half,
                        Some(new),
                        0,
                        data::line_of(data::CART_JS, new),
                        &[],
                        true,
                    ),
                )
            }
            DiffLine::Del { old } => {
                let new = match lines.get(i + 1) {
                    Some(DiffLine::Add { new }) => {
                        i += 1;
                        Some(*new)
                    }
                    _ => None,
                };
                i += 1;
                let right = match new {
                    Some(n) => diff_cell(
                        p,
                        half,
                        Some(n),
                        1,
                        data::line_of(data::CART_JS, n),
                        data::added_words(n),
                        true,
                    ),
                    None => diff_cell(p, half, None, 0, "", &[], false),
                };
                (
                    diff_cell(
                        p,
                        half,
                        Some(old),
                        -1,
                        data::line_of(data::CART_JS_OLD, old),
                        &[],
                        true,
                    ),
                    right,
                )
            }
            DiffLine::Add { new } => {
                i += 1;
                (
                    diff_cell(p, half, None, 0, "", &[], false),
                    diff_cell(
                        p,
                        half,
                        Some(new),
                        1,
                        data::line_of(data::CART_JS, new),
                        data::added_words(new),
                        true,
                    ),
                )
            }
        };
        col = col.child(hrow().w(w).child(left).child(right));
    }
    col
}

/// [`highlight`] with `words` (substrings of `code`) on a stronger tint.
fn highlight_words(code: &str, words: &[&str], p: &Pal) -> Div {
    if words.is_empty() {
        return highlight(code, p);
    }
    // Split the line at the highlighted words and tint those pieces.
    let mut row = hrow();
    let mut rest = code;
    let mut pieces: Vec<(&str, bool)> = Vec::new();
    while !rest.is_empty() {
        let hit = words
            .iter()
            .filter_map(|w| rest.find(w).map(|at| (at, *w)))
            .min_by_key(|(at, _)| *at);
        match hit {
            Some((at, w)) => {
                if at > 0 {
                    pieces.push((&rest[..at], false));
                }
                pieces.push((&rest[at..at + w.len()], true));
                rest = &rest[at + w.len()..];
            }
            None => {
                pieces.push((rest, false));
                rest = "";
            }
        }
    }
    for (piece, hot) in pieces {
        let h = highlight(piece, p);
        row = row.child(if hot {
            h.bg(p.word_add).rounded(3.0)
        } else {
            h
        });
    }
    row
}

/// A small JavaScript highlighter in Codex's token colors: comments,
/// keywords, called names, numbers, strings, and identifiers after `.`.
pub fn highlight(code: &str, p: &Pal) -> Div {
    let mut row = hrow();
    for (token, color) in tokens(code, p) {
        row = row.child(
            text(token.replace(' ', "\u{a0}"))
                .size(CODE)
                .mono()
                .color(color)
                .no_wrap(),
        );
    }
    row
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

fn file_tree(p: &Pal, w: f32, h: f32, selected: Option<&str>, review: bool) -> Div {
    let mut list = div()
        .flex_col()
        .w(w)
        .h(h)
        .px(8.0)
        .pt(8.0)
        .gap(0.0)
        .border_l(p.hairline);
    list = list.child(
        hrow()
            .h(28.0)
            .px(9.0)
            .gap(7.0)
            .rounded(7.0)
            .border(p.hairline)
            .child(ico(icons::SEARCH, 14.0, p.muted))
            .child(txt("Filter files...", BODY, p.muted)),
    );
    list = list.child(div().h(6.0));
    let files: Vec<(&str, bool)> = if review {
        vec![("cart.js", false)]
    } else {
        data::FILES.to_vec()
    };
    for (name, dir) in files {
        let sel = Some(name) == selected;
        let icon: AnyElement = if dir {
            ico(icons::CHEVRON_RIGHT, 14.0, p.muted).into_any()
        } else if name.ends_with(".js") {
            js_badge().into_any()
        } else if name.ends_with(".json") {
            svg_icon(icons::FILE_JSON, 15.0).into_any()
        } else {
            svg_icon(icons::FILE_MD, 15.0).into_any()
        };
        let name_static: &'static str = match name {
            "cart.test.js" => "cart.test.js",
            "package.json" => "package.json",
            "README.md" => "README.md",
            ".git" => ".git",
            _ => "cart.js",
        };
        list = list.child(
            hrow()
                .h(28.0)
                .px(7.0)
                .gap(10.0)
                .rounded(6.0)
                .when(sel, |d| d.bg(p.panel_tile))
                .hover_bg(p.panel_tile)
                .accessibility_role(Role::TreeItem)
                .accessibility_label(name.to_owned())
                .on_click(if dir {
                    Msg::Noop
                } else {
                    Msg::OpenFile(name_static)
                })
                .child(div().w(15.0).items_center().child(icon))
                .child(txt(name, SMALL, p.text_soft)),
        );
    }
    list
}

/// The Files tab: breadcrumbs, the open file with change markers, and the
/// file tree.
fn file_viewer(app: &Codex, p: &Pal, w: f32, h: f32) -> AnyElement {
    let file = app.open_file.unwrap_or("cart.js");
    let tree_w = 249.0_f32.min(w * 0.4);
    let code_w = w - tree_w;
    let content = match file {
        "cart.js" => data::CART_JS,
        "cart.test.js" => {
            "import { test } from \"node:test\";\nimport assert from \"node:assert\";\nimport { subtotal, applyDiscount } from \"./cart.js\";\n\ntest(\"subtotal multiplies price by quantity\", () => {\n  assert.equal(subtotal([{ price: 2, qty: 3 }]), 6);\n});\n\ntest(\"applyDiscount takes a percentage off\", () => {\n  assert.equal(applyDiscount(100, 10), 90);\n});\n"
        }
        "package.json" => {
            "{\n  \"name\": \"codex-demo\",\n  \"version\": \"0.1.0\",\n  \"type\": \"module\",\n  \"scripts\": { \"test\": \"node --test\" }\n}\n"
        }
        _ => "# codex-demo\n\nA tiny cart module for trying Codex.\n",
    };
    let crumbs = hrow()
        .w(w)
        .h(38.0)
        .pl(16.0)
        .pr(8.0)
        .gap(6.0)
        .border_b(p.hairline)
        .child(txt(data::DEMO_PROJECT, SMALL, p.muted))
        .child(ico(icons::CHEVRON_RIGHT, 13.0, p.muted))
        .child(txt(file, SMALL, p.text))
        .child(div().flex_1())
        .child(icon_button(
            p,
            icons::ELLIPSIS,
            28.0,
            15.0,
            p.icon,
            "More",
            Msg::Noop,
        ))
        .child(div().w(6.0))
        .child(
            hrow()
                .h(28.0)
                .px(9.0)
                .gap(8.0)
                .rounded(8.0)
                .border(p.hairline)
                .child(svg_icon(icons::APP_FINDER, 15.0))
                .child(txt("Open", BODY, p.text))
                .child(ico(icons::CHEVRON_DOWN, 13.0, p.muted)),
        )
        .child(div().w(6.0))
        .child(
            icon_button(
                p,
                icons::FILES,
                28.0,
                15.0,
                p.text,
                "Toggle file tree",
                Msg::Noop,
            )
            .bg(p.panel_tile),
        );
    let changed = [3u32, 7];
    let mut code = div().flex_col().w(code_w).pt(4.0);
    for (i, line) in content.lines().chain(std::iter::once("")).enumerate() {
        let n = i as u32 + 1;
        let marker = if file == "cart.js" && changed.contains(&n) {
            Color::rgba(0xf0, 0x8a, 0x3c, 255)
        } else {
            Color::TRANSPARENT
        };
        code = code.child(
            hrow()
                .h(ROW_H)
                .child(div().w(4.0).h(ROW_H).bg(marker))
                .child(
                    hrow().w(39.0).justify_end().child(
                        text(n.to_string())
                            .size(CODE)
                            .mono()
                            .color(p.line_num)
                            .no_wrap(),
                    ),
                )
                .child(div().w(16.0))
                .child(highlight(line, p)),
        );
    }
    div()
        .w(w)
        .h(h)
        .flex_col()
        .child(crumbs)
        .child(hrow().items_start().child(code).child(file_tree(
            p,
            tree_w,
            h - 38.0,
            Some(file),
            false,
        )))
        .into_any()
}
