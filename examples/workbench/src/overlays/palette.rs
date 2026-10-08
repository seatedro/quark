//! The command palette: quark's `CommandPalette` over the shared
//! [`COMMANDS`] registry plus the thread list, sized to the workbench's
//! 560-point recipe. A chosen item never acts here: it becomes a
//! [`Pick`], which the overlay layer turns into the same
//! `Effect::Command` keys and menus produce.

use quark_app::quark_ui::FocusId;
use quark_app::quark_ui::element::{AnyElement, Binding, IntoAnyElement, div};
use quark_app::quark_ui::style::Styled;
use quark_app::quark_ui::text_input::{TextEditCommand, TextEditOutcome};
use quark_app::quark_ui::theme::{Color, Theme};
use quark_components::{
    CommandPalette, PALETTE_INPUT, PaletteEvent, PaletteItem, PaletteOutcome, PaletteProvider,
};

use crate::contracts::{COMMANDS, CommandId, SurfaceCx, ThreadId};
use crate::overlays::Pick;

/// Panel width and distance from the window's top edge (design section 3).
pub const WIDTH: f32 = 560.0;
/// The margin `CommandPalette` keeps from the edges of the area it is
/// given; the area is sized so the panel comes out at [`WIDTH`].
const MARGIN: f32 = 48.0;
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
            let mut item = PaletteItem::new(i as u64, spec.title, pick(Pick::Command(spec.id)));
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
                .subtitle("Thread"),
            );
        }
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
        let mut inner = CommandPalette::new().placeholder("Search commands and threads");
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
        if !self.inner.is_open() {
            return None;
        }
        let scale = theme.metrics.ui_scale();
        // CommandPalette sizes its panel from the area it is given (640
        // points less margins); give it a column that yields 560 and paint
        // the window-wide scrim here, so its own scrim must be clear.
        let column = ((WIDTH + MARGIN) * scale).min(window.0);
        let mut clear = theme.clone();
        clear.colors.overlay_scrim = Color::TRANSPARENT;
        let panel = self
            .inner
            .render((column, window.1), &clear, focused, &on_event)?;
        Some(
            div()
                .absolute()
                .left(0.0)
                .top(0.0)
                .w(window.0)
                .h(window.1)
                .z_index(400)
                .bg(theme.colors.overlay_scrim)
                .on_click(on_event(PaletteEvent::Dismiss))
                .block_mouse()
                .child(
                    div()
                        .absolute()
                        .left(((window.0 - column) / 2.0).round())
                        .top(0.0)
                        .w(column)
                        .h(window.1)
                        .child(panel),
                )
                .into_any(),
        )
    }
}
