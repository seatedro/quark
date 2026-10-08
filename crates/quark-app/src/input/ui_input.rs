//! Input as the element tree sees it: one [`UiInput`] per raw
//! [`InputEvent`], with keys as [`Binding`]s and wheel motion in points, so
//! the adapter and devtools match on these instead of winit's types.

use quark_ui::element::{Binding, Mods, WHEEL_LINE_PX};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, TouchPhase};

use super::{InputEvent, KeyChord, scroll_delta_to_px};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerButton {
    Primary,
    Secondary,
    Middle,
    Other,
}

#[derive(Debug, Clone, PartialEq)]
pub enum UiInput {
    PointerMove {
        x: f32,
        y: f32,
    },
    PointerLeave,
    PointerDown(PointerButton),
    PointerUp(PointerButton),
    /// Wheel motion in points; positive values scroll content down and
    /// right. Line deltas count [`WHEEL_LINE_PX`] per line.
    Wheel {
        dx: f32,
        dy: f32,
        /// The fingers left the trackpad: the gesture's last event.
        ended: bool,
    },
    /// A key press that spells a binding.
    Key(Binding),
    /// Typed text.
    Text(String),
    /// Text the IME committed, ending its composition.
    ImeCommit(String),
    /// The IME composition changed; empty text ends it.
    Preedit {
        text: String,
        cursor: Option<(usize, usize)>,
    },
    /// The window gained or lost keyboard focus.
    WindowFocus(bool),
}

impl UiInput {
    /// The UI input `event` stands for, if any. Key releases, modifier
    /// changes, and file drags have none.
    pub fn from_event(event: &InputEvent) -> Option<Self> {
        Some(match event {
            InputEvent::PointerMoved { x, y } => Self::PointerMove { x: *x, y: *y },
            InputEvent::PointerLeft => Self::PointerLeave,
            InputEvent::PointerButton { button, state } => {
                let button = match button {
                    MouseButton::Left => PointerButton::Primary,
                    MouseButton::Right => PointerButton::Secondary,
                    MouseButton::Middle => PointerButton::Middle,
                    _ => PointerButton::Other,
                };
                match state {
                    ElementState::Pressed => Self::PointerDown(button),
                    ElementState::Released => Self::PointerUp(button),
                }
            }
            InputEvent::Wheel { delta, phase } => {
                let dx = match *delta {
                    MouseScrollDelta::LineDelta(x, _) => -x * WHEEL_LINE_PX,
                    MouseScrollDelta::PixelDelta(position) => -(position.x as f32),
                };
                Self::Wheel {
                    dx,
                    dy: scroll_delta_to_px(*delta, WHEEL_LINE_PX, 1.0),
                    ended: *phase == TouchPhase::Ended,
                }
            }
            InputEvent::KeyPress(chord) => Self::Key(chord.binding()?),
            InputEvent::TextInput(text) => Self::Text(text.clone()),
            InputEvent::ImeCommit(text) => Self::ImeCommit(text.clone()),
            InputEvent::ImePreedit(text, cursor) => Self::Preedit {
                text: text.clone(),
                cursor: *cursor,
            },
            InputEvent::Focused(focused) => Self::WindowFocus(*focused),
            InputEvent::KeyRelease(_)
            | InputEvent::ModifiersChanged(_)
            | InputEvent::FileHovered(_)
            | InputEvent::FileHoverCancelled
            | InputEvent::FileDropped(_) => return None,
        })
    }
}

impl KeyChord {
    /// The binding this press spells, such as `ctrl+shift+n` for Ctrl with
    /// an uppercase N. `None` for keys without a binding name.
    pub fn binding(&self) -> Option<Binding> {
        let (key, inferred_shift) = super::binding_key(&self.logical)?;
        let modifiers = self.modifiers;
        let mods = Mods {
            cmd: modifiers.super_key(),
            ctrl: modifiers.control_key(),
            alt: modifiers.alt_key(),
            shift: modifiers.shift_key() || inferred_shift,
            primary: false,
        };
        Some(Binding::new(mods, key))
    }
}
