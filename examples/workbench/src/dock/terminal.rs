//! The terminal panel (stream E): shell sessions, each a
//! `quark_terminal::TerminalState`, under a tab bar whose "+" opens
//! another. The state belongs to the panel (keyed by its `PanelId`), so
//! sessions keep running while the panel moves between windows.
//!
//! Launches run the user's login shell on a PTY (`--terminal real`). The
//! PTY's reader thread only wakes the app; [`wake`] feeds the output to
//! the terminal on the UI thread. A session's shell starts the first time
//! it is shown, sized to the panel, so it never starts at a guessed size
//! and redraws its prompt for the real one. Keys go through the VT encoder to the
//! PTY, the grid follows the panel's size (and the PTY gets the new rows
//! and columns), and a shell that exits keeps its screen with a "Restart
//! shell" bar under it. Dropping a session hangs up its shell, so closing
//! its tab or quitting the app ends it.
//!
//! `--terminal scripted`, what tests and e2e specs use, starts no process.
//! The first session shows `fixtures/terminal.ansi`; the bytes the encoder
//! would send to a PTY come back from [`TerminalState::take_input`] and
//! drive [`Shell`], which echoes them, edits the line, and answers `help`,
//! `ls`, `cat`, and `cargo test` from the in-memory fixture files.

use std::path::PathBuf;

use accesskit::Role;
use quark::view;
use quark_app::quark_ui::FocusId;
use quark_app::quark_ui::element::*;
use quark_app::quark_ui::icons::lucide;
use quark_app::quark_ui::style::Styled;
use quark_app::winit::event::{ElementState, MouseScrollDelta};
use quark_app::{InputEvent, KeyKind, UiContext, ViewContext, Waker};
use quark_components::{Button, ButtonSize, TabItem, tab_bar};
use quark_terminal::{
    KeyPress, PointerInput, PtyCommand, TerminalEnv, TerminalEvent, TerminalOutcome,
    TerminalSignal, TerminalState, terminal_view,
};

use crate::contracts::{COMMANDS, Options, SurfaceCx, TerminalMode};
use crate::design::tokens;
use crate::fixtures;
use crate::model::FileStore;

pub const FOCUS: FocusId = FocusId::from_key("workbench.terminal");

const PROMPT: &str = "\x1b[1;34m~/src/atlas\x1b[0m $ ";

/// The session tab strip, "+" included: a tab bar's height plus a little.
pub const HEADER_H: f32 = 36.0;
/// The bar under a session whose shell exited.
const EXITED_H: f32 = 36.0;
/// Pixels of touchpad motion per wheel line.
const WHEEL_LINE_PX: f32 = 20.0;

/// Each open session's terminal name. Cache entries and accessibility ids
/// need one unique in the window, `TerminalState` takes them `'static`,
/// and this many sessions is plenty for a panel.
const IDS: [&str; 8] = [
    "workbench.terminal",
    "workbench.terminal.2",
    "workbench.terminal.3",
    "workbench.terminal.4",
    "workbench.terminal.5",
    "workbench.terminal.6",
    "workbench.terminal.7",
    "workbench.terminal.8",
];

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    View(TerminalEvent),
    /// Show the session with this id.
    Select(u32),
    New,
    Close(u32),
    /// Start the shown session's shell again after it exited.
    Restart,
}

/// What sessions run.
#[derive(Debug, Clone)]
pub enum Backend {
    Scripted,
    Real(PtyCommand),
}

/// The user's login shell (`$SHELL`, else the account's, else `/bin/sh`;
/// `%ComSpec%` or `cmd.exe` on Windows) in `cwd`, else in this process's
/// directory rather than the home directory `portable-pty` would pick.
/// `TERM` and `COLORTERM` are quark-terminal's (`xterm-256color`,
/// `truecolor`).
pub fn shell_command(cwd: Option<PathBuf>) -> PtyCommand {
    let command = PtyCommand::shell();
    match cwd.or_else(|| std::env::current_dir().ok()) {
        Some(dir) => command.cwd(dir),
        None => command,
    }
}

struct Session {
    /// Its tab's id, stable while it lives.
    id: u32,
    /// Index of its name in [`IDS`].
    slot: usize,
    term: TerminalState,
    /// The scripted backend's line editor.
    shell: Shell,
    running: bool,
    /// Why the program is not running, once it exited or failed to start.
    /// Neither running nor exited, a real session starts when next shown.
    exited: Option<String>,
}

impl Session {
    fn label(&self) -> String {
        let title = self.term.title().trim();
        if title.is_empty() {
            return format!("Shell {}", self.id);
        }
        const MAX: usize = 24;
        match title.char_indices().nth(MAX) {
            Some((end, _)) => format!("{}\u{2026}", &title[..end]),
            None => title.to_owned(),
        }
    }

    /// Exits since the last call. Titles show in the tab from
    /// `TerminalState::title`; bells have nowhere to go in a panel.
    fn signals(&mut self) {
        for signal in self.term.take_signals() {
            if let TerminalSignal::Exited(code) = signal {
                self.running = false;
                self.exited = Some(match code {
                    Some(code) => format!("The shell exited with code {code}"),
                    None => "The shell exited".to_owned(),
                });
            }
        }
    }

    /// Run what the VT encoder queued for the scripted shell.
    fn pump(&mut self, files: &FileStore) {
        let input = self.term.take_input();
        if input.is_empty() {
            return;
        }
        let out = self.shell.input(&String::from_utf8_lossy(&input), files);
        self.term.feed(out.as_bytes());
    }
}

pub struct State {
    backend: Backend,
    /// Never empty.
    sessions: Vec<Session>,
    active: usize,
    next_id: u32,
    /// The app's waker, from [`init`], which the PTY threads call.
    waker: Option<Waker>,
    /// Workbench command bindings: these keys reach the app, not the
    /// terminal, so `mod+k` opens the palette while the terminal has focus.
    passthrough: Vec<Binding>,
    /// Told on the PTY threads after each wake, so tests wait for output
    /// without polling.
    #[cfg(test)]
    on_pty: Option<std::sync::mpsc::Sender<()>>,
}

impl State {
    pub fn new(options: &Options) -> Self {
        Self::with_backend(match options.terminal {
            TerminalMode::Scripted => Backend::Scripted,
            TerminalMode::Real => Backend::Real(shell_command(options.cwd.clone())),
        })
    }

    /// One session of `backend`. A real one starts when first shown.
    pub fn with_backend(backend: Backend) -> Self {
        let passthrough = COMMANDS
            .iter()
            .filter_map(|c| c.binding?.parse().ok())
            .collect();
        let mut state = Self {
            backend,
            sessions: Vec::new(),
            active: 0,
            next_id: 1,
            waker: None,
            passthrough,
            #[cfg(test)]
            on_pty: None,
        };
        state.open();
        state
    }

    fn scripted(&self) -> bool {
        matches!(self.backend, Backend::Scripted)
    }

    fn index(&self, id: u32) -> Option<usize> {
        self.sessions.iter().position(|s| s.id == id)
    }

    /// Add a session and show it. False when every name is taken.
    fn open(&mut self) -> bool {
        let Some(slot) = (0..IDS.len()).find(|i| self.sessions.iter().all(|s| s.slot != *i)) else {
            return false;
        };
        let mut term = TerminalState::new(IDS[slot], FOCUS);
        if self.scripted() {
            if self.sessions.is_empty() {
                // The scene the thread describes: the fixture test run,
                // then a prompt waiting for input.
                term.feed(PROMPT.as_bytes());
                term.feed(b"npm test\r\n");
                term.feed(fixtures::TERMINAL_ANSI);
                term.feed(b"\r\n");
            }
            term.feed(PROMPT.as_bytes());
        }
        self.sessions.push(Session {
            id: self.next_id,
            slot,
            term,
            shell: Shell::default(),
            running: false,
            exited: None,
        });
        self.next_id += 1;
        self.active = self.sessions.len() - 1;
        true
    }

    /// Start session `index`'s program if it is real and waits to start.
    fn start(&mut self, index: usize) {
        let Self {
            backend,
            sessions,
            waker,
            ..
        } = self;
        let (Backend::Real(command), Some(waker)) = (&*backend, waker.clone()) else {
            return;
        };
        #[cfg(test)]
        let notify = self.on_pty.clone();
        let session = &mut sessions[index];
        if session.running || session.exited.is_some() {
            return;
        }
        let spawned = session.term.spawn(command, move || {
            waker.wake();
            #[cfg(test)]
            if let Some(notify) = &notify {
                let _ = notify.send(());
            }
        });
        match spawned {
            Ok(()) => {
                session.running = true;
                session.exited = None;
            }
            Err(e) => {
                let program = command
                    .program
                    .as_ref()
                    .map_or("the shell".into(), |p| p.to_string_lossy());
                session.exited = Some(format!("Could not start {program}: {e}"));
            }
        }
    }

    /// The shown session's screen as text, as of its last frame.
    pub fn screen_text(&self) -> String {
        self.sessions[self.active].term.grid().text()
    }

    /// The shown session's grid size in cells.
    pub fn size(&self) -> (u16, u16) {
        self.sessions[self.active].term.size()
    }
}

impl Default for State {
    fn default() -> Self {
        Self::with_backend(Backend::Scripted)
    }
}

pub fn init(state: &mut State, cx: &mut UiContext) {
    state.waker = Some(cx.window.waker().clone());
}

/// Feed every session the output its PTY has waiting. Runs on any wake,
/// in whichever window's context; the panel may be in another, so every
/// window redraws.
pub fn wake(state: &mut State, cx: &mut UiContext) {
    let mut any = false;
    for session in &mut state.sessions {
        if session.term.read_pty() {
            session.signals();
            any = true;
        }
    }
    if any {
        cx.window.request_redraw_all();
    }
}

pub fn view(state: &mut State, scx: &SurfaceCx, vcx: &mut ViewContext) -> AnyElement {
    let (width, height) = scx.size;
    let header = header(state, width, scx);
    let scripted = state.scripted();
    let session = &mut state.sessions[state.active];
    if scripted {
        session.pump(&scx.model.files);
    }
    let bar = session
        .exited
        .clone()
        .map(|why| exited_bar(why, width, scx));
    let bar_h = if bar.is_some() { EXITED_H } else { 0.0 };
    let term_h = (height - HEADER_H - bar_h).max(0.0);
    session.term.set_viewport(width, term_h);
    let scale = vcx.frame.scale_factor();
    let text_cx = vcx.frame.text();
    session
        .term
        .prepare(&mut text_cx.system, &mut text_cx.layouts, scale, scx.theme);
    // Sized now: start a shell that waits for it.
    state.start(state.active);
    let session = &mut state.sessions[state.active];
    let env = TerminalEnv {
        focused: vcx.is_focused(FOCUS),
        accessible: vcx.frame.accessibility_active(),
    };
    let term = terminal_view(&mut session.term, scx.theme, env, |e| {
        super::Action::Terminal(Action::View(e)).into()
    });
    view! {
        <div w={width} h={height} class="flex-col">
            {header}
            <div w={width} h={term_h} class="shrink-0">{term}</div>
            {?bar}
        </div>
    }
    .into_any()
}

/// The session tabs and the "+" that opens another.
fn header(state: &State, width: f32, scx: &SurfaceCx) -> AnyElement {
    let colors = &scx.theme.colors;
    let closable = state.sessions.len() > 1;
    let tabs = state
        .sessions
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let tab = TabItem::new(s.label(), super::Action::Terminal(Action::Select(s.id)))
                .id(format!("workbench.terminal.session.{}", s.id))
                .icon(lucide::TERMINAL)
                .active(i == state.active);
            if closable {
                tab.on_close(super::Action::Terminal(Action::Close(s.id)))
            } else {
                tab
            }
        })
        .collect();
    let room = state.sessions.len() < IDS.len();
    view! {
        <div w={width} h={HEADER_H} class="flex-row items-end shrink-0 overflow-hidden"
             px={tokens::SPACE_4} border_b={colors.border_variant} bg={colors.panel}>
            {tab_bar(tabs).label("Terminal sessions")}
            <div class="flex-1" />
            if room {
                <div h={HEADER_H} class="flex-row items-center shrink-0">
                    <div id="workbench.terminal.new" accessibility_role={Role::Button}
                         aria-label="New terminal" tooltip="New terminal"
                         class="w-[24] h-[24] items-center justify-center rounded-[6]"
                         hover_bg={colors.element_hover}
                         on:click={super::Action::Terminal(Action::New)}>
                        {svg_icon(lucide::PLUS, 14.0).color(colors.text_muted)}
                    </div>
                </div>
            }
        </div>
    }
    .into_any()
}

fn exited_bar(why: String, width: f32, scx: &SurfaceCx) -> AnyElement {
    let colors = &scx.theme.colors;
    view! {
        <div w={width} h={EXITED_H} class="flex-row items-center shrink-0 gap-[8]"
             px={tokens::SPACE_8} border_t={colors.border_variant} bg={colors.panel}
             role="alert" aria-label={why.clone()}>
            <icon svg={lucide::INFO} size={12.0} color={colors.text_muted} />
            <text size={12.0} color={colors.text_muted}>{why}</text>
            <div class="flex-1" />
            <Button on:click={super::Action::Terminal(Action::Restart)} label="Restart shell"
                    icon={lucide::REFRESH} size={ButtonSize::Compact} />
        </div>
    }
    .into_any()
}

pub fn update(state: &mut State, action: Action, cx: &mut UiContext) {
    match action {
        Action::View(event) => {
            let now_ms = cx.window.elapsed().as_millis() as u64;
            let session = &mut state.sessions[state.active];
            let outcome = session.term.handle(event, now_ms);
            apply(session, outcome, cx);
        }
        Action::Select(id) => {
            if let Some(index) = state.index(id) {
                state.active = index;
            }
            cx.set_focus(Some(FOCUS));
        }
        Action::New => {
            state.open();
            cx.set_focus(Some(FOCUS));
        }
        Action::Close(id) => {
            // The last session stays: the panel always has a terminal.
            let Some(index) = state.index(id).filter(|_| state.sessions.len() > 1) else {
                return;
            };
            // Dropping it hangs up its shell.
            state.sessions.remove(index);
            if state.active > index || state.active == state.sessions.len() {
                state.active -= 1;
            }
            cx.set_focus(Some(FOCUS));
        }
        Action::Restart => {
            let index = state.active;
            let session = &mut state.sessions[index];
            if session.running {
                return;
            }
            // A full reset, so modes the old program left set (the
            // alternate screen, mouse reporting) do not outlive it. The
            // next frame starts the shell.
            session.term.feed(b"\x1bc");
            session.exited = None;
            cx.set_focus(Some(FOCUS));
        }
    }
}

fn apply(session: &mut Session, outcome: TerminalOutcome, cx: &mut UiContext) {
    match outcome {
        TerminalOutcome::Copy(text) => cx.window.set_clipboard_text(&text),
        TerminalOutcome::Paste => {
            if let Some(text) = cx.window.clipboard_text() {
                // Pasted as is: shells that could run a pasted line ask
                // for bracketed paste, which marks it as pasted.
                let _ = session.term.paste(&text, true);
            }
        }
        TerminalOutcome::OpenLink(_) | TerminalOutcome::Ignored | TerminalOutcome::Handled => {}
    }
    session.signals();
}

/// Raw input for the terminal, from the app's input hook. `here` is
/// whether the event's window shows the panel (pointer input from other
/// windows is not the terminal's). True when the terminal took it.
pub fn input(state: &mut State, event: &InputEvent, here: bool, cx: &mut UiContext) -> bool {
    let focused = cx.focus() == Some(FOCUS);
    let scripted = state.scripted();
    let passthrough = &state.passthrough;
    let session = &mut state.sessions[state.active];
    let term = &mut session.term;
    let consumed = match event {
        InputEvent::ModifiersChanged(modifiers) => {
            term.set_modifiers(*modifiers);
            false
        }
        InputEvent::Focused(window_focused) => {
            term.focus_changed(*window_focused && focused);
            false
        }
        InputEvent::KeyPress(chord) if focused => {
            let pressed = chord.binding();
            let app_key = pressed
                .as_ref()
                .is_some_and(|p| passthrough.iter().any(|b| b.matches(p)));
            // Shift+Tab moves focus on, so keyboard users can leave the
            // panel; so does Tab with the scripted shell, which completes
            // nothing. A real shell gets Tab for completion.
            let tab = pressed.as_ref().is_some_and(|p| p.key == "tab")
                && (scripted || chord.modifiers.shift_key());
            if app_key || tab {
                return false;
            }
            let (named, text) = match &chord.logical {
                KeyKind::Named(named) => (Some(*named), None),
                KeyKind::Character(text) => (None, Some(text.as_str())),
                KeyKind::Other => (None, None),
            };
            let outcome = term.key_press(&KeyPress {
                named,
                text,
                physical: chord.physical,
                modifiers: chord.modifiers,
                repeat: chord.repeat,
            });
            let consumed = outcome != TerminalOutcome::Ignored;
            apply(session, outcome, cx);
            consumed
        }
        InputEvent::TextInput(text) if focused => {
            term.text_input(text);
            true
        }
        InputEvent::ImeCommit(text) if focused => {
            term.commit_preedit(text);
            true
        }
        InputEvent::ImePreedit(text, cursor) if focused => {
            term.set_preedit(text, *cursor);
            true
        }
        // Mouse reporting for full-screen programs, and the wheel as
        // arrow keys for those that only switch to the alternate screen.
        // Presses only reach a focused terminal, so the first click on it
        // focuses it.
        InputEvent::PointerMoved { x, y } if here => {
            term.pointer(PointerInput::Moved { x: *x, y: *y })
        }
        InputEvent::PointerButton { button, state } if here && focused => {
            term.pointer(PointerInput::Button {
                button: *button,
                pressed: *state == ElementState::Pressed,
            })
        }
        InputEvent::Wheel { delta, .. } if here => {
            let lines = match delta {
                MouseScrollDelta::LineDelta(_, y) => *y,
                MouseScrollDelta::PixelDelta(p) => p.y as f32 / WHEEL_LINE_PX,
            };
            term.pointer(PointerInput::Wheel { lines })
        }
        _ => false,
    };
    // The scripted shell answers in the next frame's view, which reads
    // the file store; raw input arrives without the model.
    if consumed {
        cx.window.request_redraw();
    }
    consumed
}

/// The scripted shell: line editing over the bytes a terminal sends, and
/// a handful of commands over the fixture files.
#[derive(Debug, Default)]
pub struct Shell {
    line: String,
    /// Inside an escape sequence the encoder sent (arrow keys, focus
    /// reports): skipped, the shell has no history or cursor movement.
    escape: Escape,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum Escape {
    #[default]
    None,
    /// After ESC.
    Start,
    /// After `ESC [`, until a final byte.
    Csi,
    /// After `ESC O`: one more byte.
    Ss3,
}

impl Shell {
    /// Take `input` as typed and return what the terminal shows: echoes,
    /// command output, and prompts.
    pub fn input(&mut self, input: &str, files: &FileStore) -> String {
        let mut out = String::new();
        for c in input.chars() {
            match self.escape {
                Escape::Start => {
                    self.escape = match c {
                        '[' => Escape::Csi,
                        'O' => Escape::Ss3,
                        _ => Escape::None,
                    };
                    continue;
                }
                Escape::Csi => {
                    if ('\x40'..='\x7e').contains(&c) {
                        self.escape = Escape::None;
                    }
                    continue;
                }
                Escape::Ss3 => {
                    self.escape = Escape::None;
                    continue;
                }
                Escape::None => {}
            }
            match c {
                '\x1b' => self.escape = Escape::Start,
                '\r' | '\n' => {
                    out.push_str("\r\n");
                    let line = std::mem::take(&mut self.line);
                    out.push_str(&run(line.trim(), files));
                    out.push_str(PROMPT);
                }
                '\x7f' | '\x08' => {
                    if self.line.pop().is_some() {
                        out.push_str("\x08 \x08");
                    }
                }
                '\x03' => {
                    self.line.clear();
                    out.push_str("^C\r\n");
                    out.push_str(PROMPT);
                }
                c if c.is_control() => {}
                c => {
                    self.line.push(c);
                    out.push(c);
                }
            }
        }
        out
    }
}

/// The output of one command line, lines ending in CR LF.
pub fn run(line: &str, files: &FileStore) -> String {
    let mut words = line.split_whitespace();
    let Some(command) = words.next() else {
        return String::new();
    };
    let args: Vec<&str> = words.collect();
    let text = match (command, args.as_slice()) {
        ("help", _) => "Demo terminal: a scripted shell over the Atlas fixture files.\n\
             Commands: help, ls [dir], cat <file>, cargo test, npm test\n"
            .to_owned(),
        ("ls", []) => ls(files, ""),
        ("ls", [dir]) => ls(files, dir),
        ("cat", []) => "cat: missing file operand\n".to_owned(),
        ("cat", paths) => paths
            .iter()
            .map(|path| match files.get(path.trim_start_matches("./")) {
                Some(text) if text.ends_with('\n') => text.to_owned(),
                Some(text) => format!("{text}\n"),
                None => format!("cat: {path}: No such file or directory\n"),
            })
            .collect(),
        ("cargo", ["test", ..]) => CARGO_TEST.to_owned(),
        ("npm", ["test", ..]) => {
            return format!("{}\r\n", String::from_utf8_lossy(fixtures::TERMINAL_ANSI));
        }
        _ => format!("{command}: command not found (try help)\n"),
    };
    text.replace('\n', "\r\n")
}

/// Entries directly under `dir` ("" for the project root), directories
/// first with a trailing slash.
fn ls(files: &FileStore, dir: &str) -> String {
    let dir = dir.trim_start_matches("./").trim_end_matches('/');
    let prefix = if dir.is_empty() || dir == "." {
        String::new()
    } else {
        format!("{dir}/")
    };
    let mut dirs = Vec::new();
    let mut leaves = Vec::new();
    for path in files.paths() {
        let Some(rest) = path.strip_prefix(&prefix) else {
            continue;
        };
        match rest.split_once('/') {
            Some((sub, _)) => {
                let entry = format!("\x1b[1;34m{sub}/\x1b[0m");
                if !dirs.contains(&entry) {
                    dirs.push(entry);
                }
            }
            None => leaves.push(rest.to_owned()),
        }
    }
    if dirs.is_empty() && leaves.is_empty() {
        return format!("ls: cannot access '{dir}': No such file or directory\n");
    }
    dirs.extend(leaves);
    format!("{}\n", dirs.join("  "))
}

/// What `cargo test` prints: the scripted run of Atlas's geocoder crate.
const CARGO_TEST: &str = "\x1b[1;32m   Compiling\x1b[0m atlas-geo v0.4.0 (~/src/atlas/geo)\n\
\x1b[1;32m    Finished\x1b[0m `test` profile [unoptimized + debuginfo] target(s) in 1.84s\n\
\x1b[1;32m     Running\x1b[0m unittests src/lib.rs (target/debug/deps/atlas_geo-5f1c)\n\
\n\
running 4 tests\n\
test bbox::tests::contains_its_corners ... \x1b[32mok\x1b[0m\n\
test geocode::tests::caches_repeat_queries ... \x1b[32mok\x1b[0m\n\
test geocode::tests::retries_once_on_timeout ... \x1b[32mok\x1b[0m\n\
test tiles::tests::offline_cache_round_trips ... \x1b[32mok\x1b[0m\n\
\n\
test result: \x1b[32mok\x1b[0m. 4 passed; 0 failed; 0 ignored; finished in 0.03s\n";

#[cfg(test)]
mod tests {
    use super::*;

    fn files() -> FileStore {
        FileStore::new(
            [
                ("README.md", "# Atlas\n"),
                ("src/App.tsx", "app"),
                ("src/x.ts", "x\n"),
            ]
            .map(|(p, t)| (p.to_owned(), t.to_owned())),
        )
    }

    /// Strip SGR sequences so tables compare words.
    fn plain(s: &str) -> String {
        let mut out = String::new();
        let mut chars = s.chars();
        while let Some(c) = chars.next() {
            if c == '\x1b' {
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }

    // Catches commands answering from the wrong place: each line's output
    // comes from the fixture file store, CR LF terminated.
    #[test]
    fn commands_answer_from_the_file_store() {
        let cases = [
            ("ls", "src/  README.md\r\n"),
            ("ls src", "App.tsx  x.ts\r\n"),
            (
                "ls nope",
                "ls: cannot access 'nope': No such file or directory\r\n",
            ),
            ("cat src/App.tsx", "app\r\n"),
            ("cat ./src/x.ts", "x\r\n"),
            ("cat gone.ts", "cat: gone.ts: No such file or directory\r\n"),
            ("rm -rf /", "rm: command not found (try help)\r\n"),
            ("", ""),
        ];
        for (line, want) in cases {
            assert_eq!(plain(&run(line, &files())), want, "{line:?}");
        }
    }

    // Catches the line editor sending raw keys: typed text echoes,
    // backspace erases, arrow-key sequences are skipped, and Enter runs
    // the edited line.
    #[test]
    fn the_line_editor_runs_what_was_typed() {
        let mut shell = Shell::default();
        let out = shell.input("lx\x7fs\x1b[A\x1bOD sr\x7f\x7fsrc\r", &files());
        let out = plain(&out);
        assert!(
            out.starts_with("lx\x08 \x08s sr\x08 \x08\x08 \x08src\r\nApp.tsx  x.ts\r\n"),
            "{out:?}"
        );
        assert!(out.ends_with("~/src/atlas $ "), "{out:?}");
    }

    /// Real sessions, each running a `sh -c` script in place of the
    /// user's shell, in the workbench with the terminal panel shown.
    #[cfg(unix)]
    mod real {
        use std::sync::mpsc::{self, Receiver};
        use std::time::Duration;

        use accesskit::Role;
        use quark_app::testing::{By, UiTestHarness};

        use super::super::*;
        use crate::{Workbench, adapter};

        /// How long a script may take to answer: generous for a loaded
        /// CI machine, and only spent when a test fails.
        const DEADLINE: Duration = Duration::from_secs(20);

        /// The workbench at `size` with the terminal running `script`,
        /// shown and focused, and the channel its PTY wakes report on.
        fn harness(script: &str, size: (f32, f32)) -> (UiTestHarness<Workbench>, Receiver<()>) {
            let mut app = Workbench::new(Options::default());
            let (tx, rx) = mpsc::channel();
            let command = PtyCommand::new("/bin/sh").arg("-c").arg(script);
            let mut terminal = State::with_backend(Backend::Real(command));
            terminal.on_pty = Some(tx);
            app.dock.panels.terminal = terminal;
            let mut ui = UiTestHarness::with_adapter(adapter(app), size, 1.0);
            ui.frame();
            ui.click_node(By::role_name(Role::Tab, "Terminal"));
            ui.click_node(By::role(Role::Terminal));
            (ui, rx)
        }

        /// Deliver PTY wakes until the shown screen holds `text`; returns
        /// the screen.
        #[track_caller]
        fn wait_for(ui: &mut UiTestHarness<Workbench>, rx: &Receiver<()>, text: &str) -> String {
            loop {
                let screen = ui.app().dock.panels.terminal.screen_text();
                if screen.contains(text) {
                    return screen;
                }
                if rx.recv_timeout(DEADLINE).is_err() {
                    panic!("no {text:?} on the screen before the deadline:\n{screen}");
                }
                ui.run_until_idle();
            }
        }

        // Catches keys that never reach the PTY, or output that never
        // reaches the screen through the app's wake: a typed line is
        // echoed by the terminal, read by the program, and answered.
        #[test]
        fn typed_input_reaches_the_shell_and_its_answer_shows() {
            let (mut ui, rx) = harness("read x; echo got:$x", (1440.0, 900.0));
            ui.type_text("hello\n");
            let screen = wait_for(&mut ui, &rx, "got:hello");
            assert_eq!(screen.lines().collect::<Vec<_>>(), ["hello", "got:hello"]);
        }

        // Catches a grid that resizes without telling the program: after
        // the window gets shorter, `stty size` reports the panel's new
        // rows and columns.
        #[test]
        fn a_resized_panel_reaches_the_program() {
            let (mut ui, rx) = harness("while read x; do stty size; done", (1440.0, 900.0));
            let mut reported = Vec::new();
            for height in [900.0, 700.0] {
                ui.resize(1440.0, height);
                let (cols, rows) = ui.app().dock.panels.terminal.size();
                let size = format!("{rows} {cols}");
                ui.type_text("\n");
                wait_for(&mut ui, &rx, &size);
                reported.push(size);
            }
            assert_ne!(reported[0], reported[1]);
        }

        // Catches an exit that leaves a dead panel: the exit code shows in
        // an alert, and Restart shell runs the program again (a new pid).
        #[test]
        fn an_exited_shell_restarts_from_its_bar() {
            let (mut ui, rx) = harness("echo pid:$$; exit 3", (1440.0, 900.0));
            let exited = By::role_name(Role::Alert, "The shell exited with code 3");
            let first = wait_for(&mut ui, &rx, "pid:");
            while ui.try_find(exited.clone()).is_none() {
                rx.recv_timeout(DEADLINE)
                    .expect("no exit before the deadline");
                ui.run_until_idle();
            }

            ui.click_node(By::role_name(Role::Button, "Restart shell"));
            let second = wait_for(&mut ui, &rx, "pid:");

            assert_ne!(first.trim(), second.trim());
            assert!(second.trim().starts_with("pid:"), "{second}");
        }
    }
}
