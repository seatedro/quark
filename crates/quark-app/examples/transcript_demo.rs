//! A chat transcript of 5,000 markdown messages of varied length, with an
//! assistant answer streaming in at the bottom: headings, lists, quotes,
//! tables, and fenced code blocks, each its own selectable block. Drag to
//! select across messages (list markers and heading hashes survive the
//! copy), Ctrl/Cmd+C to copy, Ctrl/Cmd+A to select all, wheel to scroll.
//! Scrolling up while text streams shows "Jump to latest". Escape quits.
//!
//! Build with `--features syntax` for highlighted code blocks.
//!
//! Set `QUARK_TRANSCRIPT_LOG=1` to print view build times (prepare plus
//! element construction) every 120 frames.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use quark_app::quark_ui::Action;
use quark_app::quark_ui::element::{AnyElement, IntoAnyElement, div, text};
use quark_app::quark_ui::markdown::MarkdownDoc;
use quark_app::quark_ui::style::Styled;
use quark_app::quark_ui::theme::Theme;
use quark_app::quark_ui::transcript::{
    MarkdownMessage, SyntaxHighlighter, TextMeasurer, Transcript, TranscriptCommand,
    TranscriptEvent, TranscriptMessage, TranscriptRole, TranscriptStyle, key_command,
    markdown_block_row,
};
use quark_app::quark_ui::virtual_list::RowKey;
use quark_app::winit::keyboard::NamedKey;
use quark_app::{InputEvent, UiApp, UiContext, ViewContext, WindowOptions};

const HISTORY: u64 = 5_000;
const STREAM_TICK: Duration = Duration::from_millis(60);
/// Bytes of the scripted answer revealed per tick.
const STREAM_CHUNK: usize = 7;
const FONT_SIZE: f32 = 14.0;
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
        "# Run the demo with highlighted code blocks.\ncargo run --release -p quark-app --example transcript_demo --features syntax",
    ),
    (
        "go",
        "func copyText(blocks []string) string {\n\treturn strings.Join(blocks, \"\\n\\n\")\n}",
    ),
    ("klingon", "qapla' batlh Daqawlu'taH"),
];

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Transcript(TranscriptEvent),
}

impl From<Msg> for Action {
    fn from(msg: Msg) -> Self {
        Action::new(msg)
    }
}

/// Deterministic xorshift so every run shows the same transcript.
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
        "## Streaming answer {n}\n\n{}\n\n1. Rows are measured only inside the window.\n2. Selection names blocks by key, so it survives scrolling.\n   - nested markers copy too\n\n> Copying keeps `- `, `1. `, and `## ` prefixes.\n\n{}\n\n---\n\n{}",
        rng.sentence(24),
        fence(n),
        rng.sentence(30),
    )
}

/// A message's markdown source and its converted blocks.
struct Source {
    role: TranscriptRole,
    author: &'static str,
    markdown: String,
    blocks: MarkdownMessage,
}

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

struct Demo {
    sources: HashMap<RowKey, Source>,
    messages: HashMap<RowKey, TranscriptMessage>,
    transcript: Transcript,
    syntax: SyntaxHighlighter,
    rng: Rng,
    stream: Option<Stream>,
    answers: usize,
    next_key: u64,
    last_tick: Option<Duration>,
    timing: Timing,
    log: bool,
}

impl Demo {
    fn new() -> Self {
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        let sources = (0..HISTORY)
            .map(|i| {
                let key = RowKey(i);
                let user = i.is_multiple_of(2);
                let source = Source {
                    role: if user {
                        TranscriptRole::User
                    } else {
                        TranscriptRole::Assistant
                    },
                    author: if user { "You" } else { "Assistant" },
                    markdown: history_markdown(i, &mut rng),
                    blocks: MarkdownMessage::new(key),
                };
                (key, source)
            })
            .collect();
        Self {
            sources,
            messages: HashMap::new(),
            transcript: Transcript::new(TranscriptStyle::for_font_size(FONT_SIZE)),
            syntax: SyntaxHighlighter::new(),
            rng,
            stream: None,
            answers: 0,
            next_key: HISTORY,
            last_tick: None,
            timing: Timing::default(),
            log: std::env::var_os("QUARK_TRANSCRIPT_LOG").is_some(),
        }
    }

    /// Converts a message's markdown into transcript blocks.
    fn convert(&mut self, key: RowKey, theme: &Theme) -> Option<TranscriptMessage> {
        let source = self.sources.get_mut(&key)?;
        let doc = MarkdownDoc::parse(&source.markdown);
        Some(TranscriptMessage {
            key,
            role: source.role,
            author: source.author.into(),
            blocks: source.blocks.blocks(&doc, theme, &mut self.syntax),
        })
    }

    /// Builds the history on the first frame, when the theme is known.
    fn load_history(&mut self, theme: &Theme) {
        let history: Vec<TranscriptMessage> = (0..HISTORY)
            .filter_map(|i| self.convert(RowKey(i), theme))
            .collect();
        self.transcript
            .extend(&history)
            .unwrap_or_else(|e| eprintln!("{e:?}"));
        self.messages = history.into_iter().map(|m| (m.key, m)).collect();
    }

    /// Re-converts a message after its markdown or highlights changed.
    fn refresh(&mut self, key: RowKey, theme: &Theme) {
        let Some(message) = self.convert(key, theme) else {
            return;
        };
        let result = if self.messages.contains_key(&key) {
            self.transcript.update(&message)
        } else {
            self.transcript.push(&message)
        };
        if let Err(e) = result {
            eprintln!("{e:?}");
        }
        self.messages.insert(key, message);
    }

    /// Reveals the next chunk of the streaming answer; a finished answer
    /// is followed by a new one.
    fn tick(&mut self, theme: &Theme) {
        if self
            .stream
            .as_ref()
            .is_none_or(|s| s.shown >= s.script.len())
        {
            let key = RowKey(self.next_key);
            self.next_key += 1;
            let script = answer_markdown(self.answers, &mut self.rng);
            self.answers += 1;
            self.sources.insert(
                key,
                Source {
                    role: TranscriptRole::Assistant,
                    author: "Assistant (streaming)",
                    markdown: String::new(),
                    blocks: MarkdownMessage::new(key),
                },
            );
            self.stream = Some(Stream {
                key,
                script,
                shown: 0,
            });
        }
        let Some(stream) = &mut self.stream else {
            return;
        };
        let mut end = (stream.shown + STREAM_CHUNK).min(stream.script.len());
        while !stream.script.is_char_boundary(end) {
            end += 1;
        }
        stream.shown = end;
        let key = stream.key;
        if let Some(source) = self.sources.get_mut(&key) {
            source.markdown = stream.script[..end].to_owned();
        }
        self.refresh(key, theme);
    }

    fn record(&mut self, elapsed: Duration) {
        let t = &mut self.timing;
        t.frames += 1;
        t.total += elapsed;
        t.max = t.max.max(elapsed);
        if self.log && t.frames == 120 {
            eprintln!(
                "transcript view: avg {:.3} ms, max {:.3} ms over {} frames ({} rows, {} materialized)",
                t.total.as_secs_f64() * 1e3 / f64::from(t.frames),
                t.max.as_secs_f64() * 1e3,
                t.frames,
                self.transcript.len(),
                self.transcript.visible_rows().len(),
            );
            *t = Timing::default();
        }
    }
}

impl UiApp for Demo {
    type Action = Msg;
    type Message = ();

    fn view(&mut self, cx: &mut ViewContext) -> AnyElement {
        let (width, height) = cx.frame.size();
        let scale = cx.frame.scale_factor();
        let now = cx.frame.elapsed();

        if self.messages.is_empty() {
            self.load_history(cx.theme);
        }
        let due = self.last_tick.is_none_or(|last| now >= last + STREAM_TICK);
        if due {
            self.last_tick = Some(now);
            self.tick(cx.theme);
        }
        // Rebuild the messages whose code highlights arrived.
        let mut rows: Vec<RowKey> = self
            .syntax
            .poll()
            .into_iter()
            .map(markdown_block_row)
            .collect();
        rows.dedup();
        for row in rows {
            self.refresh(row, cx.theme);
        }

        let started = Instant::now();
        let header_h = 36.0;
        let body_h = (height - header_h).max(0.0);
        let font_size = self.transcript.style().font_size;
        let text_cx = cx.frame.text();
        let mut measurer =
            TextMeasurer::new(&mut text_cx.system, &mut text_cx.layouts, font_size, scale);
        self.transcript.prepare(
            width,
            body_h,
            now.as_millis() as u64,
            &self.messages,
            &mut measurer,
        );
        let element = self
            .transcript
            .element(&self.messages, cx.theme, |event| {
                Msg::Transcript(event).into()
            })
            .label("Conversation");
        self.record(started.elapsed());

        // The element schedules its own frames while a drag autoscrolls.
        cx.frame.request_frame_in(STREAM_TICK);

        let colors = &cx.theme.colors;
        let status = format!(
            "{} messages, {} materialized, {}{}",
            self.transcript.len(),
            self.transcript.visible_rows().len(),
            if self.transcript.is_stuck_to_bottom() {
                "pinned"
            } else {
                "scrolled up"
            },
            if self
                .transcript
                .selection()
                .is_some_and(|s| !s.is_collapsed())
            {
                ", selection active (Ctrl+C copies)"
            } else {
                ""
            },
        );
        div()
            .w(width)
            .h(height)
            .flex_col()
            .bg(colors.background)
            .child(
                div()
                    .w(width)
                    .h(header_h)
                    .flex_shrink_0()
                    .px(12.0)
                    .items_center()
                    .bg(colors.panel)
                    .child(text(status).size(12.0).color(colors.text_muted)),
            )
            .child(element)
            .into_any()
    }

    fn update(&mut self, msg: Msg, cx: &mut UiContext) {
        match msg {
            Msg::Transcript(event) => self.transcript.handle(event),
        }
        cx.window.request_redraw();
    }

    fn event(&mut self, event: &InputEvent, cx: &mut UiContext) -> bool {
        let InputEvent::KeyPress(chord) = event else {
            return false;
        };
        if chord.named() == Some(NamedKey::Escape) {
            cx.window.exit();
            return true;
        }
        match chord.binding_string().as_deref().and_then(key_command) {
            Some(TranscriptCommand::Copy) => {
                let copied = self.transcript.selected_text(&self.messages);
                if !copied.is_empty() {
                    cx.window.set_clipboard_text(&copied);
                    if self.log {
                        eprintln!("copied {} bytes", copied.len());
                    }
                }
                true
            }
            Some(TranscriptCommand::SelectAll) => {
                self.transcript.select_all();
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
            title: "Transcript".into(),
            size: (900.0, 700.0),
            ..WindowOptions::default()
        },
    )
}
