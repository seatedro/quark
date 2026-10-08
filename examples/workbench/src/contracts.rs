//! The frozen contracts every surface builds against: stable IDs, commands,
//! effects, the panel registry, the top-level action, and the read-only
//! surface context.
//!
//! Surface modules (`shell`, `timeline`, `composer`, `dock`, `overlays`,
//! `settings`) each export the same shape:
//!
//! ```text
//! pub struct State;                       // owned by the app, one per surface
//! pub enum Action;                        // Debug + Clone + PartialEq + 'static
//! pub fn view(state: &mut State, scx: &SurfaceCx, vcx: &mut ViewContext) -> AnyElement;
//! pub fn update(state: &mut State, action: Action, scx: &SurfaceCx, fx: &mut Effects);
//! pub fn event(state: &mut State, event: &InputEvent, scx: &SurfaceCx, fx: &mut Effects) -> bool;
//! pub fn edit_text(state: &mut State, target: FocusId, command: TextEditCommand, ecx: &EditCx)
//!     -> Option<TextEditOutcome>;          // None: not this surface's field
//! pub fn command(state: &mut State, id: CommandId, scx: &SurfaceCx, fx: &mut Effects) -> bool;
//! ```
//!
//! A surface never mutates the model or another surface: it reads the
//! model through [`SurfaceCx`] and asks for changes by pushing [`Effect`]s,
//! which `app.rs` applies in order after the update returns. Views wrap
//! their own actions with `From<surface::Action> for quark_ui::Action`
//! (implemented below), so `on:click={Action::Foo}` works in `view!`.

use std::path::PathBuf;

use quark_app::WindowHandle;
use quark_app::quark_ui::FocusId;
use quark_app::quark_ui::theme::Theme;
use quark_components::{DockRegion, HostId};
use serde::{Deserialize, Serialize};

pub use quark_components::PanelId;

use crate::model::Model;

// ---------------------------------------------------------------- IDs

macro_rules! id_type {
    ($(#[$doc:meta])* $name:ident($inner:ty)) => {
        $(#[$doc])*
        #[derive(
            Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(pub $inner);
    };
}

id_type!(
    /// A project in the sidebar. Fixture IDs are below 1,000.
    ProjectId(u32)
);
id_type!(
    /// A thread, stable across sidebar filtering, sorting, and windows.
    ThreadId(u32)
);
id_type!(
    /// A transcript row. Fixture rows use their fixture IDs; scenario rows
    /// start at [`SCENARIO_ID_BASE`]; stress rows at [`STRESS_ID_BASE`].
    /// Doubles as the timeline's `RowKey`.
    MessageId(u64)
);
id_type!(
    /// A tool call within a transcript; also its row's [`MessageId`] value.
    ToolId(u64)
);
id_type!(
    /// Counts runs per app session. Every scenario event carries the
    /// generation of the run that produced it; the model drops events from
    /// a run that was stopped or replaced.
    RunGeneration(u32)
);
id_type!(
    /// An attachment held in demo memory.
    AttachmentId(u32)
);

/// First [`MessageId`]/[`ToolId`] value the scenario allocates.
pub const SCENARIO_ID_BASE: u64 = 1_000_000;
/// First [`MessageId`] of the generated stress history.
pub const STRESS_ID_BASE: u64 = 10_000_000;
/// Attachments over this size are rejected with a visible error.
pub const ATTACHMENT_LIMIT_BYTES: usize = 10 * 1024 * 1024;

// ---------------------------------------------------------- panels

/// The dock panels. IDs are persisted in dock snapshots: never renumber.
pub mod panels {
    use super::PanelId;
    /// The thread list, docked left (shell).
    pub const SIDEBAR: PanelId = PanelId(1);
    /// The active thread: timeline over composer (center).
    pub const THREAD: PanelId = PanelId(2);
    pub const DIFF: PanelId = PanelId(10);
    pub const TERMINAL: PanelId = PanelId(11);
    pub const FILES: PanelId = PanelId(12);
    pub const PREVIEW: PanelId = PanelId(13);
}

/// One registered panel.
#[derive(Debug, Clone, Copy)]
pub struct PanelSpec {
    pub id: PanelId,
    /// Tab and window title.
    pub title: &'static str,
    /// Lucide SVG for its tab.
    pub icon: &'static str,
    /// Where it opens by default.
    pub region: DockRegion,
    /// Whether it may leave the main window.
    pub detachable: bool,
}

use quark_app::quark_ui::icons::lucide;

/// Every panel, in default tab order.
pub const PANELS: [PanelSpec; 6] = [
    PanelSpec {
        id: panels::SIDEBAR,
        title: "Threads",
        icon: lucide::ROWS,
        region: DockRegion::Left,
        detachable: false,
    },
    PanelSpec {
        id: panels::THREAD,
        title: "Thread",
        icon: lucide::SPARKLES,
        region: DockRegion::Center,
        detachable: false,
    },
    PanelSpec {
        id: panels::DIFF,
        title: "Diff",
        icon: lucide::FILE_DIFF,
        region: DockRegion::Right,
        detachable: true,
    },
    PanelSpec {
        id: panels::TERMINAL,
        title: "Terminal",
        icon: lucide::TERMINAL,
        region: DockRegion::Right,
        detachable: true,
    },
    PanelSpec {
        id: panels::FILES,
        title: "Files",
        icon: lucide::FOLDER,
        region: DockRegion::Right,
        detachable: true,
    },
    PanelSpec {
        id: panels::PREVIEW,
        title: "Snapshot preview",
        icon: lucide::EYE,
        region: DockRegion::Right,
        detachable: true,
    },
];

pub fn panel_spec(id: PanelId) -> Option<&'static PanelSpec> {
    PANELS.iter().find(|p| p.id == id)
}

pub fn panel_title(id: PanelId) -> &'static str {
    panel_spec(id).map_or("Panel", |p| p.title)
}

// --------------------------------------------------------- commands

/// Every command the palette, native menus, key bindings, and context
/// menus run. Route all of them through [`Effect::Command`] so precedence
/// lives in one place (`app.rs`, refined by `overlays`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CommandId {
    OpenPalette,
    NewThread,
    OpenSettings,
    ToggleSidebar,
    ToggleRightDock,
    ToggleTerminal,
    FindInDocument,
    FocusComposer,
    SendPrompt,
    StopRun,
    ShowDiff,
    ShowFiles,
    ShowPreview,
    ApplyDiff,
    UndoDiff,
    NextThread,
    PreviousThread,
    ThemeSystem,
    ThemeLight,
    ThemeDark,
    /// Manual clock: play the scenario up to its next event.
    AdvanceDemoStep,
    /// Reload fixtures and restart the scenario.
    ResetDemo,
}

/// A command's stable name, palette title, and default binding.
#[derive(Debug, Clone, Copy)]
pub struct CommandSpec {
    pub id: CommandId,
    /// Stable dotted name, used by menus, e2e specs, and accessibility ids.
    pub name: &'static str,
    pub title: &'static str,
    /// Keymap syntax (`mod+k`), if bound by default.
    pub binding: Option<&'static str>,
    /// Shown in the palette only under `--manual-clock`.
    pub manual_clock_only: bool,
}

const fn cmd(
    id: CommandId,
    name: &'static str,
    title: &'static str,
    binding: Option<&'static str>,
) -> CommandSpec {
    CommandSpec {
        id,
        name,
        title,
        binding,
        manual_clock_only: false,
    }
}

pub const COMMANDS: &[CommandSpec] = &[
    cmd(
        CommandId::OpenPalette,
        "workbench.palette",
        "Command palette",
        Some("mod+k"),
    ),
    cmd(
        CommandId::NewThread,
        "thread.new",
        "New thread",
        Some("mod+n"),
    ),
    cmd(
        CommandId::OpenSettings,
        "workbench.settings",
        "Settings",
        Some("mod+,"),
    ),
    cmd(
        CommandId::ToggleSidebar,
        "view.sidebar",
        "Toggle sidebar",
        Some("mod+b"),
    ),
    cmd(
        CommandId::ToggleRightDock,
        "view.dock",
        "Toggle right dock",
        Some("mod+alt+b"),
    ),
    cmd(
        CommandId::ToggleTerminal,
        "view.terminal",
        "Toggle terminal",
        Some("mod+j"),
    ),
    cmd(
        CommandId::FindInDocument,
        "timeline.find",
        "Find in thread",
        Some("mod+f"),
    ),
    cmd(
        CommandId::FocusComposer,
        "composer.focus",
        "Focus composer",
        Some("mod+l"),
    ),
    cmd(CommandId::SendPrompt, "composer.send", "Send prompt", None),
    cmd(CommandId::StopRun, "run.stop", "Stop run", None),
    cmd(CommandId::ShowDiff, "dock.diff", "Show diff", None),
    cmd(CommandId::ShowFiles, "dock.files", "Show files", None),
    cmd(
        CommandId::ShowPreview,
        "dock.preview",
        "Show snapshot preview",
        None,
    ),
    cmd(
        CommandId::ApplyDiff,
        "diff.apply",
        "Apply proposed changes",
        None,
    ),
    cmd(
        CommandId::UndoDiff,
        "diff.undo",
        "Undo applied changes",
        None,
    ),
    cmd(
        CommandId::NextThread,
        "thread.next",
        "Next thread",
        Some("mod+alt+down"),
    ),
    cmd(
        CommandId::PreviousThread,
        "thread.previous",
        "Previous thread",
        Some("mod+alt+up"),
    ),
    cmd(
        CommandId::ThemeSystem,
        "theme.system",
        "Theme: match system",
        None,
    ),
    cmd(CommandId::ThemeLight, "theme.light", "Theme: light", None),
    cmd(CommandId::ThemeDark, "theme.dark", "Theme: dark", None),
    CommandSpec {
        manual_clock_only: true,
        ..cmd(
            CommandId::AdvanceDemoStep,
            "demo.advance",
            "Advance demo step",
            Some("mod+shift+."),
        )
    },
    cmd(CommandId::ResetDemo, "demo.reset", "Reset demo", None),
];

pub fn command_spec(id: CommandId) -> &'static CommandSpec {
    COMMANDS
        .iter()
        .find(|c| c.id == id)
        .expect("every CommandId is registered in COMMANDS")
}

// ---------------------------------------------------------- effects

/// What a surface asks the app to do. Applied in push order after the
/// surface's update returns; effects may produce further effects.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    Command(CommandId),
    SelectThread(ThreadId),
    /// Start a run on `thread` with `text` as the user's turn.
    SendPrompt {
        thread: ThreadId,
        text: String,
        attachments: Vec<AttachmentId>,
    },
    StopRun(ThreadId),
    RetryTool {
        thread: ThreadId,
        tool: ToolId,
    },
    /// Show `path` from the fixture file store in the Files panel.
    OpenFile(String),
    /// Show the diff panel, scrolled to `path` when given.
    RevealDiff(Option<String>),
    ApplyDiff,
    UndoDiff,
    Toast(Toast),
    Focus(Option<FocusId>),
    Announce(String),
    CopyText(String),
    SetTheme(ThemeChoice),
    /// Show or hide a dock region (shell width policy and toggles).
    SetRegionVisible(DockRegion, bool),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Success,
    Error,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Toast {
    pub kind: ToastKind,
    pub text: String,
    /// Run when the toast's Undo is pressed.
    pub undo: Option<CommandId>,
}

/// The effect queue a surface update pushes onto.
#[derive(Debug, Default)]
pub struct Effects {
    queue: Vec<Effect>,
}

impl Effects {
    pub fn push(&mut self, effect: Effect) {
        self.queue.push(effect);
    }

    pub fn command(&mut self, id: CommandId) {
        self.push(Effect::Command(id));
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// Take the queued effects, oldest first, leaving the queue empty (and
    /// its allocation in place for the next update).
    pub fn drain(&mut self) -> std::vec::Drain<'_, Effect> {
        self.queue.drain(..)
    }
}

// ---------------------------------------------------- options/theme

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemeChoice {
    #[default]
    System,
    Light,
    Dark,
}

/// Which fixture world the app starts in (`--scenario`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScenarioKind {
    /// "Add keyboard shortcuts" mid-review: the default opening scene.
    #[default]
    Review,
    /// A new empty thread offering the "Review changes" starter prompt.
    Empty,
    /// The selected thread's last run failed with a recoverable error.
    Error,
    /// Review plus a 50,000-row history and 2,000 sidebar threads.
    Stress,
}

/// Launch options: command-line flags, with environment equivalents for
/// the e2e runner, which starts binaries without arguments.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Options {
    pub scenario: ScenarioKind,
    pub theme: ThemeChoice,
    /// Seed of the stress generator.
    pub seed: u64,
    /// Time moves only through "Advance demo step".
    pub manual_clock: bool,
    /// Restore and save dock and settings here; session-only when unset.
    pub state_dir: Option<PathBuf>,
    /// Write JSONL frame samples here and exit after the scripted run
    /// (`--perf <path>`).
    pub perf_out: Option<PathBuf>,
}

// ------------------------------------------------- surface context

/// How the shell lays out the main window at its current width.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WidthPolicy {
    /// The sidebar is docked (not an overlay).
    pub sidebar_docked: bool,
    /// The right dock is shown beside the thread.
    pub right_dock_shown: bool,
}

/// The read-only view every surface gets: the model, the theme, and where
/// it is drawn. Built by `app.rs` for each surface it renders.
#[derive(Clone, Copy)]
pub struct SurfaceCx<'a> {
    pub model: &'a Model,
    pub theme: &'a Theme,
    /// The surface's own size in logical points.
    pub size: (f32, f32),
    pub scale: f32,
    pub window: Option<WindowHandle>,
    pub host: HostId,
    pub width_policy: WidthPolicy,
    pub focus: Option<FocusId>,
    /// Scenario clock in milliseconds (the manual clock under
    /// `--manual-clock`).
    pub now_ms: u64,
    pub manual_clock: bool,
}

impl SurfaceCx<'_> {
    pub fn is_focused(&self, target: FocusId) -> bool {
        self.focus == Some(target)
    }

    pub fn reduced_motion(&self) -> bool {
        self.theme.reduced_motion
    }
}

/// What text editing sees: `UiApp::edit_text` runs without a window
/// context, possibly mid-frame.
#[derive(Clone, Copy)]
pub struct EditCx<'a> {
    pub model: &'a Model,
    pub now_ms: u64,
}

// ------------------------------------------------ top-level action

/// The app's action type. Only `app.rs` matches on it; surfaces construct
/// their own variant through the `From` impls below.
#[derive(Debug, Clone, PartialEq)]
pub enum Msg {
    Command(CommandId),
    Shell(crate::shell::Action),
    Timeline(crate::timeline::Action),
    Composer(crate::composer::Action),
    Dock(crate::dock::Action),
    Overlays(crate::overlays::Action),
    Settings(crate::settings::Action),
}

macro_rules! into_action {
    ($($variant:ident($ty:ty)),* $(,)?) => {
        $(
            impl From<$ty> for Msg {
                fn from(action: $ty) -> Self {
                    Msg::$variant(action)
                }
            }
            impl From<$ty> for quark_app::quark_ui::Action {
                fn from(action: $ty) -> Self {
                    quark_app::quark_ui::Action::new(Msg::$variant(action))
                }
            }
        )*
    };
}

into_action!(
    Command(CommandId),
    Shell(crate::shell::Action),
    Timeline(crate::timeline::Action),
    Composer(crate::composer::Action),
    Dock(crate::dock::Action),
    Overlays(crate::overlays::Action),
    Settings(crate::settings::Action),
);

impl From<Msg> for quark_app::quark_ui::Action {
    fn from(msg: Msg) -> Self {
        quark_app::quark_ui::Action::new(msg)
    }
}
