//! Named starting states, one per live capture this demo reproduces, so a
//! screenshot can be compared with its reference (`--scene NAME` or
//! `QUARK_CODEX_SCENE`). Each scene names its capture; `u*` captures are
//! ChatGPT 26.1007 in Codex mode, numbered ones Codex 26.623.

use crate::data::{self, Item, ProjectId, Status, ThreadId};
use crate::{Codex, Menu, Screen, Tab, settings::Page};

type Setup = fn(&mut Codex);

/// Scene name, reference capture, setup.
pub const SCENES: &[(&str, &str, Setup)] = &[
    ("home", "u73-home-new-chat-dark", home),
    ("home-light", "u70-home-new-chat-light", home),
    ("permissions-menu", "u03-permissions-menu-dark", |a| {
        home(a);
        a.composer.editor.set_text(data::TURN1_PROMPT);
        a.menu = Some(Menu::Permissions);
    }),
    ("model-menu", "u04-model-menu-dark", |a| {
        home(a);
        a.composer.editor.set_text(data::TURN1_PROMPT);
        a.menu = Some(Menu::Model);
    }),
    ("mode-menu", "u05-mode-switcher-dark", |a| {
        home(a);
        a.composer.editor.set_text(data::TURN1_PROMPT);
        a.menu = Some(Menu::Mode);
    }),
    ("add-menu", "u74-add-menu-dark", |a| {
        home(a);
        a.menu = Some(Menu::Add);
    }),
    ("slash-menu", "u75-slash-menu-dark", |a| {
        home(a);
        a.composer.editor.set_text("/");
    }),
    ("prompt", "u02-composer-prompt-dark", |a| {
        home(a);
        a.composer.editor.set_text(data::TURN1_PROMPT);
    }),
    ("starting", "u06-turn-starting-dark", |a| running(a, 0)),
    ("thinking", "u07-turn-thinking-dark", |a| running(a, 1)),
    ("streaming", "u08-preamble-streaming-shimmer-dark", |a| {
        running(a, 2)
    }),
    ("running-row", "u09-running-command-row-dark", |a| {
        running(a, 3)
    }),
    ("turn1", "u12-turn1-final-dark", |a| {
        turn1(a, false, false, false)
    }),
    ("worked-open", "u13-worked-for-expanded-dark", |a| {
        turn1(a, true, false, false)
    }),
    (
        "group-open",
        "u14-read-files-ran-commands-expanded-dark",
        |a| turn1(a, true, true, false),
    ),
    (
        "shell-card",
        "u15-command-card-npm-test-expanded-dark",
        |a| turn1(a, true, true, true),
    ),
    ("editing-pill", "u16-editing-files-changed-pill-dark", |a| {
        failing(a);
        let mut items = data::turn1(false, false, false);
        items.push(Item::User {
            text: data::TURN2_PROMPT.to_owned(),
            time: "11:03 PM",
        });
        items.push(Item::Work {
            took: "3s",
            running: true,
            open: true,
            steps: vec![
                data::Step::Prose {
                    text: "I’ll fix the quantity calculation and percentage discount in `cart.js`, then rerun the tests.",
                    pending: "",
                },
                data::Step::Live {
                    glyph: data::Glyph::Pencil,
                    text: "Editing files",
                },
            ],
        });
        set(a, items, Status::Running);
        a.data.thread_mut(data::FAILING_THREAD).unwrap().pill = Some((2, 2));
    }),
    (
        "file-change",
        "u20-turn2-final-file-change-card-dark",
        |a| turn2(a, false, false),
    ),
    ("diff-open", "u23-edit-row-diff-expanded-dark", |a| {
        turn2(a, true, true)
    }),
    ("changes", "u25-view-changes-review-panel-dark", |a| {
        turn2(a, true, true);
        a.side_panel = true;
    }),
    (
        "changes-no-sidebar",
        "u26-review-panel-sidebar-hidden-dark",
        |a| {
            turn2(a, true, true);
            a.side_panel = true;
            a.sidebar_open = false;
        },
    ),
    ("changes-options", "u27-changes-options-menu-dark", |a| {
        turn2(a, true, true);
        a.side_panel = true;
        a.sidebar_open = false;
        a.menu = Some(Menu::ChangesOptions);
    }),
    ("changes-scope", "u28-changes-scope-menu-dark", |a| {
        turn2(a, true, true);
        a.side_panel = true;
        a.sidebar_open = false;
        a.menu = Some(Menu::ChangesScope);
    }),
    ("changes-full", "u29-changes-full-view-dark", |a| {
        turn2(a, true, true);
        a.side_panel = true;
        a.full_view = true;
    }),
    ("approval", "u34-command-approval-prompt-dark", approval),
    ("approval-options", "u35-approval-options-menu-dark", |a| {
        approval(a);
        a.menu = Some(Menu::ApprovalOptions);
    }),
    ("activity", "u47-activity-panel-dark", |a| {
        turn2(a, false, false);
        a.activity = true;
    }),
    ("profile-menu", "u48-profile-menu-dark", |a| {
        turn2(a, false, false);
        a.activity = true;
        a.menu = Some(Menu::Profile);
    }),
    ("settings-general", "u51-settings-general-1-dark", |a| {
        settings(a, Page::General)
    }),
    (
        "settings-appearance",
        "u51-settings-appearance-1-dark",
        |a| settings(a, Page::Appearance),
    ),
    ("turn1-light", "u61-thread-finished-light", |a| {
        turn1(a, false, false, false)
    }),
    (
        "diff-light",
        "u64-edit-diff-and-file-change-card-light",
        |a| turn2(a, true, true),
    ),
    ("add-menu-light", "u71-add-menu-light", |a| {
        home(a);
        a.menu = Some(Menu::Add);
    }),
    // 26.623 surfaces the update was not captured on.
    ("errors", "27-thread-two-errors-model-changed-dark", |a| {
        a.screen = Screen::Thread(data::ERRORS_THREAD);
        set_errors(a, 5);
    }),
    ("terminal", "38-terminal-tab-dark", |a| {
        turn2(a, false, false);
        a.side_panel = true;
        a.full_view = true;
        a.tabs = vec![Tab::Changes, Tab::Terminal];
        a.tab = Tab::Terminal;
    }),
    ("file-viewer", "40-file-viewer-dark", |a| {
        turn2(a, false, false);
        a.side_panel = true;
        a.full_view = true;
        a.tabs = vec![Tab::Changes, Tab::Terminal, Tab::File];
        a.tab = Tab::File;
        a.open_file = Some("cart.js");
    }),
    ("palette", "51-search-palette-dark", |a| {
        turn2(a, false, false);
        a.palette = Some(crate::palette::State::new());
    }),
    (
        "settings-shortcuts",
        "58-settings-keyboard-shortcuts-1-dark",
        |a| settings(a, Page::KeyboardShortcuts),
    ),
    ("narrow-640", "62-narrow-640-dark", |a| {
        a.screen = Screen::Thread(data::ERRORS_THREAD);
        set_errors(a, 11);
    }),
    ("narrow-480", "63-narrow-min-dark", |a| {
        a.screen = Screen::Thread(data::ERRORS_THREAD);
        set_errors(a, 11);
    }),
];

pub fn apply(app: &mut Codex, name: &str) {
    match SCENES.iter().find(|s| s.0 == name) {
        Some((_, _, setup)) => setup(app),
        None => eprintln!("codex-demo: unknown scene {name:?}; try --list-scenes"),
    }
}

fn home(a: &mut Codex) {
    a.project = Some(ProjectId(1));
    a.screen = Screen::Home;
}

fn failing(a: &mut Codex) {
    a.project = Some(ProjectId(1));
    a.screen = Screen::Thread(data::FAILING_THREAD);
}

fn set(a: &mut Codex, items: Vec<Item>, status: Status) {
    if let Some(t) = a.data.thread_mut(data::FAILING_THREAD) {
        t.items = items;
        t.status = status;
    }
}

fn running(a: &mut Codex, stage: u8) {
    failing(a);
    if let Some(t) = a.data.thread_mut(data::FAILING_THREAD) {
        t.title = data::TURN1_PROMPT.to_owned();
    }
    set(a, data::running_turn(stage), Status::Running);
}

fn turn1(a: &mut Codex, work: bool, group: bool, shell: bool) {
    failing(a);
    if work {
        // u13 to u15 were scrolled to put the divider near the top.
        a.scroll_adjust = Some(59.0);
    }
    set(a, data::turn1(work, group, shell), Status::Idle);
}

fn turn2(a: &mut Codex, work: bool, diff: bool) {
    failing(a);
    // u20 and u23 keep the end of turn 1 in view above turn 2.
    a.scroll_adjust = Some(-133.0);
    set(
        a,
        [data::turn1(false, false, false), data::turn2(work, diff)].concat(),
        Status::Idle,
    );
}

fn approval(a: &mut Codex) {
    failing(a);
    set(
        a,
        [
            data::turn1(false, false, false),
            data::turn2(false, false),
            data::turn3_waiting(),
        ]
        .concat(),
        Status::Awaiting,
    );
    a.pending = Some(data::turn3_approval());
}

/// Settings as captured: the theme choice reads System whatever the
/// scene's forced mode.
fn settings(a: &mut Codex, page: Page) {
    a.screen = Screen::Settings(page);
    a.theme_choice = crate::ThemeChoice::System;
}

/// The 26.623 error thread, scrolled to its end as that version kept it.
fn set_errors(a: &mut Codex, n: usize) {
    a.project = Some(ProjectId(1));
    a.scroll_px = Some(1.0e6);
    if let Some(t) = a.data.thread_mut(ThreadId(2)) {
        t.items = data::error_items()[..n].to_vec();
    }
}
