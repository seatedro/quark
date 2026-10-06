//! Platform-neutral input events and the winit normalization that produces
//! them. Routing events to widgets is left to the app or a UI layer.

mod scroll;

use std::path::PathBuf;

use winit::event::{
    ElementState, Ime, KeyEvent, MouseButton, MouseScrollDelta, TouchPhase, WindowEvent,
};
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey};

pub use scroll::{quantize_scroll_delta_px, scroll_delta_to_px};

/// Pointer coordinates are physical pixels relative to the window's content
/// area, matching the coordinate space of the `Scene` the app draws.
#[derive(Debug, Clone, PartialEq)]
pub enum InputEvent {
    TextInput(String),
    KeyPress(KeyChord),
    KeyRelease(KeyChord),
    ModifiersChanged(ModifiersState),
    PointerMoved {
        x: f32,
        y: f32,
    },
    PointerLeft,
    PointerButton {
        button: MouseButton,
        state: ElementState,
    },
    Wheel {
        delta: MouseScrollDelta,
        phase: TouchPhase,
    },
    Focused(bool),
    ImePreedit(String, Option<(usize, usize)>),
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
        let (key, inferred_shift) = match &self.logical {
            KeyKind::Character(text) => character_binding_key(text)?,
            KeyKind::Named(named) => named_binding_key(*named)?,
            KeyKind::Other => return None,
        };

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
        _ => return None,
    })
}

/// Turns raw winit window events into `InputEvent`s, tracking the modifier,
/// pointer, and IME composition state that individual events don't carry.
#[derive(Debug, Default)]
pub struct InputNormalizer {
    modifiers: ModifiersState,
    pointer_position: Option<(f32, f32)>,
    ime_composing: bool,
}

impl InputNormalizer {
    pub fn modifiers(&self) -> ModifiersState {
        self.modifiers
    }

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
                let (x, y) = (position.x as f32, position.y as f32);
                self.pointer_position = Some((x, y));
                vec![InputEvent::PointerMoved { x, y }]
            }
            WindowEvent::CursorLeft { .. } => {
                self.pointer_position = None;
                vec![InputEvent::PointerLeft]
            }
            WindowEvent::MouseWheel { delta, phase, .. } => {
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
                vec![InputEvent::TextInput(text)]
            }
            Ime::Disabled => {
                self.ime_composing = false;
                Vec::new()
            }
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

    #[test]
    fn ime_commit_ends_composition_and_inserts_text() {
        let mut input = InputNormalizer::default();
        let preedit = input.normalize(WindowEvent::Ime(Ime::Preedit("ni".into(), Some((2, 2)))));
        assert_eq!(
            preedit,
            vec![InputEvent::ImePreedit("ni".into(), Some((2, 2)))]
        );
        assert!(input.ime_composing());

        let commit = input.normalize(WindowEvent::Ime(Ime::Commit("你".into())));
        assert_eq!(commit, vec![InputEvent::TextInput("你".into())]);
        assert!(!input.ime_composing());
    }
}
