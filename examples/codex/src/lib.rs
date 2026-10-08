//! A recreation of the OpenAI Codex desktop app's UI in quark, with fake,
//! deterministic data. See README.md for launch flags and scenes.
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

use data::{Data, Item, ProjectId, Status, ThreadId};
use theme::Pal;
use widgets::*;

pub const APP_TITLE: &str = "Codex";
/// The live captures' window size.
pub const INITIAL_SIZE: (f64, f64) = (983.0, 738.0);
pub const MIN_SIZE: (f64, f64) = (480.0, 500.0);
pub const SIDEBAR_W: f32 = 300.0;
pub const HEADER_H: f32 = 46.0;
/// Below this width the sidebar hides on its own (capture 62).
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
                        .map(|s| s.0)
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

/// Where the main pane is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Home,
    Thread(ThreadId),
    Settings(settings::Page),
}

/// Which popover is open. Codex shows one at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Menu {
    Approval,
    Model,
    ModelList,
    Speed,
    Add,
    ChooseProject,
    NewProject,
    WorkLocation,
    Usage,
    Branch,
    HeadlineProject,
    ChatActions,
    OpenIn,
    Summary,
    ReviewScope,
    PanelTab,
    Account,
    ProjectActions(ProjectId),
    SidebarOptions,
    ThreadContext(ThreadId),
}

/// A tab of the right-hand side panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Review,
    Terminal,
    File,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Unstaged,
    Staged,
    Branch,
    LastTurn,
}

impl Scope {
    pub fn label(self) -> &'static str {
        match self {
            Scope::Unstaged => "Unstaged",
            Scope::Staged => "Staged",
            Scope::Branch => "Branch",
            Scope::LastTurn => "Last turn",
        }
    }
}

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
pub const REASONING: [&str; 4] = ["Light", "Medium", "High", "Extra High"];
/// Model names and the short label the pill shows.
pub const MODELS: [(&str, &str); 4] = [
    ("GPT-5.4", "5.4"),
    ("GPT-5.4-Mini", "5.4-Mini"),
    ("GPT-5.3-Codex", "5.3-Codex"),
    ("GPT-5.2", "5.2"),
];

#[derive(Debug, Clone, PartialEq)]
pub enum Msg {
    NewChat,
    Select(ThreadId),
    Show(Screen),
    ToggleSidebar,
    Open(Menu),
    CloseMenus,
    Approval(usize),
    Reasoning(usize),
    Model(usize),
    Speed(usize),
    ToggleProject(ProjectId),
    ChooseProject(Option<ProjectId>),
    DismissBanner,
    Send,
    Stop,
    FocusComposer,
    ComposerScroll(i32),
    /// Pick slash command or `@` result `n`.
    PickSuggestion(usize),
    ToggleSidePanel,
    ShowTab(Tab),
    OpenTab(Tab),
    ExpandPanel,
    ToggleSplit,
    ToggleFileList,
    SetScope(Scope),
    OpenFile(&'static str),
    Palette(bool),
    PaletteScroll(i32),
    ThreadScroll(i32),
    SettingsScroll(i32),
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
    pub menu: Option<Menu>,
    /// Keyboard highlight in the open menu (capture 05b).
    pub menu_hi: Option<usize>,
    pub approval: usize,
    pub reasoning: usize,
    pub model: usize,
    pub speed: usize,
    /// The project a new chat starts in.
    pub project: Option<ProjectId>,
    pub banner: bool,
    pub collapsed: Vec<ProjectId>,
    pub composer: composer::State,
    pub side_panel: bool,
    pub tabs: Vec<Tab>,
    pub tab: Tab,
    pub panel_expanded: bool,
    pub split: bool,
    pub file_list: bool,
    pub scope: Scope,
    pub review_loading: bool,
    pub open_file: Option<&'static str>,
    pub palette: Option<palette::State>,
    pub thread_scroll: ScrollHandle,
    /// Keep the transcript scrolled to its end (until the user scrolls).
    pub stick_bottom: bool,
    pub settings_scroll: f32,
    pub settings_handle: ScrollHandle,
    pub terminal: terminal::State,
    pub theme_choice: ThemeChoice,
    pub toggles: Vec<&'static str>,
    /// The window size as of the last frame.
    pub size: (f32, f32),
    /// Milliseconds since launch, for the shimmer.
    pub now_ms: u64,
    /// When the running turn started (it fails two seconds later, as the
    /// captured account's turns did).
    pub run_started: Option<u64>,
}

impl Codex {
    pub fn new(options: Options) -> Self {
        let terminal = terminal::State::new(options.terminal);
        let mut app = Self {
            data: Data::new(),
            screen: Screen::Home,
            sidebar_open: true,
            menu: None,
            menu_hi: None,
            approval: 0,
            reasoning: 1,
            model: 0,
            speed: 0,
            project: None,
            banner: false,
            collapsed: Vec::new(),
            composer: composer::State::new(),
            side_panel: false,
            tabs: vec![Tab::Review],
            tab: Tab::Review,
            panel_expanded: false,
            split: false,
            file_list: false,
            scope: Scope::Unstaged,
            review_loading: false,
            open_file: None,
            palette: None,
            thread_scroll: ScrollHandle::new(),
            stick_bottom: true,
            settings_scroll: 0.0,
            settings_handle: ScrollHandle::new(),
            terminal,
            theme_choice: options.theme,
            toggles: vec!["auto-review", "full-access", "translucent"],
            size: (INITIAL_SIZE.0 as f32, INITIAL_SIZE.1 as f32),
            now_ms: 0,
            run_started: None,
            options,
        };
        // A plain launch opens on the captured thread; scenes start from
        // the state their capture shows.
        match app.options.scene.clone() {
            Some(name) => scenes::apply(&mut app, &name),
            None => app.screen = Screen::Thread(ThreadId(1)),
        }
        app
    }

    pub fn sidebar_shown(&self) -> bool {
        self.sidebar_open && self.size.0 >= SIDEBAR_BREAKPOINT && !self.panel_expanded
    }

    pub fn current_thread(&self) -> Option<&data::Thread> {
        match self.screen {
            Screen::Thread(id) => self.data.thread(id),
            _ => None,
        }
    }

    pub fn running(&self) -> bool {
        self.current_thread()
            .is_some_and(|t| t.items.last() == Some(&Item::Thinking))
    }

    /// The model pill's two parts.
    pub fn model_label(&self) -> (&'static str, &'static str) {
        (MODELS[self.model].1, REASONING[self.reasoning])
    }

    fn send(&mut self) {
        let text = self.composer.editor.text().trim().to_owned();
        if text.is_empty() || self.running() {
            return;
        }
        let id = match self.screen {
            Screen::Thread(id) => id,
            _ => {
                let project = self.project.unwrap_or(ProjectId(1));
                let id = ThreadId(1000 + self.data.threads.len() as u32);
                self.data.threads.insert(
                    0,
                    data::Thread {
                        id,
                        project,
                        title: text.clone(),
                        age: "now",
                        status: Status::Running,
                        cloud: false,
                        items: Vec::new(),
                    },
                );
                self.screen = Screen::Thread(id);
                id
            }
        };
        if let Some(t) = self.data.thread_mut(id) {
            t.items.push(Item::User {
                text,
                time: "10:25 PM",
            });
            t.items.push(Item::Thinking);
            t.status = Status::Running;
        }
        self.composer.editor.set_text("");
        self.run_started = Some(self.now_ms);
        self.stick_bottom = true;
    }

    /// Resolve a running turn the way the captured account's did: an
    /// unsupported-model error after two seconds.
    fn pump(&mut self) -> bool {
        let Some(start) = self.run_started else {
            return false;
        };
        if self.now_ms < start + 2000 {
            return false;
        }
        self.run_started = None;
        let model = MODELS[self.model].0.to_lowercase();
        if let Screen::Thread(id) = self.screen
            && let Some(t) = self.data.thread_mut(id)
        {
            t.items.retain(|i| *i != Item::Thinking);
            t.items.push(Item::Error(format!(
                "The '{model}' model is not supported when using Codex with a ChatGPT account."
            )));
            t.status = Status::Error;
        }
        self.stick_bottom = true;
        true
    }

    fn close_menus(&mut self) {
        self.menu = None;
        self.menu_hi = None;
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
/// where Codex has them; elsewhere the app draws look-alikes.
pub fn window_options() -> WindowOptions {
    let mut fonts = quark_text::FontSettings::default();
    // The closest bundled face to SF Pro, which Codex uses.
    fonts.ui_family = quark_text::fonts::INTER_FAMILY.to_owned();
    WindowOptions {
        title: APP_TITLE.into(),
        size: INITIAL_SIZE,
        min_size: Some(MIN_SIZE),
        chrome: WindowChrome::Custom,
        traffic_lights: Some(TrafficLights {
            left_margin: 15.0,
            center_y: 23.0,
        }),
        fonts,
        ..WindowOptions::default()
    }
}

pub const COMPOSER_FOCUS: FocusId = FocusId::from_key("codex.composer");
pub const PALETTE_FOCUS: FocusId = FocusId::from_key("codex.palette");

impl UiApp for Codex {
    type Action = Msg;
    type Message = ();

    fn init(&mut self, cx: &mut UiContext) {
        self.terminal.init(cx);
        if self.options.scene.is_none() || self.composer.focus_on_start {
            cx.set_focus(Some(COMPOSER_FOCUS));
        }
    }

    fn view(&mut self, vcx: &mut ViewContext) -> AnyElement {
        let size = vcx.frame.size();
        self.size = size;
        self.now_ms = vcx.frame.elapsed().as_millis() as u64;
        if self.pump() {
            vcx.frame.request_frame();
        }
        if let Some(start) = self.run_started {
            vcx.frame.request_frame_in(std::time::Duration::from_millis(
                (start + 2000).saturating_sub(self.now_ms),
            ));
        }
        let p: &Pal = theme::pal(vcx.theme.mode);
        let (w, h) = size;
        let sidebar_w = if self.sidebar_shown() { SIDEBAR_W } else { 0.0 };
        let panel_w = panel::width(self, w - sidebar_w);
        let main_w = (w - sidebar_w - panel_w).max(0.0);

        let mut root = div()
            .relative()
            .w(w)
            .h(h)
            .bg(p.bg)
            .overflow_hidden()
            .accessibility_role(Role::Window)
            .accessibility_label(APP_TITLE);
        if let Screen::Settings(page) = self.screen {
            root = root.child(settings::view(self, page, p, vcx));
        } else {
            if sidebar_w > 0.0 {
                root = root.child(sidebar::view(self, p, h));
            }
            let main = match self.screen {
                Screen::Home => home::view(self, p, (sidebar_w, main_w, h), vcx),
                Screen::Thread(id) => thread::view(self, id, p, (sidebar_w, main_w, h), vcx),
                Screen::Settings(_) => unreachable!(),
            };
            root = root.child(main);
            if panel_w > 0.0 {
                root = root.child(panel::view(self, p, (w - panel_w, panel_w, h), vcx));
            }
            root = root.child(chrome(self, p, sidebar_w));
        }
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
        if self.running() {
            // The shimmer moves every frame.
            vcx.frame
                .request_frame_in(std::time::Duration::from_millis(33));
        }
        root.into_any()
    }

    fn update(&mut self, msg: Msg, cx: &mut UiContext) {
        self.now_ms = cx.window.elapsed().as_millis() as u64;
        let close = !matches!(
            msg,
            Msg::Open(_)
                | Msg::Terminal(_)
                | Msg::ComposerScroll(_)
                | Msg::ThreadScroll(_)
                | Msg::PaletteScroll(_)
                | Msg::SettingsScroll(_)
                | Msg::Noop
        );
        if close {
            self.close_menus();
        }
        match msg {
            Msg::NewChat => {
                self.screen = Screen::Home;
                self.palette = None;
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
                self.settings_scroll = 0.0;
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
            Msg::Reasoning(i) => self.reasoning = i,
            Msg::Model(i) => self.model = i,
            Msg::Speed(i) => self.speed = i,
            Msg::ToggleProject(id) => {
                if let Some(at) = self.collapsed.iter().position(|p| *p == id) {
                    self.collapsed.remove(at);
                } else {
                    self.collapsed.push(id);
                }
            }
            Msg::ChooseProject(project) => {
                self.project = project;
                self.screen = Screen::Home;
            }
            Msg::DismissBanner => self.banner = false,
            Msg::Send => self.send(),
            Msg::Stop => {
                self.run_started = None;
                if let Screen::Thread(id) = self.screen
                    && let Some(t) = self.data.thread_mut(id)
                {
                    t.items.retain(|i| *i != Item::Thinking);
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
            Msg::ToggleSidePanel => {
                self.side_panel = !self.side_panel;
                if !self.side_panel {
                    self.panel_expanded = false;
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
            Msg::ExpandPanel => self.panel_expanded = !self.panel_expanded,
            Msg::ToggleSplit => self.split = !self.split,
            Msg::ToggleFileList => self.file_list = !self.file_list,
            Msg::SetScope(scope) => {
                self.scope = scope;
                self.review_loading = false;
            }
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
                cx.set_focus(open.then_some(PALETTE_FOCUS).or(Some(COMPOSER_FOCUS)));
            }
            Msg::PaletteScroll(_) => {}
            Msg::ThreadScroll(_) => self.stick_bottom = false,
            Msg::SettingsScroll(_) => {}
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

/// The window's top band over the main pane: sidebar toggle and history
/// arrows beside the traffic lights.
fn chrome(app: &Codex, p: &Pal, sidebar_w: f32) -> Div {
    // With the sidebar hidden the arrows move right of a compose button.
    let left = 82.0;
    let mut bar = hrow()
        .absolute()
        .left(left)
        .top(9.0)
        .h(28.0)
        .gap(4.0)
        .z_index(20)
        .child(icon_button(
            p,
            icons::SIDEBAR,
            28.0,
            16.0,
            p.icon,
            "Hide sidebar",
            Msg::ToggleSidebar,
        ))
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
    if sidebar_w == 0.0 && !matches!(app.screen, Screen::Settings(_)) {
        bar = bar.child(icon_button(
            p,
            icons::COMPOSE,
            28.0,
            16.0,
            p.icon,
            "New chat",
            Msg::NewChat,
        ));
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
        .top(16.0)
        .gap(9.0)
        .z_index(80)
        .child(light(0xf35a50))
        .child(light(0xfdbd01))
        .child(light(0x02bd00))
}
