//! Deterministic fake data, mirroring what the live captures show: the
//! signed-in account, the projects and threads in the sidebar, the
//! cart.js demo project with its uncommitted edit, and the one thread the
//! captures ran.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ThreadId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProjectId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Idle,
    Cloud,
    Error,
    Running,
}

#[derive(Debug, Clone)]
pub struct Thread {
    pub id: ThreadId,
    pub project: ProjectId,
    pub title: String,
    pub age: &'static str,
    pub status: Status,
    pub cloud: bool,
    pub items: Vec<Item>,
}

#[derive(Debug, Clone)]
pub struct Project {
    pub id: ProjectId,
    pub name: &'static str,
}

/// One entry in a thread's transcript.
#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    User {
        text: String,
        time: &'static str,
    },
    /// The shimmering "Thinking" line of a running turn.
    Thinking,
    Error(String),
    /// "Model changed from A to B." between turns.
    ModelChanged {
        from: &'static str,
        to: &'static str,
    },
    Assistant(String),
    /// "Ran ..." / "Explored ..." one-line tool rows.
    Tool {
        verb: &'static str,
        detail: String,
    },
    /// A command card with output.
    Command {
        command: String,
        output: String,
        exit: i32,
        secs: u32,
    },
    /// "Worked for 1m 12s" divider.
    Worked(String),
    /// "N files changed" card.
    FilesChanged(Vec<(String, u32, u32)>),
    /// A pending command approval prompt.
    Approval {
        command: String,
        reason: String,
    },
}

pub const ACCOUNT_NAME: &str = "seatedro";
pub const ACCOUNT_PLAN: &str = "Promax";
pub const ACCOUNT_EMAIL: &str = "rohit@example.com";
pub const DEMO_PROJECT: &str = "codex-demo";
pub const DEMO_PROMPT: &str = "Read the repo and make the failing tests pass.";

pub struct Data {
    pub projects: Vec<Project>,
    pub threads: Vec<Thread>,
}

fn t(id: u32, project: u32, title: &str, age: &'static str, status: Status) -> Thread {
    Thread {
        id: ThreadId(id),
        project: ProjectId(project),
        title: title.to_owned(),
        age,
        status,
        cloud: true,
        items: Vec::new(),
    }
}

/// The demo thread's transcript in full: four tries, each failing with
/// the account's "model not supported" error. Scenes show a prefix.
pub fn demo_items() -> Vec<Item> {
    let user = || Item::User {
        text: DEMO_PROMPT.to_owned(),
        time: "10:25 PM",
    };
    let err = |m: &str| {
        Item::Error(format!(
            "The '{m}' model is not supported when using Codex with a ChatGPT account."
        ))
    };
    vec![
        user(),
        err("gpt-5.4"),
        Item::ModelChanged {
            from: "GPT-5.4",
            to: "GPT-5.3-Codex",
        },
        user(),
        err("gpt-5.3-codex"),
        Item::ModelChanged {
            from: "GPT-5.3-Codex",
            to: "GPT-5.4-Mini",
        },
        user(),
        err("gpt-5.4-mini"),
        Item::ModelChanged {
            from: "GPT-5.4-Mini",
            to: "GPT-5.2",
        },
        user(),
        err("gpt-5.2"),
    ]
}

/// A successful agent turn on cart.js, as the public material shows one:
/// tool rows, a command, an approval, prose, "Worked for", and the files
/// changed card.
pub fn agent_turn_items() -> Vec<Item> {
    vec![
        Item::User {
            text: "Fix applyDiscount so the cart tests pass.".to_owned(),
            time: "10:31 PM",
        },
        Item::Tool {
            verb: "Explored",
            detail: "3 files, 1 search".to_owned(),
        },
        Item::Command {
            command: "npm test".to_owned(),
            output: "✖ applyDiscount takes a percentage off\n  expected 90, got 0\nℹ tests 2  pass 1  fail 1".to_owned(),
            exit: 1,
            secs: 1,
        },
        Item::Assistant(
            "applyDiscount subtracts the percent itself instead of that share of the total. \
             I'll compute the discount from the total and keep subtotal's quantity default."
                .to_owned(),
        ),
        Item::Tool {
            verb: "Edited",
            detail: "cart.js +2 -2".to_owned(),
        },
        Item::Command {
            command: "npm test".to_owned(),
            output: "✔ subtotal multiplies price by quantity\n✔ applyDiscount takes a percentage off\nℹ tests 2  pass 2  fail 0".to_owned(),
            exit: 0,
            secs: 1,
        },
        Item::Worked("1m 12s".to_owned()),
        Item::Assistant(
            "Both tests pass now. `applyDiscount` returns `total - (total * percent) / 100`, and \
             `subtotal` treats a missing `qty` as 1."
                .to_owned(),
        ),
        Item::FilesChanged(vec![("cart.js".to_owned(), 2, 2)]),
        Item::Approval {
            command: "git commit -am \"Fix cart discount\"".to_owned(),
            reason: "Codex wants to run a command outside the sandbox.".to_owned(),
        },
    ]
}

impl Data {
    pub fn new() -> Self {
        let projects = ["codex-demo", "v4", "shiori", "intent", "ember", "glimpse"]
            .iter()
            .enumerate()
            .map(|(i, name)| Project {
                id: ProjectId(i as u32 + 1),
                name,
            })
            .collect();
        let mut demo = t(1, 1, DEMO_PROMPT, "1m", Status::Error);
        demo.cloud = false;
        demo.items = demo_items()[..5].to_vec();
        let mut threads = vec![
            demo,
            t(
                10,
                2,
                "Rename log to reading list in cmdk menu",
                "7mo",
                Status::Cloud,
            ),
            t(
                11,
                2,
                "Add custom scrollbar with headings on hover",
                "1y",
                Status::Cloud,
            ),
            t(
                20,
                3,
                "Build cross-platform ebook reader with Qt",
                "7mo",
                Status::Cloud,
            ),
            t(
                21,
                3,
                "Create cross-platform ebook reader with EPUB support",
                "1y",
                Status::Cloud,
            ),
            t(
                22,
                3,
                "Create cross-platform ebook reader with epub support",
                "1y",
                Status::Error,
            ),
            t(
                30,
                4,
                "Design self-improving intent layer",
                "7mo",
                Status::Cloud,
            ),
            t(
                40,
                5,
                "Compare rendering practices with Bevy and Ember",
                "1y",
                Status::Cloud,
            ),
            t(41, 5, "Try compiling with zig build", "1y", Status::Error),
            t(
                42,
                5,
                "Create README and LICENSE files",
                "1y",
                Status::Cloud,
            ),
            t(
                43,
                5,
                "Identify priority subsystems for game engine",
                "1y",
                Status::Cloud,
            ),
            t(
                44,
                5,
                "Propose tasks for code fixes and improvements",
                "1y",
                Status::Cloud,
            ),
            t(45, 5, "Plan the ECS scheduler rewrite", "1y", Status::Cloud),
            t(
                50,
                6,
                "Add feature to auto-copy codebase on token limit",
                "1y",
                Status::Cloud,
            ),
        ];
        threads[1..].iter_mut().for_each(|t| {
            t.items = vec![
                Item::User {
                    text: t.title.clone(),
                    time: "Mar 3",
                },
                Item::Assistant("Done. The change is in the cloud task's branch.".to_owned()),
            ]
        });
        Self { projects, threads }
    }

    pub fn thread(&self, id: ThreadId) -> Option<&Thread> {
        self.threads.iter().find(|t| t.id == id)
    }

    pub fn thread_mut(&mut self, id: ThreadId) -> Option<&mut Thread> {
        self.threads.iter_mut().find(|t| t.id == id)
    }

    pub fn project(&self, id: ProjectId) -> Option<&Project> {
        self.projects.iter().find(|p| p.id == id)
    }

    pub fn threads_in(&self, project: ProjectId) -> impl Iterator<Item = &Thread> {
        self.threads.iter().filter(move |t| t.project == project)
    }
}

impl Default for Data {
    fn default() -> Self {
        Self::new()
    }
}

/// The demo project's files (the file tree and viewer).
pub const CART_JS: &str = "// Shopping cart helpers.
export function subtotal(items) {
  return items.reduce((sum, item) => sum + item.price * (item.qty ?? 1), 0);
}

export function applyDiscount(total, percent) {
  return total - (total * percent) / 100;
}
";

pub const CART_JS_OLD: &str = "// Shopping cart helpers.
export function subtotal(items) {
  return items.reduce((sum, item) => sum + item.price, 0);
}

export function applyDiscount(total, percent) {
  return total - percent;
}
";

pub const FILES: &[(&str, bool)] = &[
    (".git", true),
    ("cart.js", false),
    ("cart.test.js", false),
    ("package.json", false),
    ("README.md", false),
];

/// One line of a two-sided diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffLine {
    Context { old: u32, new: u32 },
    Del { old: u32 },
    Add { new: u32 },
}

/// The unstaged cart.js diff in display order, with the text of each
/// line coming from the old or new file by number.
pub fn cart_diff() -> Vec<DiffLine> {
    use DiffLine::*;
    vec![
        Context { old: 1, new: 1 },
        Context { old: 2, new: 2 },
        Del { old: 3 },
        Add { new: 3 },
        Context { old: 4, new: 4 },
        Context { old: 5, new: 5 },
        Context { old: 6, new: 6 },
        Del { old: 7 },
        Add { new: 7 },
        Context { old: 8, new: 8 },
    ]
}

pub fn line_of(text: &str, n: u32) -> &str {
    text.lines().nth(n as usize - 1).unwrap_or("")
}

/// The terminal transcript of capture 38, as a shell would print it.
pub const TERMINAL_SCENE: &str = "\x1b[32mrohit@macbox\x1b[0m:\x1b[34m/private/tmp/codex-demo\x1b[0m \x1b[33m(main)\x1b[0m % \x1b[32mgit\x1b[0m status --short && \x1b[32mnpm\x1b[0m test\r\n \x1b[31mM\x1b[0m cart.js\r\n\r\n> codex-demo@0.1.0 test\r\n> node --test\r\n\r\n\x1b[32m✔ subtotal multiplies price by quantity \x1b[90m(0.375375ms)\x1b[0m\r\n\x1b[32m✔ applyDiscount takes a percentage off \x1b[90m(0.069833ms)\x1b[0m\r\n\x1b[34mℹ tests 2\x1b[0m\r\n\x1b[34mℹ suites 0\x1b[0m\r\n\x1b[34mℹ pass 2\x1b[0m\r\n\x1b[34mℹ fail 0\x1b[0m\r\n\x1b[34mℹ cancelled 0\x1b[0m\r\n\x1b[34mℹ skipped 0\x1b[0m\r\n\x1b[34mℹ todo 0\x1b[0m\r\n\x1b[34mℹ duration_ms 122.251459\x1b[0m\r\n\x1b[32mrohit@macbox\x1b[0m:\x1b[34m/private/tmp/codex-demo\x1b[0m \x1b[33m(main)\x1b[0m % ";

/// Commands in the search palette.
pub const SUGGESTED: &[(&str, &str)] = &[
    ("New chat", "⌘N"),
    ("Open folder", "⌘O"),
    ("Settings", "⌘,"),
];
