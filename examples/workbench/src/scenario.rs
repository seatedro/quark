//! Deterministic playback of the scripted agent run (`fixtures/run.json`)
//! and the demo clock.
//!
//! [`Scenario::start`] schedules a run's events at fixed offsets from the
//! time the prompt was accepted; [`Scenario::advance_to`] releases those
//! due by `now_ms`, in order. Replaying the same commands at the same times
//! produces the same events, however the time between them is sliced.
//! Live playback feeds the runner's elapsed time; tests and
//! `--manual-clock` feed their own.

use std::collections::VecDeque;

use serde::Deserialize;

use crate::contracts::{MessageId, RunGeneration, SCENARIO_ID_BASE, ThreadId};
use crate::model::{ToolCall, ToolKind, ToolStatus};

/// Display timestamp of every scenario row.
pub const SCENARIO_TIME: &str = "10:48";

#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub generation: RunGeneration,
    pub thread: ThreadId,
    /// Fixed display timestamp.
    pub at: String,
    pub kind: EventKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EventKind {
    /// Text appended to assistant row `row`, created on its first delta.
    TextDelta {
        row: MessageId,
        text: String,
    },
    ToolStart {
        row: MessageId,
        call: ToolCall,
    },
    ToolOutput {
        row: MessageId,
        text: String,
    },
    ToolDone {
        row: MessageId,
        status: ToolStatus,
        duration: String,
    },
    /// The run edited `path` in the fixture file store.
    FileEdit {
        path: String,
    },
    Done {
        summary: String,
    },
}

#[derive(Debug, Clone, Deserialize)]
pub struct Script {
    pub chunk_bytes: usize,
    pub chunk_ms: u64,
    pub steps: Vec<Step>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Step {
    Text {
        at: u64,
        text: String,
    },
    ToolStart {
        at: u64,
        tool: ToolKind,
        verb: String,
        target: String,
    },
    ToolOutput {
        at: u64,
        text: String,
    },
    ToolDone {
        at: u64,
        status: ToolStatus,
        duration: String,
    },
    FileEdit {
        at: u64,
        path: String,
    },
    Done {
        at: u64,
        summary: String,
    },
}

impl Step {
    fn at(&self) -> u64 {
        match self {
            Step::Text { at, .. }
            | Step::ToolStart { at, .. }
            | Step::ToolOutput { at, .. }
            | Step::ToolDone { at, .. }
            | Step::FileEdit { at, .. }
            | Step::Done { at, .. } => *at,
        }
    }
}

impl Script {
    pub fn parse(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }
}

/// Split `text` into pieces of at most `bytes` bytes (more only to finish
/// a character), never inside a UTF-8 sequence.
pub fn utf8_chunks(text: &str, bytes: usize) -> impl Iterator<Item = &str> {
    let bytes = bytes.max(1);
    let mut rest = text;
    std::iter::from_fn(move || {
        if rest.is_empty() {
            return None;
        }
        let mut end = bytes.min(rest.len());
        while !rest.is_char_boundary(end) {
            end += 1;
        }
        let (head, tail) = rest.split_at(end);
        rest = tail;
        Some(head)
    })
}

/// The scripted backend: queued events of the runs in flight.
#[derive(Debug)]
pub struct Scenario {
    script: Script,
    /// `(due_ms, event)`, ordered by due time then schedule order.
    queue: VecDeque<(u64, Event)>,
    next_id: u64,
}

impl Scenario {
    pub fn new(script: Script) -> Self {
        Self {
            script,
            queue: VecDeque::new(),
            next_id: SCENARIO_ID_BASE,
        }
    }

    /// A fresh ID for a scenario-created row (the prompt row, answers,
    /// tools). Deterministic: the same sequence of calls gives the same IDs.
    pub fn allocate_id(&mut self) -> MessageId {
        let id = MessageId(self.next_id);
        self.next_id += 1;
        id
    }

    /// Schedule the script for `thread`'s run `generation`, accepted at
    /// `now_ms`. Each step starts at its offset or when the previous step's
    /// text finished streaming, whichever is later.
    pub fn start(&mut self, thread: ThreadId, generation: RunGeneration, now_ms: u64) {
        let mut cursor = 0;
        let mut tool_row = None;
        let steps = self.script.steps.clone();
        for step in steps {
            let mut due = step.at().max(cursor);
            let push = |this: &mut Self, due: u64, kind| {
                this.queue.push_back((
                    now_ms + due,
                    Event {
                        generation,
                        thread,
                        at: SCENARIO_TIME.to_owned(),
                        kind,
                    },
                ));
            };
            match step {
                Step::Text { text, .. } => {
                    let row = self.allocate_id();
                    for chunk in utf8_chunks(&text, self.script.chunk_bytes) {
                        let kind = EventKind::TextDelta {
                            row,
                            text: chunk.to_owned(),
                        };
                        push(self, due, kind);
                        due += self.script.chunk_ms;
                    }
                }
                Step::ToolStart {
                    tool, verb, target, ..
                } => {
                    let row = self.allocate_id();
                    tool_row = Some(row);
                    let call = ToolCall {
                        kind: tool,
                        verb,
                        target,
                        status: ToolStatus::Running,
                        duration: None,
                        output: String::new(),
                    };
                    push(self, due, EventKind::ToolStart { row, call });
                }
                Step::ToolOutput { text, .. } => {
                    if let Some(row) = tool_row {
                        push(self, due, EventKind::ToolOutput { row, text });
                    }
                }
                Step::ToolDone {
                    status, duration, ..
                } => {
                    if let Some(row) = tool_row.take() {
                        let kind = EventKind::ToolDone {
                            row,
                            status,
                            duration,
                        };
                        push(self, due, kind);
                    }
                }
                Step::FileEdit { path, .. } => push(self, due, EventKind::FileEdit { path }),
                Step::Done { summary, .. } => push(self, due, EventKind::Done { summary }),
            }
            cursor = due;
        }
        // Several threads may run at once: keep one global order.
        self.queue.make_contiguous().sort_by_key(|(due, _)| *due);
    }

    /// Drop every queued event of `generation` (Stop, or a replaced run).
    pub fn cancel(&mut self, generation: RunGeneration) {
        self.queue.retain(|(_, e)| e.generation != generation);
    }

    /// When the next queued event is due, if any.
    pub fn next_due(&self) -> Option<u64> {
        self.queue.front().map(|(due, _)| *due)
    }

    pub fn is_idle(&self) -> bool {
        self.queue.is_empty()
    }

    /// Move every event due by `now_ms` into `out`, in order.
    pub fn advance_to(&mut self, now_ms: u64, out: &mut Vec<Event>) {
        while self.queue.front().is_some_and(|(due, _)| *due <= now_ms) {
            let (_, event) = self.queue.pop_front().expect("front exists");
            out.push(event);
        }
    }

    /// Forget every queued event and restart ID allocation.
    pub fn reset(&mut self) {
        self.queue.clear();
        self.next_id = SCENARIO_ID_BASE;
    }
}

/// The scenario clock: the runner's elapsed time, or under
/// `--manual-clock` a time that only "Advance demo step" moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DemoClock {
    manual: Option<u64>,
}

impl DemoClock {
    pub fn live() -> Self {
        Self { manual: None }
    }

    pub fn manual() -> Self {
        Self { manual: Some(0) }
    }

    pub fn is_manual(&self) -> bool {
        self.manual.is_some()
    }

    /// The scenario time at runner time `runner_ms`.
    pub fn now(&self, runner_ms: u64) -> u64 {
        self.manual.unwrap_or(runner_ms)
    }

    /// Manual clock: jump to `ms` (never backwards). No effect when live.
    pub fn set(&mut self, ms: u64) {
        if let Some(now) = &mut self.manual {
            *now = (*now).max(ms);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn script() -> Script {
        Script::parse(include_str!("../fixtures/run.json")).expect("run.json parses")
    }

    fn play(slices: &[u64]) -> Vec<Event> {
        let mut s = Scenario::new(script());
        s.start(ThreadId(1), RunGeneration(1), 1_000);
        let mut out = Vec::new();
        for &t in slices {
            s.advance_to(t, &mut out);
        }
        out
    }

    // Catches playback depending on frame timing: advancing in many small
    // steps or one big jump yields the same events in the same order.
    #[test]
    fn replay_is_independent_of_how_time_is_sliced() {
        let fine: Vec<u64> = (0..=200).map(|i| i * 50).collect();
        let coarse = play(&[10_000]);
        assert_eq!(play(&fine), coarse);
        assert!(matches!(
            coarse.last().map(|e| &e.kind),
            Some(EventKind::Done { .. })
        ));
    }

    // Catches streamed text splitting a multibyte character or losing
    // bytes: chunks are valid UTF-8 within the budget (plus the rest of a
    // character) and rejoin to the original.
    #[test]
    fn chunks_split_only_at_character_boundaries() {
        let text = "ЙЦУКЕН and 日本語入力 ✓";
        for bytes in 1..8 {
            let chunks: Vec<&str> = utf8_chunks(text, bytes).collect();
            assert_eq!(chunks.concat(), text, "bytes={bytes}");
            assert!(chunks.iter().all(|c| c.len() < bytes + 4), "bytes={bytes}");
        }
    }

    // Catches Stop leaving queued output behind: cancelling a generation
    // removes its events and nothing else's.
    #[test]
    fn cancel_drops_only_that_generation() {
        let mut s = Scenario::new(script());
        s.start(ThreadId(1), RunGeneration(1), 0);
        s.start(ThreadId(2), RunGeneration(2), 0);
        s.cancel(RunGeneration(1));
        let mut out = Vec::new();
        s.advance_to(u64::MAX, &mut out);
        assert!(!out.is_empty());
        assert!(out.iter().all(|e| e.generation == RunGeneration(2)));
    }
}
