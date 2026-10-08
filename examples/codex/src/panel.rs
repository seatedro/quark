//! The right-hand side panel: its tab strip, the Review tab (scope menu,
//! toolbar, unified or split diff of cart.js, the floating Revert/Stage
//! pill), the Terminal tab, the file viewer with its file tree, and the
//! launcher an empty panel shows.

use accesskit::Role;
use quark_app::ViewContext;
use quark_app::quark_ui::element::*;
use quark_app::quark_ui::style::Styled;
use quark_app::quark_ui::theme::Color;

use crate::data::{self, DiffLine};
use crate::theme::{BODY, CODE, Pal, SMALL};
use crate::widgets::*;
use crate::{Codex, Menu, Msg, Tab, composer, icons};

/// The thread keeps this much beside the panel (captures 33 and 35).
pub const THREAD_KEEP: f32 = 352.0;
const ROW_H: f32 = 21.5;
const GUTTER: f32 = 46.0;

pub fn width(app: &Codex, avail: f32) -> f32 {
    if !app.side_panel || !matches!(app.screen, crate::Screen::Thread(_)) {
        0.0
    } else if app.panel_expanded {
        avail
    } else {
        (avail - THREAD_KEEP).max(300.0).min(avail)
    }
}

pub fn view(
    app: &mut Codex,
    p: &Pal,
    (x, w, h): (f32, f32, f32),
    vcx: &mut ViewContext,
) -> AnyElement {
    let expanded = app.panel_expanded;
    // Expanded, the panel's strip starts after the window controls.
    let strip_left = if expanded {
        if app.size.0 > 0.0 { 210.0 } else { 0.0 }
    } else {
        8.0
    };
    let mut pane = div()
        .absolute()
        .left(x)
        .top(0.0)
        .w(w)
        .h(h)
        .bg(p.bg)
        .when(!expanded, |d| d.border_l(p.hairline))
        .accessibility_role(Role::Complementary)
        .accessibility_label("Side panel")
        .child(tab_strip(app, p, w, strip_left));
    let body_top = 46.0;
    let body_h = h - body_top;
    let body: AnyElement = if app.tabs.is_empty() {
        launcher(p, w, body_h).into_any()
    } else {
        match app.tab {
            Tab::Review => review(app, p, w, body_h, vcx),
            Tab::Terminal => {
                let term_h = if expanded { body_h - 110.0 } else { body_h };
                div()
                    .pl(16.0)
                    .pt(4.0)
                    .child(app.terminal.view(w - 16.0, term_h.max(0.0), vcx))
                    .into_any()
            }
            Tab::File => file_viewer(app, p, w, body_h),
        }
    };
    pane = pane.child(
        div()
            .absolute()
            .left(0.0)
            .top(body_top)
            .w(w)
            .h(body_h)
            .child(body),
    );
    if expanded {
        let cw = 738.0_f32.min(w - 40.0);
        let composer = composer::compact(app, p, cw, vcx);
        pane = pane.child(
            div()
                .absolute()
                .left((w - cw) / 2.0)
                .top(h - 103.0)
                .child(composer),
        );
    }
    pane.into_any()
}

fn tab_strip(app: &Codex, p: &Pal, w: f32, left: f32) -> Div {
    let mut strip = hrow()
        .absolute()
        .left(0.0)
        .top(9.0)
        .w(w)
        .h(28.0)
        .pl(left)
        .pr(8.0)
        .gap(4.0);
    for tab in &app.tabs {
        let (icon, label) = match tab {
            Tab::Review => (icons::REVIEW, "Review"),
            Tab::Terminal => (icons::TERMINAL, crate::data::DEMO_PROJECT),
            Tab::File => (icons::DOC, app.open_file.unwrap_or("cart.js")),
        };
        let active = *tab == app.tab;
        let mut t = hrow()
            .h(28.0)
            .px(10.0)
            .gap(8.0)
            .rounded(8.0)
            .hover_bg(p.panel_tile)
            .accessibility_role(Role::Tab)
            .accessibility_label(label.to_owned())
            .accessibility_selected(active)
            .on_click(Msg::ShowTab(*tab));
        if active {
            t = t.bg(p.panel_tile);
        }
        let color = if active { p.text } else { p.muted };
        t = if *tab == Tab::File {
            t.child(js_badge())
        } else {
            t.child(ico(icon, 15.0, color))
        };
        strip = strip.child(t.child(txt(label, SMALL, color)));
    }
    strip = strip
        .child(
            icon_button(
                p,
                icons::PLUS,
                28.0,
                16.0,
                p.icon,
                "Open side panel tab",
                Msg::Open(Menu::PanelTab),
            )
            .id("panel.newtab"),
        )
        .child(div().flex_1());
    let expand = icon_button(
        p,
        if app.panel_expanded {
            icons::COLLAPSE
        } else {
            icons::EXPAND
        },
        28.0,
        15.0,
        p.icon,
        "Expand panel",
        Msg::ExpandPanel,
    );
    strip
        .child(if app.panel_expanded {
            expand.bg(p.panel_tile)
        } else {
            expand
        })
        .child(div().w(4.0))
        .child(
            icon_button(
                p,
                icons::PANEL_RIGHT,
                28.0,
                17.0,
                p.text,
                "Toggle side panel",
                Msg::ToggleSidePanel,
            )
            .bg(p.panel_tile),
        )
}

fn launcher(p: &Pal, w: f32, h: f32) -> Div {
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
        (icons::GLOBE, "Browser", Some("⌘T"), Msg::Noop),
        (icons::FILES, "Files", Some("⌘P"), Msg::OpenFile("cart.js")),
        (icons::SIDE_CHAT, "Side chat", Some("⌥⌘S"), Msg::Noop),
    ];
    let mut list = div().flex_col().gap(4.0).w(w - 56.0);
    for (icon, label, keys, msg) in entries {
        let mut row = hrow()
            .h(40.0)
            .px(10.0)
            .gap(10.0)
            .rounded(6.0)
            .bg(p.tray)
            .hover_bg(p.menu_hi)
            .accessibility_role(Role::Button)
            .accessibility_label(label)
            .on_click(msg)
            .child(ico(icon, 15.0, p.text_soft))
            .child(txt(label, SMALL, p.text_soft))
            .child(div().flex_1());
        if let Some(keys) = keys {
            row = row.child(kbd(p, keys));
        }
        list = list.child(row);
    }
    div().w(w).h(h).items_center().justify_center().child(list)
}

/// The cart.js diff, unified or split.
fn review(app: &Codex, p: &Pal, w: f32, h: f32, _vcx: &mut ViewContext) -> AnyElement {
    if app.review_loading {
        return div()
            .w(w)
            .h(h)
            .items_center()
            .justify_center()
            .child(ico(icons::CODEX_MARK, 48.0, p.sidebar_text))
            .into_any();
    }
    let expanded = app.panel_expanded;
    let tool = |svg: &'static str, label: &str, msg: Msg, on: bool| {
        let b = icon_button(p, svg, 29.0, 16.0, p.icon, label, msg);
        if on {
            b.bg(p.panel_tile).rounded(15.0)
        } else {
            b
        }
    };
    let mut toolbar = hrow().w(w).h(29.0).pl(9.0).pr(8.0).gap(2.0).child(
        hrow()
            .h(29.0)
            .px(8.0)
            .gap(6.0)
            .rounded(8.0)
            .hover_bg(p.panel_tile)
            .id("review.scope")
            .accessibility_role(Role::Button)
            .accessibility_label(app.scope.label())
            .on_click(Msg::Open(Menu::ReviewScope))
            .child(txt(app.scope.label(), BODY, p.text))
            .when(app.split || expanded, |d| {
                d.child(div().px(6.0).rounded(5.0).bg(p.panel_tile).child(txt(
                    "1",
                    SMALL,
                    p.text_soft,
                )))
                .child(ico(icons::CHEVRON_DOWN, 14.0, p.text))
            }),
    );
    if app.split || expanded {
        toolbar = toolbar
            .child(div().w(8.0))
            .child(txt("+2", BODY, p.add_num))
            .child(div().w(4.0))
            .child(txt("-2", BODY, p.del_num));
    }
    toolbar = toolbar
        .child(div().flex_1())
        .child(tool(icons::ELLIPSIS, "Review options", Msg::Noop, false))
        .child(tool(
            icons::LIST_FILTER,
            "Collapse all diffs",
            Msg::Noop,
            false,
        ))
        .child(tool(icons::FILE_SEARCH, "Jump to file", Msg::Noop, false))
        .child(
            div()
                .w(29.0)
                .h(29.0)
                .items_center()
                .justify_center()
                .rounded(7.0)
                .hover_bg(p.panel_tile)
                .accessibility_role(Role::Button)
                .accessibility_label(if app.split {
                    "Switch to unified diff"
                } else {
                    "Switch to split diff"
                })
                .on_click(Msg::ToggleSplit)
                .child(split_glyph(p)),
        )
        .child(tool(
            icons::FILES,
            "Show files",
            Msg::ToggleFileList,
            app.file_list,
        ))
        .child(tool(icons::COMMIT, "Commit or push", Msg::Noop, true))
        .child(tool(icons::PR, "Create PR", Msg::Noop, false).opacity(0.45));

    let list_w = if app.file_list { 196.0 } else { 0.0 };
    let diff_w = w - list_w;
    let file_header = hrow()
        .w(diff_w)
        .h(32.0)
        .pl(16.0)
        .pr(12.0)
        .gap(9.0)
        .bg(p.file_header)
        .accessibility_role(Role::Heading)
        .accessibility_label("cart.js")
        .child(js_badge())
        .child(txt("cart.js", BODY, p.text_soft))
        .child(div().flex_1())
        .child(txt("+2", BODY, p.add_num))
        .child(txt("-2", BODY, p.del_num))
        .child(div().w(4.0))
        .child(icon_button(
            p,
            icons::UNDO,
            28.0,
            14.0,
            p.icon,
            "Revert file",
            Msg::Noop,
        ))
        .child(icon_button(
            p,
            icons::PLUS,
            28.0,
            14.0,
            p.icon,
            "Stage file",
            Msg::Noop,
        ))
        .child(icon_button(
            p,
            icons::OPEN_EXTERNAL,
            28.0,
            14.0,
            p.icon,
            "Open in",
            Msg::OpenFile("cart.js"),
        ));
    let diff = if app.split {
        split_diff(p, diff_w)
    } else {
        unified_diff(p, diff_w)
    };
    let mut body = hrow().w(w).items_start().child(
        div()
            .w(diff_w)
            .flex_col()
            .child(file_header)
            .child(div().h(5.0))
            .child(diff)
            .overflow_hidden(),
    );
    if app.file_list {
        body = body.child(file_tree(p, list_w, h, Some("cart.js"), true));
    }
    let pill_y = if expanded { h - 181.0 } else { h - 48.0 };
    div()
        .w(w)
        .h(h)
        .relative()
        .child(div().h(5.0))
        .child(toolbar)
        .child(div().h(6.0))
        .child(body)
        .child(
            hrow()
                .absolute()
                .left(((w - list_w) - 214.0) / 2.0)
                .top(pill_y)
                .h(32.0)
                .px(16.0)
                .gap(22.0)
                .rounded(16.0)
                .bg(p.float_pill)
                .border(p.hairline)
                .child(
                    hrow()
                        .gap(7.0)
                        .child(ico(icons::UNDO, 14.0, p.float_text))
                        .child(txt("Revert all", SMALL, p.float_text)),
                )
                .child(
                    hrow()
                        .gap(7.0)
                        .child(ico(icons::PLUS, 14.0, p.float_text))
                        .child(txt("Stage all", SMALL, p.float_text)),
                ),
        )
        .flex_col()
        .into_any()
}

/// The split-diff toolbar glyph: a red and a green half.
fn split_glyph(p: &Pal) -> Div {
    div()
        .w(14.0)
        .h(12.0)
        .rounded(3.0)
        .overflow_hidden()
        .flex_col()
        .border(p.icon)
        .child(div().w_full().h(5.0).bg(p.del_bar))
        .child(div().w_full().h(5.0).bg(p.add_bar))
}

/// One diff side's gutter and code.
fn diff_cell(p: &Pal, w: f32, num: Option<u32>, kind: i8, code: &str, bar: bool) -> Div {
    let (gutter_bg, code_bg, num_color, bar_color) = match kind {
        -1 => (Some(p.del_gutter), Some(p.del_code), p.del_num, p.del_bar),
        1 => (Some(p.add_gutter), Some(p.add_code), p.add_num, p.add_bar),
        _ => (None, None, p.line_num, Color::TRANSPARENT),
    };
    let mut gutter = hrow().w(GUTTER).h(ROW_H).justify_end().pr(8.0).relative();
    if let Some(bg) = gutter_bg {
        gutter = gutter.bg(bg);
    }
    if bar && kind != 0 {
        gutter = gutter.child(
            div()
                .absolute()
                .left(0.0)
                .top(0.0)
                .w(3.0)
                .h(ROW_H)
                .bg(bar_color),
        );
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
        .pl(14.0)
        .overflow_hidden();
    if let Some(bg) = code_bg {
        code_box = code_box.bg(bg);
    }
    hrow()
        .w(w)
        .h(ROW_H)
        .child(gutter)
        .child(code_box.child(highlight(code, p)))
}

fn unified_diff(p: &Pal, w: f32) -> Div {
    let mut col = div().flex_col().w(w);
    for line in data::cart_diff() {
        let row = match line {
            DiffLine::Context { new, .. } => {
                diff_cell(p, w, Some(new), 0, data::line_of(data::CART_JS, new), true)
            }
            DiffLine::Del { old } => diff_cell(
                p,
                w,
                Some(old),
                -1,
                data::line_of(data::CART_JS_OLD, old),
                true,
            ),
            DiffLine::Add { new } => {
                diff_cell(p, w, Some(new), 1, data::line_of(data::CART_JS, new), true)
            }
        };
        col = col.child(row);
    }
    col
}

fn split_diff(p: &Pal, w: f32) -> Div {
    let half = (w / 2.0).floor();
    let mut col = div().flex_col().w(w);
    let lines = data::cart_diff();
    let mut i = 0;
    while i < lines.len() {
        let (left, right) = match lines[i] {
            DiffLine::Context { old, new } => {
                i += 1;
                (
                    diff_cell(
                        p,
                        half,
                        Some(old),
                        0,
                        data::line_of(data::CART_JS_OLD, old),
                        true,
                    ),
                    diff_cell(
                        p,
                        half,
                        Some(new),
                        0,
                        data::line_of(data::CART_JS, new),
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
                    Some(n) => {
                        diff_cell(p, half, Some(n), 1, data::line_of(data::CART_JS, n), true)
                    }
                    None => diff_cell(p, half, None, 0, "", false),
                };
                (
                    diff_cell(
                        p,
                        half,
                        Some(old),
                        -1,
                        data::line_of(data::CART_JS_OLD, old),
                        true,
                    ),
                    right,
                )
            }
            DiffLine::Add { new } => {
                i += 1;
                (
                    diff_cell(p, half, None, 0, "", false),
                    diff_cell(
                        p,
                        half,
                        Some(new),
                        1,
                        data::line_of(data::CART_JS, new),
                        true,
                    ),
                )
            }
        };
        col = col.child(hrow().w(w).child(left).child(right));
    }
    col
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
