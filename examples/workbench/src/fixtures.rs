//! Checked-in fixtures, compiled into the binary so tests and launches do
//! not depend on the working directory, plus the seeded stress generator.
//!
//! Schema of `fixtures/threads.json`:
//!
//! ```text
//! { "projects": [{ "id", "name", "path" }],
//!   "threads":  [{ "id", "project", "title", "pinned", "unread",
//!                  "status": "idle"|"running"|"failed", "updated",
//!                  "rows": [{ "id", "role": "user"|"assistant"|"tool"|"error",
//!                             "at", "markdown"?, "retry"?,
//!                             "tool"?: { "kind": "search"|"read"|"edit"|"run",
//!                                        "verb", "target",
//!                                        "status": "running"|"ok"|"failed",
//!                                        "duration"?, "output"? } }] }] }
//! ```

use serde::Deserialize;

use crate::contracts::{MessageId, Options, ProjectId, STRESS_ID_BASE, ScenarioKind, ThreadId};
use crate::model::{
    FileStore, Model, Project, Role, Row, Thread, ThreadStatus, ToolCall, ToolKind, ToolStatus,
    thread_from_rows,
};
use crate::scenario::Script;

pub const THREADS_JSON: &str = include_str!("../fixtures/threads.json");
pub const RUN_JSON: &str = include_str!("../fixtures/run.json");
pub const DIFF_PATCH: &str = include_str!("../fixtures/diff.patch");
pub const TERMINAL_ANSI: &[u8] = include_bytes!("../fixtures/terminal.ansi");

/// The Atlas project's files, as the Files panel and `cat` see them.
pub const FILES: &[(&str, &str)] = &[
    ("README.md", include_str!("../fixtures/files/README.md")),
    (
        "package.json",
        include_str!("../fixtures/files/package.json"),
    ),
    ("src/App.tsx", include_str!("../fixtures/files/src/App.tsx")),
    (
        "src/commands.ts",
        include_str!("../fixtures/files/src/commands.ts"),
    ),
    (
        "src/styles.css",
        include_str!("../fixtures/files/src/styles.css"),
    ),
    (
        "src/trips.ts",
        include_str!("../fixtures/files/src/trips.ts"),
    ),
];

/// The flagship thread ("Add keyboard shortcuts").
pub const REVIEW_THREAD: ThreadId = ThreadId(1);
/// The thread whose last run failed ("Fix flaky geocoder test").
pub const ERROR_THREAD: ThreadId = ThreadId(3);
/// Rows of the stress history and threads of the stress sidebar.
pub const STRESS_ROWS: usize = 50_000;
pub const STRESS_THREADS: usize = 2_000;

#[derive(Deserialize)]
struct ThreadsFile {
    projects: Vec<Project>,
    threads: Vec<ThreadFixture>,
}

#[derive(Deserialize)]
struct ThreadFixture {
    id: ThreadId,
    project: ProjectId,
    title: String,
    pinned: bool,
    unread: bool,
    status: ThreadStatus,
    updated: String,
    rows: Vec<Row>,
}

/// The model and run script for `options.scenario`. Stress history is
/// queued, not adopted: the caller loads it in batches (gap 5).
pub fn load(options: &Options) -> (Model, Script) {
    let file: ThreadsFile = serde_json::from_str(THREADS_JSON).expect("threads.json parses");
    let mut threads: Vec<Thread> = file
        .threads
        .into_iter()
        .map(|f| {
            let mut t = thread_from_rows(f.id, f.project, f.title, f.rows);
            t.pinned = f.pinned;
            t.unread = f.unread;
            t.status = f.status;
            t.updated = f.updated;
            t
        })
        .collect();
    if options.scenario == ScenarioKind::Stress {
        threads.extend(stress_threads(options.seed, STRESS_THREADS));
    }
    let files = FileStore::new(
        FILES
            .iter()
            .map(|(p, c)| ((*p).to_owned(), (*c).to_owned())),
    );
    let mut model = Model::new(file.projects, threads, files);
    match options.scenario {
        ScenarioKind::Review => {}
        ScenarioKind::Empty => {
            model.new_thread(ProjectId(1));
        }
        ScenarioKind::Error => {
            model.select(ERROR_THREAD);
        }
        ScenarioKind::Stress => {
            model.queue_history(REVIEW_THREAD, stress_history(options.seed, STRESS_ROWS));
        }
    }
    let script = Script::parse(RUN_JSON).expect("run.json parses");
    (model, script)
}

/// Deterministic xorshift64*.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    pub fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n.max(1)
    }

    pub fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[self.below(items.len() as u64) as usize]
    }
}

const WORDS: &[&str] = &[
    "shortcut",
    "command",
    "trip",
    "place",
    "layout",
    "render",
    "focus",
    "selection",
    "keyboard",
    "listener",
    "resolver",
    "fixture",
    "test",
    "binding",
    "modifier",
    "overlay",
    "panel",
    "map",
    "cache",
    "worker",
    "measure",
    "stream",
    "virtualized",
    "row",
    "anchor",
    "scroll",
    "pinned",
];

const CODE: &[&str] = &[
    "```ts\nexport function matches(trip: Trip, query: string): boolean {\n  return trip.name.toLowerCase().includes(query.trim().toLowerCase());\n}\n```",
    "```rust\nfn visible(range: Range<usize>, rows: &[Row]) -> &[Row] {\n    &rows[range.start.min(rows.len())..range.end.min(rows.len())]\n}\n```",
    "```json\n{ \"shortcut\": \"mod+n\", \"command\": \"trip.new\" }\n```",
];

fn sentence(rng: &mut Rng, words: usize) -> String {
    let mut out = String::new();
    for i in 0..words {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(rng.pick(WORDS));
    }
    out.push('.');
    out
}

/// `n` rows of older history for the flagship thread, oldest first, with
/// the fixture mix of prose, lists, code, and tool calls. Stable for a
/// given seed; row `i` is `STRESS_ID_BASE + i`.
pub fn stress_history(seed: u64, n: usize) -> Vec<Row> {
    let mut rng = Rng::new(seed);
    (0..n)
        .map(|i| {
            let id = MessageId(STRESS_ID_BASE + i as u64);
            let at = format!("{:02}:{:02}", 8 + (i / 60) % 2, i % 60);
            let mut row = Row {
                id,
                role: Role::Assistant,
                at,
                markdown: String::new(),
                tool: None,
                retry: false,
                streaming: false,
                rev: 0,
            };
            match i % 4 {
                0 => {
                    row.role = Role::User;
                    let words = 6 + rng.below(12) as usize;
                    row.markdown = format!("#{i}: {}", sentence(&mut rng, words));
                }
                1 | 2 => {
                    let words = 10 + rng.below(30) as usize;
                    let mut md = format!("#{i}: {}", sentence(&mut rng, words));
                    match rng.below(4) {
                        0 => md.push_str(&format!(
                            "\n\n- {}\n- {}\n  - {}",
                            sentence(&mut rng, 5),
                            sentence(&mut rng, 4),
                            sentence(&mut rng, 3)
                        )),
                        1 => {
                            md.push_str("\n\n");
                            md.push_str(rng.pick(CODE));
                        }
                        _ => {}
                    }
                    row.markdown = md;
                }
                _ => {
                    row.role = Role::Tool;
                    let failed = rng.below(16) == 0;
                    row.tool = Some(ToolCall {
                        kind: ToolKind::Run,
                        verb: "Ran".to_owned(),
                        target: format!("npm test -- {}", rng.pick(WORDS)),
                        status: if failed {
                            ToolStatus::Failed
                        } else {
                            ToolStatus::Ok
                        },
                        duration: Some(format!("{}.{}s", rng.below(3), rng.below(10))),
                        output: format!("#{i}: {} passed", 3 + rng.below(20)),
                    });
                }
            }
            row
        })
        .collect()
}

/// `n` extra sidebar threads spread over the three projects, each with an
/// empty transcript. IDs start at 10,000.
pub fn stress_threads(seed: u64, n: usize) -> Vec<Thread> {
    let mut rng = Rng::new(seed ^ 0x5eed);
    (0..n)
        .map(|i| {
            let id = ThreadId(10_000 + i as u32);
            let project = ProjectId(1 + (i % 3) as u32);
            let title = format!(
                "{} {} {}",
                rng.pick(&[
                    "Fix",
                    "Add",
                    "Refactor",
                    "Investigate",
                    "Document",
                    "Speed up"
                ]),
                rng.pick(WORDS),
                rng.pick(WORDS)
            );
            let mut t = thread_from_rows(id, project, title, Vec::new());
            t.updated = format!("{}d", 1 + i / 40);
            t.status = match rng.below(40) {
                0 => ThreadStatus::Failed,
                1 => ThreadStatus::Running,
                _ => ThreadStatus::Idle,
            };
            t.unread = rng.below(10) == 0;
            t
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Catches fixture drift: the checked-in world keeps the counts and the
    // content kinds the acceptance scenes rely on.
    #[test]
    fn fixtures_cover_the_required_content() {
        let (model, _) = load(&Options::default());
        let rows: Vec<&Row> = model
            .threads
            .iter()
            .flat_map(|t| t.transcript.rows())
            .collect();
        let any = |f: &dyn Fn(&Row) -> bool| rows.iter().any(|r| f(r));
        let counts = (model.projects.len(), model.threads.len(), rows.len());
        let kinds = [
            any(&|r| r.markdown.contains("\n|---|")),
            any(&|r| r.markdown.contains("](preview.png)")),
            any(&|r| r.markdown.contains("finalIdentifierOnWideLine")),
            any(&|r| r.markdown.contains("   - ")),
            any(&|r| !r.markdown.is_ascii()),
            any(&|r| {
                r.tool
                    .as_ref()
                    .is_some_and(|t| t.status == ToolStatus::Failed)
            }),
            any(&|r| r.role == Role::Error && r.retry),
        ];
        assert_eq!(counts, (3, 24, 120));
        assert_eq!(kinds, [true; 7]);
    }
}
