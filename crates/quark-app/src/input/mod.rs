//! Platform-neutral input events and the winit normalization that produces
//! them. Routing events to widgets is left to the app or a UI layer.
//!
//! Window changes that are not input (moves, resizes, scale changes,
//! activation tokens) reach the app as [`crate::AppEvent`]s instead.

mod scroll;
#[cfg(feature = "ui")]
mod ui_input;

use std::path::PathBuf;

use winit::dpi::PhysicalPosition;
use winit::event::{
    ElementState, Ime, KeyEvent, MouseButton, MouseScrollDelta, TouchPhase, WindowEvent,
};
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey};

pub use scroll::scroll_delta_to_px;
#[cfg(feature = "ui")]
pub use ui_input::{PointerButton, UiInput};

/// Pointer coordinates and wheel pixel deltas are logical points relative to
/// the window's content area, matching the coordinate space of the `Scene`
/// the app draws. [`InputNormalizer`] converts winit's physical values.
#[derive(Debug, Clone, PartialEq)]
pub enum InputEvent {
    /// Text a key press typed.
    TextInput(String),
    KeyPress(KeyChord),
    KeyRelease(KeyChord),
    ModifiersChanged(ModifiersState),
    PointerMoved {
        x: f32,
        y: f32,
    },
    /// The pointer came over the window's content area. Its position
    /// arrives with the [`Self::PointerMoved`] that follows.
    PointerEntered,
    PointerLeft,
    PointerButton {
        button: MouseButton,
        state: ElementState,
    },
    Wheel {
        delta: MouseScrollDelta,
        phase: TouchPhase,
    },
    /// The window became the active window (gained keyboard focus), or
    /// stopped being it.
    Focused(bool),
    /// The IME composition changed: its text, with the caret or selection
    /// at a byte range. Empty text ends it.
    ImePreedit(String, Option<(usize, usize)>),
    /// The IME committed text, ending its composition. Kept apart from
    /// [`Self::TextInput`] so a commit can be told from typing: one that
    /// belongs to an element focus has left is dropped, not inserted in
    /// the element focused now.
    ImeCommit(String),
    FileHovered(PathBuf),
    FileHoverCancelled,
    FileDropped(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyKind {
    Named(NamedKey),
    Character(String),
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyChord {
    pub logical: KeyKind,
    pub physical: Option<KeyCode>,
    pub modifiers: ModifiersState,
    pub repeat: bool,
}

impl KeyChord {
    pub fn from_key_event(event: &KeyEvent, modifiers: ModifiersState) -> Self {
        let logical = match &event.logical_key {
            Key::Named(named) => KeyKind::Named(*named),
            Key::Character(text) => KeyKind::Character(text.to_string()),
            // A key winit leaves unnamed that types one control character
            // keeps it: X11's Linefeed types "\n", which terminals send.
            Key::Unidentified(_) => match event.text.as_deref() {
                Some(text) if text.chars().count() == 1 && text.chars().all(char::is_control) => {
                    KeyKind::Character(text.to_owned())
                }
                _ => KeyKind::Other,
            },
            _ => KeyKind::Other,
        };
        let physical = match event.physical_key {
            PhysicalKey::Code(code) => Some(code),
            PhysicalKey::Unidentified(_) => None,
        };
        Self {
            logical,
            physical,
            modifiers,
            repeat: event.repeat,
        }
    }

    pub fn ctrl_or_super(&self) -> bool {
        self.modifiers.super_key() || self.modifiers.control_key()
    }

    pub fn shift(&self) -> bool {
        self.modifiers.shift_key()
    }

    pub fn alt(&self) -> bool {
        self.modifiers.alt_key()
    }

    pub fn logical_char(&self) -> Option<&str> {
        match &self.logical {
            KeyKind::Character(text) => Some(text.as_str()),
            _ => None,
        }
    }

    pub fn named(&self) -> Option<NamedKey> {
        match &self.logical {
            KeyKind::Named(named) => Some(*named),
            _ => None,
        }
    }

    pub fn binding_string(&self) -> Option<String> {
        let (key, inferred_shift) = binding_key(&self.logical)?;

        let mut parts = Vec::new();
        if self.modifiers.super_key() {
            parts.push("cmd");
        }
        if self.modifiers.control_key() {
            parts.push("ctrl");
        }
        if self.modifiers.alt_key() {
            parts.push("alt");
        }
        if self.modifiers.shift_key() || inferred_shift {
            parts.push("shift");
        }
        parts.push(key);
        Some(parts.join("+"))
    }
}

/// The key name a binding uses for `logical`, and whether the character
/// implies Shift (`?` is Shift with `/`).
fn binding_key(logical: &KeyKind) -> Option<(&'static str, bool)> {
    match logical {
        KeyKind::Character(text) => character_binding_key(text),
        KeyKind::Named(named) => named_binding_key(*named),
        KeyKind::Other => None,
    }
}

fn character_binding_key(text: &str) -> Option<(&'static str, bool)> {
    if text.chars().count() != 1 {
        return None;
    }
    let ch = text.chars().next()?;
    Some(match ch {
        ' ' => ("space", false),
        'A'..='Z' => (lower_ascii_key(ch), true),
        'a'..='z' | '0'..='9' => (lower_ascii_key(ch), false),
        '}' => ("]", true),
        '{' => ("[", true),
        '+' => ("=", true),
        '_' => ("-", true),
        '?' => ("/", true),
        ')' => ("0", true),
        '!' => ("1", true),
        '@' => ("2", true),
        '#' => ("3", true),
        '$' => ("4", true),
        '%' => ("5", true),
        '^' => ("6", true),
        '&' => ("7", true),
        '*' => ("8", true),
        '(' => ("9", true),
        ',' => (",", false),
        '.' => (".", false),
        '/' => ("/", false),
        ';' => (";", false),
        '\'' => ("'", false),
        '[' => ("[", false),
        ']' => ("]", false),
        '\\' => ("\\", false),
        '-' => ("-", false),
        '=' => ("=", false),
        '`' => ("`", false),
        _ => return None,
    })
}

fn lower_ascii_key(ch: char) -> &'static str {
    match ch.to_ascii_lowercase() {
        'a' => "a",
        'b' => "b",
        'c' => "c",
        'd' => "d",
        'e' => "e",
        'f' => "f",
        'g' => "g",
        'h' => "h",
        'i' => "i",
        'j' => "j",
        'k' => "k",
        'l' => "l",
        'm' => "m",
        'n' => "n",
        'o' => "o",
        'p' => "p",
        'q' => "q",
        'r' => "r",
        's' => "s",
        't' => "t",
        'u' => "u",
        'v' => "v",
        'w' => "w",
        'x' => "x",
        'y' => "y",
        'z' => "z",
        '0' => "0",
        '1' => "1",
        '2' => "2",
        '3' => "3",
        '4' => "4",
        '5' => "5",
        '6' => "6",
        '7' => "7",
        '8' => "8",
        '9' => "9",
        _ => unreachable!("caller only passes ascii alphanumeric keys"),
    }
}

fn named_binding_key(named: NamedKey) -> Option<(&'static str, bool)> {
    Some(match named {
        NamedKey::Enter => ("enter", false),
        NamedKey::Tab => ("tab", false),
        NamedKey::Escape => ("escape", false),
        NamedKey::Space => ("space", false),
        NamedKey::ArrowUp => ("arrowup", false),
        NamedKey::ArrowDown => ("arrowdown", false),
        NamedKey::ArrowLeft => ("arrowleft", false),
        NamedKey::ArrowRight => ("arrowright", false),
        NamedKey::PageDown => ("pagedown", false),
        NamedKey::PageUp => ("pageup", false),
        NamedKey::Home => ("home", false),
        NamedKey::End => ("end", false),
        NamedKey::Backspace => ("backspace", false),
        NamedKey::Delete => ("delete", false),
        NamedKey::F1 => ("f1", false),
        NamedKey::F2 => ("f2", false),
        NamedKey::F3 => ("f3", false),
        NamedKey::F4 => ("f4", false),
        NamedKey::F5 => ("f5", false),
        NamedKey::F6 => ("f6", false),
        NamedKey::F7 => ("f7", false),
        NamedKey::F8 => ("f8", false),
        NamedKey::F9 => ("f9", false),
        NamedKey::F10 => ("f10", false),
        NamedKey::F11 => ("f11", false),
        NamedKey::F12 => ("f12", false),
        _ => return None,
    })
}

/// Turns raw winit window events into `InputEvent`s, tracking the modifier,
/// pointer, and IME composition state that individual events don't carry.
#[derive(Debug)]
pub struct InputNormalizer {
    modifiers: ModifiersState,
    pointer_position: Option<(f32, f32)>,
    ime_composing: bool,
    scale_factor: f64,
}

impl Default for InputNormalizer {
    fn default() -> Self {
        Self::new(1.0)
    }
}

impl InputNormalizer {
    /// A normalizer for a window with `scale_factor` physical pixels per
    /// logical point.
    pub fn new(scale_factor: f64) -> Self {
        Self {
            modifiers: ModifiersState::empty(),
            pointer_position: None,
            ime_composing: false,
            scale_factor,
        }
    }

    /// The window moved to a display with another scale factor. The last
    /// pointer position is kept in logical points at the new scale.
    pub fn set_scale_factor(&mut self, scale_factor: f64) {
        if let Some((x, y)) = &mut self.pointer_position {
            let ratio = (self.scale_factor / scale_factor) as f32;
            *x *= ratio;
            *y *= ratio;
        }
        self.scale_factor = scale_factor;
    }

    pub fn modifiers(&self) -> ModifiersState {
        self.modifiers
    }

    /// Last pointer position in logical points.
    pub fn pointer_position(&self) -> Option<(f32, f32)> {
        self.pointer_position
    }

    pub fn ime_composing(&self) -> bool {
        self.ime_composing
    }

    pub fn normalize(&mut self, event: WindowEvent) -> Vec<InputEvent> {
        match event {
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers.state();
                vec![InputEvent::ModifiersChanged(self.modifiers)]
            }
            WindowEvent::Focused(focused) => {
                if !focused {
                    self.ime_composing = false;
                }
                vec![InputEvent::Focused(focused)]
            }
            WindowEvent::CursorMoved { position, .. } => {
                let position = position.to_logical::<f32>(self.scale_factor);
                let (x, y) = (position.x, position.y);
                self.pointer_position = Some((x, y));
                vec![InputEvent::PointerMoved { x, y }]
            }
            WindowEvent::CursorEntered { .. } => vec![InputEvent::PointerEntered],
            WindowEvent::CursorLeft { .. } => {
                self.pointer_position = None;
                vec![InputEvent::PointerLeft]
            }
            WindowEvent::MouseWheel { delta, phase, .. } => {
                let delta = match delta {
                    // The type says physical, but the value is now points,
                    // like every other position an app sees.
                    MouseScrollDelta::PixelDelta(pixels) => {
                        let points = pixels.to_logical::<f64>(self.scale_factor);
                        MouseScrollDelta::PixelDelta(PhysicalPosition::new(points.x, points.y))
                    }
                    lines => lines,
                };
                vec![InputEvent::Wheel { delta, phase }]
            }
            WindowEvent::MouseInput { state, button, .. } => {
                vec![InputEvent::PointerButton { button, state }]
            }
            WindowEvent::KeyboardInput {
                event,
                is_synthetic,
                ..
            } => {
                if is_synthetic {
                    return Vec::new();
                }
                self.normalize_keyboard_event(event)
            }
            WindowEvent::Ime(ime) => self.normalize_ime_event(ime),
            WindowEvent::HoveredFile(path) => vec![InputEvent::FileHovered(path)],
            WindowEvent::HoveredFileCancelled => vec![InputEvent::FileHoverCancelled],
            WindowEvent::DroppedFile(path) => vec![InputEvent::FileDropped(path)],
            _ => Vec::new(),
        }
    }

    fn normalize_keyboard_event(&mut self, event: KeyEvent) -> Vec<InputEvent> {
        let chord = KeyChord::from_key_event(&event, self.modifiers);
        let mut events = Vec::with_capacity(2);
        match event.state {
            ElementState::Pressed => {
                events.push(InputEvent::KeyPress(chord));
                if let Some(text) =
                    key_text(event.text.as_deref(), self.modifiers, self.ime_composing)
                {
                    events.push(InputEvent::TextInput(text));
                }
            }
            ElementState::Released => events.push(InputEvent::KeyRelease(chord)),
        }
        events
    }

    fn normalize_ime_event(&mut self, ime: Ime) -> Vec<InputEvent> {
        match ime {
            Ime::Enabled => Vec::new(),
            Ime::Preedit(text, cursor) => {
                self.ime_composing = !text.is_empty();
                vec![InputEvent::ImePreedit(text, cursor)]
            }
            Ime::Commit(text) => {
                self.ime_composing = false;
                vec![InputEvent::ImeCommit(text)]
            }
            // Turning IME off mid-composition drops the preedit; say so,
            // or the field keeps painting it.
            Ime::Disabled if std::mem::take(&mut self.ime_composing) => {
                vec![InputEvent::ImePreedit(String::new(), None)]
            }
            Ime::Disabled => Vec::new(),
        }
    }
}

/// Text a key press should insert. Shortcut chords and keys pressed mid-IME
/// composition produce none: the IME delivers its own commit.
fn key_text(text: Option<&str>, modifiers: ModifiersState, ime_composing: bool) -> Option<String> {
    if ime_composing || modifiers.control_key() || modifiers.super_key() {
        return None;
    }
    let text = text?;
    if text.is_empty() || text.chars().all(char::is_control) {
        return None;
    }
    Some(text.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chord(logical: KeyKind, modifiers: ModifiersState) -> KeyChord {
        KeyChord {
            logical,
            physical: None,
            modifiers,
            repeat: false,
        }
    }

    #[test]
    fn binding_string_infers_shift_from_shifted_characters() {
        let question = chord(KeyKind::Character("?".into()), ModifiersState::SHIFT);
        assert_eq!(question.binding_string().as_deref(), Some("shift+/"));

        let upper = chord(KeyKind::Character("N".into()), ModifiersState::empty());
        assert_eq!(upper.binding_string().as_deref(), Some("shift+n"));

        let save = chord(
            KeyKind::Character("s".into()),
            ModifiersState::SUPER | ModifiersState::CONTROL,
        );
        assert_eq!(save.binding_string().as_deref(), Some("cmd+ctrl+s"));

        let escape = chord(KeyKind::Named(NamedKey::Escape), ModifiersState::empty());
        assert_eq!(escape.binding_string().as_deref(), Some("escape"));
    }

    #[test]
    fn key_text_skips_shortcuts_composition_and_control_chars() {
        assert_eq!(
            key_text(Some("a"), ModifiersState::empty(), false).as_deref(),
            Some("a")
        );
        assert_eq!(key_text(Some("a"), ModifiersState::CONTROL, false), None);
        assert_eq!(key_text(Some("a"), ModifiersState::empty(), true), None);
        assert_eq!(
            key_text(Some("\u{8}"), ModifiersState::empty(), false),
            None
        );
    }

    // Hover tracking across windows needs to know when the pointer comes
    // over a window, not only when it leaves.
    #[test]
    fn cursor_entry_is_kept_as_pointer_entered() {
        let mut input = InputNormalizer::default();
        let device_id = winit::event::DeviceId::dummy();

        let entered = input.normalize(WindowEvent::CursorEntered { device_id });

        assert_eq!(entered, vec![InputEvent::PointerEntered]);
    }

    #[test]
    fn ime_commit_ends_composition_and_stays_a_commit() {
        let mut input = InputNormalizer::default();
        let preedit = input.normalize(WindowEvent::Ime(Ime::Preedit("ni".into(), Some((2, 2)))));
        assert_eq!(
            preedit,
            vec![InputEvent::ImePreedit("ni".into(), Some((2, 2)))]
        );
        assert!(input.ime_composing());

        let commit = input.normalize(WindowEvent::Ime(Ime::Commit("你".into())));
        assert_eq!(commit, vec![InputEvent::ImeCommit("你".into())]);
        assert!(!input.ime_composing());
    }
}
