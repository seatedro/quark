//! A terminal. `terminal_demo` runs `$SHELL`; `terminal_demo PROGRAM ARGS…`
//! runs that instead. The window closes when the program exits.
//!
//! Drag selects, double click selects a word, triple click a line. Copy and
//! paste are Cmd+C and Cmd+V on macOS, Ctrl+Shift+C and Ctrl+Shift+V
//! elsewhere. Ctrl+click (Cmd+click on macOS) opens a hyperlink. Shift+drag
//! selects while a program has mouse reporting on.
//!
//! Needs Zig 0.16 to build libghostty-vt (see quark-terminal's build.rs).
//! On Windows the terminal is not available yet and the demo exits.

use quark_app::quark_ui::element::AnyElement;
use quark_app::quark_ui::{Action, FocusId};
use quark_app::{UiApp, UiContext, ViewContext, WindowOptions};
use quark_terminal::{PtyCommand, PtyEvent};

#[cfg(not(windows))]
use quark_terminal::{TerminalEvent, TerminalState};

const TERM_FOCUS: FocusId = FocusId::from_key("demo.terminal");

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    #[cfg(not(windows))]
    Term(TerminalEvent),
}

impl From<Msg> for Action {
    fn from(msg: Msg) -> Self {
        Action::new(msg)
    }
}

struct Demo {
    #[cfg(not(windows))]
    term: TerminalState,
    /// What to run once the window opens; `None` runs nothing (tests feed
    /// the terminal directly).
    command: Option<PtyCommand>,
    /// Called on the PTY thread after each event is sent, so tests can
    /// wait for output without polling.
    #[cfg(all(test, not(windows)))]
    on_pty: Option<std::sync::mpsc::Sender<()>>,
    /// Allocations the last frame's `TerminalState::prepare` made.
    #[cfg(all(test, not(windows)))]
    prepare_allocations: u64,
}

impl Demo {
    fn new(command: Option<PtyCommand>) -> Self {
        Self {
            #[cfg(not(windows))]
            term: TerminalState::new("demo.terminal", TERM_FOCUS),
            command,
            #[cfg(all(test, not(windows)))]
            on_pty: None,
            #[cfg(all(test, not(windows)))]
            prepare_allocations: 0,
        }
    }
}

#[cfg(not(windows))]
mod app {
    use quark_app::InputEvent;
    use quark_app::KeyKind;
    use quark_app::winit::event::{ElementState, MouseScrollDelta};
    use quark_terminal::{
        KeyPress, PointerInput, TerminalEnv, TerminalOutcome, TerminalSignal, terminal_view,
    };

    use super::*;

    /// Pixels of touchpad motion per wheel line.
    const WHEEL_LINE_PX: f32 = 20.0;

    impl Demo {
        fn apply(&mut self, outcome: TerminalOutcome, cx: &mut UiContext) {
            match outcome {
                TerminalOutcome::Copy(text) => cx.window.set_clipboard_text(&text),
                TerminalOutcome::Paste => {
                    if let Some(text) = cx.window.clipboard_text() {
                        // A real app should ask before pasting text that
                        // could run a command (`UnsafePaste`); the demo
                        // pastes it anyway.
                        let _ = self.term.paste(&text, true);
                    }
                }
                TerminalOutcome::OpenLink(uri) => open(&uri),
                TerminalOutcome::Ignored | TerminalOutcome::Handled => {}
            }
            self.signals(cx);
            cx.window.request_redraw();
        }

        fn signals(&mut self, cx: &mut UiContext) {
            for signal in self.term.take_signals() {
                match signal {
                    TerminalSignal::Title(title) => cx.window.set_title(&title),
                    TerminalSignal::Clipboard(text) => cx.window.set_clipboard_text(&text),
                    TerminalSignal::Exited(_) => cx.window.exit(),
                    TerminalSignal::Bell => {}
                }
            }
        }
    }

    fn open(uri: &str) {
        let opener = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        if let Err(e) = std::process::Command::new(opener).arg(uri).spawn() {
            eprintln!("could not open {uri}: {e}");
        }
    }

    impl UiApp for Demo {
        type Action = Msg;
        type Message = PtyEvent;

        fn init(&mut self, cx: &mut UiContext) {
            cx.set_focus(Some(TERM_FOCUS));
            let Some(command) = self.command.clone() else {
                return;
            };
            let sender = cx.sender::<PtyEvent>();
            #[cfg(test)]
            let notify = self.on_pty.clone();
            let spawned = self.term.spawn(&command, move |event| {
                sender.send(event);
                #[cfg(test)]
                if let Some(notify) = &notify {
                    let _ = notify.send(());
                }
            });
            if let Err(e) = spawned {
                eprintln!("could not start {command:?}: {e}");
                cx.window.exit();
            }
        }

        fn view(&mut self, cx: &mut ViewContext) -> AnyElement {
            let (width, height) = cx.frame.size();
            self.term.set_viewport(width, height);
            let scale = cx.frame.scale_factor();
            let text = cx.frame.text();
            #[cfg(not(test))]
            self.term
                .prepare(&mut text.system, &mut text.layouts, scale, cx.theme);
            // Tests check what preparing alone allocates.
            #[cfg(test)]
            {
                let term = &mut self.term;
                self.prepare_allocations = quark_app::quark_ui::test_alloc::count(|| {
                    term.prepare(&mut text.system, &mut text.layouts, scale, cx.theme);
                })
                .1;
            }
            let env = TerminalEnv {
                focused: cx.is_focused(TERM_FOCUS),
                accessible: cx.frame.accessibility_active(),
            };
            terminal_view(&mut self.term, cx.theme, env, |e| Msg::Term(e).into())
        }

        fn update(&mut self, msg: Msg, cx: &mut UiContext) {
            let Msg::Term(event) = msg;
            let now_ms = cx.window.elapsed().as_millis() as u64;
            let outcome = self.term.handle(event, now_ms);
            self.apply(outcome, cx);
        }

        fn message(&mut self, event: PtyEvent, cx: &mut UiContext) {
            self.term.handle_pty(event);
            self.signals(cx);
            cx.window.request_redraw();
        }

        fn event(&mut self, event: &InputEvent, cx: &mut UiContext) -> bool {
            let focused = cx.focus() == Some(TERM_FOCUS);
            let consumed = match event {
                InputEvent::ModifiersChanged(modifiers) => {
                    self.term.set_modifiers(*modifiers);
                    false
                }
                InputEvent::Focused(focused) => {
                    self.term.focus_changed(*focused);
                    false
                }
                InputEvent::KeyPress(chord) if focused => {
                    let (named, text) = match &chord.logical {
                        KeyKind::Named(named) => (Some(*named), None),
                        KeyKind::Character(text) => (None, Some(text.as_str())),
                        KeyKind::Other => (None, None),
                    };
                    let outcome = self.term.key_press(&KeyPress {
                        named,
                        text,
                        physical: chord.physical,
                        modifiers: chord.modifiers,
                        repeat: chord.repeat,
                    });
                    let consumed = outcome != TerminalOutcome::Ignored;
                    self.apply(outcome, cx);
                    consumed
                }
                InputEvent::TextInput(text) if focused => {
                    self.term.text_input(text);
                    cx.window.request_redraw();
                    true
                }
                InputEvent::PointerMoved { x, y } => {
                    self.term.pointer(PointerInput::Moved { x: *x, y: *y })
                }
                InputEvent::PointerButton { button, state } => {
                    self.term.pointer(PointerInput::Button {
                        button: *button,
                        pressed: *state == ElementState::Pressed,
                    })
                }
                InputEvent::Wheel { delta, .. } => {
                    let lines = match delta {
                        MouseScrollDelta::LineDelta(_, y) => *y,
                        MouseScrollDelta::PixelDelta(p) => p.y as f32 / WHEEL_LINE_PX,
                    };
                    self.term.pointer(PointerInput::Wheel { lines })
                }
                _ => false,
            };
            if consumed {
                cx.window.request_redraw();
            }
            consumed
        }
    }
}

/// Without libghostty-vt (Windows) the demo has nothing to show.
#[cfg(windows)]
impl UiApp for Demo {
    type Action = Msg;
    type Message = PtyEvent;

    fn init(&mut self, cx: &mut UiContext) {
        let _ = (&self.command, TERM_FOCUS);
        eprintln!("quark-terminal is not available on Windows yet");
        cx.window.exit();
    }

    fn view(&mut self, _cx: &mut ViewContext) -> AnyElement {
        use quark_app::quark_ui::element::{IntoAnyElement, div};
        quark::view! { <div /> }
    }

    fn update(&mut self, msg: Msg, _cx: &mut UiContext) {
        match msg {}
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let command = match args.next() {
        Some(program) => args.fold(PtyCommand::new(program), PtyCommand::arg),
        None => PtyCommand::shell(),
    };
    quark_app::run_ui(
        Demo::new(Some(command)),
        WindowOptions {
            title: "Terminal".into(),
            size: (900.0, 560.0),
            ..WindowOptions::default()
        },
    )?;
    Ok(())
}

#[cfg(all(test, not(windows)))]
mod tests {
    use accesskit::Role;
    use quark_app::quark_ui::test_alloc::{self, Counting};
    use quark_app::testing::{By, UiTestHarness};

    use super::*;

    #[global_allocator]
    static ALLOCATOR: Counting = Counting;

    const SIZE: (f32, f32) = (640.0, 400.0);

    fn harness(command: Option<PtyCommand>) -> UiTestHarness<Demo> {
        UiTestHarness::new(Demo::new(command), SIZE, 1.0)
    }

    /// The window point at the middle of cell `(col, row)`.
    fn cell(ui: &UiTestHarness<Demo>, col: u16, row: u16) -> (f32, f32) {
        let node = ui.find(By::role(Role::Terminal));
        let (cols, rows) = ui.app().term.size();
        let (w, h) = (
            node.bounds.width / f32::from(cols),
            node.bounds.height / f32::from(rows),
        );
        (
            node.bounds.x + (f32::from(col) + 0.5) * w,
            node.bounds.y + (f32::from(row) + 0.5) * h,
        )
    }

    #[test]
    fn typed_input_reaches_the_program_and_its_output_shows() {
        let (tx, rx) = std::sync::mpsc::channel();
        let command = PtyCommand::new("sh").arg("-c").arg("read x; echo got:$x");
        let mut demo = Demo::new(Some(command));
        demo.on_pty = Some(tx);
        let mut ui = UiTestHarness::new(demo, SIZE, 1.0);
        ui.type_text("hello\n");
        let deadline = std::time::Duration::from_secs(20);
        while !ui.app().term.grid().text().contains("got:hello") {
            rx.recv_timeout(deadline)
                .expect("the program wrote nothing more before the deadline");
            ui.run_until_idle();
        }
        // The terminal echoed the line before the program answered it.
        let text = ui.app().term.grid().text();
        assert_eq!(text.lines().collect::<Vec<_>>(), ["hello", "got:hello"]);
    }

    #[test]
    fn dragging_selects_and_the_copy_shortcut_copies() {
        let mut ui = harness(None);
        ui.app_mut().term.feed(b"first line\r\nsecond line");
        ui.frame();
        ui.drag(cell(&ui, 6, 0), cell(&ui, 5, 1));
        ui.key(if cfg!(target_os = "macos") {
            "mod+c"
        } else {
            "ctrl+shift+c"
        });
        assert_eq!(ui.clipboard_text().as_deref(), Some("line\nsecond"));
    }

    #[test]
    fn screen_readers_get_the_screen_text() {
        let mut ui = harness(None);
        ui.app_mut().term.feed(b"\x1b]2;build\x07$ make\r\nok");
        ui.frame();
        let node = ui.find(By::role(Role::Terminal));
        assert_eq!(node.name.as_deref(), Some("build"));
        let runs: Vec<String> = ui
            .find_all(By::role(Role::TextRun))
            .into_iter()
            .filter_map(|n| n.value)
            .collect();
        assert_eq!(runs.concat(), "$ make\nok");
    }

    /// A screen reader that starts listening while the terminal is idle
    /// gets the screen without new output.
    #[test]
    fn a_screen_reader_attached_later_gets_the_current_screen() {
        let mut ui = harness(None);
        ui.set_accessibility_active(false);
        ui.app_mut().term.feed(b"$ make\r\nok");
        ui.frame();
        ui.set_accessibility_active(true);
        ui.frame();
        let runs: Vec<String> = ui
            .find_all(By::role(Role::TextRun))
            .into_iter()
            .filter_map(|n| n.value)
            .collect();
        assert_eq!(runs.concat(), "$ make\nok");
    }

    /// `count` lines of 79 columns: a green line number, then text unique
    /// to `seed`.
    fn lines(seed: usize, count: usize) -> String {
        (0..count)
            .map(|n| {
                let text = format!("output {seed}.{n} of a build step");
                format!("\x1b[32m{n:04}\x1b[0m {text:<74}\r\n")
            })
            .collect()
    }

    /// With no screen reader, preparing a changed frame allocates nothing
    /// once the rows have held lines as long: the grid updates in place and
    /// no screen text is built.
    #[test]
    fn a_changed_frame_prepares_without_allocating() {
        let cases: &[(&str, String, &str)] = &[
            ("a typed character", "o".into(), "$ echo"),
            (
                "one new line",
                "\r\nfresh output line".into(),
                "fresh output line",
            ),
            (
                "thirty new lines",
                format!("\r\n{}", lines(9, 30)),
                "output 9.29 of a build step",
            ),
        ];
        for (name, input, shown) in cases {
            let mut ui = harness(None);
            ui.set_accessibility_active(false);
            assert_eq!(ui.app().term.size(), (80, 22));
            for seed in 0..3 {
                ui.app_mut().term.feed(lines(seed, 30).as_bytes());
                ui.frame();
            }
            ui.app_mut().term.feed(b"$ ech");
            ui.frame();

            ui.app_mut().term.feed(input.as_bytes());
            ui.frame();
            assert_eq!(ui.app().prepare_allocations, 0, "{name}");
            let painted = ui.painted_text();
            assert!(painted.contains(shown), "{name}: {painted}");
        }
    }

    /// A frame that repeats the last one replays the grid from the element
    /// cache and allocates nothing.
    #[test]
    fn a_repeated_frame_allocates_nothing() {
        let mut ui = harness(None);
        ui.set_accessibility_active(false);
        let mut screen = String::new();
        for i in 0..200 {
            screen.push_str(&format!(
                "\x1b[3{}mline {i}\x1b[0m \x1b[1mbold\x1b[0m \u{6f22}\u{1f600}\r\n",
                i % 8
            ));
        }
        ui.app_mut().term.feed(screen.as_bytes());
        // Follow the output to the bottom, then let the scrollbar the jump
        // showed fade out.
        ui.frame();
        ui.advance(5_000);
        for _ in 0..3 {
            ui.frame();
        }
        let ((), sites) = test_alloc::profile(|| {
            ui.frame();
        });
        assert!(sites.is_empty(), "{sites:#?}");
    }

    /// Output that changes nothing on screen (here, resetting attributes
    /// already reset) leaves the next frame allocation-free.
    #[test]
    fn output_that_changes_nothing_visible_allocates_nothing() {
        let mut ui = harness(None);
        ui.set_accessibility_active(false);
        ui.app_mut().term.feed(b"$ make\r\nok");
        ui.frame();
        ui.advance(5_000);
        ui.frame();
        ui.app_mut().term.feed(b"\x1b[0m");
        let ((), allocations) = test_alloc::count(|| {
            ui.frame();
        });
        assert_eq!(allocations, 0);
        assert_eq!(ui.painted_text(), "$ make\nok");
    }
}
