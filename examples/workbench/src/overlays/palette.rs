//! The command palette: quark's `CommandPalette` over the shared
//! [`COMMANDS`] registry plus the thread list, 560 points wide, with
//! command icons and shortcut keycaps. A chosen item never acts here: it becomes a
//! [`Pick`], which the overlay layer turns into the same
//! `Effect::Command` keys and menus produce.

use quark_app::quark_ui::FocusId;
use quark_app::quark_ui::element::{AnyElement, Binding};
use quark_app::quark_ui::icons::lucide;
use quark_app::quark_ui::text_input::{TextEditCommand, TextEditOutcome};
use quark_app::quark_ui::theme::Theme;
use quark_components::{
    CommandPalette, PALETTE_INPUT, PaletteEvent, PaletteItem, PaletteOutcome, PaletteProvider,
};

use crate::contracts::{COMMANDS, CommandId, SurfaceCx, ThreadId};
use crate::overlays::Pick;

/// Panel width (design section 3).
pub const WIDTH: f32 = 560.0;
/// Thread items' keys start here, after every command's index.
const THREAD_KEYS: u64 = 10_000;

/// What the providers read, collected when the palette opens.
pub struct Catalog {
    manual_clock: bool,
    threads: Vec<(ThreadId, String)>,
}

impl Catalog {
    pub fn new(scx: &SurfaceCx) -> Self {
        Self {
            manual_clock: scx.manual_clock,
            threads: scx
                .model
                .threads
                .iter()
                .map(|t| (t.id, t.title.clone()))
                .collect(),
        }
    }
}

struct Commands;

impl PaletteProvider<Catalog> for Commands {
    fn title(&self) -> &str {
        "Commands"
    }

    fn collect(&self, catalog: &Catalog, out: &mut Vec<PaletteItem>) {
        for (i, spec) in COMMANDS.iter().enumerate() {
            // The palette itself is not a palette command.
            if spec.id == CommandId::OpenPalette || spec.manual_clock_only && !catalog.manual_clock
            {
                continue;
            }
            let mut item = PaletteItem::new(i as u64, spec.title, pick(Pick::Command(spec.id)))
                .icon(command_icon(spec.id));
            if let Some(binding) = spec.binding.and_then(|b| b.parse::<Binding>().ok()) {
                item = item.binding(binding);
            }
            out.push(item);
        }
    }
}

struct Threads;

impl PaletteProvider<Catalog> for Threads {
    fn title(&self) -> &str {
        "Threads"
    }

    fn collect(&self, catalog: &Catalog, out: &mut Vec<PaletteItem>) {
        for (id, title) in &catalog.threads {
            out.push(
                PaletteItem::new(
                    THREAD_KEYS + u64::from(id.0),
                    title.as_str(),
                    pick(Pick::Thread(*id)),
                )
                .icon(lucide::HASH)
                .subtitle("Thread"),
            );
        }
    }
}

/// The icon a command's palette row shows.
fn command_icon(id: CommandId) -> &'static str {
    match id {
        CommandId::OpenPalette => lucide::COMMAND,
        CommandId::NewThread => lucide::PLUS,
        CommandId::OpenSettings => lucide::SETTINGS,
        CommandId::ToggleSidebar => lucide::PANEL_LEFT,
        CommandId::ToggleRightDock => lucide::SPLIT,
        CommandId::ToggleTerminal => lucide::TERMINAL,
        CommandId::FindInDocument => lucide::SEARCH,
        CommandId::FocusComposer => lucide::PENCIL,
        CommandId::SendPrompt => lucide::ARROW_UP,
        CommandId::StopRun => lucide::X,
        CommandId::ShowDiff => lucide::FILE_DIFF,
        CommandId::ShowFiles => lucide::FOLDER,
        CommandId::ShowPreview => lucide::EYE,
        CommandId::ApplyDiff => lucide::CHECK,
        CommandId::UndoDiff => lucide::CORNER_UP_LEFT,
        CommandId::NextThread => lucide::ARROW_DOWN,
        CommandId::PreviousThread => lucide::ARROW_UP,
        CommandId::ThemeSystem => lucide::CIRCLE_DOT,
        CommandId::ThemeLight => lucide::SUN,
        CommandId::ThemeDark => lucide::MOON,
        CommandId::AdvanceDemoStep => lucide::PLAY,
        CommandId::ResetDemo => lucide::REFRESH,
    }
}

fn pick(p: Pick) -> quark_app::quark_ui::Action {
    quark_app::quark_ui::Action::new(p)
}

pub struct Palette {
    inner: CommandPalette<Catalog>,
}

impl Default for Palette {
    fn default() -> Self {
        let mut inner = CommandPalette::new()
            .placeholder("Search commands and threads")
            .width(WIDTH)
            .keycaps(true);
        inner.register(Commands);
        inner.register(Threads);
        Self { inner }
    }
}

impl Palette {
    pub fn is_open(&self) -> bool {
        self.inner.is_open()
    }

    /// Open over `scx`'s window, remembering its focus to restore. Returns
    /// the search field's focus target.
    pub fn open(&mut self, scx: &SurfaceCx) -> FocusId {
        self.inner.open(&Catalog::new(scx), scx.focus)
    }

    /// Close without choosing; the focus to restore.
    pub fn close(&mut self) -> Option<FocusId> {
        self.inner.close()
    }

    pub fn handle_key(&mut self, pressed: &Binding) -> Option<PaletteOutcome> {
        self.inner.handle_key(pressed)
    }

    pub fn apply(&mut self, event: PaletteEvent) -> Option<PaletteOutcome> {
        self.inner.apply(event)
    }

    pub fn edit(
        &mut self,
        target: FocusId,
        command: TextEditCommand,
        now_ms: u64,
    ) -> Option<TextEditOutcome> {
        (target == PALETTE_INPUT && self.inner.is_open()).then(|| self.inner.edit(command, now_ms))
    }

    pub fn set_preedit(&mut self, target: FocusId, text: String, cursor: Option<(usize, usize)>) {
        if target == PALETTE_INPUT && self.inner.is_open() {
            self.inner.set_preedit(text, cursor);
        }
    }

    /// The palette over a `window`-sized scrim, or `None` while closed.
    pub fn render(
        &self,
        window: (f32, f32),
        theme: &Theme,
        focused: bool,
        on_event: impl Fn(PaletteEvent) -> quark_app::quark_ui::Action,
    ) -> Option<AnyElement> {
        self.inner.render(window, theme, focused, on_event)
    }
}
