//! [`TerminalState`]: the app-owned terminal behind [`crate::terminal_view`].

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use quark_render::FontKind;
use quark_render::scene::Rect;
use quark_text::{LayoutCache, TextParams, TextStyle, TextSystem};
use quark_ui::FocusId;
use quark_ui::element::ScrollHandle;
use quark_ui::theme::Theme;
use winit::keyboard::{ModifiersState, NamedKey};

use crate::grid::{Grid, Rgb};
use crate::input::{self, KeyPress};
use crate::pty::{Pty, PtyCommand, PtyEvent, PtyGeometry};
use crate::vt::{
    KeyAction, KeyInput, Mode, Mods, MouseAction, MouseButton, MouseGeometry, Scroll, Terminal,
    UnsafePaste,
};

/// Clicks closer together than this count as a double or triple click.
const MULTI_CLICK_MS: u64 = 400;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TerminalStyle {
    /// Monospace text size in points.
    pub font_size: f32,
    /// Cell height as a multiple of the font size.
    pub line_height: f32,
    /// Space around the grid, in points.
    pub padding: f32,
    pub scrollback_lines: usize,
}

impl Default for TerminalStyle {
    fn default() -> Self {
        Self {
            font_size: 13.0,
            line_height: 1.3,
            padding: 6.0,
            scrollback_lines: 10_000,
        }
    }
}

/// Input from [`crate::terminal_view`], in window coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TerminalEvent {
    Press { x: f32, y: f32 },
    Drag { x: f32, y: f32 },
    Release,
}

/// What handling input asks of the app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalOutcome {
    /// Not for the terminal; let the app handle it.
    Ignored,
    Handled,
    /// Put this on the clipboard (the copy shortcut).
    Copy(String),
    /// Read the clipboard and pass it to [`TerminalState::paste`] (the
    /// paste shortcut).
    Paste,
    /// Open this URI (Ctrl+click, Cmd+click on macOS, on an OSC 8 link).
    OpenLink(String),
}

/// Things the running program did that the app may surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalSignal {
    Title(String),
    Bell,
    /// An OSC 52 clipboard write, when [`TerminalState::allow_clipboard_write`]
    /// is on.
    Clipboard(String),
    /// The program exited, with its code when the platform reports one.
    Exited(Option<u32>),
}

/// Pointer input for mouse reporting, from the app's raw input events.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PointerInput {
    Moved {
        x: f32,
        y: f32,
    },
    Button {
        button: winit::event::MouseButton,
        pressed: bool,
    },
    /// Wheel motion in lines, positive up.
    Wheel {
        lines: f32,
    },
}

/// Cell and text sizes in logical points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Metrics {
    pub font_size: f32,
    pub cell_w: f32,
    pub cell_h: f32,
    pub pad: f32,
}

impl Metrics {
    pub fn text_style(&self) -> TextStyle {
        TextStyle::new(self.font_size)
            .kind(FontKind::Mono)
            .line_height(self.cell_h)
    }
}

/// Colors the view paints with besides the cells', from the theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Palette {
    pub selection: quark::Color,
    pub cursor: quark::Color,
    pub link: quark::Color,
}

impl std::hash::Hash for Palette {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        for c in [self.selection, self.cursor, self.link] {
            [c.r, c.g, c.b, c.a].hash(state);
        }
    }
}

/// What [`crate::terminal_view`] reads besides the grid, refreshed by
/// [`TerminalState::prepare`] when the terminal changed. A plain value of
/// shared handles, so updating it allocates nothing.
#[derive(Debug, Clone)]
pub(crate) struct Frame {
    pub id: &'static str,
    pub focus: FocusId,
    pub viewport: (f32, f32),
    pub metrics: Metrics,
    /// Height of the scrollable content: every row of the scrollback.
    pub content_h: f32,
    pub scroll: ScrollHandle,
    pub title: Rc<str>,
    /// Bumped when the rows or anything above changed; a cursor that only
    /// moved leaves it alone.
    pub revision: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Unit {
    Cell,
    Word,
    Line,
}

#[derive(Debug, Clone, Copy)]
struct SelectDrag {
    anchor: (u16, u16),
    unit: Unit,
    /// The anchor's word or line, kept whole while dragging.
    anchor_span: ((u16, u16), (u16, u16)),
    moved: bool,
}

/// App-owned terminal: the VT state, the optional PTY, and what the view
/// paints. Each frame the app calls [`Self::set_viewport`] and
/// [`Self::prepare`], then builds [`crate::terminal_view`]. Output from the
/// PTY waits in pooled buffers until [`Self::read_pty`] feeds it; keys and
/// pointer reports come from the app's raw input hook through
/// [`Self::key_press`], [`Self::text_input`], and [`Self::pointer`];
/// selection comes from the view as [`TerminalEvent`]s for [`Self::handle`].
pub struct TerminalState {
    id: &'static str,
    focus: FocusId,
    vt: Terminal,
    pty: Option<Pty>,
    /// Bytes for the program while no PTY is attached (tests read them).
    outbox: Vec<u8>,
    /// Input for the PTY that did not fit its queue yet, oldest first.
    unsent: Vec<u8>,
    style: TerminalStyle,
    metrics: Option<(Metrics, u32)>,
    colors: Option<(Rgb, Rgb)>,
    viewport: (f32, f32),
    /// Grid size in cells, and the scale it was sized at.
    size: (u16, u16, u32),
    grid: Rc<Grid>,
    frame: Option<Frame>,
    /// Something the frame shows may have changed since it was built.
    dirty: bool,
    revision: u64,
    /// The screen reader text, the frame revision it was built at, and the
    /// buffer it is built in. Built only while a screen reader listens.
    screen_text: Option<(u64, Arc<str>)>,
    screen_buf: String,
    scroll: ScrollHandle,
    /// The scrollbar row the handle and the terminal last agreed on.
    synced_row: u64,
    /// An offset this state asked the handle for, applied by the next frame.
    pending_scroll: bool,
    /// Bumped each frame the handle is moving, so the view rebuilds.
    nonce: u64,
    /// The rows block's bounds in the window as of the last frame.
    bounds: Rc<Cell<Rect>>,
    drag: Option<SelectDrag>,
    last_click: Option<(u64, (u16, u16), u8)>,
    modifiers: ModifiersState,
    pointer: Option<(f32, f32)>,
    buttons_down: u8,
    /// Wheel motion not yet sent as whole lines (touchpads send fractions).
    wheel: f32,
    /// The key press just encoded typed text: skip the text event after it.
    swallow_text: bool,
    title: Rc<str>,
    signals: Vec<TerminalSignal>,
    exited: bool,
}

impl std::fmt::Debug for TerminalState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TerminalState")
            .field("id", &self.id)
            .field("size", &self.size)
            .finish_non_exhaustive()
    }
}

impl TerminalState {
    /// `id` names cache entries and accessibility ids (unique in the
    /// window); `focus` is the terminal's keyboard focus target.
    pub fn new(id: &'static str, focus: FocusId) -> Self {
        let style = TerminalStyle::default();
        let mut vt = Terminal::new(80, 24);
        vt.set_scrollback_lines(style.scrollback_lines);
        Self {
            id,
            focus,
            vt,
            pty: None,
            outbox: Vec::new(),
            unsent: Vec::new(),
            style,
            metrics: None,
            colors: None,
            viewport: (0.0, 0.0),
            size: (80, 24, 0),
            grid: Rc::default(),
            frame: None,
            dirty: true,
            revision: 0,
            screen_text: None,
            screen_buf: String::new(),
            scroll: ScrollHandle::new(),
            synced_row: 0,
            pending_scroll: false,
            nonce: 0,
            bounds: Rc::default(),
            drag: None,
            last_click: None,
            modifiers: ModifiersState::empty(),
            pointer: None,
            buttons_down: 0,
            wheel: 0.0,
            swallow_text: false,
            title: Rc::from(""),
            signals: Vec::new(),
            exited: false,
        }
    }

    pub fn with_style(mut self, style: TerminalStyle) -> Self {
        self.vt.set_scrollback_lines(style.scrollback_lines);
        self.style = style;
        self.metrics = None;
        self
    }

    /// Let programs write the clipboard with OSC 52. Off by default: any
    /// program printing to the terminal (or a file `cat`ed into it) could
    /// otherwise replace what the user copied.
    pub fn allow_clipboard_write(&mut self, allow: bool) {
        self.vt.set_clipboard_write(allow);
    }

    // ---- Queries ---------------------------------------------------------

    pub fn focus_id(&self) -> FocusId {
        self.focus
    }

    /// The grid size in cells.
    pub fn size(&self) -> (u16, u16) {
        (self.size.0, self.size.1)
    }

    /// The viewport as of the last [`Self::prepare`].
    pub fn grid(&self) -> &Grid {
        &self.grid
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn scroll_handle(&self) -> &ScrollHandle {
        &self.scroll
    }

    /// The scrollback as rows: total, first visible, visible count.
    pub fn scrollbar(&self) -> crate::vt::Scrollbar {
        self.vt.scrollbar()
    }

    pub fn selection_text(&self) -> Option<String> {
        self.vt.selection_text()
    }

    pub fn has_exited(&self) -> bool {
        self.exited
    }

    /// Whether pointer input inside the terminal should go to the program
    /// (it enabled mouse reporting and Shift is not held to select).
    pub fn wants_pointer(&self) -> bool {
        self.vt.mouse_tracking() && !self.modifiers.shift_key()
    }

    /// Whether window point `(x, y)` is over the grid.
    pub fn contains(&self, x: f32, y: f32) -> bool {
        let b = self.bounds.get();
        x >= b.x && y >= b.y && x < b.x + b.width && y < b.y + b.height
    }

    /// Bytes for the program queued while no PTY is attached.
    pub fn take_input(&mut self) -> Vec<u8> {
        self.flush();
        std::mem::take(&mut self.outbox)
    }

    /// Titles, bells, clipboard writes, and exits since the last call.
    pub fn take_signals(&mut self) -> Vec<TerminalSignal> {
        std::mem::take(&mut self.signals)
    }

    // ---- Program ---------------------------------------------------------

    /// Starts `command` on a PTY sized to the grid. `on_ready` runs when
    /// output waits (see [`Pty::spawn`]); point it at the app's waker and
    /// call [`Self::read_pty`] when it fires.
    pub fn spawn(
        &mut self,
        command: &PtyCommand,
        on_ready: impl Fn() + Send + 'static,
    ) -> std::io::Result<()> {
        let pty = Pty::spawn(command, self.geometry(), on_ready)?;
        self.pty = Some(pty);
        self.unsent.clear();
        self.exited = false;
        self.flush();
        Ok(())
    }

    fn geometry(&self) -> PtyGeometry {
        let m = self.metrics();
        let scale = f32::from_bits(self.size.2).max(1.0);
        PtyGeometry {
            cols: self.size.0,
            rows: self.size.1,
            pixel_width: (f32::from(self.size.0) * m.cell_w * scale) as u16,
            pixel_height: (f32::from(self.size.1) * m.cell_h * scale) as u16,
        }
    }

    /// Feeds the PTY's waiting output through the VT, and handles its exit
    /// once the output before it is fed. Returns whether there was any.
    pub fn read_pty(&mut self) -> bool {
        let Some(inbox) = self.pty.as_ref().map(Pty::inbox) else {
            return false;
        };
        self.send_unsent();
        let mut any = false;
        inbox.drain(|event| {
            any = true;
            self.handle_pty(event);
        });
        any
    }

    pub fn handle_pty(&mut self, event: PtyEvent<'_>) {
        match event {
            PtyEvent::Output(bytes) => self.feed(bytes),
            PtyEvent::Exited(code) => {
                self.exited = true;
                self.pty = None;
                self.unsent.clear();
                self.signals.push(TerminalSignal::Exited(code));
            }
        }
    }

    /// Feeds program output through the VT parser.
    pub fn feed(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        self.vt.write(bytes);
        let effects = self.vt.take_effects();
        if !effects.pty.is_empty() {
            self.send(&effects.pty);
        }
        if effects.title_changed {
            self.title = Rc::from(self.vt.title());
            self.signals
                .push(TerminalSignal::Title(self.title.to_string()));
        }
        for _ in 0..effects.bells.min(1) {
            self.signals.push(TerminalSignal::Bell);
        }
        if let Some(text) = effects.clipboard {
            self.signals.push(TerminalSignal::Clipboard(text));
        }
        self.dirty = true;
    }

    fn send(&mut self, bytes: &[u8]) {
        if self.pty.is_none() {
            self.outbox.extend_from_slice(bytes);
            return;
        }
        self.unsent.extend_from_slice(bytes);
        self.send_unsent();
    }

    /// Offers the PTY the input its queue turned away. The PTY wakes the
    /// app ([`Self::read_pty`]) once there is room again.
    fn send_unsent(&mut self) {
        let Some(pty) = &mut self.pty else {
            return;
        };
        if self.unsent.is_empty() {
            return;
        }
        match pty.write(&self.unsent) {
            Ok(n) => drop(self.unsent.drain(..n)),
            // The program has exited; its exit event is on the way.
            Err(_) => self.unsent.clear(),
        }
    }

    /// Sends what the encoders queued in the terminal.
    fn flush(&mut self) {
        let queued = std::mem::take(self.vt.pty_output());
        if !queued.is_empty() {
            self.send(&queued);
        }
    }

    // ---- Keyboard --------------------------------------------------------

    /// Tracks the modifiers, for Shift+drag selection and link clicks.
    pub fn set_modifiers(&mut self, modifiers: ModifiersState) {
        self.modifiers = modifiers;
    }

    /// A key press while the terminal is focused. Copy and paste shortcuts
    /// (Cmd+C and Cmd+V on macOS, Ctrl+Shift+C and Ctrl+Shift+V elsewhere)
    /// come back as outcomes; every other key is encoded for the program's
    /// keyboard modes (including the Kitty protocol) and sent.
    pub fn key_press(&mut self, press: &KeyPress) -> TerminalOutcome {
        self.modifiers = press.modifiers;
        self.swallow_text = false;
        let Some((key, unshifted)) = press.key() else {
            return TerminalOutcome::Ignored;
        };
        let m = press.modifiers;
        let shortcut = if cfg!(target_os = "macos") {
            m.super_key() && !m.control_key() && !m.alt_key()
        } else {
            m.control_key() && m.shift_key() && !m.alt_key() && !m.super_key()
        };
        if shortcut {
            match unshifted {
                Some('c') => {
                    return match self.vt.selection_text() {
                        Some(text) if !text.is_empty() => TerminalOutcome::Copy(text),
                        _ => TerminalOutcome::Handled,
                    };
                }
                Some('v') => return TerminalOutcome::Paste,
                _ => {}
            }
        }
        if cfg!(target_os = "macos") && m.super_key() {
            // Cmd shortcuts belong to the app.
            return TerminalOutcome::Ignored;
        }
        let input = KeyInput {
            key,
            mods: input::mods(m),
            text: press.typed(),
            unshifted,
            action: if press.repeat {
                KeyAction::Repeat
            } else {
                KeyAction::Press
            },
        };
        if self.vt.key(&input) {
            self.swallow_text = press.typed().is_some();
            self.after_input();
        }
        TerminalOutcome::Handled
    }

    /// Text from the platform (an IME commit, or the character a key press
    /// typed). Skipped right after [`Self::key_press`] already sent it.
    pub fn text_input(&mut self, text: &str) {
        if std::mem::take(&mut self.swallow_text) || text.is_empty() {
            return;
        }
        // Committed text is inserted as typed, not as a paste.
        self.vt.pty_output().extend_from_slice(text.as_bytes());
        self.after_input();
    }

    /// Pastes `text` framed for the program's modes (bracketed paste when
    /// enabled). Text that could run a command outside bracketed paste is
    /// refused unless `allow_unsafe`; ask the user, then call again.
    pub fn paste(&mut self, text: &str, allow_unsafe: bool) -> Result<(), UnsafePaste> {
        self.vt.paste(text, allow_unsafe)?;
        self.after_input();
        Ok(())
    }

    /// Reports focus to programs that asked (mode 1004).
    pub fn focus_changed(&mut self, focused: bool) {
        self.vt.focus(focused);
        self.flush();
        self.dirty = true;
    }

    /// Input goes to the live screen: send it, drop the selection, and
    /// scroll back down.
    fn after_input(&mut self) {
        self.flush();
        self.vt.scroll(Scroll::Bottom);
        self.vt.clear_selection();
        self.dirty = true;
    }

    // ---- Pointer ---------------------------------------------------------

    /// Raw pointer input, for mouse reporting and alternate scroll. Returns
    /// whether the terminal consumed it; the app should skip its own
    /// handling then. Moves are always tracked.
    pub fn pointer(&mut self, input: PointerInput) -> bool {
        match input {
            PointerInput::Moved { x, y } => {
                self.pointer = Some((x, y));
                self.wants_pointer()
                    && self.contains(x, y)
                    && self.report(MouseAction::Motion, None)
            }
            PointerInput::Button { button, pressed } => {
                let Some((x, y)) = self.pointer else {
                    return false;
                };
                let button = match button {
                    winit::event::MouseButton::Left => MouseButton::Left,
                    winit::event::MouseButton::Right => MouseButton::Right,
                    winit::event::MouseButton::Middle => MouseButton::Middle,
                    _ => return false,
                };
                // A release ends a press the program saw even off the grid.
                let ours = self.contains(x, y) || (!pressed && self.buttons_down > 0);
                if !self.wants_pointer() || !ours {
                    return false;
                }
                let bit = 1 << (button as u8);
                if pressed {
                    self.buttons_down |= bit;
                } else {
                    self.buttons_down &= !bit;
                }
                let action = if pressed {
                    MouseAction::Press
                } else {
                    MouseAction::Release
                };
                self.report(action, Some(button));
                true
            }
            PointerInput::Wheel { lines } => {
                let Some((x, y)) = self.pointer else {
                    return false;
                };
                let to_program = self.wants_pointer()
                    || (self.vt.alternate_screen() && self.vt.mode(Mode::ALT_SCROLL));
                if !self.contains(x, y) || !to_program {
                    self.wheel = 0.0;
                    return false;
                }
                self.wheel += lines;
                let whole = self.wheel.trunc();
                self.wheel -= whole;
                let steps = whole.abs() as usize;
                let lines = whole;
                if self.wants_pointer() {
                    let button = if lines > 0.0 {
                        MouseButton::WheelUp
                    } else {
                        MouseButton::WheelDown
                    };
                    for _ in 0..steps {
                        self.report(MouseAction::Press, Some(button));
                    }
                    return true;
                }
                // Full-screen programs without mouse reporting get arrow
                // keys for the wheel (alternate scroll, 1007).
                let named = if lines > 0.0 {
                    NamedKey::ArrowUp
                } else {
                    NamedKey::ArrowDown
                };
                let key = input::key_from_named(named).expect("arrow key");
                for _ in 0..steps {
                    self.vt.key(&KeyInput {
                        key,
                        mods: Mods::default(),
                        text: None,
                        unshifted: None,
                        action: KeyAction::Press,
                    });
                }
                self.flush();
                true
            }
        }
    }

    fn report(&mut self, action: MouseAction, button: Option<MouseButton>) -> bool {
        let Some((x, y)) = self.pointer else {
            return false;
        };
        let b = self.bounds.get();
        let m = self.metrics();
        let scale = f32::from_bits(self.size.2).max(1.0);
        let geometry = MouseGeometry {
            width: (b.width * scale) as u32,
            height: (b.height * scale) as u32,
            cell_width: (m.cell_w * scale).round() as u32,
            cell_height: (m.cell_h * scale).round() as u32,
        };
        let at = ((x - b.x) * scale, (y - b.y) * scale);
        let sent = self.vt.mouse(
            action,
            button,
            at,
            input::mods(self.modifiers),
            geometry,
            self.buttons_down != 0,
        );
        self.flush();
        sent
    }

    // ---- Selection -------------------------------------------------------

    /// Applies an event from [`crate::terminal_view`]. `now_ms` is the
    /// window clock, for double and triple clicks.
    pub fn handle(&mut self, event: TerminalEvent, now_ms: u64) -> TerminalOutcome {
        match event {
            TerminalEvent::Press { x, y } => {
                let cell = self.cell_at(x, y);
                let count = match self.last_click {
                    Some((at, last, n))
                        if now_ms.saturating_sub(at) < MULTI_CLICK_MS && last == cell =>
                    {
                        n % 3 + 1
                    }
                    _ => 1,
                };
                self.last_click = Some((now_ms, cell, count));
                let unit = match count {
                    1 => Unit::Cell,
                    2 => Unit::Word,
                    _ => Unit::Line,
                };
                let span = match unit {
                    Unit::Cell => None,
                    Unit::Word => self.vt.word_at(cell),
                    Unit::Line => self.vt.line_at(cell),
                };
                self.drag = Some(SelectDrag {
                    anchor: cell,
                    unit,
                    anchor_span: span.unwrap_or((cell, cell)),
                    moved: false,
                });
                match span {
                    Some((a, b)) => self.vt.select(a, b),
                    None => self.vt.clear_selection(),
                }
                self.dirty = true;
                TerminalOutcome::Handled
            }
            TerminalEvent::Drag { x, y } => {
                let Some(mut drag) = self.drag else {
                    return TerminalOutcome::Ignored;
                };
                let cell = self.cell_at(x, y);
                if !drag.moved && cell == drag.anchor {
                    return TerminalOutcome::Handled;
                }
                drag.moved = true;
                self.drag = Some(drag);
                let (start, end) = match drag.unit {
                    Unit::Cell => (drag.anchor, cell),
                    unit => {
                        let span = match unit {
                            Unit::Word => self.vt.word_at(cell),
                            _ => self.vt.line_at(cell),
                        }
                        .unwrap_or((cell, cell));
                        let (a, b) = drag.anchor_span;
                        if (cell.1, cell.0) < (a.1, a.0) {
                            (b, span.0)
                        } else {
                            (a, span.1)
                        }
                    }
                };
                self.vt.select(start, end);
                self.dirty = true;
                TerminalOutcome::Handled
            }
            TerminalEvent::Release => {
                let Some(drag) = self.drag.take() else {
                    return TerminalOutcome::Ignored;
                };
                let link_mod = if cfg!(target_os = "macos") {
                    self.modifiers.super_key()
                } else {
                    self.modifiers.control_key()
                };
                if !drag.moved
                    && drag.unit == Unit::Cell
                    && link_mod
                    && let Some(uri) = self.vt.hyperlink_at(drag.anchor.0, drag.anchor.1)
                {
                    return TerminalOutcome::OpenLink(uri);
                }
                TerminalOutcome::Handled
            }
        }
    }

    /// Selects viewport cells `anchor` through `focus`, inclusive.
    pub fn select(&mut self, anchor: (u16, u16), focus: (u16, u16)) {
        self.vt.select(anchor, focus);
        self.dirty = true;
    }

    /// The viewport cell under window point `(x, y)`, clamped to the grid.
    pub fn cell_at(&self, x: f32, y: f32) -> (u16, u16) {
        let b = self.bounds.get();
        let m = self.metrics();
        let col = ((x - b.x) / m.cell_w).floor().max(0.0) as u16;
        let row = ((y - b.y) / m.cell_h).floor().max(0.0) as u16;
        (
            col.min(self.size.0.saturating_sub(1)),
            row.min(self.size.1.saturating_sub(1)),
        )
    }

    // ---- Frame -----------------------------------------------------------

    /// The terminal's size in points, padding included.
    pub fn set_viewport(&mut self, width: f32, height: f32) {
        let viewport = (width.max(0.0), height.max(0.0));
        if viewport != self.viewport {
            self.viewport = viewport;
            self.dirty = true;
        }
    }

    pub(crate) fn metrics(&self) -> Metrics {
        self.metrics.map_or_else(
            || {
                let font_size = self.style.font_size;
                Metrics {
                    font_size,
                    cell_w: font_size * 0.6,
                    cell_h: (font_size * self.style.line_height).round(),
                    pad: self.style.padding,
                }
            },
            |(m, _)| m,
        )
    }

    /// Sizes the grid to the viewport, follows the scroll handle, and
    /// refreshes what the view paints. Does nothing (and allocates nothing)
    /// when no output, input, or size change arrived since the last call.
    /// `scale` must be the frame's scale factor.
    pub fn prepare(
        &mut self,
        text: &mut TextSystem,
        layouts: &mut LayoutCache,
        scale: f32,
        theme: &Theme,
    ) {
        if self.metrics.is_none_or(|(_, s)| s != scale.to_bits()) {
            let style = TextStyle::new(self.style.font_size).kind(FontKind::Mono);
            let params = TextParams::new("0000000000", style).scale_factor(scale);
            let cell_w = layouts
                .layout(text, &params)
                .map_or(self.style.font_size * 0.6, |l| l.size().0 / 10.0);
            let mut m = self.metrics();
            m.cell_w = cell_w;
            self.metrics = Some((m, scale.to_bits()));
            self.dirty = true;
        }
        let c = &theme.colors;
        let colors = (
            Rgb::new(c.text.r, c.text.g, c.text.b),
            Rgb::new(c.editor_surface.r, c.editor_surface.g, c.editor_surface.b),
        );
        if self.colors != Some(colors) {
            self.colors = Some(colors);
            self.vt.set_default_colors(colors.0, colors.1);
            self.dirty = true;
        }
        let m = self.metrics();
        let cols = ((self.viewport.0 - m.pad * 2.0) / m.cell_w)
            .floor()
            .max(1.0) as u16;
        let rows = ((self.viewport.1 - m.pad * 2.0) / m.cell_h)
            .floor()
            .max(1.0) as u16;
        if (cols, rows, scale.to_bits()) != self.size {
            self.size = (cols, rows, scale.to_bits());
            self.vt.resize(
                cols,
                rows,
                (m.cell_w * scale).round() as u32,
                (m.cell_h * scale).round() as u32,
            );
            let geometry = self.geometry();
            if let Some(pty) = &mut self.pty {
                let _ = pty.resize(geometry);
            }
            self.flush();
            self.dirty = true;
        }
        self.sync_scroll(m);
        if !self.dirty && self.frame.is_some() {
            return;
        }
        self.dirty = false;
        // The last frame's views of the grid are dropped by now, so this
        // updates it in place; a copy an app still holds is cloned first.
        let changes = self.vt.snapshot(Rc::make_mut(&mut self.grid));
        let sb = self.vt.scrollbar();
        let leftover = (self.viewport.1 - f32::from(rows) * m.cell_h).max(0.0);
        let content_h = sb.total as f32 * m.cell_h + leftover;
        // The cursor boundary reads the grid itself, so a cursor that only
        // moved needs no new frame.
        let unchanged = self.frame.as_ref().is_some_and(|f| {
            !changes.rows
                && f.viewport == self.viewport
                && f.metrics == m
                && f.content_h == content_h
                && Rc::ptr_eq(&f.title, &self.title)
        });
        if unchanged {
            return;
        }
        self.revision += 1;
        self.frame = Some(Frame {
            id: self.id,
            focus: self.focus,
            viewport: self.viewport,
            metrics: m,
            content_h,
            scroll: self.scroll.clone(),
            title: self.title.clone(),
            revision: self.revision,
        });
    }

    /// The visible text for screen readers and the cursor's byte in it.
    /// The text is rebuilt only when the frame revision moved since the
    /// last call, so a cursor move reuses it.
    pub(crate) fn screen_text(&mut self) -> (Arc<str>, usize) {
        let grid = &self.grid;
        let caret = grid.cursor.at.map_or(0, |(x, y)| grid.text_offset(x, y));
        match &self.screen_text {
            Some((revision, text)) if *revision == self.revision => (text.clone(), caret),
            _ => {
                grid.write_text(&mut self.screen_buf);
                let text: Arc<str> = Arc::from(self.screen_buf.as_str());
                self.screen_text = Some((self.revision, text.clone()));
                (text, caret)
            }
        }
    }

    /// Keeps the scroll handle and the terminal's viewport on the same row:
    /// a handle the user moved scrolls the terminal, and a terminal that
    /// moved (output while following the bottom) moves the handle.
    fn sync_scroll(&mut self, m: Metrics) {
        if !self.scroll.is_settled() {
            self.nonce += 1;
        }
        let handle_row = (self.scroll.offset().1 / m.cell_h).round().max(0.0) as u64;
        if std::mem::take(&mut self.pending_scroll) {
            self.synced_row = handle_row;
        } else if handle_row != self.synced_row {
            self.vt.scroll(Scroll::Row(handle_row as usize));
            self.synced_row = handle_row;
            self.dirty = true;
        }
        let offset = self.vt.scrollbar().offset;
        if offset != self.synced_row {
            self.scroll.set_offset(0.0, offset as f32 * m.cell_h);
            self.synced_row = offset;
            self.pending_scroll = true;
            self.dirty = true;
        }
    }

    /// Where the rows block sits in the scroll content: the handle's offset,
    /// or the one requested this frame, so rows stay pinned to the top of
    /// the viewport.
    pub(crate) fn scroll_top(&self) -> f32 {
        if self.pending_scroll {
            self.synced_row as f32 * self.metrics().cell_h
        } else {
            self.scroll.offset().1
        }
    }

    pub(crate) fn nonce(&self) -> u64 {
        self.nonce
    }

    pub(crate) fn frame(&self) -> Option<Frame> {
        self.frame.clone()
    }

    pub(crate) fn shared_grid(&self) -> Rc<Grid> {
        self.grid.clone()
    }

    pub(crate) fn bounds_cell(&self) -> Rc<Cell<Rect>> {
        self.bounds.clone()
    }
}

#[cfg(test)]
impl TerminalState {
    /// A state sized to `cols` by `rows` cells with no window: metrics are
    /// the estimates, so tests need no fonts.
    pub(crate) fn headless(cols: u16, rows: u16) -> Self {
        let mut state = Self::new("test", FocusId::from_key("test.terminal"));
        let m = state.metrics();
        state.metrics = Some((m, 1f32.to_bits()));
        state.size = (cols, rows, 1f32.to_bits());
        state
            .vt
            .resize(cols, rows, m.cell_w as u32, m.cell_h as u32);
        state.bounds.set(Rect {
            x: 0.0,
            y: 0.0,
            width: f32::from(cols) * m.cell_w,
            height: f32::from(rows) * m.cell_h,
        });
        state
    }

    /// Snapshot without a frame.
    pub(crate) fn refresh(&mut self) -> &Grid {
        self.vt.snapshot(Rc::make_mut(&mut self.grid));
        &self.grid
    }

    pub(crate) fn vt_mut(&mut self) -> &mut Terminal {
        &mut self.vt
    }
}

/// The palette the view uses from the theme.
pub(crate) fn palette(theme: &Theme) -> Palette {
    use quark_ui::design::Alpha;
    let c = &theme.colors;
    Palette {
        selection: c.accent.with_alpha(Alpha::SOFT),
        cursor: c.text,
        link: c.text_accent,
    }
}
