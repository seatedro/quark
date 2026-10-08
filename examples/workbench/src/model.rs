//! The app's one model: projects, threads, transcripts, active runs, and
//! the in-memory fixture file store. Only `app.rs` mutates it, by applying
//! effects and scenario events; surfaces read it through `SurfaceCx`.

use std::collections::{BTreeMap, HashMap};

use serde::Deserialize;

use crate::contracts::{MessageId, ProjectId, RunGeneration, ThreadId, ToolId};
use crate::scenario::{Event, EventKind};

#[derive(Debug, Clone, Deserialize)]
pub struct Project {
    pub id: ProjectId,
    pub name: String,
    /// Display path, never touched on disk.
    pub path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ThreadStatus {
    #[default]
    Idle,
    Running,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    Tool,
    /// A recoverable failure card; `Row::retry` offers Retry.
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolKind {
    Search,
    Read,
    Edit,
    Run,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolStatus {
    Running,
    Ok,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ToolCall {
    pub kind: ToolKind,
    /// Past-tense verb for the header ("Searched", "Ran").
    pub verb: String,
    /// What it acted on (`npm test`, `src/App.tsx`).
    pub target: String,
    pub status: ToolStatus,
    /// Fixed display duration ("0.4s"), absent while running.
    #[serde(default)]
    pub duration: Option<String>,
    /// Plain-text output, shown in a monospace block when expanded.
    #[serde(default)]
    pub output: String,
}

/// One transcript row. Prose rows carry markdown; tool rows carry a
/// [`ToolCall`] and an empty `markdown`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Row {
    pub id: MessageId,
    pub role: Role,
    /// Fixed display timestamp ("10:42").
    pub at: String,
    #[serde(default)]
    pub markdown: String,
    #[serde(default)]
    pub tool: Option<ToolCall>,
    /// Error rows: whether Retry is offered.
    #[serde(default)]
    pub retry: bool,
    /// Still receiving deltas from an active run.
    #[serde(skip)]
    pub streaming: bool,
    /// Bumped on every change to this row.
    #[serde(skip)]
    pub rev: u32,
}

impl Row {
    pub fn tool_id(&self) -> Option<ToolId> {
        self.tool.as_ref().map(|_| ToolId(self.id.0))
    }

    pub fn author(&self) -> &'static str {
        match self.role {
            Role::User => "You",
            Role::Assistant | Role::Tool => "Assistant",
            Role::Error => "Demo backend",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Appended,
    /// A batch of older history landed above the existing rows.
    Prepended,
    Updated,
}

/// One entry of a transcript's change log, for consumers (the timeline)
/// that mirror rows incrementally instead of diffing every frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RowChange {
    pub seq: u64,
    pub row: MessageId,
    pub kind: ChangeKind,
}

/// A thread's rows in display order, with a position index and change log.
#[derive(Debug, Default)]
pub struct Transcript {
    rows: Vec<Row>,
    index: HashMap<MessageId, usize>,
    log: Vec<RowChange>,
}

impl Transcript {
    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn get(&self, id: MessageId) -> Option<&Row> {
        self.index.get(&id).map(|&i| &self.rows[i])
    }

    /// Changes with `seq > after`, oldest first. A prepend logs one entry
    /// per row, in display order.
    pub fn changes_since(&self, after: u64) -> &[RowChange] {
        let start = self.log.partition_point(|c| c.seq <= after);
        &self.log[start..]
    }

    /// The newest change's sequence number, 0 when unchanged since load.
    pub fn last_seq(&self) -> u64 {
        self.log.last().map_or(0, |c| c.seq)
    }

    fn push(&mut self, mut row: Row, seq: &mut u64) {
        debug_assert!(!self.index.contains_key(&row.id), "duplicate {:?}", row.id);
        row.rev = 1;
        *seq += 1;
        self.log.push(RowChange {
            seq: *seq,
            row: row.id,
            kind: ChangeKind::Appended,
        });
        self.index.insert(row.id, self.rows.len());
        self.rows.push(row);
    }

    fn prepend(&mut self, batch: Vec<Row>, seq: &mut u64) {
        for row in &batch {
            *seq += 1;
            self.log.push(RowChange {
                seq: *seq,
                row: row.id,
                kind: ChangeKind::Prepended,
            });
        }
        let mut rows = batch;
        rows.append(&mut self.rows);
        self.rows = rows;
        self.index.clear();
        self.index
            .extend(self.rows.iter().enumerate().map(|(i, r)| (r.id, i)));
    }

    fn update(&mut self, id: MessageId, seq: &mut u64, f: impl FnOnce(&mut Row)) -> bool {
        let Some(&i) = self.index.get(&id) else {
            return false;
        };
        let row = &mut self.rows[i];
        f(row);
        row.rev += 1;
        *seq += 1;
        self.log.push(RowChange {
            seq: *seq,
            row: id,
            kind: ChangeKind::Updated,
        });
        true
    }
}

#[derive(Debug)]
pub struct Thread {
    pub id: ThreadId,
    pub project: ProjectId,
    pub title: String,
    pub pinned: bool,
    pub unread: bool,
    pub status: ThreadStatus,
    /// Fixed display age ("12m").
    pub updated: String,
    pub transcript: Transcript,
}

/// A run in progress on one thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Run {
    pub generation: RunGeneration,
    pub thread: ThreadId,
    /// The assistant row receiving text deltas, once the first arrives.
    pub answer: Option<MessageId>,
    /// The tool row receiving output, while a tool runs.
    pub tool: Option<MessageId>,
}

/// The fixture project's files, edited in memory only.
#[derive(Debug, Default)]
pub struct FileStore {
    files: BTreeMap<String, String>,
    /// Contents before the last applied diff, for Undo.
    undo: Option<Vec<(String, String)>>,
    /// Paths a run or an applied diff changed, for badges.
    changed: Vec<String>,
}

impl FileStore {
    pub fn new(files: impl IntoIterator<Item = (String, String)>) -> Self {
        Self {
            files: files.into_iter().collect(),
            undo: None,
            changed: Vec::new(),
        }
    }

    pub fn get(&self, path: &str) -> Option<&str> {
        self.files.get(path).map(String::as_str)
    }

    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.files.keys().map(String::as_str)
    }

    pub fn changed(&self) -> &[String] {
        &self.changed
    }

    pub fn can_undo(&self) -> bool {
        self.undo.is_some()
    }

    /// Apply every file of `patch` to the store. All or nothing: a file
    /// that does not match leaves the store unchanged and names it.
    pub fn apply(&mut self, patch: &quark_diff::DiffDocument) -> Result<(), String> {
        let mut next = Vec::new();
        for file in 0..patch.file_count() {
            let path = patch.path(file).to_owned();
            let old = self.files.get(&path).ok_or_else(|| path.clone())?;
            let new = quark_diff::apply(patch, file, old).map_err(|e| format!("{path}: {e}"))?;
            next.push((path, new));
        }
        let mut undo = Vec::with_capacity(next.len());
        for (path, new) in next {
            let old = self.files.insert(path.clone(), new).unwrap_or_default();
            if !self.changed.contains(&path) {
                self.changed.push(path.clone());
            }
            undo.push((path, old));
        }
        self.undo = Some(undo);
        Ok(())
    }

    /// Revert the last [`Self::apply`]. Returns false when there is none.
    pub fn undo(&mut self) -> bool {
        let Some(undo) = self.undo.take() else {
            return false;
        };
        for (path, old) in undo {
            self.changed.retain(|p| *p != path);
            self.files.insert(path, old);
        }
        true
    }

    fn mark_changed(&mut self, path: &str) {
        if !self.changed.iter().any(|p| p == path) {
            self.changed.push(path.to_owned());
        }
    }
}

#[derive(Debug, Default)]
pub struct Model {
    pub projects: Vec<Project>,
    /// Every thread, in fixture order (the sidebar sorts and filters).
    pub threads: Vec<Thread>,
    pub selected: ThreadId,
    pub runs: HashMap<ThreadId, Run>,
    pub files: FileStore,
    /// Shown in the title bar: the backend is simulated.
    pub workspace_label: &'static str,
    /// Stress history not yet adopted into the selected thread (gap 5:
    /// the visible tail loads first, older batches follow).
    pending_history: Vec<Row>,
    next_generation: u32,
    next_thread: u32,
    seq: u64,
}

/// Rows adopted per history batch.
pub const HISTORY_BATCH: usize = 2_000;

impl Model {
    pub fn new(projects: Vec<Project>, threads: Vec<Thread>, files: FileStore) -> Self {
        let selected = threads.first().map_or(ThreadId(0), |t| t.id);
        let next_thread = threads.iter().map(|t| t.id.0).max().unwrap_or(0) + 1;
        Self {
            projects,
            threads,
            selected,
            runs: HashMap::new(),
            files,
            workspace_label: "Demo workspace",
            pending_history: Vec::new(),
            next_generation: 1,
            next_thread,
            seq: 0,
        }
    }

    pub fn thread(&self, id: ThreadId) -> Option<&Thread> {
        self.threads.iter().find(|t| t.id == id)
    }

    fn thread_mut(&mut self, id: ThreadId) -> Option<&mut Thread> {
        self.threads.iter_mut().find(|t| t.id == id)
    }

    pub fn selected_thread(&self) -> Option<&Thread> {
        self.thread(self.selected)
    }

    pub fn project(&self, id: ProjectId) -> Option<&Project> {
        self.projects.iter().find(|p| p.id == id)
    }

    pub fn run(&self, thread: ThreadId) -> Option<&Run> {
        self.runs.get(&thread)
    }

    /// Select `thread`, clearing its unread dot. False for unknown IDs.
    pub fn select(&mut self, thread: ThreadId) -> bool {
        let Some(t) = self.thread_mut(thread) else {
            return false;
        };
        t.unread = false;
        self.selected = thread;
        true
    }

    /// A new empty thread in `project`, selected.
    pub fn new_thread(&mut self, project: ProjectId) -> ThreadId {
        let id = ThreadId(self.next_thread);
        self.next_thread += 1;
        self.threads.insert(
            0,
            Thread {
                id,
                project,
                title: "New thread".to_owned(),
                pinned: false,
                unread: false,
                status: ThreadStatus::Idle,
                updated: "now".to_owned(),
                transcript: Transcript::default(),
            },
        );
        self.selected = id;
        id
    }

    /// Start a run on `thread`, replacing (and so silencing) any earlier
    /// one. The user's turn is appended as `prompt_row`.
    pub fn begin_run(
        &mut self,
        thread: ThreadId,
        prompt_row: MessageId,
        text: &str,
        at: &str,
    ) -> Option<RunGeneration> {
        let generation = RunGeneration(self.next_generation);
        let seq = &mut self.seq;
        let t = self.threads.iter_mut().find(|t| t.id == thread)?;
        self.next_generation += 1;
        t.status = ThreadStatus::Running;
        t.updated = "now".to_owned();
        if t.title == "New thread" {
            t.title = text.lines().next().unwrap_or("").chars().take(48).collect();
        }
        t.transcript.push(
            Row {
                id: prompt_row,
                role: Role::User,
                at: at.to_owned(),
                markdown: text.to_owned(),
                tool: None,
                retry: false,
                streaming: false,
                rev: 0,
            },
            seq,
        );
        self.runs.insert(
            thread,
            Run {
                generation,
                thread,
                answer: None,
                tool: None,
            },
        );
        Some(generation)
    }

    /// Stop `thread`'s run: its streaming rows finish where they are and
    /// any later event from its generation is dropped.
    pub fn stop_run(&mut self, thread: ThreadId) -> Option<RunGeneration> {
        let run = self.runs.remove(&thread)?;
        let seq = &mut self.seq;
        if let Some(t) = self.threads.iter_mut().find(|t| t.id == thread) {
            t.status = ThreadStatus::Idle;
            for id in [run.answer, run.tool].into_iter().flatten() {
                t.transcript.update(id, seq, |row| {
                    row.streaming = false;
                    if let Some(tool) = &mut row.tool
                        && tool.status == ToolStatus::Running
                    {
                        tool.status = ToolStatus::Failed;
                        tool.output.push_str("\nStopped.");
                    }
                });
            }
        }
        Some(run.generation)
    }

    /// Apply one scenario event. Returns false, changing nothing, for an
    /// event from a run that is no longer the thread's active one.
    pub fn apply(&mut self, event: &Event) -> bool {
        let Some(run) = self.runs.get(&event.thread).copied() else {
            return false;
        };
        if run.generation != event.generation {
            return false;
        }
        let seq = &mut self.seq;
        let Some(t) = self.threads.iter_mut().find(|t| t.id == event.thread) else {
            return false;
        };
        let transcript = &mut t.transcript;
        match &event.kind {
            EventKind::TextDelta { row, text } => {
                if run.answer != Some(*row) {
                    transcript.push(
                        Row {
                            id: *row,
                            role: Role::Assistant,
                            at: event.at.clone(),
                            markdown: String::new(),
                            tool: None,
                            retry: false,
                            streaming: true,
                            rev: 0,
                        },
                        seq,
                    );
                    self.runs.get_mut(&event.thread).expect("run").answer = Some(*row);
                }
                transcript.update(*row, seq, |r| r.markdown.push_str(text));
            }
            EventKind::ToolStart { row, call } => {
                // A tool ends the prose segment before it.
                if let Some(answer) = run.answer {
                    transcript.update(answer, seq, |r| r.streaming = false);
                }
                transcript.push(
                    Row {
                        id: *row,
                        role: Role::Tool,
                        at: event.at.clone(),
                        markdown: String::new(),
                        tool: Some(call.clone()),
                        retry: false,
                        streaming: true,
                        rev: 0,
                    },
                    seq,
                );
                let r = self.runs.get_mut(&event.thread).expect("run");
                r.answer = None;
                r.tool = Some(*row);
            }
            EventKind::ToolOutput { row, text } => {
                transcript.update(*row, seq, |r| {
                    if let Some(tool) = &mut r.tool {
                        tool.output.push_str(text);
                    }
                });
            }
            EventKind::ToolDone {
                row,
                status,
                duration,
            } => {
                transcript.update(*row, seq, |r| {
                    r.streaming = false;
                    if let Some(tool) = &mut r.tool {
                        tool.status = *status;
                        tool.duration = Some(duration.clone());
                    }
                });
                self.runs.get_mut(&event.thread).expect("run").tool = None;
            }
            EventKind::FileEdit { path } => self.files.mark_changed(path),
            EventKind::Done { .. } => {
                if let Some(answer) = run.answer {
                    transcript.update(answer, seq, |r| r.streaming = false);
                }
                t.status = ThreadStatus::Idle;
                if t.id != self.selected {
                    t.unread = true;
                }
                self.runs.remove(&event.thread);
            }
        }
        true
    }

    /// Queue `rows` (oldest first) as history of `thread` still to load,
    /// and adopt the newest batch now.
    pub fn queue_history(&mut self, thread: ThreadId, rows: Vec<Row>) {
        debug_assert_eq!(
            thread, self.selected,
            "history loads into the selected thread"
        );
        self.pending_history = rows;
        self.load_history_batch(thread);
    }

    /// Prepend the next (newer-first) batch of queued history. Returns the
    /// rows still queued afterwards.
    pub fn load_history_batch(&mut self, thread: ThreadId) -> usize {
        let take = self.pending_history.len().min(HISTORY_BATCH);
        let batch = self
            .pending_history
            .split_off(self.pending_history.len() - take);
        let seq = &mut self.seq;
        if let Some(t) = self.threads.iter_mut().find(|t| t.id == thread) {
            t.transcript.prepend(batch, seq);
        }
        self.pending_history.len()
    }

    pub fn history_pending(&self) -> usize {
        self.pending_history.len()
    }
}

/// Build a thread from fixture parts (fixtures and the stress generator).
pub fn thread_from_rows(id: ThreadId, project: ProjectId, title: String, rows: Vec<Row>) -> Thread {
    let mut transcript = Transcript::default();
    let mut seq = 0;
    for row in rows {
        transcript.push(row, &mut seq);
    }
    // Fixture rows are the starting state, not changes.
    transcript.log.clear();
    Thread {
        id,
        project,
        title,
        pinned: false,
        unread: false,
        status: ThreadStatus::Idle,
        updated: String::new(),
        transcript,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: u64) -> Row {
        Row {
            id: MessageId(id),
            role: Role::Assistant,
            at: String::new(),
            markdown: format!("row {id}"),
            tool: None,
            retry: false,
            streaming: false,
            rev: 0,
        }
    }

    // Catches the change log or index drifting after a prepend: rows come
    // back in display order, lookups find moved rows, and only the prepend
    // and the later update are reported.
    #[test]
    fn prepended_history_keeps_order_index_and_change_log() {
        let mut t = Transcript::default();
        let mut seq = 0;
        t.push(row(10), &mut seq);
        let before = t.last_seq();
        t.prepend(vec![row(1), row(2)], &mut seq);
        t.update(MessageId(10), &mut seq, |r| r.markdown.push('!'));

        let order: Vec<u64> = t.rows().iter().map(|r| r.id.0).collect();
        let changes: Vec<(u64, ChangeKind)> = t
            .changes_since(before)
            .iter()
            .map(|c| (c.row.0, c.kind))
            .collect();
        assert_eq!(order, [1, 2, 10]);
        assert_eq!(
            t.get(MessageId(10)).map(|r| r.markdown.as_str()),
            Some("row 10!")
        );
        assert_eq!(
            changes,
            [
                (1, ChangeKind::Prepended),
                (2, ChangeKind::Prepended),
                (10, ChangeKind::Updated)
            ]
        );
    }
}
