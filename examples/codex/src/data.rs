//! Deterministic fake data, mirroring what the live captures show: the
//! signed-in account, the codex-demo project and its two chats, the
//! Recents list, the cart.js demo project, and the three agent turns the
//! update captures ran (explain failures, fix them, a network command
//! behind an approval).

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ThreadId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProjectId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Idle,
    Error,
    Running,
    /// Blocked on an approval: the sidebar shows "Awaiting approval".
    Awaiting,
}

#[derive(Debug, Clone)]
pub struct Thread {
    pub id: ThreadId,
    /// `None` for chats outside a project (Recents only).
    pub project: Option<ProjectId>,
    pub title: String,
    pub status: Status,
    pub items: Vec<Item>,
    /// The floating "1 file changed" pill above the composer, counting
    /// the shared change (`diff::Changes`).
    pub pill: bool,
}

#[derive(Debug, Clone)]
pub struct Project {
    pub id: ProjectId,
    pub name: &'static str,
}

/// Inline text in an answer.
#[derive(Debug, Clone, PartialEq)]
pub enum Span {
    Text(&'static str),
    Bold(&'static str),
    Code(&'static str),
    /// A file chip: name, and the line it points at.
    File(&'static str, Option<u32>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Para(Vec<Span>),
    Bullets(Vec<Vec<Span>>),
}

/// An expanded command's card.
#[derive(Debug, Clone, PartialEq)]
pub struct Shell {
    pub command: &'static str,
    pub output: &'static str,
    /// "Exit code unknown" and the like, at the bottom right.
    pub footer: Option<&'static str>,
}

/// One finished step inside a work group.
#[derive(Debug, Clone, PartialEq)]
pub enum Row {
    Ran {
        command: &'static str,
        took: Option<&'static str>,
        shell: Option<Shell>,
        open: bool,
    },
    Read(&'static str),
    /// Counts and lines come from the shared change (`diff::Changes`).
    Edited {
        file: &'static str,
        open: bool,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// Preamble prose; `pending` is the streamed tail not yet settled,
    /// drawn dim.
    Prose {
        text: &'static str,
        pending: &'static str,
    },
    /// "Read files, ran commands" and its rows.
    Group {
        label: &'static str,
        glyph: Glyph,
        open: bool,
        rows: Vec<Row>,
    },
    /// A finished single row outside a group.
    Row(Row),
    /// The step under way: glyph and shimmering text ("Running ...").
    Live { glyph: Glyph, text: &'static str },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Glyph {
    Terminal,
    Book,
    Pencil,
}

/// One entry in a thread's transcript.
#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    User {
        text: String,
        time: &'static str,
    },
    /// Right after send: grey "Starting your task".
    Starting,
    /// The shimmering "Thinking" line.
    Thinking,
    Error(String),
    /// "Model changed from A to B." between turns (26.623).
    ModelChanged {
        from: &'static str,
        to: &'static str,
    },
    /// The "Working for" / "Worked for" divider and the work under it.
    Work {
        took: &'static str,
        running: bool,
        open: bool,
        steps: Vec<Step>,
    },
    /// The final answer and its action row.
    Answer {
        blocks: Vec<Block>,
        time: &'static str,
    },
    /// "Edited cart.js +2 -2" with Undo and View changes; the counts come
    /// from the shared change.
    FileChange {
        file: &'static str,
    },
}

/// A command waiting for the user's approval; it replaces the composer.
#[derive(Debug, Clone, PartialEq)]
pub struct Approval {
    pub category: &'static str,
    pub question: &'static str,
    pub command: &'static str,
}

pub const ACCOUNT_NAME: &str = "seatedro";
pub const ACCOUNT_PLAN: &str = "Pro";
pub const DEMO_PROJECT: &str = "codex-demo";
pub const TURN1_PROMPT: &str =
    "Run the tests and explain briefly why they fail. Do not edit any files yet.";
pub const TURN2_PROMPT: &str = "Fix both bugs in cart.js, then rerun the tests.";
pub const TURN3_PROMPT: &str = "Check the latest published version of the left-pad package on npm by running npm view left-pad version.";
pub const FAILING_THREAD: ThreadId = ThreadId(1);
pub const ERRORS_THREAD: ThreadId = ThreadId(2);

pub struct Data {
    pub projects: Vec<Project>,
    pub threads: Vec<Thread>,
}

fn recent(id: u32, title: &str, status: Status) -> Thread {
    Thread {
        id: ThreadId(id),
        project: None,
        title: title.to_owned(),
        status,
        items: vec![Item::User {
            text: title.to_owned(),
            time: "Mar 3",
        }],
        pill: false,
    }
}

/// The 26.623 session's transcript: four tries, each failing with the
/// account's "model not supported" error. Scenes show a prefix.
pub fn error_items() -> Vec<Item> {
    let user = || Item::User {
        text: "Read the repo and make the failing tests pass.".to_owned(),
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

pub fn npm_test_shell() -> Shell {
    Shell {
        command: "npm test",
        output: "    generatedMessage: true,\n    code: 'ERR_ASSERTION',\n    actual: 6,\n    expected: 13,\n    operator: 'strictEqual'\n  }\nℹ tests 2\nℹ pass 0\nℹ fail 2",
        footer: None,
    }
}

/// Turn 1: explain the failures, no edits.
pub fn turn1(work_open: bool, group_open: bool, shell_open: bool) -> Vec<Item> {
    vec![
        Item::User {
            text: TURN1_PROMPT.to_owned(),
            time: "11:00 PM",
        },
        Item::Work {
            took: "15s",
            running: false,
            open: work_open,
            steps: vec![
                Step::Prose {
                    text: "I’ll inspect the project’s test setup, run the tests, and briefly explain any failures without editing files.",
                    pending: "",
                },
                Step::Group {
                    label: "Read files, ran commands",
                    glyph: Glyph::Book,
                    open: group_open,
                    rows: vec![
                        Row::Ran {
                            command: "pwd; rg --files -g 'AGENTS.md' -g 'package.json' -g 'pyproject.toml' -g 'pytest.ini' -g 'Cargo.toml' -g 'Makefile' -g 'README*' -g '*lock*'",
                            took: None,
                            shell: None,
                            open: false,
                        },
                        Row::Ran {
                            command: "cat package.json README.md; rg --files -g '!node_modules' -g '!package-lock.json'; git status --short",
                            took: None,
                            shell: None,
                            open: false,
                        },
                        Row::Read("cart.js"),
                        Row::Read("cart.test.js"),
                        Row::Ran {
                            command: "npm test",
                            took: None,
                            shell: Some(npm_test_shell()),
                            open: shell_open,
                        },
                    ],
                },
            ],
        },
        Item::Answer {
            blocks: vec![
                Block::Para(vec![
                    Span::Text("Ran "),
                    Span::Code("npm test"),
                    Span::Text(": "),
                    Span::Bold("both tests fail"),
                    Span::Text("."),
                ]),
                Block::Bullets(vec![
                    vec![
                        Span::File("`subtotal`", Some(3)),
                        Span::Text(" adds prices without multiplying by quantity, returning "),
                        Span::Bold("6"),
                        Span::Text(" instead of "),
                        Span::Bold("13"),
                        Span::Text("."),
                    ],
                    vec![
                        Span::File("`applyDiscount`", Some(7)),
                        Span::Text(" subtracts "),
                        Span::Code("10"),
                        Span::Text(" directly instead of taking 10% off "),
                        Span::Code("200"),
                        Span::Text(", returning "),
                        Span::Bold("190"),
                        Span::Text(" instead of "),
                        Span::Bold("180"),
                        Span::Text("."),
                    ],
                ]),
                Block::Para(vec![Span::Text("No files were edited.")]),
            ],
            time: "11:01 PM",
        },
    ]
}

/// Turn 2: fix both bugs. `diff_open` expands the edit row's inline diff.
pub fn turn2(work_open: bool, diff_open: bool) -> Vec<Item> {
    vec![
        Item::User {
            text: TURN2_PROMPT.to_owned(),
            time: "11:03 PM",
        },
        Item::Work {
            took: "6s",
            running: false,
            open: work_open,
            steps: vec![
                Step::Prose {
                    text: "I’ll fix the quantity calculation and percentage discount in `cart.js`, then rerun the tests.",
                    pending: "",
                },
                Step::Group {
                    label: "Edited a file, ran a command",
                    glyph: Glyph::Pencil,
                    open: true,
                    rows: vec![
                        Row::Edited {
                            file: "cart.js",
                            open: diff_open,
                        },
                        Row::Ran {
                            command: "npm test",
                            took: None,
                            shell: None,
                            open: false,
                        },
                    ],
                },
            ],
        },
        Item::Answer {
            blocks: vec![
                Block::Para(vec![
                    Span::Text("Fixed both bugs in "),
                    Span::File("cart.js", None),
                    Span::Text(
                        ": subtotal now multiplies price by quantity, and discounts apply as percentages.",
                    ),
                ]),
                Block::Para(vec![
                    Span::Code("npm test"),
                    Span::Text(" passes: "),
                    Span::Bold("2 passed, 0 failed"),
                    Span::Text("."),
                ]),
            ],
            time: "11:03 PM",
        },
        Item::FileChange { file: "cart.js" },
    ]
}

/// Turn 3 while it waits on the network command's approval.
pub fn turn3_waiting() -> Vec<Item> {
    vec![
        Item::User {
            text: TURN3_PROMPT.to_owned(),
            time: "11:06 PM",
        },
        Item::Work {
            took: "2m 21s",
            running: true,
            open: true,
            steps: vec![
                Step::Prose {
                    text: "I’ll run `npm view left-pad version` to check the latest published version.",
                    pending: "",
                },
                Step::Row(Row::Ran {
                    command: "npm view left-pad version",
                    took: Some("1m 11s"),
                    shell: None,
                    open: false,
                }),
                Step::Prose {
                    text: "The npm registry request is taking longer than expected. I’m waiting for the command to return.",
                    pending: "",
                },
                Step::Live {
                    glyph: Glyph::Terminal,
                    text: "Running npm view left-pad version",
                },
            ],
        },
    ]
}

pub fn turn3_approval() -> Approval {
    Approval {
        category: "Terminal",
        question: "May I rerun the npm version lookup with network access after the sandbox DNS request failed?",
        command: "npm view left-pad version",
    }
}

/// A running turn's frames, as the burst captures show them.
pub fn running_turn(stage: u8) -> Vec<Item> {
    let user = Item::User {
        text: TURN1_PROMPT.to_owned(),
        time: "11:00 PM",
    };
    match stage {
        0 => vec![user, Item::Starting],
        1 => vec![user, Item::Thinking],
        2 => vec![
            user,
            Item::Work {
                took: "3s",
                running: true,
                open: true,
                steps: vec![Step::Prose {
                    text: "I’ll inspect the project’s ",
                    pending: "test setup, run the tests,",
                }],
            },
            Item::Thinking,
        ],
        _ => vec![
            user,
            Item::Work {
                took: "9s",
                running: true,
                open: true,
                steps: vec![
                    Step::Prose {
                        text: "I’ll inspect the project’s test setup, run the tests, and briefly explain any failures without editing files.",
                        pending: "",
                    },
                    Step::Live {
                        glyph: Glyph::Terminal,
                        text: "Running pwd; rg --files -g 'AGENTS.md' -g 'package.json' -g 'pyproject.toml' -g 'pytest.ini'",
                    },
                ],
            },
        ],
    }
}

impl Data {
    pub fn new() -> Self {
        let projects = vec![Project {
            id: ProjectId(1),
            name: DEMO_PROJECT,
        }];
        let mut failing = recent(1, "Run tests and explain failures", Status::Idle);
        failing.project = Some(ProjectId(1));
        failing.items = [turn1(false, false, false), turn2(false, false)].concat();
        let mut errors = recent(2, "Fix failing repository tests", Status::Idle);
        errors.project = Some(ProjectId(1));
        errors.items = error_items();
        let threads = vec![
            failing,
            errors,
            recent(10, "Rename log to reading list in cmdk menu", Status::Idle),
            recent(
                11,
                "Build cross-platform ebook reader with Qt",
                Status::Idle,
            ),
            recent(12, "Design self-improving intent layer", Status::Idle),
            recent(
                13,
                "Compare rendering practices with Bevy and Ember",
                Status::Idle,
            ),
            recent(
                14,
                "Create cross-platform ebook reader with EPUB support",
                Status::Idle,
            ),
            recent(
                15,
                "Create cross-platform ebook reader with epub support",
                Status::Error,
            ),
            recent(
                16,
                "Add custom scrollbar with headings on hover",
                Status::Idle,
            ),
            recent(
                17,
                "Add feature to auto-copy codebase on token limit",
                Status::Idle,
            ),
            recent(18, "Try compiling with zig build", Status::Error),
            recent(19, "Create README and LICENSE files", Status::Idle),
            recent(
                20,
                "Identify priority subsystems for game engine",
                Status::Idle,
            ),
            recent(
                21,
                "Propose tasks for code fixes and improvements",
                Status::Idle,
            ),
        ];
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
        self.threads
            .iter()
            .filter(move |t| t.project == Some(project))
    }
}

impl Default for Data {
    fn default() -> Self {
        Self::new()
    }
}

/// cart.js after the agent's fix, and before it.
pub const CART_JS: &str = "// Shopping cart helpers.
export function subtotal(items) {
  return items.reduce((sum, item) => sum + item.price * item.qty, 0);
}

export function applyDiscount(total, percent) {
  return total - total * percent / 100;
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

/// The terminal transcript of capture 38, as a shell would print it.
pub const TERMINAL_SCENE: &str = "\x1b[32mrohit@macbox\x1b[0m:\x1b[34m/private/tmp/codex-demo\x1b[0m \x1b[33m(main)\x1b[0m % \x1b[32mgit\x1b[0m status --short && \x1b[32mnpm\x1b[0m test\r\n \x1b[31mM\x1b[0m cart.js\r\n\r\n> codex-demo@0.1.0 test\r\n> node --test\r\n\r\n\x1b[32m✔ subtotal multiplies price by quantity \x1b[90m(0.375375ms)\x1b[0m\r\n\x1b[32m✔ applyDiscount takes a percentage off \x1b[90m(0.069833ms)\x1b[0m\r\n\x1b[34mℹ tests 2\x1b[0m\r\n\x1b[34mℹ suites 0\x1b[0m\r\n\x1b[34mℹ pass 2\x1b[0m\r\n\x1b[34mℹ fail 0\x1b[0m\r\n\x1b[34mℹ cancelled 0\x1b[0m\r\n\x1b[34mℹ skipped 0\x1b[0m\r\n\x1b[34mℹ todo 0\x1b[0m\r\n\x1b[34mℹ duration_ms 122.251459\x1b[0m\r\n\x1b[32mrohit@macbox\x1b[0m:\x1b[34m/private/tmp/codex-demo\x1b[0m \x1b[33m(main)\x1b[0m % ";

/// Commands in the search palette.
pub const SUGGESTED: &[(&str, &str)] = &[
    ("New chat", "⌘N"),
    ("Open folder", "⌘O"),
    ("Settings", "⌘,"),
];
