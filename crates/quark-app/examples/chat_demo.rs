//! A chat of 5,000 markdown messages of varied length, with an assistant
//! answer streaming in at the bottom: headings, lists, quotes,
//! tables, and fenced code blocks, each its own selectable block. Drag to
//! select across messages (list markers and heading hashes survive the
//! copy), Ctrl/Cmd+C to copy, Ctrl/Cmd+A to select all, wheel to scroll.
//! Scrolling up while text streams shows "Jump to latest". Ctrl/Cmd+F
//! opens find: type to highlight matches (they follow the streaming text),
//! Enter and Shift+Enter step through them, Escape closes it. Escape
//! without find open quits.
//!
//! Build with `--features syntax` for highlighted code blocks, after
//! building the grammar packs with `cargo run -p syntax-pack -- build`
//! (`QUARK_SYNTAX_PACKS` names another pack root). Each answer
//! shows an image block, decoded on the image worker from a generated
//! gradient; it reserves a placeholder until its pixels arrive.
//!
//! The app keeps only markdown strings: `MarkdownDocument` parses them
//! (incrementally while streaming), converts them to blocks, allocates the
//! block keys, and routes arriving highlights back to their messages.
//!
//! The document knows nothing about chat. This demo adds the chat chrome
//! on top, as a reference for apps: each message's role and author ride in
//! its `RowChrome`, `ChatChrome` draws the author line and the tint behind
//! user messages from it, and the "Jump to latest" button is an overlay
//! shown while `Document::has_content_below`.
//!
//! Rows outside the window are measured on a background thread, so the
//! scrollbar and scroll positions become exact while the demo idles.
//!
//! Set `QUARK_CHAT_LOG=1` to print view build times (prepare plus element
//! construction) and how many rows have exact heights every 120 frames.
//! `QUARK_CHAT_HISTORY=<n>` sets the history length, `QUARK_CHAT_IDLE=1`
//! turns streaming off, and `QUARK_CHAT_SYNC=1` turns background
//! measurement off, for comparing frame times.

use std::sync::Arc;
use std::time::{Duration, Instant};

use quark::view;
use quark_app::quark_ui::accessibility::Politeness;
use quark_app::quark_ui::design::Alpha;
use quark_app::quark_ui::document::{
    Block, BlockMeasurer, DocumentCommand, DocumentEvent, DocumentStyle, FindBarActions,
    LoadedImage, MarkdownDocument, MarkdownEntry, MeasureKey, RowChrome, RowDecorator,
    TextGeometry, TextMeasurer, find_bar, key_command,
};
use quark_app::quark_ui::element::{AnyElement, IntoAnyElement, div, text};
use quark_app::quark_ui::style::Styled;
use quark_app::quark_ui::text_input::{TextEditCommand, TextEditOutcome, TextField};
use quark_app::quark_ui::theme::{Color, Theme};
use quark_app::quark_ui::virtual_list::RowKey;
use quark_app::quark_ui::virtual_list::ScrollAlign;
use quark_app::quark_ui::{Action, FocusId};
use quark_app::winit::keyboard::NamedKey;
use quark_app::{InputEvent, UiApp, UiContext, ViewContext, WindowOptions};

const HISTORY: u64 = 5_000;
const STREAM_TICK: Duration = Duration::from_millis(60);
/// Bytes of the scripted answer revealed per tick.
const STREAM_CHUNK: usize = 7;
const FONT_SIZE: f32 = 14.0;
const FIND_FIELD: FocusId = FocusId::from_key("chat.find");
const WORDS: &[&str] = &[
    "virtualized",
    "rows",
    "measure",
    "only",
    "what",
    "is",
    "visible",
    "and",
    "the",
    "selection",
    "survives",
    "scrolling",
    "because",
    "it",
    "names",
    "blocks",
    "by",
    "key",
    "not",
    "index",
    "streaming",
    "text",
    "keeps",
    "the",
    "view",
    "pinned",
    "to",
    "the",
    "bottom",
    "while",
    "history",
    "prepends",
    "above",
    "without",
    "moving",
    "anything",
    "on",
    "screen",
    "layout",
    "shaping",
    "happens",
    "once",
    "per",
    "frame",
    "through",
    "a",
    "shared",
    "cache",
];

/// Code samples, one per built-in grammar, plus one in an unknown language
/// that renders plain.
const SAMPLES: &[(&str, &str)] = &[
    (
        "rust",
        "fn visible_rows(list: &VariableList, overscan: f32) -> Range<usize> {\n    // Only rows inside the window are measured.\n    let window = list.window(overscan);\n    window.range\n}",
    ),
    (
        "python",
        "def measure(rows, width):\n    \"\"\"Heights for the rows entering the window.\"\"\"\n    return [row.height(width) for row in rows if row.visible]",
    ),
    (
        "ts",
        "interface Block { key: number; text: string }\nexport const copy = (blocks: Block[]): string =>\n  blocks.map((b) => b.text).join(\"\\n\\n\");",
    ),
    (
        "json",
        "{\n  \"rows\": 5000,\n  \"materialized\": 14,\n  \"pinned\": true\n}",
    ),
    (
        "bash",
        "# Build the grammar packs, then run the demo with highlighted code blocks.\ncargo run -p syntax-pack -- build\ncargo run --release -p quark-app --example chat_demo --features syntax",
    ),
    (
        "go",
        "func copyText(blocks []string) string {\n\treturn strings.Join(blocks, \"\\n\\n\")\n}",
    ),
    ("klingon", "qapla' batlh Daqawlu'taH"),
];

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Document(DocumentEvent),
    JumpToLatest,
    FindNext,
    FindPrev,
    CloseFind,
}

impl From<Msg> for Action {
    fn from(msg: Msg) -> Self {
        Action::new(msg)
    }
}

/// Who wrote a message. Stored in the row's `RowChrome::kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    User = 0,
    Assistant = 1,
}

/// Height of the author line above each message's blocks.
const AUTHOR_LINE: f32 = FONT_SIZE * 1.6;

/// A message's chrome: the author names the row and fills its header.
fn chat_chrome(role: Role, author: &str) -> RowChrome {
    RowChrome {
        header_height: AUTHOR_LINE.round(),
        label: Some(author.into()),
        kind: role as u32,
    }
}

/// Draws the chat chrome the document leaves to the app: the author line
/// above every message and a soft tint behind the user's.
struct ChatChrome;

impl RowDecorator for ChatChrome {
    fn background(&self, chrome: &RowChrome, theme: &Theme) -> Option<Color> {
        (chrome.kind == Role::User as u32).then(|| theme.colors.surface.with_alpha(Alpha::SOFT))
    }

    fn header(&self, chrome: &RowChrome, _width: f32, _theme: &Theme) -> Option<AnyElement> {
        let author = chrome.label.as_deref()?;
        Some(view! {
            <text size={FONT_SIZE * 0.85} class="font-semibold">{author.to_owned()}</text>
        })
    }
}

/// The overlay shown while new content arrived below the view.
fn jump_to_latest(theme: &Theme, width: f32, height: f32) -> AnyElement {
    let label = quark_app::quark_ui::i18n::tr("quark-jump-to-latest");
    // Wide enough for longer translations ("Zum Neuesten springen").
    let w =
        (label.chars().count() as f32 * FONT_SIZE * 0.55 + FONT_SIZE * 2.0).max(FONT_SIZE * 10.0);
    let h = FONT_SIZE * 2.4;
    view! {
        <div class="absolute" left={((width - w) * 0.5).max(0.0)}
             top={(height - h - FONT_SIZE).max(0.0)}
             class="w-[w] h-[h] rounded-[h * 0.5] items-center justify-center bg-[theme.colors.accent]"
             hover_bg={theme.colors.accent_strong}
             accessibility_id="chat.jump-to-latest" accessibility_role={accesskit::Role::Button}
             aria-label={label.clone()} on:click={Msg::JumpToLatest}>
            <text size={FONT_SIZE * 0.9} class="font-semibold" color={theme.colors.text_strong}>
                {label}
            </text>
        </div>
    }
}

/// Deterministic xorshift so every run shows the same chat.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }

    fn word(&mut self) -> &'static str {
        WORDS[self.below(WORDS.len() as u64) as usize]
    }

    fn sentence(&mut self, words: usize) -> String {
        let mut out = String::new();
        for i in 0..words {
            if i > 0 {
                out.push(' ');
            }
            out.push_str(self.word());
        }
        out.push('.');
        out
    }
}

fn fence(sample: usize) -> String {
    let (lang, code) = SAMPLES[sample % SAMPLES.len()];
    format!("```{lang}\n{code}\n```")
}

/// Markdown of history message `i`. User messages are one line; assistant
/// messages mix paragraphs with lists, quotes, tables, and code.
fn history_markdown(i: u64, rng: &mut Rng) -> String {
    let user = i.is_multiple_of(2);
    let words = 3 + rng.below(if user { 25 } else { 60 }) as usize;
    // The first block starts with the message number (e2e specs read it).
    let mut parts = vec![format!("#{i}: {}", rng.sentence(words))];
    if user {
        return parts.remove(0);
    }
    for _ in 0..rng.below(3) {
        parts.push(match rng.below(8) {
            0 => format!("### {}", rng.sentence(3)),
            1 | 2 => (0..2 + rng.below(3))
                .map(|_| format!("- **{}** {}", rng.word(), rng.sentence(6)))
                .collect::<Vec<_>>()
                .join("\n"),
            3 => format!("> {}", rng.sentence(12)),
            4 => format!(
                "| rows | {} |\n|---|---|\n| {} | `{}` |",
                rng.word(),
                rng.below(5000),
                rng.word()
            ),
            5 | 6 => fence(rng.below(SAMPLES.len() as u64) as usize),
            _ => format!("{} *{}*", rng.sentence(20), rng.sentence(3)),
        });
    }
    parts.join("\n\n")
}

/// The scripted answer streamed at the bottom; `n` picks its code sample.
fn answer_markdown(n: usize, rng: &mut Rng) -> String {
    format!(
        "## Streaming answer {n}\n\n{}\n\n![A generated gradient](gradient-480x120)\n\n1. Rows are measured only inside the window.\n2. Selection names blocks by key, so it survives scrolling.\n   - nested markers copy too\n\n> Copying keeps `- `, `1. `, and `## ` prefixes.\n\n{}\n\n---\n\n{}",
        rng.sentence(24),
        fence(n),
        rng.sentence(30),
    )
}

/// The demo's image loader: `gradient-<w>x<h>` is a generated gradient of
/// that size, standing in for an app fetching image bytes.
fn gradient_image(src: &str) -> Option<LoadedImage> {
    let (w, h) = src.strip_prefix("gradient-")?.split_once('x')?;
    let (width, height): (u32, u32) = (w.parse().ok()?, h.parse().ok()?);
    let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
    for y in 0..height {
        for x in 0..width {
            pixels.extend_from_slice(&[
                (x * 255 / width.max(1)) as u8,
                (y * 255 / height.max(1)) as u8,
                200,
                255,
            ]);
        }
    }
    Some(LoadedImage::Rgba {
        width,
        height,
        pixels,
    })
}

/// Sent by the timer thread every [`STREAM_TICK`].
struct StreamTick;

/// The answer currently streaming in.
struct Stream {
    key: RowKey,
    script: String,
    shown: usize,
}

#[derive(Default)]
struct Timing {
    frames: u32,
    total: Duration,
    max: Duration,
}

/// A text measurer that offers no background spec, so only the rows in
/// the window are ever measured (`QUARK_CHAT_SYNC=1`).
struct WindowOnly<'a>(TextMeasurer<'a>);

impl BlockMeasurer for WindowOnly<'_> {
    type Geometry = TextGeometry;

    fn measure(&mut self, block: &Block, width: f32) -> TextGeometry {
        self.0.measure(block, width)
    }

    fn settings_key(&self) -> MeasureKey {
        self.0.settings_key()
    }
}

fn env_flag(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|v| v != "0")
}

struct Demo {
    chat: MarkdownDocument,
    rng: Rng,
    stream: Option<Stream>,
    answers: usize,
    next_key: u64,
    timing: Timing,
    log: bool,
    idle: bool,
    sync: bool,
    /// Frames drawn so far, and whether every row's height has been exact.
    frames: u64,
    all_exact: bool,
    started: Instant,
    /// The find query field, while find is open.
    find: Option<TextField>,
}

/// Grammars from the pack root `$QUARK_SYNTAX_PACKS`, or the one
/// `cargo run -p syntax-pack -- build` writes (`target/syntax-packs`).
#[cfg(feature = "syntax")]
fn grammar_store() -> quark_app::quark_ui::quark_syntax::GrammarStore {
    use quark_app::quark_ui::quark_syntax::{GrammarStore, StoreConfig};
    let root = std::env::var_os("QUARK_SYNTAX_PACKS")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/syntax-packs")
        });
    let config = StoreConfig::new().local_packs(root);
    // With `syntax-download`, grammars missing locally come from quark's
    // pack host, verified against its index key.
    #[cfg(feature = "syntax-download")]
    let config = {
        use quark_app::quark_ui::quark_syntax::{Downloads, PublicKey};
        const INDEX: &str = "https://quark.seated.ro/v1/{target}/index.json";
        const KEY: &str = "2194429b3227f613ac19401deddbb0bdc2b4b283e1ecec0d0d38892c28d63955";
        let key = PublicKey::from_hex(KEY).expect("valid pack index key");
        config.downloads(Downloads::new("quark-demos", INDEX, &[key]))
    };
    GrammarStore::new(config)
}

impl Demo {
    fn new() -> Self {
        let history_len = std::env::var("QUARK_CHAT_HISTORY")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(HISTORY);
        Self::with_history(history_len)
    }

    fn with_history(history_len: u64) -> Self {
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        let history: Vec<MarkdownEntry> = (0..history_len)
            .map(|i| {
                let (role, author) = if i.is_multiple_of(2) {
                    (Role::User, "You")
                } else {
                    (Role::Assistant, "Assistant")
                };
                MarkdownEntry {
                    row: RowKey(i),
                    chrome: chat_chrome(role, author),
                    markdown: history_markdown(i, &mut rng),
                }
            })
            .collect();
        let mut chat = MarkdownDocument::new(DocumentStyle::for_font_size(FONT_SIZE));
        chat.set_decorator(ChatChrome);
        #[cfg(feature = "syntax")]
        chat.set_grammar_store(grammar_store());
        chat.set_image_loader(Arc::new(gradient_image));
        chat.extend(history).unwrap_or_else(|e| eprintln!("{e:?}"));
        Self {
            chat,
            rng,
            stream: None,
            answers: 0,
            next_key: history_len,
            timing: Timing::default(),
            log: std::env::var_os("QUARK_CHAT_LOG").is_some(),
            idle: env_flag("QUARK_CHAT_IDLE"),
            sync: env_flag("QUARK_CHAT_SYNC"),
            frames: 0,
            all_exact: false,
            started: Instant::now(),
            find: None,
        }
    }

    /// Reveals the next chunk of the streaming answer; a finished answer
    /// is followed by a new one. Returns whether this chunk finished it.
    fn tick(&mut self) -> bool {
        if self
            .stream
            .as_ref()
            .is_none_or(|s| s.shown >= s.script.len())
        {
            let key = RowKey(self.next_key);
            self.next_key += 1;
            let script = answer_markdown(self.answers, &mut self.rng);
            self.answers += 1;
            let pushed = self.chat.push(MarkdownEntry {
                row: key,
                chrome: chat_chrome(Role::Assistant, "Assistant (streaming)"),
                markdown: String::new(),
            });
            if let Err(e) = pushed {
                eprintln!("{e:?}");
            }
            self.stream = Some(Stream {
                key,
                script,
                shown: 0,
            });
        }
        let Some(stream) = &mut self.stream else {
            return false;
        };
        let mut end = (stream.shown + STREAM_CHUNK).min(stream.script.len());
        while !stream.script.is_char_boundary(end) {
            end += 1;
        }
        stream.shown = end;
        if let Err(e) = self.chat.set_markdown(stream.key, &stream.script[..end]) {
            eprintln!("{e:?}");
        }
        end == stream.script.len()
    }

    fn record(&mut self, elapsed: Duration) {
        self.frames += 1;
        let rows = self.chat.document().list().rows();
        let exact = rows.measured_count();
        if self.log && !self.all_exact && exact == rows.len() {
            eprintln!(
                "chat view: all {} rows exact after {} frames ({:.2} s)",
                rows.len(),
                self.frames,
                self.started.elapsed().as_secs_f64(),
            );
        }
        self.all_exact = exact == rows.len();
        let t = &mut self.timing;
        t.frames += 1;
        t.total += elapsed;
        t.max = t.max.max(elapsed);
        if self.log && t.frames == 120 {
            eprintln!(
                "chat view: avg {:.3} ms, max {:.3} ms over {} frames ({} rows, {} exact, {} materialized)",
                t.total.as_secs_f64() * 1e3 / f64::from(t.frames),
                t.max.as_secs_f64() * 1e3,
                t.frames,
                rows.len(),
                exact,
                self.chat.document().visible_rows().len(),
            );
            *t = Timing::default();
        }
    }
}

impl Demo {
    fn close_find(&mut self, cx: &mut UiContext) {
        self.find = None;
        self.chat.close_find();
        cx.set_focus(None);
    }
}

impl UiApp for Demo {
    type Action = Msg;
    /// A stream tick from the timer thread.
    type Message = StreamTick;

    fn init(&mut self, cx: &mut UiContext) {
        // The scripted answer arrives on a timer thread, the way a real
        // model's chunks arrive from a socket reader.
        if self.idle {
            return;
        }
        let sender = cx.sender::<StreamTick>();
        std::thread::spawn(move || {
            while sender.send(StreamTick) {
                std::thread::sleep(STREAM_TICK);
            }
        });
    }

    fn message(&mut self, _tick: StreamTick, cx: &mut UiContext) {
        if self.tick() {
            cx.announce("Response complete", Politeness::Polite);
        }
        cx.window.request_redraw();
    }

    fn view(&mut self, cx: &mut ViewContext) -> AnyElement {
        let (width, height) = cx.frame.size();
        let scale = cx.frame.scale_factor();
        let now = cx.frame.elapsed();

        // Rebuild the messages whose code highlights arrived. Ticks redraw
        // often enough to pick them up while streaming.
        self.chat.poll_highlights();
        self.chat.poll_images();
        if self.chat.is_loading_images() {
            cx.frame.request_frame();
        }

        let started = Instant::now();
        let header_h = 36.0;
        let body_h = (height - header_h).max(0.0);
        let font_size = self.chat.document().style().font_size;
        let text_cx = cx.frame.text();
        let measurer =
            TextMeasurer::new(&mut text_cx.system, &mut text_cx.layouts, font_size, scale);
        let now_ms = now.as_millis() as u64;
        if self.sync {
            self.chat
                .prepare(width, body_h, now_ms, &mut WindowOnly(measurer));
        } else {
            self.chat.prepare(width, body_h, now_ms, &mut { measurer });
        }
        let element = self
            .chat
            .element(cx.theme, |event| Msg::Document(event).into())
            .label("Conversation");
        self.record(started.elapsed());

        // The element schedules its own frames while a drag autoscrolls.
        // Background heights land in the frames after they are measured.
        if self.chat.is_measuring() {
            cx.frame.request_frame();
        }

        let find = self.find.as_ref().map(|field| {
            find_bar(
                self.chat.document().find(),
                field,
                FIND_FIELD,
                cx.is_focused(FIND_FIELD),
                FindBarActions {
                    next: Msg::FindNext.into(),
                    prev: Msg::FindPrev.into(),
                    close: Msg::CloseFind.into(),
                },
                cx.theme,
            )
        });
        let jump = self
            .chat
            .document()
            .has_content_below()
            .then(|| jump_to_latest(cx.theme, width, body_h));
        let colors = &cx.theme.colors;
        let view = self.chat.document();
        let status = format!(
            "{} messages, {} materialized, {}{}",
            view.len(),
            view.visible_rows().len(),
            if view.is_stuck_to_bottom() {
                "pinned"
            } else {
                "scrolled up"
            },
            if view.selection().is_some_and(|s| !s.is_collapsed()) {
                ", selection active (Ctrl+C copies)"
            } else {
                ""
            },
        );
        view! {
            <div w={width} h={height} class="flex-col bg-[colors.background]">
                <div w={width} h={header_h}
                     class="shrink-0 px-3 items-center bg-[colors.panel] flex-row justify-between">
                    <text size={12.0} color={colors.text_muted}>{status}</text>
                    {?find}
                </div>
                <div w={width} h={body_h} class="shrink-0">
                    {element}
                    {?jump}
                </div>
            </div>
        }
    }

    fn update(&mut self, msg: Msg, cx: &mut UiContext) {
        match msg {
            Msg::Document(event) => self.chat.handle(event),
            Msg::JumpToLatest => self.chat.document_mut().scroll_to_bottom(),
            Msg::FindNext => {
                self.chat.find_next(ScrollAlign::Center);
            }
            Msg::FindPrev => {
                self.chat.find_prev(ScrollAlign::Center);
            }
            Msg::CloseFind => self.close_find(cx),
        }
        cx.window.request_redraw();
    }

    fn edit_text(&mut self, target: FocusId, command: TextEditCommand) -> TextEditOutcome {
        let Some(field) = self.find.as_mut().filter(|_| target == FIND_FIELD) else {
            return TextEditOutcome::default();
        };
        let now_ms = self.started.elapsed().as_millis() as u64;
        let outcome = field.apply_at(command, now_ms);
        self.chat.set_find_query(field.text());
        // Typing jumps to the first match, as browsers do.
        self.chat.reveal_current_match(ScrollAlign::Center);
        outcome
    }

    fn set_text_value(&mut self, target: FocusId, value: String, _cx: &mut UiContext) {
        if let Some(field) = self.find.as_mut().filter(|_| target == FIND_FIELD) {
            field.set_text(value);
            self.chat.set_find_query(field.text());
        }
    }

    fn event(&mut self, event: &InputEvent, cx: &mut UiContext) -> bool {
        let InputEvent::KeyPress(chord) = event else {
            return false;
        };
        if self.find.is_some() && cx.focus() == Some(FIND_FIELD) {
            match chord.named() {
                Some(NamedKey::Escape) => {
                    self.close_find(cx);
                    return true;
                }
                Some(NamedKey::Enter) => {
                    if chord.shift() {
                        self.chat.find_prev(ScrollAlign::Center);
                    } else {
                        self.chat.find_next(ScrollAlign::Center);
                    }
                    cx.window.request_redraw();
                    return true;
                }
                _ => {}
            }
        }
        if chord.named() == Some(NamedKey::Escape) {
            cx.window.exit();
            return true;
        }
        match chord.binding().as_ref().and_then(key_command) {
            Some(DocumentCommand::Copy) => {
                let copied = self.chat.selected_text();
                if !copied.is_empty() {
                    cx.window.set_clipboard_text(&copied);
                    if self.log {
                        eprintln!("copied {} bytes", copied.len());
                    }
                }
                true
            }
            Some(DocumentCommand::SelectAll) => {
                self.chat.document_mut().select_all();
                cx.window.request_redraw();
                true
            }
            Some(DocumentCommand::Find) => {
                self.find.get_or_insert_with(|| TextField::new(""));
                cx.set_focus(Some(FIND_FIELD));
                cx.window.request_redraw();
                true
            }
            None => false,
        }
    }
}

fn main() -> Result<(), quark_app::RunError> {
    quark_app::run_ui(
        Demo::new(),
        WindowOptions {
            title: "Chat".into(),
            size: (900.0, 700.0),
            ..WindowOptions::default()
        },
    )
}

#[cfg(test)]
mod tests {
    use quark_app::testing::{By, UiTestHarness};

    use super::*;

    /// The demo with `history` messages, no stream timer, and only the
    /// window measured, in a 600x400 window.
    fn chat(history: u64) -> UiTestHarness<Demo> {
        let mut demo = Demo::with_history(history);
        demo.idle = true;
        demo.sync = true;
        UiTestHarness::new(demo, (600.0, 400.0), 1.0)
    }

    fn jump_button() -> By {
        By::role_name(accesskit::Role::Button, "Jump to latest")
    }

    // Catches the chat's jump affordance not following the document's
    // content below: hidden while pinned, shown once an answer streams in
    // under a scrolled up view, and taking the view back down when clicked.
    #[test]
    fn jump_to_latest_shows_for_new_content_below_and_scrolls_back_down() {
        let mut ui = chat(40);
        let pinned = ui.try_find(jump_button()).is_some();
        ui.app_mut().chat.document_mut().set_scroll_offset(0.0);
        ui.send_message(StreamTick);
        ui.frame();
        let scrolled_up = ui.try_find(jump_button()).is_some();

        ui.click_node(jump_button());
        ui.frame();

        let after = (
            ui.try_find(jump_button()).is_some(),
            ui.app().chat.document().is_stuck_to_bottom(),
        );
        assert_eq!((pinned, scrolled_up, after), (false, true, (false, true)));
    }

    /// The document spans y=36..400 under the header; its vertical track
    /// runs 6 points in from either end.
    const TRACK_MIDDLE: f32 = (36.0 + 400.0) / 2.0;

    /// Center of the painted scrollbar thumb, at the window's right edge.
    fn thumb_center(ui: &UiTestHarness<Demo>) -> (f32, f32) {
        ui.scene()
            .primitives
            .iter()
            .find_map(|p| match p {
                // The track is the faint one.
                quark::scene::Primitive::RoundedRect(r) if r.rect.x > 590.0 && r.color.a != 10 => {
                    Some((
                        r.rect.x + r.rect.width / 2.0,
                        r.rect.y + r.rect.height / 2.0,
                    ))
                }
                _ => None,
            })
            .expect("a scrollbar thumb")
    }

    /// Message numbers (`#n:`) painted on screen.
    fn messages_on_screen(ui: &UiTestHarness<Demo>) -> Vec<u64> {
        let mut shown: Vec<u64> = ui
            .painted_texts()
            .iter()
            .filter_map(|t| t.text.strip_prefix('#')?.split(':').next()?.parse().ok())
            .collect();
        shown.sort_unstable();
        shown.dedup();
        shown
    }

    fn offset_fraction(ui: &UiTestHarness<Demo>) -> f32 {
        let doc = ui.app().chat.document();
        doc.scroll_offset() / doc.max_scroll_offset()
    }

    // Catches the transcript's thumb not dragging: grabbing it at the
    // bottom and moving its center to the track's middle scrolls to the
    // middle of the history, unpinning the view.
    #[test]
    fn dragging_the_thumb_to_the_middle_shows_the_middle_of_the_history() {
        let mut ui = chat(40);
        let before = messages_on_screen(&ui);
        let grab = thumb_center(&ui);
        ui.drag(grab, (grab.0, TRACK_MIDDLE));

        let after = messages_on_screen(&ui);
        let fraction = offset_fraction(&ui);
        // Rows measured on arrival replace estimated heights, which moves
        // the fraction a little after the drag lands.
        assert!((fraction - 0.5).abs() < 0.05, "scrolled to {fraction}");
        assert!(!ui.app().chat.document().is_stuck_to_bottom());
        assert!(before.contains(&39), "starts at the newest: {before:?}");
        assert!(
            !after.is_empty() && after.iter().all(|&n| (8..32).contains(&n)),
            "middle messages: {after:?}"
        );
    }

    // Catches track presses on the transcript's bar doing nothing or
    // moving by wheel lines: each press pages toward it, by the viewport
    // less two lines (364 - 40 points).
    #[test]
    fn pressing_the_transcript_track_pages_toward_the_press() {
        let mut ui = chat(40);
        ui.app_mut().chat.document_mut().set_scroll_offset(0.0);
        ui.frame();
        let got: Vec<f32> = [380.0, 380.0, 60.0]
            .into_iter()
            .map(|y| {
                ui.click((596.0, y));
                ui.app().chat.document().scroll_offset().round()
            })
            .collect();
        assert_eq!(got, [324.0, 648.0, 324.0]);
    }

    // Catches a thumb drag to the bottom leaving the view unpinned while
    // an answer streams: the content grows during the drag, and releasing
    // at the bottom still follows the stream afterwards.
    #[test]
    fn dragging_the_thumb_to_the_bottom_while_streaming_pins_the_view() {
        let mut ui = chat(40);
        ui.app_mut().chat.document_mut().set_scroll_offset(0.0);
        ui.send_message(StreamTick);
        ui.frame();
        let grab = thumb_center(&ui);
        ui.pointer_down(grab);
        ui.pointer_move((grab.0, TRACK_MIDDLE));
        for _ in 0..20 {
            ui.send_message(StreamTick);
        }
        ui.frame();
        ui.pointer_move((grab.0, 420.0));
        ui.pointer_up((grab.0, 420.0));
        let released = ui.app().chat.document().is_stuck_to_bottom();

        for _ in 0..20 {
            ui.send_message(StreamTick);
        }
        ui.frame();
        let doc = ui.app().chat.document();
        let following = (doc.max_scroll_offset() - doc.scroll_offset()).round();
        assert_eq!((released, following), (true, 0.0));
    }

    // Catches the chat chrome not reaching the screen: every message gets
    // its author line drawn above its first block.
    #[test]
    fn each_message_draws_its_author_line_above_its_text() {
        let mut ui = chat(4);
        ui.frame();

        let texts = ui.painted_texts();
        let author_above_text = |i: u64, author: &str| {
            let message = texts
                .iter()
                .find(|t| t.text.starts_with(&format!("#{i}:")))
                .map(|t| t.bounds.y);
            let line = texts
                .iter()
                .filter(|t| t.text == author)
                .map(|t| t.bounds.y)
                .filter(|y| message.is_some_and(|m| *y < m))
                .reduce(f32::max);
            line.zip(message).map(|(l, m)| m - l)
        };
        // The author line sits right above its message, within one line.
        let gaps: Vec<bool> = [(0, "You"), (1, "Assistant"), (2, "You"), (3, "Assistant")]
            .into_iter()
            .map(|(i, author)| author_above_text(i, author).is_some_and(|gap| gap <= AUTHOR_LINE))
            .collect();
        assert_eq!(gaps, [true; 4]);
    }
}
