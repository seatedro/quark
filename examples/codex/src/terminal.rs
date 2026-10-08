//! The side panel's terminal tab: one `quark_terminal` session. Launches
//! run the user's login shell on a real PTY; `--terminal scripted` (what
//! scenes and tests use) starts no process and shows capture 38's
//! transcript instead.

use quark_app::quark_ui::FocusId;
use quark_app::quark_ui::element::*;
use quark_app::quark_ui::style::Styled;
use quark_app::winit::event::{ElementState, MouseScrollDelta};
use quark_app::{InputEvent, KeyKind, UiContext, ViewContext, Waker};
use quark_terminal::{
    KeyPress, PointerInput, PtyCommand, TerminalEnv, TerminalEvent, TerminalOutcome, TerminalState,
    terminal_view,
};

use crate::{Msg, TerminalMode, data};

pub const FOCUS: FocusId = FocusId::from_key("codex.terminal");
const WHEEL_LINE_PX: f32 = 20.0;

pub struct State {
    mode: TerminalMode,
    term: TerminalState,
    started: bool,
    waker: Option<Waker>,
}

impl State {
    pub fn new(mode: TerminalMode) -> Self {
        let mut term = TerminalState::new("codex.terminal", FOCUS);
        if mode == TerminalMode::Scripted {
            term.feed(data::TERMINAL_SCENE.as_bytes());
        }
        Self {
            mode,
            term,
            started: false,
            waker: None,
        }
    }

    pub fn init(&mut self, cx: &mut UiContext) {
        self.waker = Some(cx.window.waker().clone());
    }

    pub fn wake(&mut self, cx: &mut UiContext) {
        if self.term.read_pty() {
            self.term.take_signals();
            cx.window.request_redraw_all();
        }
    }

    /// The terminal sized to `w` x `h`; a real shell starts the first
    /// time it is shown, at that size.
    pub fn view(&mut self, w: f32, h: f32, vcx: &mut ViewContext) -> AnyElement {
        self.term.set_viewport(w, h);
        let scale = vcx.frame.scale_factor();
        let text = vcx.frame.text();
        self.term
            .prepare(&mut text.system, &mut text.layouts, scale, vcx.theme);
        if self.mode == TerminalMode::Real
            && !self.started
            && let Some(waker) = self.waker.clone()
        {
            self.started = true;
            let command = match std::env::current_dir() {
                Ok(dir) => PtyCommand::shell().cwd(dir),
                Err(_) => PtyCommand::shell(),
            };
            if let Err(e) = self.term.spawn(&command, move || waker.wake()) {
                self.term
                    .feed(format!("Could not start the shell: {e}\r\n").as_bytes());
            }
        }
        let env = TerminalEnv {
            focused: vcx.is_focused(FOCUS),
            accessible: vcx.frame.accessibility_active(),
        };
        let view = terminal_view(&mut self.term, vcx.theme, env, |e| Msg::Terminal(e).into());
        div().w(w).h(h).child(view).into_any()
    }

    pub fn handle(&mut self, event: TerminalEvent, cx: &mut UiContext) {
        let now = cx.window.elapsed().as_millis() as u64;
        let outcome = self.term.handle(event, now);
        self.apply(outcome, cx);
    }

    fn apply(&mut self, outcome: TerminalOutcome, cx: &mut UiContext) {
        match outcome {
            TerminalOutcome::Copy(text) => cx.window.set_clipboard_text(&text),
            TerminalOutcome::Paste => {
                if let Some(text) = cx.window.clipboard_text() {
                    let _ = self.term.paste(&text, true);
                }
            }
            _ => {}
        }
    }

    /// Raw input while the terminal tab shows. True when it took it.
    pub fn input(&mut self, event: &InputEvent, cx: &mut UiContext) -> bool {
        let focused = cx.focus() == Some(FOCUS);
        let consumed = match event {
            InputEvent::ModifiersChanged(m) => {
                self.term.set_modifiers(*m);
                false
            }
            InputEvent::Focused(f) => {
                self.term.focus_changed(*f && focused);
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
                true
            }
            InputEvent::PointerMoved { x, y } => {
                self.term.pointer(PointerInput::Moved { x: *x, y: *y })
            }
            InputEvent::PointerButton { button, state } if focused => {
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
