//! Named starting states, one per live capture this demo reproduces, so a
//! screenshot can be compared with its reference (`--scene NAME` or
//! `QUARK_CODEX_SCENE`). Each scene names its capture.

use crate::data::{self, Item, ProjectId, Status, ThreadId};
use crate::{Codex, Menu, Scope, Screen, Tab, settings::Page};

type Setup = fn(&mut Codex);

/// Scene name, reference capture, setup.
pub const SCENES: &[(&str, &str, Setup)] = &[
    ("home", "04b-home-no-banner", home),
    ("home-banner", "04-home-new-chat", |a| {
        home(a);
        a.banner = true;
    }),
    ("approval-menu", "05-approval-mode-menu", |a| {
        home(a);
        a.banner = true;
        a.menu = Some(Menu::Approval);
    }),
    ("model-menu", "06-model-picker", |a| {
        home(a);
        a.banner = true;
        a.menu = Some(Menu::Model);
    }),
    ("model-submenu", "07-model-submenu", |a| {
        home(a);
        a.banner = true;
        a.menu = Some(Menu::ModelList);
    }),
    ("speed-submenu", "08-speed-submenu", |a| {
        home(a);
        a.banner = true;
        a.menu = Some(Menu::Speed);
    }),
    ("add-menu", "09-add-menu", |a| {
        home(a);
        a.menu = Some(Menu::Add);
    }),
    ("choose-project", "11-choose-project-menu", |a| {
        home(a);
        a.menu = Some(Menu::ChooseProject);
    }),
    ("project-home", "13-new-chat-in-project", project_home),
    ("work-location", "14-work-location-menu", |a| {
        project_home(a);
        a.menu = Some(Menu::WorkLocation);
    }),
    ("branch-menu", "15-branch-menu", |a| {
        project_home(a);
        a.menu = Some(Menu::Branch);
    }),
    ("headline-project", "17-headline-project-picker", |a| {
        project_home(a);
        a.menu = Some(Menu::HeadlineProject);
    }),
    ("slash-menu", "18-slash-menu", |a| {
        project_home(a);
        a.composer.editor.set_text("/");
    }),
    ("at-mention", "21-at-mention-file-search", |a| {
        project_home(a);
        a.composer.editor.set_text("@car");
    }),
    ("prompt", "22-composer-with-prompt", |a| {
        project_home(a);
        a.composer.editor.set_text(data::DEMO_PROMPT);
    }),
    ("thinking", "23-turn-thinking", |a| {
        thread(a, 1);
        if let Some(t) = a.data.thread_mut(ThreadId(1)) {
            t.items.push(Item::Thinking);
            t.status = Status::Running;
        }
    }),
    ("error", "24-turn-error-model-unsupported", |a| thread(a, 2)),
    ("thread", "27-thread-two-errors-model-changed", |a| {
        thread(a, 5);
        a.model = 2;
    }),
    ("chat-actions", "28-thread-chat-actions-menu", |a| {
        thread(a, 5);
        a.model = 2;
        a.menu = Some(Menu::ChatActions);
    }),
    ("open-in", "29-open-in-menu", |a| {
        thread(a, 5);
        a.model = 2;
        a.menu = Some(Menu::OpenIn);
    }),
    ("summary", "30-summary-panel", |a| {
        thread(a, 5);
        a.model = 2;
        a.menu = Some(Menu::Summary);
    }),
    ("review-scope", "32-review-scope-menu", |a| {
        review(a);
        a.scope = Scope::Branch;
        a.menu = Some(Menu::ReviewScope);
    }),
    ("review", "33-review-unstaged-diff", review),
    ("review-no-sidebar", "34-review-diff-sidebar-hidden", |a| {
        review(a);
        a.sidebar_open = false;
    }),
    ("review-split", "35-review-split-diff", |a| {
        review(a);
        a.sidebar_open = false;
        a.split = true;
    }),
    ("review-expanded", "36-review-expanded-panel", |a| {
        review(a);
        a.split = true;
        a.panel_expanded = true;
    }),
    ("panel-tab-menu", "37-side-panel-tab-menu", |a| {
        review(a);
        a.split = true;
        a.panel_expanded = true;
        a.menu = Some(Menu::PanelTab);
    }),
    ("terminal", "38-terminal-tab", |a| {
        review(a);
        a.panel_expanded = true;
        a.tabs = vec![Tab::Review, Tab::Terminal];
        a.tab = Tab::Terminal;
    }),
    ("file-viewer", "40-file-viewer", |a| {
        review(a);
        a.panel_expanded = true;
        a.tabs = vec![Tab::Review, Tab::Terminal, Tab::File];
        a.tab = Tab::File;
        a.open_file = Some("cart.js");
    }),
    ("thread-full", "43-try-gpt52-light", |a| {
        thread(a, 11);
        a.model = 3;
        a.reasoning = 0;
    }),
    ("account-menu", "48-account-menu", |a| {
        thread(a, 11);
        a.model = 3;
        a.reasoning = 0;
        a.menu = Some(Menu::Account);
    }),
    ("palette", "51-search-palette", |a| {
        thread(a, 11);
        a.model = 3;
        a.reasoning = 0;
        a.palette = Some(crate::palette::State::new());
    }),
    ("settings-general", "58-settings-general-1", |a| {
        a.screen = Screen::Settings(Page::General)
    }),
    ("settings-appearance", "58-settings-appearance-1", |a| {
        a.screen = Screen::Settings(Page::Appearance)
    }),
    ("narrow-640", "62-narrow-640", |a| {
        thread(a, 11);
        a.model = 3;
        a.reasoning = 0;
    }),
    ("narrow-480", "63-narrow-min", |a| {
        thread(a, 11);
        a.model = 3;
        a.reasoning = 0;
    }),
    ("launcher", "77-side-panel-launcher", |a| {
        thread(a, 11);
        a.model = 3;
        a.reasoning = 0;
        a.side_panel = true;
        a.tabs.clear();
    }),
    ("agent-turn", "pub-03 / pub-04 (public)", |a| {
        a.project = Some(ProjectId(1));
        let id = ThreadId(2);
        a.data.threads.insert(
            0,
            data::Thread {
                id,
                project: ProjectId(1),
                title: "Fix applyDiscount so the cart tests pass.".to_owned(),
                age: "now",
                status: Status::Idle,
                cloud: false,
                items: data::agent_turn_items(),
            },
        );
        a.screen = Screen::Thread(id);
    }),
];

pub fn apply(app: &mut Codex, name: &str) {
    match SCENES.iter().find(|s| s.0 == name) {
        Some((_, _, setup)) => setup(app),
        None => eprintln!("codex-demo: unknown scene {name:?}; try --list-scenes"),
    }
}

/// The home screen before any project was opened: no codex-demo.
fn home(a: &mut Codex) {
    a.data.threads.retain(|t| t.project != ProjectId(1));
    a.project = None;
    a.screen = Screen::Home;
}

fn project_home(a: &mut Codex) {
    a.data.threads.retain(|t| t.project != ProjectId(1));
    a.project = Some(ProjectId(1));
    a.screen = Screen::Home;
}

/// The demo thread with its first `items` transcript entries.
fn thread(a: &mut Codex, items: usize) {
    a.project = Some(ProjectId(1));
    if let Some(t) = a.data.thread_mut(ThreadId(1)) {
        t.items = data::demo_items()[..items].to_vec();
        t.status = Status::Error;
    }
    a.screen = Screen::Thread(ThreadId(1));
}

fn review(a: &mut Codex) {
    thread(a, 5);
    a.model = 2;
    a.side_panel = true;
    a.tabs = vec![Tab::Review];
    a.tab = Tab::Review;
}
