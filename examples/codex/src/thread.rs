//! A thread: the transcript in the main card and the composer (or, while a
//! command waits for approval, the approval card) pinned under it. The
//! title and chat actions live in the title bar (see `lib.rs`).
//!
//! Agent turns follow the update captures (u07 to u46): the "Working for"
//! / "Worked for" divider that folds the work, preamble prose (its
//! unsettled tail dim while streaming), grouped and nested tool rows, the
//! Shell card, the inline diff card with word highlights, the final
//! answer with its action row, and the file change card.

use accesskit::Role;
use quark::view;
use quark_app::ViewContext;
use quark_app::quark_ui::element::*;
use quark_app::quark_ui::style::Styled;
use quark_app::quark_ui::theme::{Color, ThemeMode};

use crate::data::{self, Block, Glyph, Item, Row, Shell, Span, Step, ThreadId};
use crate::theme::{BODY, CODE, Pal, SMALL};
use crate::widgets::*;
use crate::{Codex, Menu, Msg, composer, icons, panel};

/// Turn column's widest on wide windows.
pub const MAX_COLUMN: f32 = 768.0;
const LINE: f32 = 1.64;

pub fn view(
    app: &mut Codex,
    id: ThreadId,
    p: &Pal,
    (x, w, h): (f32, f32, f32),
    vcx: &mut ViewContext,
) -> AnyElement {
    let Some(thread) = app.data.thread(id) else {
        return view! { <div /> };
    };
    let items = thread.items.clone();
    let pill = thread.pill;
    if app.full_view {
        // The Changes tab fills the window; the thread waits in its tab.
        return view! { <div class="absolute top-0" left={x} w={w} h={h} /> };
    }
    let column = (w - 32.0).clamp(0.0, MAX_COLUMN);
    let cx = ((w - column) / 2.0).round();
    let (bottom, bottom_h) = match app.pending.clone() {
        Some(approval) => approval_card(p, &approval, column),
        None => composer::card(app, p, column, "Work with Codex", vcx),
    };
    let bottom_top = h - 16.0 - bottom_h;
    let list_h = (bottom_top - 4.0).max(0.0);
    // Room under the turns so the latest one can scroll to the top, as
    // the app keeps a new turn's prompt at the top of the view. Legacy
    // scenes pinned to an offset follow 26.623, which kept the end in view.
    let spacer = if app.scroll_px.is_some() {
        0.0
    } else {
        (list_h - 160.0).max(0.0)
    };
    if let Some(dy) = app.take_adjust() {
        let (_, y) = app.thread_scroll.offset();
        app.thread_scroll.set_offset(0.0, (y + dy).max(0.0));
    } else if app.adjust_pending() {
        vcx.frame.request_frame();
    }
    if app.stick_bottom {
        // Pending requests: the handle resolves them once laid out.
        match app.scroll_px {
            Some(y) => app.thread_scroll.set_offset(0.0, y),
            None => {
                let last = items
                    .iter()
                    .rposition(|i| matches!(i, Item::User { .. }))
                    .unwrap_or(0);
                app.thread_scroll
                    .scroll_to_item(&format!("item-{last}"), ScrollAlign::Start);
                if let Some(dy) = app.scroll_adjust {
                    app.defer_adjust(dy);
                    vcx.frame.request_frame();
                }
            }
        }
        app.stick_bottom = false;
    }
    view! {
        <div class="absolute top-0" left={x} w={w} h={h}>
            <div class="absolute left-0 top-0 flex-col items-center overflow-y-scroll" w={w}
                 h={list_h} track_scroll={&app.thread_scroll} scrollbar_auto_hide
                 accessibility_role={Role::Log} aria-label="Transcript">
                <transcript(&items, p, column, app.now_ms)>
                    <div h={spacer} />
                </transcript>
            </div>
            <div class="absolute" left={cx} top={bottom_top}>{bottom}</div>
            if let Some((adds, dels)) = pill {
                <div class="absolute left-0 flex-row justify-center" top={bottom_top - 42.0} w={w}>
                    <div class="flex-row items-center h-8 px-[14] gap-[5] rounded-[10]" bg={p.shell}
                         border={p.shell_border} role="button" aria-label="View changes"
                         on:click={Msg::OpenTab(crate::Tab::Changes)}>
                        <txt("1 file changed", BODY, p.text_soft) />
                        <txt(format!("+{adds}"), BODY, p.success) />
                        <txt(format!("-{dels}"), BODY, p.error) />
                    </div>
                </div>
            }
        </div>
    }
}

/// The turns, top to bottom, in a `column`-wide column.
pub fn transcript(items: &[Item], p: &Pal, column: f32, now_ms: u64) -> Div {
    view! { -> Div,
        <div class="flex-col shrink-0 pt-2" w={column}>
            for (i, item) in items.iter().enumerate() {
                let key = format!("item-{i}");
                match item {
                    Item::User { text: body, .. } => {
                        <div class="w-full flex-col items-end pt-[33]" key={key}>
                            <div class="px-4 py-[9] rounded-[16]" max_w={(column * 0.7).floor()}
                                 bg={p.bubble} accessibility_role={Role::Group}
                                 aria-label="You said:">
                                <text size={BODY} color={p.bubble_text} line_height={LINE}>
                                    {body.clone()}
                                </text>
                            </div>
                            <div class="h-12" />
                        </div>
                    }
                    Item::Starting => {
                        <div class="w-full h-10" key={key}>
                            <txt("Starting your task", BODY, p.faint) />
                        </div>
                    }
                    Item::Thinking => {
                        <div class="w-full h-10" key={key}>{shimmer("Thinking", p, now_ms, BODY)}</div>
                    }
                    Item::Error(msg) => {
                        <div class="w-full flex-col" key={key}>
                            <div class="flex-row items-center w-full px-[13] py-[10] gap-3 rounded-[11]"
                                 bg={p.notice} border={p.notice_border} role="alert">
                                <div class="self-start pt-0.5">
                                    <icon svg={icons::ALERT} size={17.0} color={p.text} />
                                </div>
                                <div class="flex-1 min-w-0">
                                    <text size={BODY} color={p.text} line_height={1.5}>{msg.clone()}</text>
                                </div>
                            </div>
                            <div class="h-5" />
                        </div>
                    }
                    Item::ModelChanged { from, to } => {
                        <div class="w-full flex-col" key={key}>
                            <div class="flex-row items-center w-full h-[22] gap-2">
                                <div class="flex-1 h-px" bg={p.hairline} />
                                <icon svg={icons::CUBE} size={14.0} color={p.divider_text} />
                                <txt(format!("Model changed from {from} to {to}."), BODY, p.divider_text) />
                                <icon svg={icons::INFO} size={12.0} color={p.divider_text} />
                                <div class="flex-1 h-px" bg={p.hairline} />
                            </div>
                            <div class="h-[23]" />
                        </div>
                    }
                    Item::Work {
                        took,
                        running,
                        open,
                        steps,
                    } => {
                        <work(p, i, took, *running, *open, steps, column, now_ms) key={key} />
                    }
                    Item::Answer { blocks, .. } => {
                        <answer(p, blocks, column) key={key} />
                    }
                    Item::FileChange { file, adds, dels } => {
                        <file_change(p, file, *adds, *dels) key={key} />
                    }
                }
            }
        </div>
    }
}

/// Text with a light band sweeping across it: each run's color is mixed
/// toward the highlight by its distance from the band. quark has no
/// gradient text fill, so the label is split into runs of equal tone.
pub fn shimmer(label: &str, p: &Pal, now_ms: u64, size: f32) -> AnyElement {
    let base = p.faint;
    let high = p.text;
    let n = label.chars().count() as f32;
    // The band crosses the text and a gap as wide again every 1.6 s.
    let period = 1600.0 + n * 20.0;
    let phase = (now_ms as f32 % period) / period;
    let center = phase * (n * 1.6) - n * 0.3;
    let tone = |d: f32| base.lerp(high, (1.0 - d) * 0.85);
    let mut runs = Vec::new();
    let mut run = String::new();
    let mut run_d = 1.0f32;
    for (i, ch) in label.chars().enumerate() {
        let d = ((i as f32 - center).abs() / 3.0).min(1.0);
        if (d - run_d).abs() > 0.05 && !run.is_empty() {
            runs.push((std::mem::take(&mut run), tone(run_d)));
        }
        if run.is_empty() {
            run_d = d;
        }
        run.push(ch);
    }
    if !run.is_empty() {
        runs.push((run, tone(run_d)));
    }
    view! {
        <div class="flex-row items-center" role="status" aria-label={label.to_owned()}>
            for (run, color) in runs {
                <txt(run.replace(' ', "\u{a0}"), size, color) />
            }
        </div>
    }
}

fn glyph(g: Glyph) -> &'static str {
    match g {
        Glyph::Terminal => icons::TERMINAL,
        Glyph::Book => icons::BOOK,
        Glyph::Pencil => icons::PENCIL,
    }
}

fn chevron(open: bool) -> &'static str {
    if open {
        icons::CHEVRON_DOWN
    } else {
        icons::CHEVRON_RIGHT
    }
}

#[allow(clippy::too_many_arguments)]
fn work(
    p: &Pal,
    index: usize,
    took: &str,
    running: bool,
    open: bool,
    steps: &[Step],
    column: f32,
    now_ms: u64,
) -> Div {
    let label = if running {
        format!("Working for {took}")
    } else {
        format!("Worked for {took}")
    };
    view! { -> Div,
        <div class="w-full flex-col">
            <div class="flex-row items-center h-[22] gap-1.5" role="button"
                 aria-label={label.clone()} aria-expanded={open || running}
                 on:click={Msg::ToggleWork(index)}>
                <txt(label, BODY, p.muted) />
                if !running {
                    <icon svg={chevron(open)} size={13.0} color={p.muted} />
                }
            </div>
            <div class="h-[3]" />
            <div class="w-full h-px" bg={p.hairline} />
            <div class="h-[17]" />
            if open || running {
                for (s, step) in steps.iter().enumerate() {
                    match step {
                        Step::Prose { text, pending } => {
                            <div class="w-full flex-col">
                                {inline_flow(&parse_spans(text), p, p.text_soft, Some(pending))}
                                <div class="h-3.5" />
                            </div>
                        }
                        Step::Group {
                            label,
                            glyph: g,
                            open,
                            rows,
                        } => {
                            <div class="w-full flex-col">
                                <div class="flex-row items-center h-[22] gap-2" role="button"
                                     aria-label={(*label).to_owned()} aria-expanded={*open}
                                     on:click={Msg::ToggleGroup(index, s)}>
                                    <icon svg={glyph(*g)} size={14.0} color={p.muted} />
                                    <txt(*label, BODY, p.muted) />
                                    <icon svg={chevron(*open)} size={12.0} color={p.muted} />
                                </div>
                                if *open {
                                    for (r, row) in rows.iter().enumerate() {
                                        <div class="h-[3]" />
                                        {tool_row(p, row, index, s, r, column)}
                                    }
                                }
                                <div class="h-3.5" />
                            </div>
                        }
                        Step::Row(row) => {
                            <div class="w-full flex-col">
                                {tool_row(p, row, index, s, 0, column)}
                                <div class="h-3.5" />
                            </div>
                        }
                        Step::Live { glyph: g, text } => {
                            <div class="w-full flex-col">
                                <div class="flex-row items-center w-full h-[22] gap-2 overflow-hidden">
                                    <icon svg={glyph(*g)} size={14.0} color={p.text_soft} />
                                    {shimmer(text, p, now_ms, BODY)}
                                </div>
                            </div>
                        }
                    }
                }
            }
            <div class="h-1" />
        </div>
    }
}

/// A step's one-line header: glyph, then the caller's children.
fn tool_line(p: &Pal, glyph: &'static str) -> Div {
    view! { -> Div,
        <div class="flex-row items-center w-full h-[22] gap-2 overflow-hidden">
            <icon svg={glyph} size={14.0} color={p.muted} />
        </div>
    }
}

/// A finished step: "Ran ...", "Read <file>", "Edited <file> +2 -2", and
/// its Shell card or diff when expanded.
fn tool_row(p: &Pal, row: &Row, item: usize, step: usize, index: usize, column: f32) -> AnyElement {
    let ink = p.text_soft.lerp(p.muted, 0.35);
    match row {
        Row::Ran {
            command,
            took,
            shell,
            open,
        } => {
            let mut label = format!("Ran {command}");
            if let Some(took) = took {
                label.push_str(&format!(" in {took}"));
            }
            view! {
                <div class="w-full flex-col">
                    <tool_line(p, icons::TERMINAL) role="button" aria-label={label.clone()}
                               on:click={Msg::ToggleRow(item, step, index)}>
                        <div class="flex-1 min-w-0 overflow-hidden">
                            <txt(label, BODY, ink) class="truncate" />
                        </div>
                        if shell.is_some() {
                            <icon svg={chevron(*open)} size={12.0} color={p.muted} />
                        }
                    </tool_line>
                    if let (Some(shell), true) = (shell, *open) {
                        <div class="h-1.5" />
                        {shell_card(p, shell)}
                    }
                </div>
            }
        }
        Row::Read(file) => view! {
            <tool_line(p, icons::BOOK)>
                <txt("Read", BODY, ink) />
                <div class="relative">
                    <txt(*file, BODY, p.muted) />
                    {dotted_under(p)}
                </div>
            </tool_line>
        },
        Row::Edited {
            file,
            adds,
            dels,
            open,
        } => view! {
            <div class="w-full flex-col">
                <tool_line(p, icons::PENCIL) role="button"
                           aria-label={format!("Toggle diff for {file}")}
                           on:click={Msg::ToggleRow(item, step, index)}>
                    if *open {
                        <txt("Edited file", BODY, ink) />
                        <div />
                        <div />
                    } else {
                        <txt(format!("Edited {file}"), BODY, ink) />
                        <txt(format!("+{adds}"), BODY, p.success) />
                        <txt(format!("-{dels}"), BODY, p.error) />
                    }
                    <icon svg={chevron(*open)} size={12.0} color={p.muted} />
                </tool_line>
                if *open {
                    <div class="h-1.5" />
                    {diff_card(p, file, *adds, *dels, column)}
                }
            </div>
        },
    }
}

fn dotted_under(p: &Pal) -> AnyElement {
    view! {
        <div class="flex-row items-center absolute left-0 right-0 top-[18] h-px gap-0.5
                    overflow-hidden">
            for _ in 0..40 {
                <div class="w-px h-px shrink-0" bg={p.muted} />
            }
        </div>
    }
}

fn shell_card(p: &Pal, shell: &Shell) -> AnyElement {
    view! {
        <div class="w-full flex-col rounded-[8] overflow-hidden relative" bg={p.shell}
             border={p.shell_border} accessibility_role={Role::Group}
             aria-label={format!("Shell: {}", shell.command)}
             h={if shell.footer.is_some() { 128.0 } else { 100.0 }}>
            <div class="flex-row items-center h-7 px-2">
                <txt("Shell", SMALL, p.text_soft) />
            </div>
            <div class="flex-row items-center h-[26] px-2 gap-2">
                <text size={CODE + 1.0} color={p.muted} class="font-mono whitespace-nowrap">"$"</text>
                <text size={CODE + 1.0} color={p.text} class="font-mono whitespace-nowrap">
                    {shell.command}
                </text>
            </div>
            <div class="w-full flex-col px-[10] gap-[3] h-11 overflow-hidden">
                for line in shell.output.lines() {
                    <text size={CODE} color={p.faint} class="font-mono whitespace-nowrap">
                        {line.replace(' ', "\u{a0}")}
                    </text>
                }
            </div>
            if let Some(footer) = shell.footer {
                <div class="flex-row items-center h-7 px-[10] justify-end">
                    <txt(footer, SMALL, p.muted) />
                </div>
            } else {
                // Output past the card's height fades out.
                <div class="absolute left-0 right-0 top-[70] h-[30]"
                     bg_effect={linear_gradient(
                         std::f32::consts::FRAC_PI_2,
                         p.shell.with_alpha(0),
                         p.shell,
                     )} />
            }
        </div>
    }
}

/// The inline diff of an edit row: header, two hunks with a separator,
/// word highlights on the added text.
fn diff_card(p: &Pal, file: &str, adds: u32, dels: u32, column: f32) -> AnyElement {
    use data::DiffLine::*;
    view! {
        <div class="w-full flex-col rounded-[8] overflow-hidden" bg={p.change_card}
             border={p.shell_border} accessibility_role={Role::Group}
             aria-label={format!("{file} diff")}>
            <div class="flex-row items-center h-7 px-[10] gap-1.5" bg={p.shell}>
                <txt(file.to_owned(), SMALL, p.text_soft) />
                <txt(format!("+{adds}"), SMALL, p.success) />
                <txt(format!("-{dels}"), SMALL, p.error) />
                <div class="flex-1" />
                <icon svg={icons::COPY} size={13.0} color={p.muted} />
            </div>
            <div class="w-full flex-col">
                for line in data::cart_diff(false) {
                    match line {
                        Context { old: 1, .. } | Context { old: 8, .. } => {}
                        Context { old: 5, .. } => <div class="w-full h-1" bg={p.fold} />
                        _ => panel::diff_row(p, column, line, true),
                    }
                }
            </div>
        </div>
    }
}

/// Parse `code` and **bold** markers into spans.
pub fn parse_spans(s: &'static str) -> Vec<Span> {
    let mut out = Vec::new();
    let mut rest = s;
    while !rest.is_empty() {
        let next = rest.find(['`', '*']).unwrap_or(rest.len());
        if next > 0 {
            out.push(Span::Text(&rest[..next]));
            rest = &rest[next..];
            continue;
        }
        if let Some(body) = rest.strip_prefix("**")
            && let Some(end) = body.find("**")
        {
            out.push(Span::Bold(&body[..end]));
            rest = &body[end + 2..];
        } else if let Some(body) = rest.strip_prefix('`')
            && let Some(end) = body.find('`')
        {
            out.push(Span::Code(&body[..end]));
            rest = &body[end + 1..];
        } else {
            out.push(Span::Text(&rest[..1]));
            rest = &rest[1..];
        }
    }
    out
}

pub fn parse_inline(s: &'static str) -> Block {
    Block::Para(parse_spans(s))
}

/// Spans flowed word by word so chips and links wrap with the prose
/// (quark's text element takes one style per run). `pending`, while a
/// preamble streams, follows in a dimmer tone.
fn inline_flow(spans: &[Span], p: &Pal, color: Color, pending: Option<&str>) -> AnyElement {
    let word = |w: &str, c: Color, bold: bool| {
        view! {
            <text size={BODY} color={c} class="whitespace-nowrap" line_height={LINE}
                  @when {bold} { class="font-semibold" }>
                {w.replace(' ', "\u{a0}")}
            </text>
        }
    };
    view! {
        <div class="w-full flex-row flex-wrap items-center">
            for span in spans {
                match span {
                    Span::Text(t) | Span::Bold(t) => {
                        let bold = matches!(span, Span::Bold(_));
                        for w in t.split_inclusive(' ') {
                            {word(w, if bold { p.text } else { color }, bold)}
                        }
                    }
                    Span::Code(c) => {
                        <div class="flex-row items-center px-[5] h-5 rounded-[5]" bg={p.chip}>
                            <text size={CODE + 0.5} color={p.text} class="font-mono whitespace-nowrap">
                                {*c}
                            </text>
                        </div>
                    }
                    Span::File(name, line) => {
                        <div class="flex-row items-center gap-[5]" role="link"
                             aria-label={(*name).to_owned()} on:click={Msg::OpenFile("cart.js")}>
                            <div class="w-[13] h-[13] rounded-[3] bg-[#2f6fd8] items-center justify-center">
                                <text size={6.5} class="font-bold text-white whitespace-nowrap">"js"</text>
                            </div>
                            {word(name, p.link, false)}
                            if let Some(n) = line {
                                {word(&format!(" (line {n})"), p.link, false)}
                            }
                        </div>
                    }
                }
            }
            if let Some(pending) = pending.filter(|s| !s.is_empty()) {
                for w in pending.split_inclusive(' ') {
                    {word(w, p.faint.lerp(p.bg, 0.3), false)}
                }
            }
        </div>
    }
}

fn answer(p: &Pal, blocks: &[Block], column: f32) -> Div {
    view! { -> Div,
        <div class="w-full flex-col">
            for block in blocks {
                match block {
                    Block::Para(spans) => {
                        {inline_flow(spans, p, p.text_soft, None)}
                        <div class="h-2" />
                    }
                    Block::Bullets(items) => {
                        for spans in items {
                            <div class="flex-row items-center w-full items-start">
                                <div class="w-7 pl-[5]">
                                    <text size={BODY} color={p.text_soft} line_height={LINE}>"•"</text>
                                </div>
                                <div w={column - 28.0}>{inline_flow(spans, p, p.text_soft, None)}</div>
                            </div>
                        }
                        <div class="h-2" />
                    }
                }
            }
            <div class="flex-row items-center gap-0.5">
                <icon_button(p, icons::COPY, 26.0, 14.0, p.muted, "Copy", Msg::Noop) />
                <icon_button(p, icons::READ_ALOUD, 26.0, 14.0, p.muted, "Read aloud", Msg::Noop) />
                <icon_button(p, icons::RATE, 26.0, 14.0, p.muted, "Rate response", Msg::Noop) />
                <icon_button(p, icons::FORK_CHAT, 26.0, 14.0, p.muted, "Fork chat from here",
                             Msg::Noop) />
            </div>
            <div class="h-[15]" />
        </div>
    }
}

fn file_change(p: &Pal, file: &str, adds: u32, dels: u32) -> Div {
    let light = p.mode == ThemeMode::Light;
    view! { -> Div,
        <div class="w-full flex-col">
            <div class="flex-row items-center w-full h-16 px-3 gap-3 rounded-[10]" bg={p.change_card}
                 border={p.change_border} accessibility_role={Role::Group}
                 aria-label={format!("Edited {file}")}>
                <div class="w-10 h-10 rounded-[8] items-center justify-center" bg={p.tile}>
                    <icon svg={icons::REVIEW} size={17.0} color={p.text_soft} />
                </div>
                <div class="flex-col gap-0.5">
                    <txt(format!("Edited {file}"), BODY, p.text) />
                    <div class="flex-row items-center gap-[5]">
                        <txt(format!("+{adds}"), SMALL, p.success) />
                        <txt(format!("-{dels}"), SMALL, p.error) />
                    </div>
                </div>
                <div class="flex-1" />
                <div class="flex-row items-center h-7 px-2 gap-1.5" role="button" aria-label="Undo">
                    <txt("Undo", SMALL, p.text_soft) />
                    <icon svg={icons::UNDO} size={13.0} color={p.text_soft} />
                </div>
                <div class="flex-row items-center h-7 px-[10] rounded-[8]"
                     border={if light { p.change_border } else { p.shell_border }} role="button"
                     aria-label="View changes" on:click={Msg::OpenTab(crate::Tab::Changes)}>
                    <txt("View changes", SMALL, p.text_soft) />
                </div>
            </div>
            <div class="h-4" />
        </div>
    }
}

/// The approval card that replaces the composer while a command waits:
/// category, the model's question, the command, Deny and Allow once.
fn approval_card(p: &Pal, a: &data::Approval, w: f32) -> (AnyElement, f32) {
    let h = 183.0;
    let card = view! {
        <div w={w} h={h} class="flex-col px-4 pt-[14] gap-[10] rounded-[20]"
             bg={p.composer.lerp(p.bg, 0.15)} border={p.composer_border}
             accessibility_role={Role::AlertDialog} aria-label={a.question}>
            <div class="flex-row items-center gap-2">
                <icon svg={icons::TERMINAL} size={15.0} color={p.muted} />
                <txt(a.category, SMALL, p.muted) />
            </div>
            <text size={BODY} color={p.text} class="font-medium" line_height={1.5}
                  wrap_width={w - 32.0}>{a.question}</text>
            <div margin_left={-4.0}>
                <div class="flex-row items-center h-9 px-[9] rounded-[8]" w={w - 24.0} bg={p.tray}>
                    <text size={CODE + 0.5} color={p.muted} class="font-mono whitespace-nowrap">
                        {a.command}
                    </text>
                </div>
            </div>
            <div class="flex-row items-center gap-2">
                <div class="flex-1" />
                <div class="flex-row items-center h-7 pl-[10] pr-1 gap-1.5 rounded-[14]"
                     border={p.composer_border.lerp(p.text, 0.1)} role="button" aria-label="Deny"
                     on:click={Msg::Decide(false)}>
                    <txt("Deny", SMALL, p.text) />
                    {kbd(p, "Esc")}
                </div>
                <div class="flex-row items-center h-[30] pl-[10] pr-2 gap-2 rounded-[15]"
                     bg={p.send_active}>
                    <div class="flex-row items-center gap-1.5" role="button" aria-label="Allow once"
                         on:click={Msg::Decide(true)}>
                        <txt("Allow once", SMALL, p.send_active_glyph) />
                        <div class="px-[3] rounded-[4]" bg={p.send_active_glyph.with_alpha(30)}>
                            <icon svg={icons::RETURN_KEY} size={13.0} color={p.send_active_glyph} />
                        </div>
                    </div>
                    <div id="approval.options" role="button" aria-label="More options"
                         on:click={Msg::Open(Menu::ApprovalOptions)}>
                        <icon svg={icons::CHEVRON_DOWN} size={12.0}
                              color={p.send_active_glyph.with_alpha(160)} />
                    </div>
                </div>
            </div>
        </div>
    };
    (card, h)
}

#[cfg(test)]
mod tests {
    use super::{Span, parse_spans};

    // Catches code and bold markers leaking into the prose or swallowing
    // text when a marker is unmatched.
    #[test]
    fn markers_become_spans() {
        assert_eq!(
            parse_spans("Ran `npm test`: **both fail**."),
            vec![
                Span::Text("Ran "),
                Span::Code("npm test"),
                Span::Text(": "),
                Span::Bold("both fail"),
                Span::Text("."),
            ]
        );
        assert_eq!(
            parse_spans("2 * 3 `x"),
            vec![
                Span::Text("2 "),
                Span::Text("*"),
                Span::Text(" 3 "),
                Span::Text("`"),
                Span::Text("x")
            ]
        );
    }
}
