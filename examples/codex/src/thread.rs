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
        return div().into_any();
    };
    let items = thread.items.clone();
    let pill = thread.pill;
    let mut pane = div().absolute().left(x).top(0.0).w(w).h(h);
    if app.full_view {
        // The Changes tab fills the window; the thread waits in its tab.
        return pane.into_any();
    }
    let column = (w - 32.0).clamp(0.0, MAX_COLUMN);
    let cx = ((w - column) / 2.0).round();
    let (bottom, bottom_h): (Div, f32) = match app.pending.clone() {
        Some(approval) => approval_card(p, &approval, column),
        None => composer::card(app, p, column, "Work with Codex", vcx),
    };
    let bottom_top = h - 16.0 - bottom_h;
    let list_h = (bottom_top - 4.0).max(0.0);
    // Room under the turns so the latest one can scroll to the top, as
    // the app keeps a new turn's prompt at the top of the view.
    let transcript =
        transcript(&items, p, column, app.now_ms).child(div().h((list_h - 160.0).max(0.0)));
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
            }
        }
        app.stick_bottom = false;
    }
    pane = pane
        .child(
            div()
                .absolute()
                .left(0.0)
                .top(0.0)
                .w(w)
                .h(list_h)
                .flex_col()
                .items_center()
                .overflow_y_scroll()
                .track_scroll(&app.thread_scroll)
                .scrollbar_auto_hide()
                .accessibility_role(Role::Log)
                .accessibility_label("Transcript")
                .child(transcript),
        )
        .child(div().absolute().left(cx).top(bottom_top).child(bottom));
    if let Some((adds, dels)) = pill {
        pane = pane.child(
            div()
                .absolute()
                .left(0.0)
                .top(bottom_top - 42.0)
                .w(w)
                .flex_row()
                .justify_center()
                .child(
                    hrow()
                        .h(32.0)
                        .px(14.0)
                        .gap(5.0)
                        .rounded(10.0)
                        .bg(p.shell)
                        .border(p.shell_border)
                        .accessibility_role(Role::Button)
                        .accessibility_label("View changes")
                        .on_click(Msg::OpenTab(crate::Tab::Changes))
                        .child(txt("1 file changed", BODY, p.text_soft))
                        .child(txt(format!("+{adds}"), BODY, p.success))
                        .child(txt(format!("-{dels}"), BODY, p.error)),
                ),
        );
    }
    pane.into_any()
}

/// The turns, top to bottom, in a `column`-wide column.
pub fn transcript(items: &[Item], p: &Pal, column: f32, now_ms: u64) -> Div {
    let mut col = div().w(column).flex_col().flex_shrink_0().pt(8.0);
    for (i, item) in items.iter().enumerate() {
        let el: Div =
            match item {
                Item::User { text: body, .. } => div()
                    .w_full()
                    .flex_col()
                    .items_end()
                    .pt(33.0)
                    .child(
                        div()
                            .max_w((column * 0.7).floor())
                            .px(16.0)
                            .py(9.0)
                            .bg(p.bubble)
                            .rounded(16.0)
                            .accessibility_role(Role::Group)
                            .accessibility_label("You said:")
                            .child(
                                text(body.clone())
                                    .size(BODY)
                                    .color(p.bubble_text)
                                    .line_height(LINE),
                            ),
                    )
                    .child(div().h(48.0)),
                Item::Starting => {
                    div()
                        .w_full()
                        .h(40.0)
                        .child(txt("Starting your task", BODY, p.faint))
                }
                Item::Thinking => div()
                    .w_full()
                    .h(40.0)
                    .child(shimmer("Thinking", p, now_ms, BODY)),
                Item::Error(msg) => div()
                    .w_full()
                    .flex_col()
                    .child(
                        hrow()
                            .w_full()
                            .px(13.0)
                            .py(10.0)
                            .gap(12.0)
                            .bg(p.notice)
                            .border(p.notice_border)
                            .rounded(11.0)
                            .accessibility_role(Role::Alert)
                            .child(div().self_start().pt(2.0).child(ico(
                                icons::ALERT,
                                17.0,
                                p.text,
                            )))
                            .child(div().flex_1().min_w(0.0).child(
                                text(msg.clone()).size(BODY).color(p.text).line_height(1.5),
                            )),
                    )
                    .child(div().h(20.0)),
                Item::ModelChanged { from, to } => {
                    let line = || div().flex_1().h(1.0).bg(p.hairline);
                    div()
                        .w_full()
                        .flex_col()
                        .child(
                            hrow()
                                .w_full()
                                .h(22.0)
                                .gap(8.0)
                                .child(line())
                                .child(ico(icons::CUBE, 14.0, p.divider_text))
                                .child(txt(
                                    format!("Model changed from {from} to {to}."),
                                    BODY,
                                    p.divider_text,
                                ))
                                .child(ico(icons::INFO, 12.0, p.divider_text))
                                .child(line()),
                        )
                        .child(div().h(23.0))
                }
                Item::Work {
                    took,
                    running,
                    open,
                    steps,
                } => work(p, i, took, *running, *open, steps, column, now_ms),
                Item::Answer { blocks, .. } => answer(p, blocks, column),
                Item::FileChange { file, adds, dels } => file_change(p, file, *adds, *dels),
            };
        col = col.child(el.key(format!("item-{i}")));
    }
    col
}

/// Text with a light band sweeping across it: each run's color is mixed
/// toward the highlight by its distance from the band. quark has no
/// gradient text fill, so the label is split into runs of equal tone.
pub fn shimmer(label: &str, p: &Pal, now_ms: u64, size: f32) -> Div {
    let base = p.faint;
    let high = p.text;
    let n = label.chars().count() as f32;
    // The band crosses the text and a gap as wide again every 1.6 s.
    let period = 1600.0 + n * 20.0;
    let phase = (now_ms as f32 % period) / period;
    let center = phase * (n * 1.6) - n * 0.3;
    let tone = |d: f32| base.lerp(high, (1.0 - d) * 0.85);
    let mut row = hrow()
        .accessibility_role(Role::Status)
        .accessibility_label(label.to_owned());
    let mut run = String::new();
    let mut run_d = 1.0f32;
    for (i, ch) in label.chars().enumerate() {
        let d = ((i as f32 - center).abs() / 3.0).min(1.0);
        if (d - run_d).abs() > 0.05 && !run.is_empty() {
            row = row.child(txt(
                std::mem::take(&mut run).replace(' ', "\u{a0}"),
                size,
                tone(run_d),
            ));
        }
        if run.is_empty() {
            run_d = d;
        }
        run.push(ch);
    }
    if !run.is_empty() {
        row = row.child(txt(run.replace(' ', "\u{a0}"), size, tone(run_d)));
    }
    row
}

fn glyph(g: Glyph) -> &'static str {
    match g {
        Glyph::Terminal => icons::TERMINAL,
        Glyph::Book => icons::BOOK,
        Glyph::Pencil => icons::PENCIL,
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
    let mut head = hrow()
        .h(22.0)
        .gap(6.0)
        .accessibility_role(Role::Button)
        .accessibility_label(label.clone())
        .accessibility_expanded(open || running)
        .on_click(Msg::ToggleWork(index))
        .child(txt(label, BODY, p.muted));
    if !running {
        head = head.child(ico(
            if open {
                icons::CHEVRON_DOWN
            } else {
                icons::CHEVRON_RIGHT
            },
            13.0,
            p.muted,
        ));
    }
    let mut col = div()
        .w_full()
        .flex_col()
        .child(head)
        .child(div().h(3.0))
        .child(div().w_full().h(1.0).bg(p.hairline))
        .child(div().h(17.0));
    if open || running {
        for (s, step) in steps.iter().enumerate() {
            col = col.child(match step {
                Step::Prose { text, pending } => div()
                    .w_full()
                    .flex_col()
                    .child(inline_flow(
                        &parse_spans(text),
                        p,
                        p.text_soft,
                        Some(pending),
                    ))
                    .child(div().h(14.0)),
                Step::Group {
                    label,
                    glyph: g,
                    open,
                    rows,
                } => {
                    let mut group = div().w_full().flex_col().child(
                        hrow()
                            .h(22.0)
                            .gap(8.0)
                            .accessibility_role(Role::Button)
                            .accessibility_label((*label).to_owned())
                            .accessibility_expanded(*open)
                            .on_click(Msg::ToggleGroup(index, s))
                            .child(ico(glyph(*g), 14.0, p.muted))
                            .child(txt(*label, BODY, p.muted))
                            .child(ico(
                                if *open {
                                    icons::CHEVRON_DOWN
                                } else {
                                    icons::CHEVRON_RIGHT
                                },
                                12.0,
                                p.muted,
                            )),
                    );
                    if *open {
                        for (r, row) in rows.iter().enumerate() {
                            group = group
                                .child(div().h(3.0))
                                .child(tool_row(p, row, index, s, r, column));
                        }
                    }
                    group.child(div().h(14.0))
                }
                Step::Row(row) => div()
                    .w_full()
                    .flex_col()
                    .child(tool_row(p, row, index, s, 0, column))
                    .child(div().h(14.0)),
                Step::Live { glyph: g, text } => div().w_full().flex_col().child(
                    hrow()
                        .w_full()
                        .h(22.0)
                        .gap(8.0)
                        .overflow_hidden()
                        .child(ico(glyph(*g), 14.0, p.text_soft))
                        .child(shimmer(text, p, now_ms, BODY)),
                ),
            });
        }
    }
    col.child(div().h(4.0))
}

/// A finished step: "Ran ...", "Read <file>", "Edited <file> +2 -2", and
/// its Shell card or diff when expanded.
fn tool_row(p: &Pal, row: &Row, item: usize, step: usize, index: usize, column: f32) -> Div {
    let ink = p.text_soft.lerp(p.muted, 0.35);
    let line = |g: &'static str, children: Vec<AnyElement>| {
        hrow()
            .w_full()
            .h(22.0)
            .gap(8.0)
            .overflow_hidden()
            .child(ico(g, 14.0, p.muted))
            .children(children)
    };
    let chevron = |open: bool| {
        ico(
            if open {
                icons::CHEVRON_DOWN
            } else {
                icons::CHEVRON_RIGHT
            },
            12.0,
            p.muted,
        )
        .into_any()
    };
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
            let mut children = vec![
                div()
                    .flex_1()
                    .min_w(0.0)
                    .overflow_hidden()
                    .child(txt(label.clone(), BODY, ink).truncate())
                    .into_any(),
            ];
            if shell.is_some() {
                children.push(chevron(*open));
            }
            let mut d = div().w_full().flex_col().child(
                line(icons::TERMINAL, children)
                    .accessibility_role(Role::Button)
                    .accessibility_label(label)
                    .on_click(Msg::ToggleRow(item, step, index)),
            );
            if let (Some(shell), true) = (shell, *open) {
                d = d.child(div().h(6.0)).child(shell_card(p, shell));
            }
            d
        }
        Row::Read(file) => line(
            icons::BOOK,
            vec![
                txt("Read", BODY, ink).into_any(),
                div()
                    .relative()
                    .child(txt(*file, BODY, p.muted))
                    .child(dotted_under(p))
                    .into_any(),
            ],
        ),
        Row::Edited {
            file,
            adds,
            dels,
            open,
        } => {
            let mut d = div().w_full().flex_col().child(
                line(
                    icons::PENCIL,
                    vec![
                        txt(format!("Edited {file}"), BODY, ink).into_any(),
                        txt(format!("+{adds}"), BODY, p.success).into_any(),
                        txt(format!("-{dels}"), BODY, p.error).into_any(),
                        chevron(*open),
                    ],
                )
                .accessibility_role(Role::Button)
                .accessibility_label(format!("Toggle diff for {file}"))
                .on_click(Msg::ToggleRow(item, step, index)),
            );
            if *open {
                d = d
                    .child(div().h(6.0))
                    .child(diff_card(p, file, *adds, *dels, column));
            }
            d
        }
    }
}

fn dotted_under(p: &Pal) -> Div {
    let mut row = hrow()
        .absolute()
        .left(0.0)
        .right(0.0)
        .top(18.0)
        .h(1.0)
        .gap(2.0)
        .overflow_hidden();
    for _ in 0..40 {
        row = row.child(div().w(1.0).h(1.0).flex_shrink_0().bg(p.muted));
    }
    row
}

fn shell_card(p: &Pal, shell: &Shell) -> Div {
    let mut out = div()
        .w_full()
        .flex_col()
        .px(10.0)
        .gap(3.0)
        .h(44.0)
        .overflow_hidden();
    for line in shell.output.lines() {
        out = out.child(
            text(line.replace(' ', "\u{a0}"))
                .size(CODE)
                .mono()
                .color(p.faint)
                .no_wrap(),
        );
    }
    let mut card = div()
        .w_full()
        .flex_col()
        .bg(p.shell)
        .border(p.shell_border)
        .rounded(8.0)
        .overflow_hidden()
        .relative()
        .accessibility_role(Role::Group)
        .accessibility_label(format!("Shell: {}", shell.command))
        .child(
            hrow()
                .h(28.0)
                .px(8.0)
                .child(txt("Shell", SMALL, p.text_soft)),
        )
        .child(
            hrow()
                .h(26.0)
                .px(8.0)
                .gap(8.0)
                .child(text("$").size(CODE + 1.0).mono().color(p.muted).no_wrap())
                .child(
                    text(shell.command)
                        .size(CODE + 1.0)
                        .mono()
                        .color(p.text)
                        .no_wrap(),
                ),
        )
        .child(out);
    if let Some(footer) = shell.footer {
        card = card.child(
            hrow()
                .h(28.0)
                .px(10.0)
                .justify_end()
                .child(txt(footer, SMALL, p.muted)),
        );
    } else {
        // Output past the card's height fades out.
        card = card.child(
            div()
                .absolute()
                .left(0.0)
                .right(0.0)
                .top(70.0)
                .h(30.0)
                .bg_effect(linear_gradient(
                    std::f32::consts::FRAC_PI_2,
                    p.shell.with_alpha(0),
                    p.shell,
                )),
        );
    }
    card.h(if shell.footer.is_some() { 128.0 } else { 100.0 })
}

/// The inline diff of an edit row: header, two hunks with a separator,
/// word highlights on the added text.
fn diff_card(p: &Pal, file: &str, adds: u32, dels: u32, column: f32) -> Div {
    use data::DiffLine::*;
    let mut rows = div().w_full().flex_col();
    for line in data::cart_diff(false) {
        match line {
            Context { old: 1, .. } | Context { old: 8, .. } => continue,
            Context { old: 5, .. } => rows = rows.child(div().w_full().h(4.0).bg(p.fold)),
            _ => rows = rows.child(panel::diff_row(p, column, line, true)),
        }
    }
    div()
        .w_full()
        .flex_col()
        .bg(p.change_card)
        .border(p.shell_border)
        .rounded(8.0)
        .overflow_hidden()
        .accessibility_role(Role::Group)
        .accessibility_label(format!("{file} diff"))
        .child(
            hrow()
                .h(28.0)
                .px(10.0)
                .gap(6.0)
                .bg(p.shell)
                .child(txt(file.to_owned(), SMALL, p.text_soft))
                .child(txt(format!("+{adds}"), SMALL, p.success))
                .child(txt(format!("-{dels}"), SMALL, p.error))
                .child(div().flex_1())
                .child(ico(icons::COPY, 13.0, p.muted)),
        )
        .child(rows)
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
fn inline_flow(spans: &[Span], p: &Pal, color: Color, pending: Option<&str>) -> Div {
    let mut flow = div().w_full().flex_row().flex_wrap().items_center();
    let word = |w: &str, c: Color, bold: bool| {
        let t = text(w.replace(' ', "\u{a0}"))
            .size(BODY)
            .color(c)
            .no_wrap()
            .line_height(LINE);
        if bold { t.semibold() } else { t }
    };
    for span in spans {
        match span {
            Span::Text(t) | Span::Bold(t) => {
                let bold = matches!(span, Span::Bold(_));
                for w in t.split_inclusive(' ') {
                    flow = flow.child(word(w, if bold { p.text } else { color }, bold));
                }
            }
            Span::Code(c) => {
                flow = flow.child(
                    hrow()
                        .px(5.0)
                        .h(20.0)
                        .rounded(5.0)
                        .bg(p.chip)
                        .child(text(*c).size(CODE + 0.5).mono().color(p.text).no_wrap()),
                )
            }
            Span::File(name, line) => {
                let mut chip = hrow()
                    .gap(5.0)
                    .accessibility_role(Role::Link)
                    .accessibility_label((*name).to_owned())
                    .on_click(Msg::OpenFile("cart.js"))
                    .child(
                        div()
                            .w(13.0)
                            .h(13.0)
                            .rounded(3.0)
                            .bg(Color::rgba(0x2f, 0x6f, 0xd8, 255))
                            .items_center()
                            .justify_center()
                            .child(
                                text("js")
                                    .size(6.5)
                                    .bold()
                                    .color(Color::rgba(255, 255, 255, 255))
                                    .no_wrap(),
                            ),
                    )
                    .child(word(name, p.link, false));
                if let Some(n) = line {
                    chip = chip.child(word(&format!(" (line {n})"), p.link, false));
                }
                flow = flow.child(chip);
            }
        }
    }
    if let Some(pending) = pending.filter(|s| !s.is_empty()) {
        for w in pending.split_inclusive(' ') {
            flow = flow.child(word(w, p.faint.lerp(p.bg, 0.3), false));
        }
    }
    flow
}

fn answer(p: &Pal, blocks: &[Block], column: f32) -> Div {
    let mut col = div().w_full().flex_col();
    for block in blocks {
        match block {
            Block::Para(spans) => {
                col = col
                    .child(inline_flow(spans, p, p.text_soft, None))
                    .child(div().h(8.0));
            }
            Block::Bullets(items) => {
                for spans in items {
                    col =
                        col.child(
                            hrow()
                                .w_full()
                                .items_start()
                                .child(div().w(28.0).pl(5.0).child(
                                    text("•").size(BODY).color(p.text_soft).line_height(LINE),
                                ))
                                .child(div().w(column - 28.0).child(inline_flow(
                                    spans,
                                    p,
                                    p.text_soft,
                                    None,
                                ))),
                        );
                }
                col = col.child(div().h(8.0));
            }
        }
    }
    let actions = hrow()
        .gap(2.0)
        .child(icon_button(
            p,
            icons::COPY,
            26.0,
            14.0,
            p.muted,
            "Copy",
            Msg::Noop,
        ))
        .child(icon_button(
            p,
            icons::READ_ALOUD,
            26.0,
            14.0,
            p.muted,
            "Read aloud",
            Msg::Noop,
        ))
        .child(icon_button(
            p,
            icons::RATE,
            26.0,
            14.0,
            p.muted,
            "Rate response",
            Msg::Noop,
        ))
        .child(icon_button(
            p,
            icons::FORK_CHAT,
            26.0,
            14.0,
            p.muted,
            "Fork chat from here",
            Msg::Noop,
        ));
    col.child(actions).child(div().h(15.0))
}

fn file_change(p: &Pal, file: &str, adds: u32, dels: u32) -> Div {
    let light = p.mode == ThemeMode::Light;
    let card = hrow()
        .w_full()
        .h(64.0)
        .px(12.0)
        .gap(12.0)
        .bg(p.change_card)
        .border(p.change_border)
        .rounded(10.0)
        .accessibility_role(Role::Group)
        .accessibility_label(format!("Edited {file}"))
        .child(
            div()
                .w(40.0)
                .h(40.0)
                .rounded(8.0)
                .bg(p.tile)
                .items_center()
                .justify_center()
                .child(ico(icons::REVIEW, 17.0, p.text_soft)),
        )
        .child(
            div()
                .flex_col()
                .gap(2.0)
                .child(txt(format!("Edited {file}"), BODY, p.text))
                .child(
                    hrow()
                        .gap(5.0)
                        .child(txt(format!("+{adds}"), SMALL, p.success))
                        .child(txt(format!("-{dels}"), SMALL, p.error)),
                ),
        )
        .child(div().flex_1())
        .child(
            hrow()
                .h(28.0)
                .px(8.0)
                .gap(6.0)
                .accessibility_role(Role::Button)
                .accessibility_label("Undo")
                .child(txt("Undo", SMALL, p.text_soft))
                .child(ico(icons::UNDO, 13.0, p.text_soft)),
        )
        .child(
            hrow()
                .h(28.0)
                .px(10.0)
                .rounded(8.0)
                .border(if light {
                    p.change_border
                } else {
                    p.shell_border
                })
                .accessibility_role(Role::Button)
                .accessibility_label("View changes")
                .on_click(Msg::OpenTab(crate::Tab::Changes))
                .child(txt("View changes", SMALL, p.text_soft)),
        );
    div().w_full().flex_col().child(card).child(div().h(16.0))
}

/// The approval card that replaces the composer while a command waits:
/// category, the model's question, the command, Deny and Allow once.
fn approval_card(p: &Pal, a: &data::Approval, w: f32) -> (Div, f32) {
    let h = 183.0;
    let well = hrow()
        .w(w - 24.0)
        .h(36.0)
        .px(9.0)
        .rounded(8.0)
        .bg(p.tray)
        .child(
            text(a.command)
                .size(CODE + 0.5)
                .mono()
                .color(p.muted)
                .no_wrap(),
        );
    let deny = hrow()
        .h(28.0)
        .pl(10.0)
        .pr(4.0)
        .gap(6.0)
        .rounded(14.0)
        .border(p.composer_border.lerp(p.text, 0.1))
        .accessibility_role(Role::Button)
        .accessibility_label("Deny")
        .on_click(Msg::Decide(false))
        .child(txt("Deny", SMALL, p.text))
        .child(kbd(p, "Esc"));
    let allow = hrow()
        .h(30.0)
        .pl(10.0)
        .pr(8.0)
        .gap(8.0)
        .rounded(15.0)
        .bg(p.send_active)
        .child(
            hrow()
                .gap(6.0)
                .accessibility_role(Role::Button)
                .accessibility_label("Allow once")
                .on_click(Msg::Decide(true))
                .child(txt("Allow once", SMALL, p.send_active_glyph))
                .child(
                    div()
                        .px(3.0)
                        .rounded(4.0)
                        .bg(p.send_active_glyph.with_alpha(30))
                        .child(ico(icons::RETURN_KEY, 13.0, p.send_active_glyph)),
                ),
        )
        .child(
            div()
                .id("approval.options")
                .accessibility_role(Role::Button)
                .accessibility_label("More options")
                .on_click(Msg::Open(Menu::ApprovalOptions))
                .child(ico(
                    icons::CHEVRON_DOWN,
                    12.0,
                    p.send_active_glyph.with_alpha(160),
                )),
        );
    let card = div()
        .w(w)
        .h(h)
        .flex_col()
        .px(16.0)
        .pt(14.0)
        .gap(10.0)
        .bg(p.composer.lerp(p.bg, 0.15))
        .border(p.composer_border)
        .rounded(20.0)
        .accessibility_role(Role::AlertDialog)
        .accessibility_label(a.question)
        .child(
            hrow()
                .gap(8.0)
                .child(ico(icons::TERMINAL, 15.0, p.muted))
                .child(txt(a.category, SMALL, p.muted)),
        )
        .child(
            text(a.question)
                .size(BODY)
                .medium()
                .color(p.text)
                .line_height(1.5)
                .wrap_width(w - 32.0),
        )
        .child(div().margin_left(-4.0).child(well))
        .child(
            hrow()
                .gap(8.0)
                .child(div().flex_1())
                .child(deny)
                .child(allow),
        );
    (card, h)
}
