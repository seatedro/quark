//! Keyboard and pointer input from winit's types (what quark-app's
//! `InputEvent`s carry) into libghostty-vt's key and mouse encoders.

use winit::keyboard::{KeyCode, ModifiersState, NamedKey};

use crate::sys;
use crate::vt::{Key, Mods};

/// Defines the US-layout tables: physical codes, named keys, and the
/// characters keys type without Shift.
macro_rules! keys {
    ($($code:ident => $key:ident),* $(,)?) => {
        /// The Ghostty key for a physical key.
        pub fn key_from_code(code: KeyCode) -> Option<Key> {
            Some(Key(match code {
                $(KeyCode::$code => sys::$key,)*
                _ => return None,
            }))
        }
    };
}

keys! {
    KeyA => GHOSTTY_KEY_A, KeyB => GHOSTTY_KEY_B, KeyC => GHOSTTY_KEY_C,
    KeyD => GHOSTTY_KEY_D, KeyE => GHOSTTY_KEY_E, KeyF => GHOSTTY_KEY_F,
    KeyG => GHOSTTY_KEY_G, KeyH => GHOSTTY_KEY_H, KeyI => GHOSTTY_KEY_I,
    KeyJ => GHOSTTY_KEY_J, KeyK => GHOSTTY_KEY_K, KeyL => GHOSTTY_KEY_L,
    KeyM => GHOSTTY_KEY_M, KeyN => GHOSTTY_KEY_N, KeyO => GHOSTTY_KEY_O,
    KeyP => GHOSTTY_KEY_P, KeyQ => GHOSTTY_KEY_Q, KeyR => GHOSTTY_KEY_R,
    KeyS => GHOSTTY_KEY_S, KeyT => GHOSTTY_KEY_T, KeyU => GHOSTTY_KEY_U,
    KeyV => GHOSTTY_KEY_V, KeyW => GHOSTTY_KEY_W, KeyX => GHOSTTY_KEY_X,
    KeyY => GHOSTTY_KEY_Y, KeyZ => GHOSTTY_KEY_Z,
    Digit0 => GHOSTTY_KEY_DIGIT_0, Digit1 => GHOSTTY_KEY_DIGIT_1,
    Digit2 => GHOSTTY_KEY_DIGIT_2, Digit3 => GHOSTTY_KEY_DIGIT_3,
    Digit4 => GHOSTTY_KEY_DIGIT_4, Digit5 => GHOSTTY_KEY_DIGIT_5,
    Digit6 => GHOSTTY_KEY_DIGIT_6, Digit7 => GHOSTTY_KEY_DIGIT_7,
    Digit8 => GHOSTTY_KEY_DIGIT_8, Digit9 => GHOSTTY_KEY_DIGIT_9,
    Backquote => GHOSTTY_KEY_BACKQUOTE, Backslash => GHOSTTY_KEY_BACKSLASH,
    BracketLeft => GHOSTTY_KEY_BRACKET_LEFT, BracketRight => GHOSTTY_KEY_BRACKET_RIGHT,
    Comma => GHOSTTY_KEY_COMMA, Equal => GHOSTTY_KEY_EQUAL, Minus => GHOSTTY_KEY_MINUS,
    Period => GHOSTTY_KEY_PERIOD, Quote => GHOSTTY_KEY_QUOTE,
    Semicolon => GHOSTTY_KEY_SEMICOLON, Slash => GHOSTTY_KEY_SLASH,
    IntlBackslash => GHOSTTY_KEY_INTL_BACKSLASH, IntlRo => GHOSTTY_KEY_INTL_RO,
    IntlYen => GHOSTTY_KEY_INTL_YEN,
    AltLeft => GHOSTTY_KEY_ALT_LEFT, AltRight => GHOSTTY_KEY_ALT_RIGHT,
    ControlLeft => GHOSTTY_KEY_CONTROL_LEFT, ControlRight => GHOSTTY_KEY_CONTROL_RIGHT,
    ShiftLeft => GHOSTTY_KEY_SHIFT_LEFT, ShiftRight => GHOSTTY_KEY_SHIFT_RIGHT,
    SuperLeft => GHOSTTY_KEY_META_LEFT, SuperRight => GHOSTTY_KEY_META_RIGHT,
    Backspace => GHOSTTY_KEY_BACKSPACE, CapsLock => GHOSTTY_KEY_CAPS_LOCK,
    ContextMenu => GHOSTTY_KEY_CONTEXT_MENU, Enter => GHOSTTY_KEY_ENTER,
    Space => GHOSTTY_KEY_SPACE, Tab => GHOSTTY_KEY_TAB, Escape => GHOSTTY_KEY_ESCAPE,
    Delete => GHOSTTY_KEY_DELETE, End => GHOSTTY_KEY_END, Home => GHOSTTY_KEY_HOME,
    Help => GHOSTTY_KEY_HELP, Insert => GHOSTTY_KEY_INSERT,
    PageDown => GHOSTTY_KEY_PAGE_DOWN, PageUp => GHOSTTY_KEY_PAGE_UP,
    ArrowDown => GHOSTTY_KEY_ARROW_DOWN, ArrowLeft => GHOSTTY_KEY_ARROW_LEFT,
    ArrowRight => GHOSTTY_KEY_ARROW_RIGHT, ArrowUp => GHOSTTY_KEY_ARROW_UP,
    NumLock => GHOSTTY_KEY_NUM_LOCK,
    Numpad0 => GHOSTTY_KEY_NUMPAD_0, Numpad1 => GHOSTTY_KEY_NUMPAD_1,
    Numpad2 => GHOSTTY_KEY_NUMPAD_2, Numpad3 => GHOSTTY_KEY_NUMPAD_3,
    Numpad4 => GHOSTTY_KEY_NUMPAD_4, Numpad5 => GHOSTTY_KEY_NUMPAD_5,
    Numpad6 => GHOSTTY_KEY_NUMPAD_6, Numpad7 => GHOSTTY_KEY_NUMPAD_7,
    Numpad8 => GHOSTTY_KEY_NUMPAD_8, Numpad9 => GHOSTTY_KEY_NUMPAD_9,
    NumpadAdd => GHOSTTY_KEY_NUMPAD_ADD, NumpadSubtract => GHOSTTY_KEY_NUMPAD_SUBTRACT,
    NumpadMultiply => GHOSTTY_KEY_NUMPAD_MULTIPLY, NumpadDivide => GHOSTTY_KEY_NUMPAD_DIVIDE,
    NumpadDecimal => GHOSTTY_KEY_NUMPAD_DECIMAL, NumpadEnter => GHOSTTY_KEY_NUMPAD_ENTER,
    NumpadEqual => GHOSTTY_KEY_NUMPAD_EQUAL, NumpadComma => GHOSTTY_KEY_NUMPAD_COMMA,
    F1 => GHOSTTY_KEY_F1, F2 => GHOSTTY_KEY_F2, F3 => GHOSTTY_KEY_F3, F4 => GHOSTTY_KEY_F4,
    F5 => GHOSTTY_KEY_F5, F6 => GHOSTTY_KEY_F6, F7 => GHOSTTY_KEY_F7, F8 => GHOSTTY_KEY_F8,
    F9 => GHOSTTY_KEY_F9, F10 => GHOSTTY_KEY_F10, F11 => GHOSTTY_KEY_F11,
    F12 => GHOSTTY_KEY_F12, F13 => GHOSTTY_KEY_F13, F14 => GHOSTTY_KEY_F14,
    F15 => GHOSTTY_KEY_F15, F16 => GHOSTTY_KEY_F16, F17 => GHOSTTY_KEY_F17,
    F18 => GHOSTTY_KEY_F18, F19 => GHOSTTY_KEY_F19, F20 => GHOSTTY_KEY_F20,
    F21 => GHOSTTY_KEY_F21, F22 => GHOSTTY_KEY_F22, F23 => GHOSTTY_KEY_F23,
    F24 => GHOSTTY_KEY_F24, F25 => GHOSTTY_KEY_F25,
    PrintScreen => GHOSTTY_KEY_PRINT_SCREEN, ScrollLock => GHOSTTY_KEY_SCROLL_LOCK,
    Pause => GHOSTTY_KEY_PAUSE,
}

/// The Ghostty key for a named (non-character) key.
pub fn key_from_named(named: NamedKey) -> Option<Key> {
    let code = match named {
        NamedKey::Enter => KeyCode::Enter,
        NamedKey::Tab => KeyCode::Tab,
        NamedKey::Space => KeyCode::Space,
        NamedKey::Backspace => KeyCode::Backspace,
        NamedKey::Escape => KeyCode::Escape,
        NamedKey::Delete => KeyCode::Delete,
        NamedKey::Insert => KeyCode::Insert,
        NamedKey::Home => KeyCode::Home,
        NamedKey::End => KeyCode::End,
        NamedKey::PageUp => KeyCode::PageUp,
        NamedKey::PageDown => KeyCode::PageDown,
        NamedKey::ArrowUp => KeyCode::ArrowUp,
        NamedKey::ArrowDown => KeyCode::ArrowDown,
        NamedKey::ArrowLeft => KeyCode::ArrowLeft,
        NamedKey::ArrowRight => KeyCode::ArrowRight,
        NamedKey::ContextMenu => KeyCode::ContextMenu,
        NamedKey::Help => KeyCode::Help,
        NamedKey::Pause => KeyCode::Pause,
        NamedKey::PrintScreen => KeyCode::PrintScreen,
        NamedKey::ScrollLock => KeyCode::ScrollLock,
        NamedKey::NumLock => KeyCode::NumLock,
        NamedKey::CapsLock => KeyCode::CapsLock,
        NamedKey::Shift => KeyCode::ShiftLeft,
        NamedKey::Control => KeyCode::ControlLeft,
        NamedKey::Alt => KeyCode::AltLeft,
        NamedKey::Super | NamedKey::Meta => KeyCode::SuperLeft,
        NamedKey::F1 => KeyCode::F1,
        NamedKey::F2 => KeyCode::F2,
        NamedKey::F3 => KeyCode::F3,
        NamedKey::F4 => KeyCode::F4,
        NamedKey::F5 => KeyCode::F5,
        NamedKey::F6 => KeyCode::F6,
        NamedKey::F7 => KeyCode::F7,
        NamedKey::F8 => KeyCode::F8,
        NamedKey::F9 => KeyCode::F9,
        NamedKey::F10 => KeyCode::F10,
        NamedKey::F11 => KeyCode::F11,
        NamedKey::F12 => KeyCode::F12,
        _ => return None,
    };
    key_from_code(code)
}

/// The US key that types `c`, and the character it types without Shift.
pub fn key_from_char(c: char) -> Option<(Key, char)> {
    const SHIFTED: &str = "~!@#$%^&*()_+{}|:\"<>?";
    const PLAIN: &str = "`1234567890-=[]\\;',./";
    let unshifted = match SHIFTED.find(c) {
        Some(i) => PLAIN.as_bytes()[i] as char,
        None => c.to_ascii_lowercase(),
    };
    let code = match unshifted {
        'a'..='z' => {
            const LETTERS: [KeyCode; 26] = [
                KeyCode::KeyA,
                KeyCode::KeyB,
                KeyCode::KeyC,
                KeyCode::KeyD,
                KeyCode::KeyE,
                KeyCode::KeyF,
                KeyCode::KeyG,
                KeyCode::KeyH,
                KeyCode::KeyI,
                KeyCode::KeyJ,
                KeyCode::KeyK,
                KeyCode::KeyL,
                KeyCode::KeyM,
                KeyCode::KeyN,
                KeyCode::KeyO,
                KeyCode::KeyP,
                KeyCode::KeyQ,
                KeyCode::KeyR,
                KeyCode::KeyS,
                KeyCode::KeyT,
                KeyCode::KeyU,
                KeyCode::KeyV,
                KeyCode::KeyW,
                KeyCode::KeyX,
                KeyCode::KeyY,
                KeyCode::KeyZ,
            ];
            LETTERS[(unshifted as u8 - b'a') as usize]
        }
        '0'..='9' => {
            const DIGITS: [KeyCode; 10] = [
                KeyCode::Digit0,
                KeyCode::Digit1,
                KeyCode::Digit2,
                KeyCode::Digit3,
                KeyCode::Digit4,
                KeyCode::Digit5,
                KeyCode::Digit6,
                KeyCode::Digit7,
                KeyCode::Digit8,
                KeyCode::Digit9,
            ];
            DIGITS[(unshifted as u8 - b'0') as usize]
        }
        '`' => KeyCode::Backquote,
        '-' => KeyCode::Minus,
        '=' => KeyCode::Equal,
        '[' => KeyCode::BracketLeft,
        ']' => KeyCode::BracketRight,
        '\\' => KeyCode::Backslash,
        ';' => KeyCode::Semicolon,
        '\'' => KeyCode::Quote,
        ',' => KeyCode::Comma,
        '.' => KeyCode::Period,
        '/' => KeyCode::Slash,
        ' ' => KeyCode::Space,
        _ => return None,
    };
    Some((key_from_code(code)?, unshifted))
}

/// Ghostty modifier bits for winit's modifier state.
pub fn mods(state: ModifiersState) -> Mods {
    let mut mods = Mods::default();
    for (on, bit) in [
        (state.shift_key(), Mods::SHIFT),
        (state.control_key(), Mods::CTRL),
        (state.alt_key(), Mods::ALT),
        (state.super_key(), Mods::SUPER),
    ] {
        if on {
            mods = mods | bit;
        }
    }
    mods
}

/// What a key press carries, as winit reports it: the logical key (named
/// or the text it types, with Shift applied), the physical key, and the
/// modifiers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KeyPress<'a> {
    pub named: Option<NamedKey>,
    pub text: Option<&'a str>,
    pub physical: Option<KeyCode>,
    pub modifiers: ModifiersState,
    pub repeat: bool,
}

impl KeyPress<'_> {
    /// The Ghostty key and unshifted character: from the typed character
    /// when it is on a US layout (so Ctrl+C is C on any layout that types
    /// a c), else the named key, else the physical key.
    pub fn key(&self) -> Option<(Key, Option<char>)> {
        if let Some(named) = self.named {
            return key_from_named(named).map(|k| (k, None));
        }
        if self.is_linefeed() {
            return key_from_code(KeyCode::KeyJ).map(|k| (k, Some('j')));
        }
        let mut chars = self.text.unwrap_or("").chars();
        if let (Some(c), None) = (chars.next(), chars.next())
            && let Some((key, unshifted)) = key_from_char(c)
        {
            return Some((key, Some(unshifted)));
        }
        let key = key_from_code(self.physical?)?;
        let unshifted = self.text.and_then(|t| t.chars().next());
        Some((key, unshifted.map(|c| c.to_lowercase().next().unwrap_or(c))))
    }

    /// Text the key types, for the encoder: named keys type none except
    /// Space, and neither does Linefeed.
    pub fn typed(&self) -> Option<&str> {
        match self.named {
            Some(NamedKey::Space) => Some(" "),
            Some(_) => None,
            None if self.is_linefeed() => None,
            None => self.text,
        }
    }

    /// Modifiers for the encoder. Linefeed (X11's keysym, which types
    /// "\n") encodes as Ctrl+J, so it sends LF as xterm does.
    pub fn mods(&self) -> Mods {
        let mods = mods(self.modifiers);
        if self.is_linefeed() {
            mods | Mods::CTRL
        } else {
            mods
        }
    }

    fn is_linefeed(&self) -> bool {
        self.named.is_none() && self.text == Some("\n")
    }
}
