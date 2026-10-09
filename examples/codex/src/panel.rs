//! The side panel card: the Changes tab (scope pill, options, split diff
//! of cart.js with "1 unmodified line" folds and word highlights), the
//! Terminal tab, and the file viewer with its tree. Its tab strip lives in
//! the title bar (`tab_strip`). The full view hands it the whole window
//! and floats the composer over it (u29).

use accesskit::Role;
use quark::{Path, StrokePattern, StrokeStyle, view};
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
        <div class="absolute top-0" left={x} w={w} h={h} bg={p.bg}
             @when {!full} { border_l={p.frame_border.lerp(p.text, 0.06)} }
             accessibility_role={Role::Complementary} aria-label="Side panel">
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
            <div class="flex-row items-center h-[30] pl-[10] pr-1.5 gap-2 rounded-[8]" w={w}
                 hover_bg={p.rail_tile.with_alpha(120)} role="tab" aria-label={label.clone()}
                 aria-selected={active} on:click={msg}
                 @when {active} { bg={p.rail_tile} border={p.frame_border.lerp(p.text, 0.08)} }>
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
                    <tab(view! { <icon svg={icons::CHAT} size={14.0} color={p.muted} /> },
                         t.title.clone(), false, 230.0, Msg::FullView)>
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
                        <icon_button(p, icons::CLOSE, 20.0, 11.0, p.muted, "Close tab",
                                     Msg::CloseTab(*t)) />
                    </tab>
                }
                <div class="w-1.5" />
                <icon_button(p, icons::PLUS, 28.0, 16.0, p.icon, "New tab",
                             Msg::Open(Menu::PanelTab)) id="panel.newtab" />
            </div>
            <div class="flex-row items-center absolute top-2 gap-1.5 w-[121] justify-end"
                 left={f.w - 129.0}>
                if full {
                    <icon_button(p, icons::SUMMARY, 28.0, 16.0, p.icon, "Toggle summary",
                                 Msg::Open(Menu::Summary)) id="header.summary" />
                }
                <icon_button(p, if full { icons::COLLAPSE } else { icons::EXPAND }, 28.0, 14.0,
                             p.icon, if full { "Exit full view" } else { "Enter full view" },
                             Msg::FullView) bg={if full { p.rail_tile }} />
                <icon_button(p, icons::PANEL_RIGHT, 28.0, 16.0, p.text, "Hide tabs",
                             Msg::ToggleSidePanel) bg={p.rail_tile} />
            </div>
        </div>
    }
}

/// The Changes tab.
fn changes(app: &Codex, p: &Pal, w: f32, h: f32) -> AnyElement {
    let full = app.full_view;
    let tool = |svg: &'static str, label: &str, msg: Msg| {
        icon_button(p, svg, 28.0, 14.0, p.icon, label, msg)
    };
    view! {
        <div class="flex-col" w={w} h={h}>
            <div class="h-1" />
            <div class="flex-row items-center h-12 px-2" w={w}>
                <div class="flex-row items-center h-8 pl-[14] pr-[10] gap-1.5 rounded-[16]"
                     bg={p.tray} id="changes.scope" role="button" aria-label={SCOPES[app.scope]}
                     on:click={Msg::Open(Menu::ChangesScope)}>
                    <txt(SCOPES[app.scope], SMALL, p.text) />
                    <icon svg={icons::CHEVRON_DOWN} size={11.0} color={p.text} />
                    <div class="w-1" />
                    <txt("+2", SMALL, p.add_num) />
                    <txt("-2", SMALL, p.del_num) />
                </div>
                <div class="flex-1" />
                <div class="flex-row items-center h-8 px-1 gap-0.5 rounded-[16]" bg={p.tray}>
                    <tool(icons::ELLIPSIS, "Options", Msg::Open(Menu::ChangesOptions))
                          id="changes.options" />
                    <tool(icons::FILE_SEARCH, "Jump to file", Msg::Noop) />
                    if full {
                        <tool(icons::REFRESH_CW, "Refresh", Msg::Noop) />
                        <tool(icons::WRAP, "Word wrap", Msg::Noop) />
                        <tool(icons::LIST_FILTER, "Expand all diffs", Msg::Noop) />
                        <div class="w-7 h-7 items-center justify-center">{split_glyph(p)}</div>
                    }
                    <tool(icons::FILES, "Show files", Msg::Noop) />
                </div>
            </div>
            <div class="flex-row items-center h-9 pl-4 pr-[10] gap-[9]" w={w}
                 border_b={p.hairline} border_t={p.hairline} role="heading" aria-label="cart.js">
                {js_badge()}
                <txt("cart.js", BODY, p.text_soft) />
                <div class="flex-1" />
                <txt("+2", BODY, p.add_num) />
                <txt("-2", BODY, p.del_num) />
                <div class="w-0.5" />
                <icon_button(p, icons::OPEN_EXTERNAL, 24.0, 13.0, p.icon, "Open in",
                             Msg::OpenFile("cart.js")) />
                <icon_button(p, icons::ELLIPSIS, 24.0, 13.0, p.icon, "File options", Msg::Noop) />
            </div>
            {split_diff(p, w)}
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

/// The striped bar a deleted row carries (a dashed stroke, six segments
/// to a row); added rows get a solid one.
fn bar(p: &Pal, kind: i8) -> AnyElement {
    let color = p.del_bar;
    let dashes = move |painter: &mut CanvasPainter<'_>| {
        let (w, h) = painter.size();
        let mut line = Path::builder();
        line.move_to(w / 2.0, 0.0).line_to(w / 2.0, h);
        let dash = h / 6.0;
        painter.stroke(
            line.build(),
            color,
            StrokeStyle::new(w).pattern(StrokePattern::dashed(dash, dash)),
        );
    };
    view! {
        if kind < 0 {
            <div class="absolute left-0 top-0">
                <path_canvas(dashes) class="w-1" h={ROW_H} />
            </div>
        } else {
            <div class="absolute left-0 top-0 w-1" h={ROW_H} bg={p.add_bar} />
        }
    }
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
) -> AnyElement {
    let (bg, num_color) = match kind {
        -1 => (Some(p.del_code), p.del_num),
        1 => (Some(p.add_code), p.add_num),
        _ => (None, p.line_num),
    };
    view! {
        <div class="flex-row items-center" w={w} h={ROW_H}>
            <div class="flex-row items-center justify-end pr-[10] relative" w={GUTTER} h={ROW_H}
                 bg={if let Some(bg) = bg { bg }}>
                if with_bar && kind != 0 {
                    {bar(p, kind)}
                }
                if let Some(n) = num {
                    <text size={CODE} color={num_color} class="font-mono whitespace-nowrap">
                        {n.to_string()}
                    </text>
                }
            </div>
            <div class="flex-row items-center pl-3 overflow-hidden" w={(w - GUTTER).max(0.0)}
                 h={ROW_H} bg={if let Some(bg) = bg { bg }}>
                {highlight_words(code, words, p)}
            </div>
        </div>
    }
}

/// A unified diff row `w` wide, for the inline diff card.
pub fn diff_row(p: &Pal, w: f32, line: DiffLine, with_bar: bool) -> AnyElement {
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

fn fold_bar(p: &Pal, w: f32, n: u32) -> AnyElement {
    view! {
        <div class="flex-row items-center h-[33]" w={w}>
            <div class="h-[33]" w={GUTTER} />
            <div class="flex-row items-center flex-1 h-[33] pl-2" bg={p.fold}>
                <txt(format!("{n} unmodified line{}", if n == 1 { "" } else { "s" }), SMALL, p.muted) />
            </div>
        </div>
    }
}

fn split_diff(p: &Pal, w: f32) -> AnyElement {
    let half = (w / 2.0).floor();
    let lines = data::cart_diff(true);
    // view!: pairing each deletion with the addition after it looks ahead
    // and skips lines, which markup's `for` cannot; the cells are built
    // here and the view places the rows.
    let mut rows = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let (left, right) = match lines[i] {
            DiffLine::Fold(n) => {
                i += 1;
                (
                    fold_bar(p, half, n),
                    view! {
                        <div class="flex-row items-center h-[33] pl-1" w={half}>
                            <div class="h-[33]" w={half - 4.0} bg={p.fold} />
                        </div>
                    },
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
        rows.push((left, right));
    }
    view! {
        <div class="flex-col pt-0.5" w={w}>
            for (left, right) in rows {
                <div class="flex-row items-center" w={w}>{left} {right}</div>
            }
        </div>
    }
}

/// [`highlight`] with `words` (substrings of `code`) on a stronger tint.
fn highlight_words(code: &str, words: &[&str], p: &Pal) -> AnyElement {
    if words.is_empty() {
        return view! { <highlight(code, p) /> };
    }
    // Split the line at the highlighted words and tint those pieces.
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
    view! {
        <div class="flex-row items-center">
            for (piece, hot) in pieces {
                <highlight(piece, p) @when {hot} { bg={p.word_add} class="rounded-[3]" } />
            }
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

fn file_tree(p: &Pal, w: f32, h: f32, selected: Option<&str>, review: bool) -> AnyElement {
    let files: Vec<(&str, bool)> = if review {
        vec![("cart.js", false)]
    } else {
        data::FILES.to_vec()
    };
    view! {
        <div class="flex-col px-2 pt-2 gap-0" w={w} h={h} border_l={p.hairline}>
            <div class="flex-row items-center h-7 px-[9] gap-[7] rounded-[7]" border={p.hairline}>
                <icon svg={icons::SEARCH} size={14.0} color={p.muted} />
                <txt("Filter files...", BODY, p.muted) />
            </div>
            <div class="h-1.5" />
            for (name, dir) in files {
                let sel = Some(name) == selected;
                let name_static: &'static str = match name {
                    "cart.test.js" => "cart.test.js",
                    "package.json" => "package.json",
                    "README.md" => "README.md",
                    ".git" => ".git",
                    _ => "cart.js",
                };
                <div class="flex-row items-center h-7 px-[7] gap-[10] rounded-[6]"
                     bg={if sel { p.panel_tile }} hover_bg={p.panel_tile} role="treeitem"
                     aria-label={name.to_owned()}
                     on:click={if dir { Msg::Noop } else { Msg::OpenFile(name_static) }}>
                    <div class="w-[15] items-center">
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
                    <txt(name, SMALL, p.text_soft) />
                </div>
            }
        </div>
    }
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
    let changed = [3u32, 7];
    view! {
        <div class="flex-col" w={w} h={h}>
            <div class="flex-row items-center h-[38] pl-4 pr-2 gap-1.5" w={w} border_b={p.hairline}>
                <txt(data::DEMO_PROJECT, SMALL, p.muted) />
                <icon svg={icons::CHEVRON_RIGHT} size={13.0} color={p.muted} />
                <txt(file, SMALL, p.text) />
                <div class="flex-1" />
                <icon_button(p, icons::ELLIPSIS, 28.0, 15.0, p.icon, "More", Msg::Noop) />
                <div class="w-1.5" />
                <div class="flex-row items-center h-7 px-[9] gap-2 rounded-[8]" border={p.hairline}>
                    <icon svg={icons::APP_FINDER} size={15.0} />
                    <txt("Open", BODY, p.text) />
                    <icon svg={icons::CHEVRON_DOWN} size={13.0} color={p.muted} />
                </div>
                <div class="w-1.5" />
                <icon_button(p, icons::FILES, 28.0, 15.0, p.text, "Toggle file tree", Msg::Noop)
                             bg={p.panel_tile} />
            </div>
            <div class="flex-row items-start">
                <div class="flex-col pt-1" w={code_w}>
                    for (i, line) in content.lines().chain(std::iter::once("")).enumerate() {
                        let n = i as u32 + 1;
                        let marker = if file == "cart.js" && changed.contains(&n) {
                            Color::rgba(0xf0, 0x8a, 0x3c, 255)
                        } else {
                            Color::TRANSPARENT
                        };
                        <div class="flex-row items-center" h={ROW_H}>
                            <div class="w-1" h={ROW_H} bg={marker} />
                            <div class="flex-row items-center w-[39] justify-end">
                                <text size={CODE} color={p.line_num} class="font-mono whitespace-nowrap">
                                    {n.to_string()}
                                </text>
                            </div>
                            <div class="w-4" />
                            {highlight(line, p)}
                        </div>
                    }
                </div>
                {file_tree(p, tree_w, h - 38.0, Some(file), false)}
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
