//! The terminal panel (stream E): `quark_terminal::TerminalState` fed
//! from `fixtures/terminal.ansi` and a scripted command interpreter.
//!
//! Nothing here starts a process. Keys go through the real VT encoder;
//! the bytes it would send to a PTY come back from
//! [`TerminalState::take_input`] and drive [`Shell`], which echoes them,
//! edits the line, and answers `help`, `ls`, `cat`, and `cargo test` from
//! the in-memory fixture files.

use quark_app::quark_ui::FocusId;
use quark_app::quark_ui::element::{AnyElement, Binding};
use quark_app::{InputEvent, KeyKind, UiContext, ViewContext};
use quark_terminal::{KeyPress, TerminalEnv, TerminalEvent, TerminalOutcome, terminal_view};

use crate::contracts::{COMMANDS, SurfaceCx};
use crate::fixtures;
use crate::model::FileStore;

pub const FOCUS: FocusId = FocusId::from_key("workbench.terminal");

const PROMPT: &str = "\x1b[1;34m~/src/atlas\x1b[0m $ ";

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    View(TerminalEvent),
}

pub struct State {
    term: quark_terminal::TerminalState,
    shell: Shell,
    /// Workbench command bindings: these keys reach the app, not the
    /// terminal, so `mod+k` opens the palette while the terminal has focus.
    passthrough: Vec<Binding>,
}

impl State {
    pub fn new() -> Self {
        let mut term = quark_terminal::TerminalState::new("workbench.terminal", FOCUS);
        // The scene the thread describes: the fixture test run, then a
        // prompt waiting for input.
        term.feed(PROMPT.as_bytes());
        term.feed(b"npm test\r\n");
        term.feed(fixtures::TERMINAL_ANSI);
        term.feed(b"\r\n");
        term.feed(PROMPT.as_bytes());
        let passthrough = COMMANDS
            .iter()
            .filter_map(|c| c.binding?.parse().ok())
            .collect();
        Self {
            term,
            shell: Shell::default(),
            passthrough,
        }
    }

    /// The whole scrollback and screen as text, for tests.
    pub fn text(&self) -> String {
        self.term.grid().text()
    }

    /// Run what the VT encoder queued for the (absent) program.
    fn pump(&mut self, files: &FileStore) {
        let input = self.term.take_input();
        if input.is_empty() {
            return;
        }
        let out = self.shell.input(&String::from_utf8_lossy(&input), files);
        self.term.feed(out.as_bytes());
    }
}

impl Default for State {
    fn default() -> Self {
        Self::new()
    }
}

pub fn view(state: &mut State, scx: &SurfaceCx, vcx: &mut ViewContext) -> AnyElement {
    let (width, height) = scx.size;
    state.pump(&scx.model.files);
    state.term.set_viewport(width, height);
    let scale = vcx.frame.scale_factor();
    let text_cx = vcx.frame.text();
    state
        .term
        .prepare(&mut text_cx.system, &mut text_cx.layouts, scale, scx.theme);
    let env = TerminalEnv {
        focused: vcx.is_focused(FOCUS),
        accessible: vcx.frame.accessibility_active(),
    };
    terminal_view(&mut state.term, scx.theme, env, |e| {
        super::Action::Terminal(Action::View(e)).into()
    })
}

pub fn update(state: &mut State, action: Action, cx: &mut UiContext) {
    let Action::View(event) = action;
    let now_ms = cx.window.elapsed().as_millis() as u64;
    let outcome = state.term.handle(event, now_ms);
    apply(state, outcome, cx);
}

fn apply(state: &mut State, outcome: TerminalOutcome, cx: &mut UiContext) {
    match outcome {
        TerminalOutcome::Copy(text) => cx.window.set_clipboard_text(&text),
        TerminalOutcome::Paste => {
            if let Some(text) = cx.window.clipboard_text() {
                // Nothing runs a pasted line but the scripted shell, so the
                // unsafe-paste prompt a real terminal needs has no purpose.
                let _ = state.term.paste(&text, true);
            }
        }
        TerminalOutcome::OpenLink(_) | TerminalOutcome::Ignored | TerminalOutcome::Handled => {}
    }
    // Titles and bells have nowhere to go in a panel.
    let _ = state.term.take_signals();
}

/// Raw input for the terminal, from the app's input hook. True when the
/// terminal took it.
pub fn input(state: &mut State, event: &InputEvent, cx: &mut UiContext) -> bool {
    let focused = cx.focus() == Some(FOCUS);
    let consumed = match event {
        InputEvent::ModifiersChanged(modifiers) => {
            state.term.set_modifiers(*modifiers);
            false
        }
        InputEvent::Focused(window_focused) => {
            state.term.focus_changed(*window_focused && focused);
            false
        }
        InputEvent::KeyPress(chord) if focused => {
            let pressed = chord.binding();
            let app_key = pressed
                .as_ref()
                .is_some_and(|p| state.passthrough.iter().any(|b| b.matches(p)));
            // Tab and Shift+Tab move focus on: the scripted shell completes
            // nothing, and keyboard users must be able to leave the panel.
            let tab = pressed.as_ref().is_some_and(|p| p.key == "tab");
            if app_key || tab {
                return false;
            }
            let (named, text) = match &chord.logical {
                KeyKind::Named(named) => (Some(*named), None),
                KeyKind::Character(text) => (None, Some(text.as_str())),
                KeyKind::Other => (None, None),
            };
            let outcome = state.term.key_press(&KeyPress {
                named,
                text,
                physical: chord.physical,
                modifiers: chord.modifiers,
                repeat: chord.repeat,
            });
            let consumed = outcome != TerminalOutcome::Ignored;
            apply(state, outcome, cx);
            consumed
        }
        InputEvent::TextInput(text) if focused => {
            state.term.text_input(text);
            true
        }
        InputEvent::ImeCommit(text) if focused => {
            state.term.commit_preedit(text);
            true
        }
        InputEvent::ImePreedit(text, cursor) if focused => {
            state.term.set_preedit(text, *cursor);
            true
        }
        _ => false,
    };
    // The shell answers in the next frame's view, which reads the file
    // store; raw input arrives without the model.
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
}
