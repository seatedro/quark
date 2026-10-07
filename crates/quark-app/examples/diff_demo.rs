//! A diff viewer. `diff_demo OLD NEW` diffs two files, `diff_demo FILE`
//! shows a unified diff (`git diff > FILE`), and with no arguments it shows
//! a built-in sample of several files.
//!
//! The file list on the left jumps to a file. In the view: `n` and `p` (or
//! Alt+Down and Alt+Up) step through hunks, `]` and `[` through files, drag
//! selects (side by side, the side you start on), Ctrl/Cmd+C copies,
//! Ctrl/Cmd+A selects all. The toolbar switches between unified and side by
//! side and turns word wrap on. Build with `--features syntax` for syntax
//! colors.

use std::rc::Rc;

use quark_app::quark_ui::element::{AnyElement, IntoAnyElement, cached, div, inputs_hash, text};
use quark_app::quark_ui::style::Styled;
use quark_app::quark_ui::{Action, FocusId};
use quark_app::{UiApp, UiContext, ViewContext, WindowOptions};
use quark_components::{
    CollectionEnv, DiffEvent, DiffOutcome, DiffStyle, DiffViewState, diff_view,
};
use quark_diff::{DiffDocument, Mode, diff_texts, parse_unified};

const DIFF_FOCUS: FocusId = FocusId::from_key("demo.diff");
const TOOLBAR_H: f32 = 36.0;
const SIDEBAR_W: f32 = 240.0;

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Diff(DiffEvent),
    Mode(Mode),
    ToggleWrap,
    OpenFile(u32),
}

impl From<Msg> for Action {
    fn from(msg: Msg) -> Self {
        Action::new(msg)
    }
}

struct Demo {
    diff: DiffViewState,
    /// File list rows: path, status, additions, deletions.
    files: Rc<[(String, &'static str, u32, u32)]>,
    title: Rc<str>,
}

impl Demo {
    fn new(doc: DiffDocument, title: String) -> Self {
        let files = doc
            .summaries()
            .map(|s| {
                (
                    s.path.to_string(),
                    s.status.name(),
                    s.additions,
                    s.deletions,
                )
            })
            .collect();
        Self {
            diff: DiffViewState::new("demo.diff", DIFF_FOCUS, doc).with_label("Changes"),
            files,
            title: title.into(),
        }
    }
}

impl UiApp for Demo {
    type Action = Msg;
    type Message = ();

    fn view(&mut self, cx: &mut ViewContext) -> AnyElement {
        let (width, height) = cx.frame.size();
        let sidebar = if width > SIDEBAR_W * 2.5 {
            SIDEBAR_W
        } else {
            0.0
        };
        self.diff
            .set_viewport((width - sidebar).max(0.0), (height - TOOLBAR_H).max(0.0));
        let scale = cx.frame.scale_factor();
        let now_ms = cx.frame.elapsed().as_millis() as u64;
        let text_cx = cx.frame.text();
        self.diff
            .prepare(&mut text_cx.system, &mut text_cx.layouts, scale, now_ms);
        let env = CollectionEnv {
            focused: cx.is_focused(DIFF_FOCUS),
            accessible: cx.frame.accessibility_active(),
        };
        let view = diff_view(&mut self.diff, cx.theme, env, |e| Msg::Diff(e).into());
        let colors = cx.theme.colors;

        let (mode, wrap) = (self.diff.mode(), self.diff.style().wrap);
        let title = self.title.clone();
        let toolbar = cached(
            "demo.toolbar",
            inputs_hash(&(mode, wrap, width.to_bits())),
            {
                move || {
                    let toggle = |label: &'static str, on: bool, msg: Msg| {
                        let mut b = div()
                            .px(10.0)
                            .h(24.0)
                            .rounded(4.0)
                            .flex_row()
                            .items_center()
                            .hover_bg(colors.element_hover)
                            .on_click(msg)
                            .child(text(label).text_sm().color(colors.text));
                        if on {
                            b = b.bg(colors.element_selected);
                        }
                        b
                    };
                    div()
                        .w(width)
                        .h(TOOLBAR_H)
                        .flex_row()
                        .items_center()
                        .gap(6.0)
                        .px(10.0)
                        .bg(colors.title_bar_background)
                        .border_b(colors.border)
                        .child(text(&*title).text_sm().semibold().color(colors.text))
                        .child(div().flex_1())
                        .child(toggle(
                            "Unified",
                            mode == Mode::Unified,
                            Msg::Mode(Mode::Unified),
                        ))
                        .child(toggle(
                            "Side by side",
                            mode == Mode::Split,
                            Msg::Mode(Mode::Split),
                        ))
                        .child(toggle("Wrap", wrap, Msg::ToggleWrap))
                }
            },
        );
        let files = self.files.clone();
        let list = cached(
            "demo.files",
            inputs_hash(&(sidebar.to_bits(), height.to_bits())),
            {
                move || {
                    let mut list = div()
                        .w(sidebar)
                        .h(height - TOOLBAR_H)
                        .flex_col()
                        .clip()
                        .bg(colors.sidebar_background)
                        .border_r(colors.border);
                    for (i, (path, status, adds, dels)) in files.iter().enumerate() {
                        list = list.child(
                            div()
                                .w_full()
                                .h(28.0)
                                .flex_row()
                                .items_center()
                                .gap(6.0)
                                .px(10.0)
                                .hover_bg(colors.sidebar_row_hover)
                                .on_click(Msg::OpenFile(i as u32))
                                .child(text(*status).text_xs().color(colors.text_muted))
                                .child(text(path.as_str()).text_sm().color(colors.text).truncate())
                                .child(div().flex_1())
                                .child(
                                    text(format!("+{adds}"))
                                        .text_xs()
                                        .color(colors.line_add_text),
                                )
                                .child(
                                    text(format!("-{dels}"))
                                        .text_xs()
                                        .color(colors.line_del_text),
                                ),
                        );
                    }
                    list
                }
            },
        );
        let mut body = div().w(width).h(height - TOOLBAR_H).flex_row();
        if sidebar > 0.0 {
            body = body.child(list.w(sidebar).h(height - TOOLBAR_H));
        }
        div()
            .w(width)
            .h(height)
            .flex_col()
            .bg(colors.background)
            .child(toolbar.w(width).h(TOOLBAR_H))
            .child(body.child(view))
            .into_any()
    }

    fn update(&mut self, msg: Msg, cx: &mut UiContext) {
        match msg {
            Msg::Diff(event) => {
                if let DiffOutcome::Copy(copied) = self.diff.handle(event)
                    && !copied.is_empty()
                {
                    cx.window.set_clipboard_text(&copied);
                }
            }
            Msg::Mode(mode) => self.diff.set_mode(mode),
            Msg::ToggleWrap => {
                let style = self.diff.style();
                self.diff.set_style(DiffStyle {
                    wrap: !style.wrap,
                    ..style
                });
            }
            Msg::OpenFile(file) => {
                let row = self.diff.projection().file_rows[file as usize];
                self.diff.scroll_to_row(row);
            }
        }
    }
}

/// Two versions of a small crate: a modified source file, a renamed and
/// edited README, a new file, and a deleted one.
fn sample() -> DiffDocument {
    let old_lib = r#"//! A tiny tokenizer.

use std::fmt;

/// A token of the input.
#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    Number(i64),
    Word(String),
    Symbol(char),
}

impl fmt::Display for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Token::Number(n) => write!(f, "{n}"),
            Token::Word(w) => write!(f, "{w}"),
            Token::Symbol(c) => write!(f, "{c}"),
        }
    }
}

/// Splits `input` into tokens.
pub fn tokenize(input: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_whitespace() {
            continue;
        }
        if c.is_ascii_digit() {
            let mut n = c.to_digit(10).unwrap() as i64;
            while let Some(d) = chars.peek().and_then(|d| d.to_digit(10)) {
                n = n * 10 + d as i64;
                chars.next();
            }
            tokens.push(Token::Number(n));
        } else if c.is_alphabetic() {
            let mut w = String::from(c);
            while let Some(&d) = chars.peek() {
                if !d.is_alphanumeric() {
                    break;
                }
                w.push(d);
                chars.next();
            }
            tokens.push(Token::Word(w));
        } else {
            tokens.push(Token::Symbol(c));
        }
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_words_and_numbers() {
        assert_eq!(tokenize("a 1").len(), 2);
    }
}
"#;
    let new_lib = old_lib
        .replace("//! A tiny tokenizer.", "//! A tiny tokenizer with spans.")
        .replace(
            "    Symbol(char),\n}",
            "    Symbol(char),\n    /// Text in double quotes, without them.\n    Quoted(String),\n}",
        )
        .replace(
            "            Token::Symbol(c) => write!(f, \"{c}\"),\n",
            "            Token::Symbol(c) => write!(f, \"{c}\"),\n            Token::Quoted(q) => write!(f, \"\\\"{q}\\\"\"),\n",
        )
        .replace("let mut n = c.to_digit(10).unwrap() as i64;", "let mut n = i64::from(c.to_digit(10).unwrap_or(0));")
        .replace("n = n * 10 + d as i64;", "n = n * 10 + i64::from(d);")
        .replace(
            "        } else {\n            tokens.push(Token::Symbol(c));",
            "        } else if c == '\"' {\n            let quoted: String = chars.by_ref().take_while(|&d| d != '\"').collect();\n            tokens.push(Token::Quoted(quoted));\n        } else {\n            tokens.push(Token::Symbol(c));",
        )
        .replace("assert_eq!(tokenize(\"a 1\").len(), 2);", "assert_eq!(tokenize(\"a 1 \\\"x y\\\"\").len(), 3);");
    let old_readme = "# tok\n\nSplits text into tokens.\n\nUsage: call `tokenize`.\n";
    let new_readme = "# tok\n\nSplits text into tokens: numbers, words, symbols, and quoted text.\n\nUsage: call `tokenize` — it never fails.\n";
    let mut doc = diff_texts(
        Some("src/lib.rs"),
        Some("src/lib.rs"),
        Some(old_lib),
        Some(&new_lib),
        3,
    );
    doc.append(diff_texts(
        Some("README"),
        Some("README.md"),
        Some(old_readme),
        Some(new_readme),
        3,
    ));
    doc.append(diff_texts(
        None,
        Some("src/span.rs"),
        None,
        Some("/// A byte range of the input.\npub type Span = std::ops::Range<usize>;\n"),
        3,
    ));
    doc.append(diff_texts(
        Some("TODO"),
        None,
        Some("quoted strings\n"),
        None,
        3,
    ));
    doc
}

fn load(args: &[String]) -> std::io::Result<(DiffDocument, String)> {
    match args {
        [old, new] => {
            let (a, b) = (std::fs::read_to_string(old)?, std::fs::read_to_string(new)?);
            let doc = diff_texts(Some(old), Some(new), Some(&a), Some(&b), 3);
            Ok((doc, format!("{old} \u{2192} {new}")))
        }
        [patch] => {
            let text = std::fs::read_to_string(patch)?;
            let doc = parse_unified(&text)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            Ok((doc, patch.clone()))
        }
        _ => Ok((sample(), "Sample changes".to_owned())),
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (doc, title) = load(&args)?;
    let mut demo = Demo::new(doc, title);
    demo.diff.enable_syntax();
    quark_app::run_ui(
        demo,
        WindowOptions {
            title: "Diff".into(),
            size: (1100.0, 720.0),
            ..WindowOptions::default()
        },
    )?;
    Ok(())
}

/// The view driven headlessly the way a user drives it: drags, clicks,
/// and keys on lines found by role and name.
#[cfg(test)]
mod tests {
    use accesskit::Role;
    use quark_app::quark_ui::test_alloc::{self, Counting};
    use quark_app::testing::{By, UiTestHarness};
    use quark_diff::RowKind;

    use super::*;

    #[global_allocator]
    static ALLOCATOR: Counting = Counting;

    /// Narrow enough that the file list is hidden.
    const SIZE: (f32, f32) = (600.0, 400.0);

    fn harness(doc: DiffDocument) -> UiTestHarness<Demo> {
        UiTestHarness::new(Demo::new(doc, "test".into()), SIZE, 1.0)
    }

    fn line(name: &str) -> By {
        By::role_name(Role::ListItem, name)
    }

    fn numbered(lines: std::ops::Range<u32>) -> String {
        lines.map(|i| format!("line {i}\n")).collect()
    }

    /// Lines 0..60 with lines 10 and 40 changed, one line of context: two
    /// hunks with a collapsed gap between them.
    fn two_hunks() -> DiffDocument {
        let old = numbered(0..60);
        let new = old
            .replace("line 10\n", "line ten\n")
            .replace("line 40\n", "line forty\n");
        diff_texts(Some("f.txt"), Some("f.txt"), Some(&old), Some(&new), 1)
    }

    #[test]
    fn a_hundred_thousand_line_file_builds_only_the_window() {
        let lines = numbered(0..100_000);
        let doc = diff_texts(None, Some("big.txt"), None, Some(&lines), 3);
        let mut ui = harness(doc);
        // 364 points of body, 20-point lines, eight lines of overscan below.
        let built = ui.find_all(By::role(Role::ListItem)).len();
        assert!((18..=30).contains(&built), "{built} lines built");
        ui.click_node(line("line 3"));
        ui.key("end");
        let items = ui.find_all(By::role(Role::ListItem));
        assert!(items.len() <= 30, "{} lines built", items.len());
        assert_eq!(items.last().unwrap().name.as_deref(), Some("line 99999"));
    }

    #[test]
    fn side_by_side_puts_changed_lines_on_one_row_and_pads_the_shorter_side() {
        let doc = diff_texts(
            Some("f"),
            Some("f"),
            Some("keep\nold a\nold b\nend\n"),
            Some("keep\nnew a\nnew b\nnew c\nend\n"),
            3,
        );
        let mut ui = harness(doc);
        ui.app_mut().diff.set_mode(Mode::Split);
        ui.frame();
        let y = |name: &str| ui.find(line(name)).bounds.y;
        let x = |name: &str| ui.find(line(name)).bounds.x;
        assert_eq!(y("old a"), y("new a"));
        assert_eq!(y("old b"), y("new b"));
        assert!(x("old a") < x("new a"));
        // Unchanged lines show on both sides, level with each other.
        let keeps = ui.find_all(line("end"));
        assert_eq!(keeps.len(), 2);
        assert_eq!(keeps[0].bounds.y, keeps[1].bounds.y);
        // "new c" has no old line beside it: "end" sits one row lower.
        assert!(y("new c") < keeps[0].bounds.y);
        assert!(
            ui.find_all(By::role(Role::ListItem))
                .iter()
                .all(|n| n.bounds.y != y("new c") || n.name.as_deref() == Some("new c"))
        );
    }

    /// Clicks the `n`th button named `name`, in tree order.
    fn click_nth(ui: &mut UiTestHarness<Demo>, name: &str, n: usize) {
        let at = ui.find_all(By::role_name(Role::Button, name))[n].center();
        ui.click(at);
    }

    #[test]
    fn expand_controls_reveal_hidden_lines() {
        let mut ui = harness(two_hunks());
        assert!(ui.try_find(line("line 25")).is_none());
        // The gap between the hunks hides lines 12 to 38.
        click_nth(&mut ui, "Show 20 more lines below the previous change", 1);
        assert!(ui.try_find(line("line 25")).is_some());
        // Of 27 hidden lines, 7 are left.
        ui.app_mut().diff.scroll_to_row(10);
        ui.frame();
        let gap = By::role_name(Role::Label, "7 unchanged lines    @@ -40,3 +40,3 @@");
        let label_y = ui.find(gap.clone()).bounds.y;
        let all = ui
            .find_all(By::role_name(Role::Button, "Show all unchanged lines"))
            .into_iter()
            .find(|b| (b.bounds.y - label_y).abs() < 20.0)
            .unwrap();
        ui.click(all.center());
        assert!(ui.try_find(gap).is_none());
        ui.app_mut().diff.scroll_to_row(30);
        ui.frame();
        assert!(ui.try_find(line("line 37")).is_some());
    }

    #[test]
    fn dragging_across_hunks_copies_the_new_text() {
        let mut ui = harness(two_hunks());
        let from = ui.find(line("line 9")).bounds;
        let to = ui.find(line("line forty")).bounds;
        ui.drag(
            (from.x + 1.0, from.y + 5.0),
            (to.x + to.width - 1.0, to.y + 5.0),
        );
        ui.key("ctrl+c");
        // Removed lines and hidden lines are not copied.
        assert_eq!(
            ui.clipboard_text().as_deref(),
            Some("line 9\nline ten\nline 11\nline 39\nline forty")
        );
    }

    // Selection drags past the view's top or bottom edge scroll it on the
    // clock, with the selection end following, until release.
    #[test]
    fn a_drag_held_past_an_edge_scrolls_until_release() {
        let lines = numbered(0..400);
        let cases = [
            ("top", TOOLBAR_H - 30.0, -1.0),
            ("bottom", SIZE.1 + 30.0, 1.0),
        ];
        for (edge, pointer_y, direction) in cases {
            let mut ui = harness(diff_texts(None, Some("f"), None, Some(&lines), 3));
            ui.app_mut().diff.scroll_to_row(200);
            ui.frame();
            let pressed = ui.find(line("line 205")).center();
            ui.pointer_down(pressed);
            ui.pointer_move((pressed.0, pointer_y));
            let start = ui.app().diff.scroll_offset();
            ui.advance(500);
            let held = ui.app().diff.scroll_offset();
            ui.pointer_up((pressed.0, pointer_y));
            ui.advance(500);

            assert!(
                (held - start) * direction > 50.0,
                "{edge}: {start} -> {held}"
            );
            assert_eq!(
                ui.app().diff.scroll_offset(),
                held,
                "{edge}: stopped on release"
            );
            ui.key("ctrl+c");
            let copied = ui.clipboard_text().unwrap_or_default().lines().count();
            assert!(
                copied > 10,
                "{edge}: the selection followed, {copied} lines"
            );
        }
    }

    #[test]
    fn side_by_side_copies_the_side_the_drag_started_on() {
        let mut ui = harness(two_hunks());
        ui.app_mut().diff.set_mode(Mode::Split);
        ui.frame();
        let olds = ui.find_all(line("line 9"));
        let from = olds[0].bounds;
        let to = ui.find_all(line("line 11"))[0].bounds;
        ui.drag(
            (from.x + 1.0, from.y + 5.0),
            (to.x + to.width - 1.0, to.y + 5.0),
        );
        ui.key("ctrl+c");
        assert_eq!(
            ui.clipboard_text().as_deref(),
            Some("line 9\nline 10\nline 11")
        );
    }

    #[test]
    fn hunk_keys_step_between_hunks() {
        let lines = numbered(0..600);
        let mut new = lines.clone();
        for n in [100, 200, 300, 400, 500] {
            new = new.replace(&format!("line {n}\n"), &format!("changed {n}\n"));
        }
        let mut ui = harness(diff_texts(
            Some("f"),
            Some("f"),
            Some(&lines),
            Some(&new),
            3,
        ));
        ui.click_node(line("line 98"));
        let top_kind = |ui: &UiTestHarness<Demo>| {
            let state = &ui.app().diff;
            let row = state.top_row().unwrap();
            let p = state.projection();
            (p.kind[row as usize], p.hunk.get(row as usize + 1).copied())
        };
        // Each hunk starts at the collapsed gap above it, under the file
        // header.
        ui.key("n");
        assert_eq!(top_kind(&ui), (RowKind::Gap, Some(0)));
        ui.key("n");
        assert_eq!(top_kind(&ui), (RowKind::Gap, Some(1)));
        assert!(ui.try_find(line("changed 200")).is_some());
        ui.key("alt+arrowdown");
        assert_eq!(top_kind(&ui), (RowKind::Gap, Some(2)));
        ui.key("p");
        assert_eq!(top_kind(&ui), (RowKind::Gap, Some(1)));
    }

    fn long_lines() -> DiffDocument {
        let long = "word ".repeat(80);
        let old = format!("short\nold {long}\n");
        let new = format!("short\nnew {long}\n");
        diff_texts(Some("f"), Some("f"), Some(&old), Some(&new), 3)
    }

    #[test]
    fn each_side_scrolls_sideways_on_its_own() {
        let mut ui = harness(long_lines());
        ui.app_mut().diff.set_mode(Mode::Split);
        ui.frame();
        let old = ui.find_all(line("short"))[0].bounds;
        ui.pointer_move((old.x + 20.0, old.y + 5.0));
        ui.wheel(120.0, 0.0);
        ui.frame();
        let offset = |side| ui.app().diff.horizontal_scroll(side).offset().0;
        assert!(offset(quark_diff::Side::Old) > 0.0);
        assert_eq!(offset(quark_diff::Side::New), 0.0);
    }

    #[test]
    fn wrapped_lines_grow_their_row() {
        let mut ui = harness(long_lines());
        let style = ui.app().diff.style();
        ui.app_mut().diff.set_style(DiffStyle {
            wrap: true,
            ..style
        });
        ui.frame();
        let short = ui.find(line("short")).bounds.height;
        let long = ui
            .find_all(By::role(Role::ListItem))
            .into_iter()
            .find(|n| n.name.as_deref().is_some_and(|n| n.starts_with("new word")))
            .unwrap();
        assert!(
            long.bounds.height >= short * 3.0,
            "{} vs {short}",
            long.bounds.height
        );
    }

    /// A frame that repeats the last one, with no screen reader connected,
    /// replays the view from the element cache.
    #[test]
    fn a_repeated_frame_allocates_nothing() {
        let lines = numbered(0..100_000);
        let new = lines.replace("line 5\n", "line five\n");
        let mut ui = harness(diff_texts(
            Some("f"),
            Some("f"),
            Some(&lines),
            Some(&new),
            3,
        ));
        ui.set_accessibility_active(false);
        ui.app_mut().diff.set_mode(Mode::Split);
        for _ in 0..3 {
            ui.frame();
        }
        let ((), sites) = test_alloc::profile(|| {
            ui.frame();
        });
        assert!(sites.is_empty(), "{sites:#?}");
    }
}
