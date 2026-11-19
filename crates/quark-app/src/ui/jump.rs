//! Jump mode: short home row labels on everything that clicks or takes
//! text, typed to focus and click it (see [`JumpHints`]).

use quark_ui::element::{JumpChoice, JumpHints, JumpKey};

use super::{Redraw, UiAdapter, UiApp, UiContext, redraw};
use crate::EventContext;
use crate::input::UiInput;

/// A window's jump mode.
#[derive(Default)]
pub(super) struct JumpState {
    pub(super) hints: JumpHints,
    /// The last input was a key press jump mode took, so the text it
    /// types, which comes next, goes nowhere.
    took_key: bool,
}

impl UiContext<'_, '_> {
    /// Start jump mode in this window: every enabled element on screen
    /// that clicks or takes text gets a short home row label, drawn above
    /// everything (see [`quark_ui::element::JumpHints`]). Typing a label
    /// focuses its element and clicks it, as a click from assistive tech
    /// does; with Shift held it only takes focus, as a text field always
    /// does. Backspace takes a key back; Escape, any other key, a press,
    /// or wheel motion ends the mode. Labels the last painted frame, so
    /// start it from input such as a key binding. Returns how many
    /// elements got a label; with none, the mode does not start.
    pub fn start_jump_hints(&mut self) -> usize {
        let win = &mut *self.windows.current;
        let count = win.jump.hints.start(win.router.frame());
        if count > 0 {
            redraw(Redraw::Jump, self.windows.handle, self.window);
        }
        count
    }

    /// Whether jump mode ([`Self::start_jump_hints`]) is on in this window.
    pub fn jump_hints_active(&self) -> bool {
        self.windows.current.jump.hints.is_active()
    }
}

impl<U: UiApp> UiAdapter<U> {
    /// Jump mode's share of `input`: while it is on, keys and text go to
    /// it, and a press, wheel motion, or the window losing focus ends it
    /// before going on as usual. True when the input stops here.
    pub(super) fn jump_input(&mut self, input: Option<&UiInput>, cx: &mut EventContext) -> bool {
        let jump = &mut self.win.jump;
        let took_key = std::mem::take(&mut jump.took_key);
        match input {
            Some(UiInput::Key(binding)) => {
                if !jump.hints.is_active() {
                    return false;
                }
                jump.took_key = true;
                let outcome = jump.hints.key(binding);
                redraw(Redraw::Jump, self.win_handle, cx);
                if let JumpKey::Chosen(choice) = outcome {
                    self.jump_to(choice, cx);
                }
                true
            }
            Some(UiInput::Text(_) | UiInput::ImeCommit(_) | UiInput::Preedit { .. }) => {
                jump.hints.is_active() || took_key
            }
            Some(UiInput::PointerDown(_) | UiInput::Wheel { .. } | UiInput::WindowFocus(false)) => {
                if jump.hints.is_active() {
                    jump.hints.cancel();
                    redraw(Redraw::Jump, self.win_handle, cx);
                }
                false
            }
            _ => false,
        }
    }

    /// Focus the element a completed jump label names, then click it, as a
    /// click from assistive tech does.
    fn jump_to(&mut self, choice: JumpChoice, cx: &mut EventContext) {
        self.show_focus(cx);
        if let Some(focus) = choice.focus {
            self.focus_on(focus, cx);
        }
        if choice.click {
            let delivery = self.win.router.click_node(choice.node);
            self.deliver(delivery, cx);
        }
    }
}
