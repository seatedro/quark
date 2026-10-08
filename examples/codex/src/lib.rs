//! A recreation of the OpenAI Codex desktop app's UI in quark, with fake,
//! deterministic data. It follows ChatGPT 26.1007 in Codex mode (icon
//! rail, inset sidebar and main cards); surfaces that version was not
//! captured on (terminal, files, search palette, most settings pages)
//! follow Codex 26.623. See README.md for launch flags and scenes.
//!
//! One [`Codex`] holds every surface's state; each surface module renders
//! from it (`sidebar`, `home`, `composer`, `thread`, `menus`, `panel`,
//! `terminal`, `palette`, `settings`). Actions are one flat [`Msg`].

pub mod composer;
pub mod data;
pub mod home;
pub mod icons;
pub mod menus;
pub mod palette;
pub mod panel;
pub mod scenes;
pub mod settings;
pub mod sidebar;
pub mod terminal;
pub mod theme;
pub mod thread;
pub mod widgets;

use accesskit::Role;
use quark_app::quark_ui::FocusId;
use quark_app::quark_ui::element::*;
use quark_app::quark_ui::style::Styled;
use quark_app::quark_ui::text_input::{TextEditCommand, TextEditOutcome};
use quark_app::quark_ui::theme::{Color, ThemeMode};
use quark_app::{
    InputEvent, TrafficLights, UiAdapter, UiApp, UiContext, ViewContext, WindowChrome,
    WindowOptions,
};

use data::{Data, Item, ProjectId, Status, Step, ThreadId};
use theme::Pal;
use widgets::*;

pub const APP_TITLE: &str = "ChatGPT";
/// The live captures' window size.
pub const INITIAL_SIZE: (f64, f64) = (984.0, 738.0);
pub const MIN_SIZE: (f64, f64) = (480.0, 500.0);
/// The unified title bar.
pub const TITLE_H: f32 = 44.0;
/// The icon rail left of the cards.
pub const RAIL_W: f32 = 52.0;
/// The inset sidebar card's width (x 52 to 340).
pub const SIDEBAR_W: f32 = 288.0;
/// Frame visible right of and under the cards.
pub const FRAME_INSET: f32 = 3.0;
/// Below this width the sidebar hides on its own.
pub const SIDEBAR_BREAKPOINT: f32 = 720.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemeChoice {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TerminalMode {
    /// The user's login shell on a PTY.
    Real,
    /// The transcript of capture 38, no process.
    #[default]
    Scripted,
}

#[derive(Debug, Clone, Default)]
pub struct Options {
    pub theme: ThemeChoice,
    pub scene: Option<String>,
    pub terminal: TerminalMode,
}

impl Options {
    /// Flags, then `QUARK_CODEX_THEME`, `QUARK_CODEX_SCENE`, and
    /// `QUARK_CODEX_TERMINAL` for runners that pass no arguments.
    pub fn from_env() -> Result<Self, String> {
        let mut o = Options {
            terminal: TerminalMode::Real,
            ..Options::default()
        };
        let theme = |s: &str| match s {
            "system" => Ok(ThemeChoice::System),
            "light" => Ok(ThemeChoice::Light),
            "dark" => Ok(ThemeChoice::Dark),
            _ => Err(format!("unknown theme {s:?}")),
        };
        let terminal = |s: &str| match s {
            "real" => Ok(TerminalMode::Real),
            "scripted" => Ok(TerminalMode::Scripted),
            _ => Err(format!("unknown terminal {s:?}")),
        };
        if let Ok(v) = std::env::var("QUARK_CODEX_THEME") {
            o.theme = theme(&v)?;
        }
        o.scene = std::env::var("QUARK_CODEX_SCENE").ok();
        if let Ok(v) = std::env::var("QUARK_CODEX_TERMINAL") {
            o.terminal = terminal(&v)?;
        }
        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            let mut value = || args.next().ok_or_else(|| format!("{arg} needs a value"));
            match arg.as_str() {
                "--theme" => o.theme = theme(&value()?)?,
                "--scene" => o.scene = Some(value()?),
                "--terminal" => o.terminal = terminal(&value()?)?,
                "--list-scenes" => {
                    return Err(scenes::SCENES
                        .iter()
                        .map(|s| format!("{:<22} {}", s.0, s.1))
                        .collect::<Vec<_>>()
                        .join("\n"));
                }
                _ => {
                    return Err(format!(
                        "usage: codex-demo [--theme system|light|dark] [--scene NAME] \
                         [--terminal real|scripted] [--list-scenes]\nunknown argument {arg:?}"
                    ));
                }
            }
        }
        Ok(o)
    }
}

/// Where the main card is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Home,
    Thread(ThreadId),
    Settings(settings::Page),
}

/// Which popover is open. Codex shows one at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Menu {
    Permissions,
    Model,
    Add,
    Mode,
    Profile,
    ChatActions,
    Summary,
    ApprovalOptions,
    ChangesScope,
    ChangesOptions,
    PanelTab,
    WorkIn,
    ProjectPicker,
}

/// A tab of the side panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Changes,
    Terminal,
    File,
}

pub const SCOPES: [&str; 6] = [
    "Last Turn",
    "Uncommitted",
    "Unstaged",
    "Staged",
    "Committed",
    "Branch",
];

pub const APPROVALS: [(&str, &str, &str); 3] = [
    (
        "Ask for approval",
        "Always ask to edit external files and use the internet",
        icons::HAND,
    ),
    (
        "Approve for me",
        "Only ask for actions detected as potentially unsafe",
        icons::SHIELD_TERM,
    ),
    (
        "Full access",
        "Unrestricted access to the internet and any file on your computer",
        icons::SHIELD_ALERT,
    ),
];
/// The effort slider's five stops.
pub const EFFORTS: [&str; 5] = ["Light", "Low", "Medium", "High", "Extra High"];
pub const MODEL: &str = "GPT-6.1 Sol";

#[derive(Debug, Clone, PartialEq)]
pub enum Msg {
    NewChat,
    Select(ThreadId),
    Show(Screen),
    ToggleSidebar,
    Open(Menu),
    CloseMenus,
    Approval(usize),
    Effort(usize),
    ChooseProject(Option<ProjectId>),
    Send,
    Stop,
    FocusComposer,
    ComposerScroll(i32),
    /// Pick slash command or `@` result `n`.
    PickSuggestion(usize),
    /// Fold or unfold transcript item `n`'s work.
    ToggleWork(usize),
    /// Fold or unfold item `n`'s step `m` (a group).
    ToggleGroup(usize, usize),
    /// Fold or unfold item `n`, step `m`, row `k` (shell card, diff).
    ToggleRow(usize, usize, usize),
    /// Answer the pending approval: allow or deny.
    Decide(bool),
    ToggleActivity,
    ToggleSidePanel,
    ShowTab(Tab),
    OpenTab(Tab),
    CloseTab(Tab),
    FullView,
    SetScope(usize),
    OpenFile(&'static str),
    Palette(bool),
    SetTheme(ThemeChoice),
    SetToggle(&'static str),
    Terminal(quark_terminal::TerminalEvent),
    Noop,
}

impl From<Msg> for quark_app::quark_ui::Action {
    fn from(msg: Msg) -> Self {
        quark_app::quark_ui::Action::new(msg)
    }
}

pub struct Codex {
    pub options: Options,
    pub data: Data,
    pub screen: Screen,
    /// The user's sidebar choice; narrow windows hide it regardless.
    pub sidebar_open: bool,
    /// The bell's activity list replaces the projects and recents.
    pub activity: bool,
    pub menu: Option<Menu>,
    /// Keyboard highlight in the open menu.
    pub menu_hi: Option<usize>,
    pub approval: usize,
    pub effort: usize,
    /// The project a new chat starts in.
    pub project: Option<ProjectId>,
    pub composer: composer::State,
    /// A command waiting for approval; replaces the composer.
    pub pending: Option<data::Approval>,
    pub side_panel: bool,
    pub tabs: Vec<Tab>,
    pub tab: Tab,
    /// The side panel fills the window, the thread becomes a tab.
    pub full_view: bool,
    pub scope: usize,
    pub open_file: Option<&'static str>,
    pub palette: Option<palette::State>,
    pub thread_scroll: ScrollHandle,
    /// Scroll the transcript to the latest turn on the next frame.
    pub stick_bottom: bool,
    /// A scene's exact transcript offset, instead of the latest turn.
    pub scroll_px: Option<f32>,
    pub settings_handle: ScrollHandle,
    pub terminal: terminal::State,
    pub theme_choice: ThemeChoice,
    pub toggles: Vec<&'static str>,
    /// The window size as of the last frame.
    pub size: (f32, f32),
    /// Milliseconds since launch, for the shimmer and the fake run.
    pub now_ms: u64,
    /// When the running turn started.
    pub run_started: Option<u64>,
}

impl Codex {
    pub fn new(options: Options) -> Self {
        let terminal = terminal::State::new(options.terminal);
        let mut app = Self {
            data: Data::new(),
            screen: Screen::Home,
            sidebar_open: true,
            activity: false,
            menu: None,
            menu_hi: None,
            approval: 0,
            effort: 0,
            project: Some(ProjectId(1)),
            composer: composer::State::new(),
            pending: None,
            side_panel: false,
            tabs: vec![Tab::Changes],
            tab: Tab::Changes,
            full_view: false,
            scope: 0,
            open_file: None,
            palette: None,
            thread_scroll: ScrollHandle::new(),
            stick_bottom: true,
            scroll_px: None,
            settings_handle: ScrollHandle::new(),
            terminal,
            theme_choice: options.theme,
            toggles: vec![
                "default-permissions",
                "full-access",
                "translucent",
                "smart-punctuation",
            ],
            size: (INITIAL_SIZE.0 as f32, INITIAL_SIZE.1 as f32),
            now_ms: 0,
            run_started: None,
            options,
        };
        if let Some(name) = app.options.scene.clone() {
            scenes::apply(&mut app, &name);
        }
        app
    }

    pub fn sidebar_shown(&self) -> bool {
        self.sidebar_open && self.size.0 >= SIDEBAR_BREAKPOINT && !self.full_view
    }

    pub fn current_thread(&self) -> Option<&data::Thread> {
        match self.screen {
            Screen::Thread(id) => self.data.thread(id),
            _ => None,
        }
    }

    pub fn running(&self) -> bool {
        self.current_thread()
            .is_some_and(|t| matches!(t.status, Status::Running | Status::Awaiting))
    }

    fn send(&mut self) {
        let text = self.composer.editor.text().trim().to_owned();
        if text.is_empty() || self.running() {
            return;
        }
        let id = match self.screen {
            Screen::Thread(id) => id,
            _ => {
                let id = ThreadId(1000 + self.data.threads.len() as u32);
                let title: String = text.chars().take(48).collect();
                self.data.threads.insert(
                    0,
                    data::Thread {
                        id,
                        project: self.project,
                        title,
                        status: Status::Running,
                        items: Vec::new(),
                        pill: None,
                    },
                );
                self.screen = Screen::Thread(id);
                id
            }
        };
        if let Some(t) = self.data.thread_mut(id) {
            t.items.push(Item::User {
                text,
                time: "11:08 PM",
            });
            t.items.push(Item::Starting);
            t.status = Status::Running;
        }
        self.composer.editor.set_text("");
        self.run_started = Some(self.now_ms);
        self.stick_bottom = true;
    }

    /// Advance the fake run: starting, thinking, a streamed preamble, a
    /// live command row, then a finished turn with its answer.
    fn pump(&mut self) -> bool {
        let Some(start) = self.run_started else {
            return false;
        };
        let Screen::Thread(id) = self.screen else {
            return false;
        };
        let t = self.now_ms.saturating_sub(start);
        let stage = match t {
            0..600 => 0,
            600..1800 => 1,
            1800..3000 => 2,
            3000..4500 => 3,
            _ => 4,
        };
        let Some(thread) = self.data.thread_mut(id) else {
            return false;
        };
        let at = thread
            .items
            .iter()
            .rposition(|i| matches!(i, Item::User { .. }))
            .map_or(0, |i| i + 1);
        let before = thread.items.len();
        thread.items.truncate(at);
        if stage < 4 {
            thread
                .items
                .extend(data::running_turn(stage).into_iter().skip(1));
        } else {
            let mut done = data::turn1(false, false, false);
            done.remove(0);
            thread.items.extend(done);
            thread.status = Status::Idle;
            self.run_started = None;
        }
        self.stick_bottom = true;
        before != thread.items.len() || stage == 4
    }

    fn close_menus(&mut self) {
        self.menu = None;
        self.menu_hi = None;
    }

    /// The step tree under item `item` of the shown thread.
    fn steps_mut(&mut self, item: usize) -> Option<&mut Vec<Step>> {
        let Screen::Thread(id) = self.screen else {
            return None;
        };
        match self.data.thread_mut(id)?.items.get_mut(item)? {
            Item::Work { steps, .. } => Some(steps),
            _ => None,
        }
    }
}

/// `app` with the Codex themes for its theme choice.
pub fn adapter(app: Codex) -> UiAdapter<Codex> {
    let (light, dark) = themes_for(app.options.theme);
    UiAdapter::new(app, APP_TITLE).with_themes(light, dark)
}

pub fn themes_for(
    choice: ThemeChoice,
) -> (
    quark_app::quark_ui::theme::Theme,
    quark_app::quark_ui::theme::Theme,
) {
    let light = theme::quark_theme(ThemeMode::Light);
    let dark = theme::quark_theme(ThemeMode::Dark);
    match choice {
        ThemeChoice::System => (light, dark),
        ThemeChoice::Light => (light.clone(), light),
        ThemeChoice::Dark => (dark.clone(), dark),
    }
}

/// Custom chrome everywhere: macOS keeps its traffic lights, inset to
/// where the app has them; elsewhere the app draws look-alikes.
pub fn window_options() -> WindowOptions {
    let mut fonts = quark_text::FontSettings::default();
    // The closest bundled face to SF Pro, which the app uses.
    fonts.ui_family = quark_text::fonts::INTER_FAMILY.to_owned();
    WindowOptions {
        title: APP_TITLE.into(),
        size: INITIAL_SIZE,
        min_size: Some(MIN_SIZE),
        chrome: WindowChrome::Custom,
        traffic_lights: Some(TrafficLights {
            left_margin: 15.0,
            center_y: 22.0,
        }),
        fonts,
        ..WindowOptions::default()
    }
}

pub const COMPOSER_FOCUS: FocusId = FocusId::from_key("codex.composer");
pub const PALETTE_FOCUS: FocusId = FocusId::from_key("codex.palette");

/// The cards' layout for a window: where the sidebar, main, and panel
/// cards sit.
#[derive(Debug, Clone, Copy)]
pub struct Frame {
    pub w: f32,
    pub h: f32,
    /// Left edge of the card group (the rail's right edge).
    pub left: f32,
    pub sidebar_w: f32,
    pub main_x: f32,
    pub main_w: f32,
    pub panel_x: f32,
    pub panel_w: f32,
    pub top: f32,
    pub bottom: f32,
}

impl Frame {
    pub fn card_h(&self) -> f32 {
        self.bottom - self.top
    }
}

pub fn frame(app: &Codex) -> Frame {
    let (w, h) = app.size;
    let left = RAIL_W;
    let right = w - FRAME_INSET;
    let sidebar_w = if app.sidebar_shown() && !matches!(app.screen, Screen::Settings(_))
        || matches!(app.screen, Screen::Settings(_)) && w >= SIDEBAR_BREAKPOINT
    {
        SIDEBAR_W
    } else {
        0.0
    };
    let main_x = left + sidebar_w;
    let avail = (right - main_x).max(0.0);
    let panel_w = panel::width(app, avail);
    Frame {
        w,
        h,
        left,
        sidebar_w,
        main_x,
        main_w: avail - panel_w,
        panel_x: right - panel_w,
        panel_w,
        top: TITLE_H,
        bottom: h - FRAME_INSET,
    }
}

impl UiApp for Codex {
    type Action = Msg;
    type Message = ();

    fn init(&mut self, cx: &mut UiContext) {
        self.terminal.init(cx);
        cx.set_focus(Some(COMPOSER_FOCUS));
    }

    fn view(&mut self, vcx: &mut ViewContext) -> AnyElement {
        let size = vcx.frame.size();
        self.size = size;
        self.now_ms = vcx.frame.elapsed().as_millis() as u64;
        if self.pump() {
            vcx.frame.request_frame();
        }
        let p: &Pal = theme::pal(vcx.theme.mode);
        let f = frame(self);
        let (w, h) = size;

        let mut cards = div()
            .absolute()
            .left(f.left)
            .top(f.top)
            .w(w - f.left - FRAME_INSET)
            .h(f.card_h())
            .rounded(10.0)
            .border(p.frame_border)
            .overflow_hidden()
            .bg(p.bg);
        let local = |x: f32| x - f.left;
        if let Screen::Settings(page) = self.screen {
            cards = cards.child(settings::view(self, page, p, &f, vcx));
        } else {
            if f.sidebar_w > 0.0 {
                cards = cards.child(sidebar::view(self, p, f.sidebar_w, f.card_h()));
            }
            let main = match self.screen {
                Screen::Home => home::view(self, p, (local(f.main_x), f.main_w, f.card_h()), vcx),
                Screen::Thread(id) => {
                    thread::view(self, id, p, (local(f.main_x), f.main_w, f.card_h()), vcx)
                }
                Screen::Settings(_) => unreachable!(),
            };
            cards = cards.child(main);
            if f.panel_w > 0.0 {
                cards = cards.child(panel::view(
                    self,
                    p,
                    (local(f.panel_x), f.panel_w, f.card_h()),
                    vcx,
                ));
            }
        }
        let mut root = div()
            .relative()
            .w(w)
            .h(h)
            .bg(p.frame)
            .overflow_hidden()
            .accessibility_role(Role::Window)
            .accessibility_label(APP_TITLE)
            .child(rail(self, p, h))
            .child(cards)
            .child(title_bar(self, p, &f));
        if !cfg!(target_os = "macos") {
            root = root.child(traffic_lights());
        }
        if let Some(menu) = menus::view(self, p, vcx) {
            if self.menu.is_some() {
                root = root.child(click_catcher(w, h, Msg::CloseMenus));
            }
            root = root.child(menu);
        }
        if self.palette.is_some() {
            root = root.child(palette::view(self, p, vcx));
        }
        if self.run_started.is_some() || self.has_live_text() {
            // The shimmer moves every frame; the fake run steps on its own.
            vcx.frame
                .request_frame_in(std::time::Duration::from_millis(33));
        }
        root.into_any()
    }

    fn update(&mut self, msg: Msg, cx: &mut UiContext) {
        self.now_ms = cx.window.elapsed().as_millis() as u64;
        let keep = matches!(
            msg,
            Msg::Open(_) | Msg::Terminal(_) | Msg::ComposerScroll(_) | Msg::Noop | Msg::Effort(_)
        );
        if !keep {
            self.close_menus();
        }
        match msg {
            Msg::NewChat => {
                self.screen = Screen::Home;
                self.palette = None;
                self.side_panel = false;
                self.full_view = false;
                cx.set_focus(Some(COMPOSER_FOCUS));
            }
            Msg::Select(id) => {
                self.screen = Screen::Thread(id);
                self.palette = None;
                self.stick_bottom = true;
            }
            Msg::Show(screen) => {
                self.screen = screen;
                self.palette = None;
                self.settings_handle.set_offset(0.0, 0.0);
            }
            Msg::ToggleSidebar => self.sidebar_open = !self.sidebar_open,
            Msg::Open(menu) => {
                self.menu = if self.menu == Some(menu) {
                    None
                } else {
                    Some(menu)
                };
                self.menu_hi = None;
            }
            Msg::CloseMenus => {}
            Msg::Approval(i) => self.approval = i,
            Msg::Effort(i) => self.effort = i,
            Msg::ChooseProject(project) => {
                self.project = project;
                self.screen = Screen::Home;
            }
            Msg::Send => self.send(),
            Msg::Stop => {
                self.run_started = None;
                if let Screen::Thread(id) = self.screen
                    && let Some(t) = self.data.thread_mut(id)
                {
                    t.items
                        .retain(|i| !matches!(i, Item::Thinking | Item::Starting));
                    t.status = Status::Idle;
                }
            }
            Msg::FocusComposer => cx.set_focus(Some(COMPOSER_FOCUS)),
            Msg::ComposerScroll(lines) => {
                let step = self.composer.editor.scroll_line_height_px();
                self.composer.editor.scroll(lines as f32 * step);
            }
            Msg::PickSuggestion(i) => {
                self.composer.pick(i);
                cx.set_focus(Some(COMPOSER_FOCUS));
            }
            Msg::ToggleWork(item) => {
                if let Screen::Thread(id) = self.screen
                    && let Some(Item::Work { open, .. }) =
                        self.data.thread_mut(id).and_then(|t| t.items.get_mut(item))
                {
                    *open = !*open;
                }
            }
            Msg::ToggleGroup(item, step) => {
                if let Some(Step::Group { open, .. }) =
                    self.steps_mut(item).and_then(|s| s.get_mut(step))
                {
                    *open = !*open;
                }
            }
            Msg::ToggleRow(item, step, row) => {
                let target =
                    self.steps_mut(item)
                        .and_then(|s| s.get_mut(step))
                        .and_then(|s| match s {
                            Step::Group { rows, .. } => rows.get_mut(row),
                            Step::Row(r) => Some(r),
                            _ => None,
                        });
                match target {
                    Some(data::Row::Ran {
                        open,
                        shell: Some(_),
                        ..
                    })
                    | Some(data::Row::Edited { open, .. }) => *open = !*open,
                    _ => {}
                }
            }
            Msg::Decide(allow) => {
                self.pending = None;
                if let Screen::Thread(id) = self.screen
                    && let Some(t) = self.data.thread_mut(id)
                {
                    t.status = Status::Idle;
                    if let Some(Item::Work {
                        running,
                        open,
                        steps,
                        took,
                    }) = t.items.last_mut()
                    {
                        *running = false;
                        *open = false;
                        *took = "4m 8s";
                        steps.retain(|s| !matches!(s, Step::Live { .. }));
                    }
                    let answer: &'static str = if allow {
                        "`npm view left-pad version` returned **1.3.0**."
                    } else {
                        "I didn’t run the command: the network request was declined."
                    };
                    t.items.push(Item::Answer {
                        blocks: vec![thread::parse_inline(answer)],
                        time: "11:09 PM",
                    });
                }
                self.stick_bottom = true;
                cx.set_focus(Some(COMPOSER_FOCUS));
            }
            Msg::ToggleActivity => self.activity = !self.activity,
            Msg::ToggleSidePanel => {
                self.side_panel = !self.side_panel;
                if !self.side_panel {
                    self.full_view = false;
                }
            }
            Msg::ShowTab(tab) => {
                self.tab = tab;
                if tab == Tab::Terminal {
                    cx.set_focus(Some(terminal::FOCUS));
                }
            }
            Msg::OpenTab(tab) => {
                self.side_panel = true;
                if !self.tabs.contains(&tab) {
                    self.tabs.push(tab);
                }
                self.tab = tab;
                if tab == Tab::Terminal {
                    cx.set_focus(Some(terminal::FOCUS));
                }
            }
            Msg::CloseTab(tab) => {
                self.tabs.retain(|t| *t != tab);
                if let Some(first) = self.tabs.first() {
                    self.tab = *first;
                } else {
                    self.side_panel = false;
                    self.full_view = false;
                }
            }
            Msg::FullView => self.full_view = !self.full_view,
            Msg::SetScope(scope) => self.scope = scope,
            Msg::OpenFile(name) => {
                self.open_file = Some(name);
                if !self.tabs.contains(&Tab::File) {
                    self.tabs.push(Tab::File);
                }
                self.tab = Tab::File;
                self.side_panel = true;
            }
            Msg::Palette(open) => {
                self.palette = open.then(palette::State::new);
                cx.set_focus(if open {
                    Some(PALETTE_FOCUS)
                } else {
                    Some(COMPOSER_FOCUS)
                });
            }
            Msg::SetTheme(choice) => {
                self.theme_choice = choice;
                let (light, dark) = themes_for(choice);
                cx.set_themes(light, dark);
            }
            Msg::SetToggle(key) => {
                if let Some(at) = self.toggles.iter().position(|k| *k == key) {
                    self.toggles.remove(at);
                } else {
                    self.toggles.push(key);
                }
            }
            Msg::Terminal(event) => self.terminal.handle(event, cx),
            Msg::Noop => {}
        }
        cx.window.request_redraw();
    }

    fn event(&mut self, event: &InputEvent, cx: &mut UiContext) -> bool {
        if self.side_panel && self.tab == Tab::Terminal && self.terminal.input(event, cx) {
            return true;
        }
        let InputEvent::KeyPress(chord) = event else {
            return false;
        };
        let Some(b) = chord.binding() else {
            return false;
        };
        let m = b.mods;
        let cmd = m.cmd || m.ctrl;
        match b.key.as_str() {
            "escape" if self.pending.is_some() && self.menu.is_none() => {
                self.update(Msg::Decide(false), cx);
            }
            "enter" if self.pending.is_some() && cx.focus() != Some(PALETTE_FOCUS) => {
                self.update(Msg::Decide(true), cx);
            }
            "escape" if self.menu.is_some() || self.palette.is_some() => {
                self.close_menus();
                self.palette = None;
            }
            "k" | "p" if cmd && !m.shift => {
                self.palette = Some(palette::State::new());
                cx.set_focus(Some(PALETTE_FOCUS));
            }
            "n" if cmd => {
                self.screen = Screen::Home;
                cx.set_focus(Some(COMPOSER_FOCUS));
            }
            "b" if cmd => self.sidebar_open = !self.sidebar_open,
            "," if cmd => self.screen = Screen::Settings(settings::Page::General),
            "j" if cmd => self.side_panel = !self.side_panel,
            "down" if self.menu.is_some() => {
                self.menu_hi = Some(self.menu_hi.map_or(0, |i| i + 1));
            }
            "up" if self.menu.is_some() => {
                self.menu_hi = Some(self.menu_hi.map_or(0, |i| i.saturating_sub(1)));
            }
            "enter" if cx.focus() == Some(COMPOSER_FOCUS) && !m.shift => {
                if self.composer.suggestions_open() {
                    self.composer.pick(self.composer.hi);
                } else {
                    self.send();
                }
            }
            "down" if cx.focus() == Some(COMPOSER_FOCUS) && self.composer.suggestions_open() => {
                self.composer.hi += 1;
            }
            "up" if cx.focus() == Some(COMPOSER_FOCUS) && self.composer.suggestions_open() => {
                self.composer.hi = self.composer.hi.saturating_sub(1);
            }
            "enter" if cx.focus() == Some(PALETTE_FOCUS) => {
                if let Some(id) = self
                    .palette
                    .as_ref()
                    .and_then(|s| s.first_match(&self.data))
                {
                    self.screen = Screen::Thread(id);
                }
                self.palette = None;
            }
            _ => return false,
        }
        cx.window.request_redraw();
        true
    }

    fn edit_text(&mut self, target: FocusId, command: TextEditCommand) -> TextEditOutcome {
        if target == COMPOSER_FOCUS {
            self.composer.edit(command, self.now_ms)
        } else if target == PALETTE_FOCUS
            && let Some(palette) = &mut self.palette
        {
            palette.field.apply_at(command, self.now_ms)
        } else {
            TextEditOutcome::default()
        }
    }

    fn wake(&mut self, cx: &mut UiContext) {
        self.terminal.wake(cx);
    }
}

impl Codex {
    /// Whether the shown thread has shimmering text that must animate.
    fn has_live_text(&self) -> bool {
        self.current_thread().is_some_and(|t| {
            t.items.iter().any(|i| match i {
                Item::Thinking | Item::Starting => true,
                Item::Work {
                    steps,
                    running: true,
                    ..
                } => steps.iter().any(|s| matches!(s, Step::Live { .. })),
                _ => false,
            })
        })
    }
}

/// The icon rail: Home (selected), Space, Scheduled, Plugins, Explore,
/// then Code Review and Sites, and the profile avatar at the bottom.
fn rail(app: &Codex, p: &Pal, h: f32) -> Div {
    let item = |svg: &'static str, label: &str, y: f32, selected: bool, dot: bool, msg: Msg| {
        let mut b = div()
            .absolute()
            .left(8.0)
            .top(y)
            .w(36.0)
            .h(36.0)
            .items_center()
            .justify_center()
            .rounded(9.0)
            .hover_bg(p.rail_tile.with_alpha(150))
            .accessibility_role(Role::Button)
            .accessibility_label(label.to_owned())
            .on_click(msg)
            .child(ico(svg, 19.0, if selected { p.text } else { p.rail_icon }));
        if selected {
            b = b.bg(p.rail_tile);
        }
        if dot {
            b = b.child(
                div()
                    .absolute()
                    .left(24.0)
                    .top(5.0)
                    .w(7.0)
                    .h(7.0)
                    .rounded(4.0)
                    .bg(p.accent),
            );
        }
        b
    };
    let home = matches!(app.screen, Screen::Home | Screen::Thread(_));
    let _ = home;
    div()
        .absolute()
        .left(0.0)
        .top(0.0)
        .w(RAIL_W)
        .h(h)
        .accessibility_role(Role::Navigation)
        .accessibility_label("Rail")
        .child(item(icons::HOME, "Home", 52.0, home, true, Msg::NewChat))
        .child(item(icons::SPACE, "Space", 96.0, false, false, Msg::Noop))
        .child(item(
            icons::CLOCK,
            "Scheduled",
            140.0,
            false,
            false,
            Msg::Noop,
        ))
        .child(item(icons::AT, "Plugins", 184.0, false, false, Msg::Noop))
        .child(item(
            icons::ELLIPSIS,
            "Explore",
            228.0,
            false,
            true,
            Msg::Noop,
        ))
        .child(
            div()
                .absolute()
                .left(14.0)
                .top(272.0)
                .w(24.0)
                .h(1.0)
                .bg(p.frame_border),
        )
        .child(item(
            icons::PR,
            "Code Review",
            281.0,
            false,
            false,
            Msg::Noop,
        ))
        .child(item(icons::SITES, "Sites", 325.0, false, false, Msg::Noop))
        .child(
            div()
                .absolute()
                .left(14.0)
                .top(h - 38.0)
                .w(24.0)
                .h(24.0)
                .rounded(12.0)
                .bg(p.avatar)
                .items_center()
                .justify_center()
                .id("rail.profile")
                .accessibility_role(Role::Button)
                .accessibility_label("Open profile menu")
                .on_click(Msg::Open(Menu::Profile))
                .child(txt("SE", 9.0, Color::rgba(255, 255, 255, 255))),
        )
}

/// The unified title bar: history and sidebar controls beside the
/// traffic lights, the chat toolbar over the main card, the panel's tab
/// strip over the panel card, and the new-tab button.
fn title_bar(app: &Codex, p: &Pal, f: &Frame) -> Div {
    let mut bar = div()
        .absolute()
        .left(0.0)
        .top(0.0)
        .w(f.w)
        .h(TITLE_H)
        .z_index(20);
    let mut left = hrow()
        .absolute()
        .left(88.0)
        .top(8.0)
        .h(28.0)
        .gap(4.0)
        .child(icon_button(
            p,
            icons::ARROW_LEFT,
            28.0,
            16.0,
            p.icon,
            "Back",
            Msg::Noop,
        ))
        .child(icon_button(
            p,
            icons::ARROW_RIGHT,
            28.0,
            16.0,
            p.icon_faint,
            "Forward",
            Msg::Noop,
        ));
    let settings = matches!(app.screen, Screen::Settings(_));
    if !settings {
        left = left.child(div().w(8.0)).child(icon_button(
            p,
            icons::SIDEBAR,
            28.0,
            16.0,
            p.icon,
            "Hide sidebar",
            Msg::ToggleSidebar,
        ));
    }
    let collapsed = f.sidebar_w == 0.0;
    if collapsed && !matches!(app.screen, Screen::Settings(_)) {
        left = left
            .child(div().w(16.0))
            .child(icon_button(
                p,
                icons::COMPOSE,
                28.0,
                16.0,
                p.icon,
                "New chat",
                Msg::NewChat,
            ))
            .child(div().w(10.0))
            .child(div().w(1.0).h(16.0).bg(p.frame_border));
    }
    bar = bar.child(left);
    let toolbar_x = if collapsed { 232.0 } else { f.main_x };
    if let Some(t) = app.current_thread()
        && !app.full_view
    {
        let right_edge = if f.panel_w > 0.0 {
            f.panel_x
        } else {
            f.w - 40.0
        };
        let title_w = (right_edge - toolbar_x - 180.0).max(40.0);
        bar = bar.child(
            hrow()
                .absolute()
                .left(toolbar_x)
                .top(8.0)
                .w(right_edge - toolbar_x)
                .h(28.0)
                .pl(8.0)
                .accessibility_role(Role::Toolbar)
                .accessibility_label("Chat toolbar")
                .child(
                    div()
                        .w(28.0)
                        .h(28.0)
                        .items_center()
                        .justify_center()
                        .child(ico(icons::FOLDER, 16.0, p.text)),
                )
                .child(div().w(8.0))
                .child(
                    div().max_w(title_w).min_w(0.0).overflow_hidden().child(
                        text(t.title.clone())
                            .size(theme::BODY)
                            .medium()
                            .color(p.text)
                            .no_wrap()
                            .truncate(),
                    ),
                )
                .child(div().flex_1().min_w(8.0))
                .child(
                    icon_button(
                        p,
                        icons::ELLIPSIS,
                        28.0,
                        16.0,
                        p.icon,
                        "Chat actions",
                        Msg::Open(Menu::ChatActions),
                    )
                    .id("header.actions"),
                )
                .child(div().w(6.0))
                .child(
                    icon_button(
                        p,
                        icons::SUMMARY,
                        28.0,
                        16.0,
                        p.icon,
                        "Toggle summary",
                        Msg::Open(Menu::Summary),
                    )
                    .id("header.summary"),
                )
                .child(div().w(6.0)),
        );
    }
    if settings {
    } else if f.panel_w > 0.0 || app.full_view {
        bar = bar.child(panel::tab_strip(app, p, f));
    } else {
        bar = bar.child(
            div().absolute().left(f.w - 36.0).top(8.0).child(
                icon_button(
                    p,
                    icons::NEW_TAB,
                    28.0,
                    16.0,
                    p.icon,
                    "New tab",
                    Msg::Open(Menu::PanelTab),
                )
                .id("panel.newtab"),
            ),
        );
    }
    bar
}

/// macOS traffic lights, drawn where the platform has no native ones.
fn traffic_lights() -> Div {
    let light = |c: u32| {
        div().w(14.0).h(14.0).rounded(7.0).bg(Color::rgba(
            (c >> 16) as u8,
            (c >> 8) as u8,
            c as u8,
            255,
        ))
    };
    hrow()
        .absolute()
        .left(16.0)
        .top(15.0)
        .gap(9.0)
        .z_index(80)
        .child(light(0xf35a50))
        .child(light(0xfdbd01))
        .child(light(0x02bd00))
}
