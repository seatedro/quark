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
pub mod diff;
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
use quark::view;
use quark_app::platform::material::{MaterialOptions, WindowBackground};
use quark_app::quark_ui::FocusId;
use quark_app::quark_ui::element::*;
use quark_app::quark_ui::style::Styled;
use quark_app::quark_ui::text_input::{TextEditCommand, TextEditOutcome};
use quark_app::quark_ui::theme::{Color, ThemeMode};
use quark_app::{
    AppEvent, InputEvent, TrafficLights, UiAdapter, UiApp, UiContext, ViewContext, WindowChrome,
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
    /// The inline diff card, the Changes diff, and their controls.
    Diff(diff::DiffMsg),
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
    /// The agent's change, shared by the inline card and the Changes tab.
    pub changes: diff::Changes,
    pub open_file: Option<&'static str>,
    pub palette: Option<palette::State>,
    pub thread_scroll: ScrollHandle,
    /// Scroll the transcript to the latest turn on the next frame.
    pub stick_bottom: bool,
    /// A scene's exact transcript offset, instead of the latest turn.
    pub scroll_px: Option<f32>,
    /// A scene's nudge from the latest turn's position: the transcript
    /// scrolls this many points past the turn's top.
    pub scroll_adjust: Option<f32>,
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
    /// The window got its native material: the frame and sidebar leave
    /// their fills off so the material shows. Otherwise (no compositor,
    /// reduced transparency, a platform without materials) every surface
    /// paints its captured color, as on a window capture.
    pub vibrant: bool,
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
            changes: diff::Changes::cart(),
            open_file: None,
            palette: None,
            thread_scroll: ScrollHandle::new(),
            stick_bottom: true,
            scroll_px: None,
            scroll_adjust: None,
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
            vibrant: false,
            options,
        };
        if let Some(store) = grammar_store() {
            app.changes.enable_syntax(store);
        }
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
                        pill: false,
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

/// Grammar packs for the diff's syntax colors: `$QUARK_SYNTAX_PACKS`,
/// else the workspace's `target/syntax-packs` when it holds packs for this
/// target. Without packs, code shows in the plain code color.
pub fn grammar_store() -> Option<quark_app::quark_ui::quark_syntax::GrammarStore> {
    use quark_app::quark_ui::quark_syntax::{GrammarStore, StoreConfig, pack::TARGET};
    let root = std::env::var_os("QUARK_SYNTAX_PACKS")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            let root =
                std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/syntax-packs");
            root.join(TARGET).is_dir().then_some(root)
        })?;
    Some(GrammarStore::new(StoreConfig::new().local_packs(root)))
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
    WindowOptions {
        title: APP_TITLE.into(),
        size: INITIAL_SIZE,
        min_size: Some(MIN_SIZE),
        chrome: WindowChrome::Custom,
        traffic_lights: Some(TrafficLights {
            left_margin: 15.0,
            center_y: 22.0,
        }),
        fonts: fonts(),
        // The app is Electron: match how CSS blends, and show the
        // platform's window material behind the frame and sidebar where
        // it has one (the frame's dark capture color elsewhere).
        compositing: quark::UiCompositing::WebCompatible,
        background: WindowBackground::Material(MaterialOptions::new(
            quark::MaterialKind::WindowBackground,
            theme::DARK.frame,
        )),
        ..WindowOptions::default()
    }
}

/// The app's fonts. On macOS `system-ui` resolves to SF Pro and
/// `ui-monospace` to SF Mono, the faces the app itself uses. Elsewhere the
/// platform's UI font is not the reference's (DejaVu Sans or Segoe UI), so
/// the bundled Inter, the closest face to SF Pro, stands in;
/// `QUARK_CODEX_FONTS=system` asks for the platform fonts there too.
pub fn fonts() -> quark_text::FontSettings {
    let system = cfg!(target_os = "macos")
        || std::env::var("QUARK_CODEX_FONTS").is_ok_and(|v| v == "system");
    if system {
        return quark_text::FontSettings::system();
    }
    let mut fonts = quark_text::FontSettings::default();
    fonts.ui_family = quark_text::fonts::INTER_FAMILY.to_owned();
    fonts
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

impl Codex {
    /// Whether the window's material took, from what the runner resolved.
    fn refresh_surface(&mut self, cx: &mut UiContext) {
        let surface = cx
            .window_handle()
            .and_then(|window| cx.window.window_surface(window));
        self.vibrant = surface.is_some_and(|s| {
            matches!(
                s.background,
                quark_app::platform::material::EffectiveBackground::Material { .. }
            )
        });
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
        let waker = cx.window.waker().clone();
        self.changes.set_syntax_wake(move || waker.wake());
        cx.set_focus(Some(COMPOSER_FOCUS));
        self.refresh_surface(cx);
    }

    fn app_event(&mut self, event: AppEvent, cx: &mut UiContext) {
        if let AppEvent::WindowOpened(_) | AppEvent::WindowSurfaceChanged(_) = event {
            self.refresh_surface(cx);
        }
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

        let local = |x: f32| x - f.left;
        let cards = match self.screen {
            Screen::Settings(page) => vec![settings::view(self, page, p, &f, vcx)],
            screen => {
                let mut cards = Vec::new();
                if f.sidebar_w > 0.0 {
                    cards.push(sidebar::view(self, p, f.sidebar_w, f.card_h()));
                }
                let main = (local(f.main_x), f.main_w, f.card_h());
                cards.push(match screen {
                    Screen::Thread(id) => thread::view(self, id, p, main, vcx),
                    _ => home::view(self, p, main, vcx),
                });
                if f.panel_w > 0.0 {
                    let panel = (local(f.panel_x), f.panel_w, f.card_h());
                    cards.push(panel::view(self, p, panel, vcx));
                }
                cards
            }
        };
        let menu = menus::view(self, p, vcx);
        let palette = self.palette.is_some().then(|| palette::view(self, p, vcx));
        // Over a native material only the main and panel cards are opaque.
        let vibrant = self.vibrant && f.sidebar_w > 0.0;
        let card_w = w - f.left - FRAME_INSET;
        let root = view! {
            <div class="relative overflow-hidden" w={w} h={h}
                 bg={if !vibrant { p.frame }} accessibility_role={Role::Window}
                 aria-label={APP_TITLE}>
                {rail(self, p, h)}
                <div class="absolute rounded-[10] overflow-hidden" left={f.left} top={f.top}
                     w={card_w} h={f.card_h()} border={p.frame_border}
                     bg={if !vibrant { p.bg }}>
                    if vibrant {
                        <div class="absolute top-0" left={f.sidebar_w} w={card_w - f.sidebar_w}
                             h={f.card_h()} bg={p.bg} />
                    }
                    {...cards}
                </div>
                {title_bar(self, p, &f)}
                if !cfg!(target_os = "macos") {
                    {traffic_lights()}
                }
                if let Some(menu) = menu {
                    if self.menu.is_some() {
                        {click_catcher(w, h, Msg::CloseMenus)}
                    }
                    {menu}
                }
                {?palette}
            </div>
        };
        if self.run_started.is_some() {
            // The fake run steps on its own; shimmering text asks for its
            // own frames.
            vcx.frame
                .request_frame_in(std::time::Duration::from_millis(33));
        }
        root
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
            Msg::Diff(msg) => diff::update(self, msg, cx),
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
            "enter" if cx.focus() == Some(diff::FIND_FOCUS) => {
                self.update(Msg::Diff(diff::DiffMsg::FindStep(!m.shift)), cx);
            }
            "escape" if cx.focus() == Some(diff::FIND_FOCUS) && self.menu.is_none() => {
                self.update(Msg::Diff(diff::DiffMsg::Find(false)), cx);
            }
            "f" if cmd && self.side_panel && self.tab == Tab::Changes => {
                self.update(Msg::Diff(diff::DiffMsg::Find(true)), cx);
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
        } else if target == diff::FIND_FOCUS {
            self.changes.edit_find(command, self.now_ms)
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
        // Syntax colors may have arrived; the next frame takes them.
        cx.window.request_redraw();
    }
}

/// The icon rail: Home (selected), Space, Scheduled, Plugins, Explore,
/// then Code Review and Sites, and the profile avatar at the bottom.
fn rail(app: &Codex, p: &Pal, h: f32) -> AnyElement {
    let item = |svg: &'static str, label: &str, y: f32, selected: bool, dot: bool, msg: Msg| {
        view! {
            <div class="absolute left-2 w-9 h-9 items-center justify-center rounded-[9]" top={y}
                 hover_bg={p.rail_tile.with_alpha(150)} role="button"
                 aria-label={label.to_owned()} on:click={msg} bg={if selected { p.rail_tile }}>
                <icon svg={svg} size={19.0} color={if selected { p.text } else { p.rail_icon }} />
                if dot {
                    <div class="absolute left-6 top-[5] w-[7] h-[7] rounded-[4]" bg={p.accent} />
                }
            </div>
        }
    };
    let home = matches!(app.screen, Screen::Home | Screen::Thread(_));
    view! {
        <div class="absolute left-0 top-0" w={RAIL_W} h={h} accessibility_role={Role::Navigation}
             aria-label="Rail">
            {item(icons::HOME, "Home", 52.0, home, true, Msg::NewChat)}
            {item(icons::SPACE, "Space", 96.0, false, false, Msg::Noop)}
            {item(icons::CLOCK, "Scheduled", 140.0, false, false, Msg::Noop)}
            {item(icons::AT, "Plugins", 184.0, false, false, Msg::Noop)}
            {item(icons::ELLIPSIS, "Explore", 228.0, false, true, Msg::Noop)}
            <div class="absolute left-[14] top-[272] w-6 h-px" bg={p.frame_border} />
            {item(icons::PR, "Code Review", 281.0, false, false, Msg::Noop)}
            {item(icons::SITES, "Sites", 325.0, false, false, Msg::Noop)}
            <div class="absolute left-[14] w-6 h-6 rounded-[12] items-center justify-center"
                 top={h - 38.0} bg={p.avatar} id="rail.profile" role="button"
                 aria-label="Open profile menu" on:click={Msg::Open(Menu::Profile)}>
                <txt("SE", 9.0, Color::rgba(255, 255, 255, 255)) />
            </div>
        </div>
    }
}

/// The unified title bar: history and sidebar controls beside the
/// traffic lights, the chat toolbar over the main card, the panel's tab
/// strip over the panel card, and the new-tab button.
fn title_bar(app: &Codex, p: &Pal, f: &Frame) -> AnyElement {
    let settings = matches!(app.screen, Screen::Settings(_));
    let collapsed = f.sidebar_w == 0.0;
    let toolbar_x = if collapsed { 232.0 } else { f.main_x };
    let thread = app.current_thread().filter(|_| !app.full_view);
    let right_edge = if f.panel_w > 0.0 {
        f.panel_x
    } else {
        f.w - 40.0
    };
    let title_w = (right_edge - toolbar_x - 180.0).max(40.0);
    view! {
        // Blank title bar moves the window; its buttons still click.
        <div class="absolute left-0 top-0 z-20" w={f.w} h={TITLE_H} window_drag_region>
            <div class="flex-row items-center absolute left-[88] top-2 h-7 gap-1">
                <icon_button(p, icons::ARROW_LEFT, 28.0, 16.0, p.icon, "Back", Msg::Noop) />
                <icon_button(p, icons::ARROW_RIGHT, 28.0, 16.0, p.icon_faint, "Forward",
                             Msg::Noop) />
                if !settings {
                    <div class="w-2" />
                    <icon_button(p, icons::SIDEBAR, 28.0, 16.0, p.icon, "Hide sidebar",
                                 Msg::ToggleSidebar) />
                }
                if collapsed && !settings {
                    <div class="w-4" />
                    <icon_button(p, icons::COMPOSE, 28.0, 16.0, p.icon, "New chat", Msg::NewChat) />
                    <div class="w-[10]" />
                    <div class="w-px h-4" bg={p.frame_border} />
                }
            </div>
            if let Some(t) = thread {
                <div class="flex-row items-center absolute top-2 h-7 pl-2" left={toolbar_x}
                     w={right_edge - toolbar_x} role="toolbar" aria-label="Chat toolbar">
                    <div class="w-7 h-7 items-center justify-center">
                        <icon svg={icons::FOLDER} size={16.0} color={p.text} />
                    </div>
                    <div class="w-2" />
                    <div class="min-w-0 overflow-hidden" max_w={title_w}>
                        <text size={theme::BODY} color={p.text}
                              class="font-medium whitespace-nowrap truncate">{t.title.clone()}</text>
                    </div>
                    <div class="flex-1 min-w-2" />
                    <icon_button(p, icons::ELLIPSIS, 28.0, 16.0, p.icon, "Chat actions",
                                 Msg::Open(Menu::ChatActions)) id="header.actions" />
                    <div class="w-1.5" />
                    <icon_button(p, icons::SUMMARY, 28.0, 16.0, p.icon, "Toggle summary",
                                 Msg::Open(Menu::Summary)) id="header.summary" />
                    <div class="w-1.5" />
                </div>
            }
            if settings {
            } else if f.panel_w > 0.0 || app.full_view {
                {panel::tab_strip(app, p, f)}
            } else {
                <div class="absolute top-2" left={f.w - 36.0}>
                    <icon_button(p, icons::NEW_TAB, 28.0, 16.0, p.icon, "New tab",
                                 Msg::Open(Menu::PanelTab)) id="panel.newtab" />
                </div>
            }
        </div>
    }
}

/// macOS traffic lights, drawn where the platform has no native ones.
fn traffic_lights() -> AnyElement {
    view! {
        <div class="flex-row items-center absolute left-4 top-[15] gap-[9] z-80">
            <div class="w-[14] h-[14] rounded-[7] bg-[#f35a50]" />
            <div class="w-[14] h-[14] rounded-[7] bg-[#fdbd01]" />
            <div class="w-[14] h-[14] rounded-[7] bg-[#02bd00]" />
        </div>
    }
}
