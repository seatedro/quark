//! A chat transcript of 5,000 messages of varied length, with an assistant
//! message streaming in at the bottom. Drag to select across messages,
//! Ctrl/Cmd+C to copy, Ctrl/Cmd+A to select all, wheel to scroll. Scrolling
//! up while text streams shows "Jump to latest". Escape quits.
//!
//! Set `QUARK_TRANSCRIPT_LOG=1` to print view build times (prepare plus
//! element construction) every 120 frames.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use quark::selection::BlockKey;
use quark_app::quark_ui::element::{AnyElement, IntoAnyElement, StyledSpan, div, text};
use quark_app::quark_ui::style::Styled;
use quark_app::quark_ui::transcript::{
    TextMeasurer, Transcript, TranscriptBlock, TranscriptCommand, TranscriptEvent,
    TranscriptMessage, TranscriptRole, TranscriptStyle, key_command,
};
use quark_app::quark_ui::virtual_list::RowKey;
use quark_app::quark_ui::{Action, element::StyledSpan as Span};
use quark_app::winit::keyboard::NamedKey;
use quark_app::{InputEvent, UiApp, UiContext, ViewContext, WindowOptions};
use quark_render::{FontKind, FontWeight};

const HISTORY: u64 = 5_000;
const STREAM_TICK: Duration = Duration::from_millis(60);
const WORDS_PER_STREAM: usize = 160;
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

    fn sentence(&mut self, words: usize) -> String {
        let mut out = String::new();
        for i in 0..words {
            if i > 0 {
                out.push(' ');
            }
            out.push_str(WORDS[self.below(WORDS.len() as u64) as usize]);
        }
        out.push('.');
        out
    }
}

fn span(text: String, weight: FontWeight, italic: bool, kind: FontKind) -> StyledSpan {
    StyledSpan {
        font_weight: weight,
        italic,
        font_kind: kind,
        ..Span::plain(text)
    }
}

fn history_message(i: u64, rng: &mut Rng) -> TranscriptMessage {
    let user = i % 2 == 0;
    let mut blocks = Vec::new();
    let paragraphs = if user { 1 } else { 1 + rng.below(3) };
    for p in 0..paragraphs {
        let key = BlockKey(i * 16 + p);
        let words = 3 + rng.below(if user { 25 } else { 70 }) as usize;
        if rng.below(4) == 0 {
            blocks.push(TranscriptBlock::prose(
                key,
                vec![
                    span(rng.sentence(2), FontWeight::Bold, false, FontKind::Ui),
                    span(" ".into(), FontWeight::Normal, false, FontKind::Ui),
                    span(rng.sentence(words), FontWeight::Normal, false, FontKind::Ui),
                    span(" ".into(), FontWeight::Normal, false, FontKind::Ui),
                    span(
                        "measure_visible".into(),
                        FontWeight::Normal,
                        false,
                        FontKind::Mono,
                    ),
                    span(" ".into(), FontWeight::Normal, false, FontKind::Ui),
                    span(rng.sentence(4), FontWeight::Normal, true, FontKind::Ui),
                ],
            ));
        } else {
            blocks.push(TranscriptBlock::plain(
                key,
                format!("#{i}: {}", rng.sentence(words)),
            ));
        }
    }
    if !user && rng.below(6) == 0 {
        let lines = (0..2 + rng.below(5))
            .map(|n| {
                vec![Span::plain(format!(
                    "let row_{n} = list.measure_visible(width, overscan, measure);"
                ))]
            })
            .collect();
        blocks.push(TranscriptBlock::code(BlockKey(i * 16 + 8), lines));
    }
    TranscriptMessage {
        key: RowKey(i),
        role: if user {
            TranscriptRole::User
        } else {
            TranscriptRole::Assistant
        },
        author: if user { "You" } else { "Assistant" }.into(),
        blocks,
    }
}

/// The message currently streaming in.
struct Stream {
    key: RowKey,
    words: usize,
    text: String,
}

#[derive(Default)]
struct Timing {
    frames: u32,
    total: Duration,
    max: Duration,
}

struct Demo {
    messages: HashMap<RowKey, TranscriptMessage>,
    transcript: Transcript,
    rng: Rng,
    stream: Stream,
    next_key: u64,
    last_tick: Option<Duration>,
    timing: Timing,
    log: bool,
}

impl Demo {
    fn new() -> Self {
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        let history: Vec<TranscriptMessage> =
            (0..HISTORY).map(|i| history_message(i, &mut rng)).collect();
        let mut transcript = Transcript::new(TranscriptStyle::for_font_size(FONT_SIZE));
        transcript
            .extend(&history)
            .unwrap_or_else(|e| eprintln!("{e:?}"));
        let mut messages: HashMap<RowKey, TranscriptMessage> =
            history.into_iter().map(|m| (m.key, m)).collect();
        let stream = Self::start_stream(HISTORY, &mut messages, &mut transcript);
        Self {
            messages,
            transcript,
            rng,
            stream,
            next_key: HISTORY + 1,
            last_tick: None,
            timing: Timing::default(),
            log: std::env::var_os("QUARK_TRANSCRIPT_LOG").is_some(),
        }
    }

    fn start_stream(
        key: u64,
        messages: &mut HashMap<RowKey, TranscriptMessage>,
        transcript: &mut Transcript,
    ) -> Stream {
        let message = TranscriptMessage {
            key: RowKey(key),
            role: TranscriptRole::Assistant,
            author: "Assistant (streaming)".into(),
            blocks: vec![TranscriptBlock::plain(BlockKey(key * 16), "")],
        };
        if let Err(e) = transcript.push(&message) {
            eprintln!("{e:?}");
        }
        messages.insert(message.key, message);
        Stream {
            key: RowKey(key),
            words: 0,
            text: String::new(),
        }
    }

    /// Appends a word to the streaming message; a finished message is
    /// followed by a new one.
    fn tick(&mut self) {
        if self.stream.words >= WORDS_PER_STREAM {
            let key = self.next_key;
            self.next_key += 1;
            self.stream = Self::start_stream(key, &mut self.messages, &mut self.transcript);
        }
        let word = WORDS[self.rng.below(WORDS.len() as u64) as usize];
        if !self.stream.text.is_empty() {
            self.stream.text.push(' ');
        }
        self.stream.text.push_str(word);
        self.stream.words += 1;
        let Some(message) = self.messages.get_mut(&self.stream.key) else {
            return;
        };
        // A paragraph break every 60 words exercises block insertion.
        let paragraph = (self.stream.words - 1) / 60;
        let key = BlockKey(self.stream.key.0 * 16 + paragraph as u64);
        if self.stream.words % 60 == 1 && paragraph > 0 {
            self.stream.text = word.to_owned();
            message.blocks.push(TranscriptBlock::plain(key, ""));
        }
        if let Some(last) = message.blocks.last_mut() {
            *last = TranscriptBlock::plain(key, self.stream.text.clone());
        }
        if let Err(e) = self.transcript.update(message) {
            eprintln!("{e:?}");
        }
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

    fn view(&mut self, cx: &mut ViewContext) -> AnyElement {
        let (width, height) = cx.frame.size();
        let scale = cx.frame.scale_factor();
        let now = cx.frame.elapsed();

        let due = self.last_tick.is_none_or(|last| now >= last + STREAM_TICK);
        if due {
            self.last_tick = Some(now);
            self.tick();
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
