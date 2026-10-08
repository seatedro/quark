//! Encodes key presses, typed text, pastes, focus changes, and mouse events
//! under fuzzed keyboard and mouse protocol modes, through quark-terminal's
//! state and libghostty-vt wrapper, and checks the bytes that would go to
//! the program. No PTY or process is involved.
//!
//! Input: four header bytes that set the modes the program asked for, then
//! events, each a tag byte and its fields (see `Event::parse`). Header:
//!
//! - byte 0, one bit each: cursor keys (DECCKM), application keypad,
//!   bracketed paste, focus reports, alternate screen, alternate scroll,
//!   modifyOtherKeys 2
//! - byte 1: Kitty keyboard flags (low five bits), pushed when nonzero
//! - byte 2: mouse tracking (bits 0-1: off, normal, button, any motion) and
//!   format (bits 2-3: X10, UTF-8, SGR, SGR pixels)
//! - byte 3: the mouse grid's columns, `1 + b % 240`
//!
//! Under ASan, RSS grows by tens of KiB per run once a terminal has used
//! the alternate screen (a plain build stays flat, so this is ASan's
//! allocator holding libghostty-vt's buffers), which reaches libFuzzer's
//! 2 GB limit within a minute: give campaigns `-rss_limit_mb=8192` or
//! build with `-s none`. ASan and coverage see only the Rust side; the Zig
//! library is linked uninstrumented (see crates/quark-terminal/build.rs).

#![no_main]

use libfuzzer_sys::fuzz_target;
use quark_terminal::input;
use quark_terminal::vt::{MouseAction, MouseButton, MouseGeometry, Terminal};
use quark_terminal::{KeyPress, TerminalState};
use quark_ui::FocusId;
use winit::keyboard::{KeyCode, ModifiersState, NamedKey};

const MAX_INPUT: usize = 16 * 1024;
const MAX_EVENTS: usize = 256;
const ROWS: u16 = 24;
const CELL: (u32, u32) = (8, 16);

const CODES: &[KeyCode] = &[
    KeyCode::KeyA,
    KeyCode::KeyC,
    KeyCode::KeyV,
    KeyCode::KeyZ,
    KeyCode::Digit1,
    KeyCode::Digit0,
    KeyCode::Backquote,
    KeyCode::Backslash,
    KeyCode::BracketLeft,
    KeyCode::Comma,
    KeyCode::Minus,
    KeyCode::Slash,
    KeyCode::IntlBackslash,
    KeyCode::AltLeft,
    KeyCode::ControlRight,
    KeyCode::ShiftLeft,
    KeyCode::SuperLeft,
    KeyCode::Backspace,
    KeyCode::CapsLock,
    KeyCode::Enter,
    KeyCode::Space,
    KeyCode::Tab,
    KeyCode::Escape,
    KeyCode::Delete,
    KeyCode::End,
    KeyCode::Home,
    KeyCode::Insert,
    KeyCode::PageDown,
    KeyCode::ArrowDown,
    KeyCode::ArrowLeft,
    KeyCode::NumLock,
    KeyCode::Numpad0,
    KeyCode::Numpad7,
    KeyCode::NumpadAdd,
    KeyCode::NumpadDecimal,
    KeyCode::NumpadEnter,
    KeyCode::NumpadEqual,
    KeyCode::F1,
    KeyCode::F5,
    KeyCode::F12,
    KeyCode::F13,
    KeyCode::F25,
    KeyCode::PrintScreen,
    KeyCode::Pause,
    KeyCode::ContextMenu,
];

const NAMED: &[NamedKey] = &[
    NamedKey::Enter,
    NamedKey::Tab,
    NamedKey::Space,
    NamedKey::Backspace,
    NamedKey::Escape,
    NamedKey::Delete,
    NamedKey::Insert,
    NamedKey::Home,
    NamedKey::End,
    NamedKey::PageUp,
    NamedKey::PageDown,
    NamedKey::ArrowUp,
    NamedKey::ArrowDown,
    NamedKey::ArrowLeft,
    NamedKey::ArrowRight,
    NamedKey::Shift,
    NamedKey::Control,
    NamedKey::Alt,
    NamedKey::Super,
    NamedKey::CapsLock,
    NamedKey::F1,
    NamedKey::F4,
    NamedKey::F12,
    // Not mapped to a Ghostty key: ignored.
    NamedKey::MediaPlay,
];

const BUTTONS: &[MouseButton] = &[
    MouseButton::Left,
    MouseButton::Right,
    MouseButton::Middle,
    MouseButton::WheelUp,
    MouseButton::WheelDown,
    MouseButton::WheelLeft,
    MouseButton::WheelRight,
];

/// Reads fields off the front of the input.
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn byte(&mut self) -> Option<u8> {
        let (&b, rest) = self.0.split_first()?;
        self.0 = rest;
        Some(b)
    }

    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes([self.byte()?, self.byte()?]))
    }

    /// A length byte, then that many bytes (capped at 64) as lossy UTF-8.
    fn text(&mut self) -> Option<String> {
        let len = usize::from(self.byte()? % 65).min(self.0.len());
        let (text, rest) = self.0.split_at(len);
        self.0 = rest;
        Some(String::from_utf8_lossy(text).into_owned())
    }
}

fn modifiers(bits: u8) -> ModifiersState {
    let mut m = ModifiersState::empty();
    for (bit, flag) in [
        (1, ModifiersState::SHIFT),
        (2, ModifiersState::CONTROL),
        (4, ModifiersState::ALT),
        (8, ModifiersState::SUPER),
    ] {
        if bits & bit != 0 {
            m |= flag;
        }
    }
    m
}

#[derive(Debug)]
enum Event {
    /// A key press and, when `then_text`, the text event the platform
    /// sends after it.
    Key {
        named: Option<NamedKey>,
        physical: Option<KeyCode>,
        modifiers: ModifiersState,
        repeat: bool,
        text: Option<String>,
        then_text: bool,
    },
    Text(String),
    Paste {
        text: String,
        allow_unsafe: bool,
    },
    Focus(bool),
    Mouse {
        action: MouseAction,
        button: Option<MouseButton>,
        at: (f32, f32),
        modifiers: ModifiersState,
        any_pressed: bool,
    },
}

impl Event {
    /// Tags: 0 key (named, physical, flags, text), 1 text, 2 paste (flags,
    /// text), 3 focus (gained), 4 mouse (action, button, x, y, flags).
    fn parse(r: &mut Reader) -> Option<Self> {
        Some(match r.byte()? % 5 {
            0 => {
                let named = r.byte()?;
                let physical = r.byte()?;
                let flags = r.byte()?;
                let text = r.text()?;
                Event::Key {
                    named: NAMED.get(usize::from(named)).copied(),
                    physical: CODES.get(usize::from(physical)).copied(),
                    modifiers: modifiers(flags),
                    repeat: flags & 0x10 != 0,
                    text: (flags & 0x20 != 0).then_some(text),
                    then_text: flags & 0x40 != 0,
                }
            }
            1 => Event::Text(r.text()?),
            2 => {
                let flags = r.byte()?;
                Event::Paste {
                    allow_unsafe: flags & 1 != 0,
                    text: r.text()?,
                }
            }
            3 => Event::Focus(r.byte()? & 1 != 0),
            _ => {
                let action = r.byte()?;
                let button = r.byte()?;
                let (x, y) = (r.u16()?, r.u16()?);
                let flags = r.byte()?;
                Event::Mouse {
                    action: [
                        MouseAction::Press,
                        MouseAction::Release,
                        MouseAction::Motion,
                    ][usize::from(action % 3)],
                    button: BUTTONS.get(usize::from(button)).copied(),
                    // Quarter pixels, reaching past the grid.
                    at: (f32::from(x) / 4.0, f32::from(y) / 4.0),
                    modifiers: modifiers(flags),
                    any_pressed: flags & 0x10 != 0,
                }
            }
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MouseFormat {
    X10,
    Utf8,
    Sgr,
    SgrPixels,
}

/// The bytes a program sends to turn on the header's modes.
fn modes(header: [u8; 4]) -> (Vec<u8>, bool, bool, Option<MouseFormat>) {
    let mut out = Vec::new();
    let flags = header[0];
    for (bit, seq) in [
        (0x01, &b"\x1b[?1h"[..]),
        (0x02, b"\x1b="),
        (0x04, b"\x1b[?2004h"),
        (0x08, b"\x1b[?1004h"),
        (0x10, b"\x1b[?1049h"),
        (0x20, b"\x1b[?1007h"),
        (0x40, b"\x1b[>4;2m"),
    ] {
        if flags & bit != 0 {
            out.extend_from_slice(seq);
        }
    }
    let kitty = header[1] & 0x1f;
    if kitty != 0 {
        out.extend_from_slice(format!("\x1b[>{kitty}u").as_bytes());
    }
    let tracking = match header[2] & 3 {
        0 => None,
        1 => Some(1000),
        2 => Some(1002),
        _ => Some(1003),
    };
    let format = match (header[2] >> 2) & 3 {
        0 => MouseFormat::X10,
        1 => MouseFormat::Utf8,
        2 => MouseFormat::Sgr,
        _ => MouseFormat::SgrPixels,
    };
    if let Some(mode) = tracking {
        out.extend_from_slice(format!("\x1b[?{mode}h").as_bytes());
        let format_mode = match format {
            MouseFormat::X10 => None,
            MouseFormat::Utf8 => Some(1005),
            MouseFormat::Sgr => Some(1006),
            MouseFormat::SgrPixels => Some(1016),
        };
        if let Some(mode) = format_mode {
            out.extend_from_slice(format!("\x1b[?{mode}h").as_bytes());
        }
    }
    (
        out,
        flags & 0x04 != 0,
        flags & 0x08 != 0,
        tracking.map(|_| format),
    )
}

/// Checks that `report` is one or more mouse reports in `format`.
fn check_mouse(report: &[u8], format: MouseFormat) {
    let mut rest = report;
    while !rest.is_empty() {
        match format {
            MouseFormat::X10 | MouseFormat::Utf8 => {
                assert!(rest.starts_with(b"\x1b[M"), "{report:?}");
                // The button is one byte; the column and row are one byte
                // each in X10, one UTF-8 character each in UTF-8 mode.
                assert!(rest.len() > 3, "short report {report:?}");
                rest = &rest[4..];
                for _ in 0..2 {
                    let len = match (format, rest.first()) {
                        (_, None) => panic!("short report {report:?}"),
                        (MouseFormat::X10, Some(_)) | (_, Some(0..0x80)) => 1,
                        (_, Some(0xc0..0xe0)) => 2,
                        (_, Some(b)) => panic!("byte {b:#x} in {report:?}"),
                    };
                    assert!(rest.len() >= len, "short report {report:?}");
                    rest = &rest[len..];
                }
            }
            MouseFormat::Sgr | MouseFormat::SgrPixels => {
                assert!(rest.starts_with(b"\x1b[<"), "{report:?}");
                let end = rest
                    .iter()
                    .position(|&b| b == b'M' || b == b'm')
                    .unwrap_or_else(|| panic!("unterminated report {report:?}"));
                let fields: Vec<&[u8]> = rest[3..end].split(|&b| b == b';').collect();
                assert_eq!(fields.len(), 3, "{report:?}");
                for field in fields {
                    assert!(
                        !field.is_empty() && field.iter().all(u8::is_ascii_digit),
                        "{report:?}"
                    );
                }
                rest = &rest[end + 1..];
            }
        }
    }
}

fuzz_target!(|data: &[u8]| {
    if data.len() > MAX_INPUT {
        return;
    }
    let Some((&header, body)) = data.split_first_chunk::<4>() else {
        return;
    };
    let (setup, bracketed, focus_reports, mouse) = modes(header);

    // Keys, text, pastes, and focus go through the state, as the app sends
    // them; mouse reports through the wrapper with explicit geometry.
    let mut state = TerminalState::new("fuzz", FocusId::from_key("fuzz.terminal"));
    state.feed(&setup);
    let _ = state.take_input();
    let cols = 1 + u16::from(header[3]) % 240;
    let mut vt = Terminal::new(cols, ROWS);
    vt.resize(cols, ROWS, CELL.0, CELL.1);
    vt.write(&setup);
    let _ = vt.take_effects();
    let geometry = MouseGeometry {
        width: u32::from(cols) * CELL.0,
        height: u32::from(ROWS) * CELL.1,
        cell_width: CELL.0,
        cell_height: CELL.1,
    };

    let mut reader = Reader(body);
    // The last key press encoded the text it typed, so the platform's text
    // event for it is skipped.
    let mut swallow = false;
    for _ in 0..MAX_EVENTS {
        let Some(event) = Event::parse(&mut reader) else {
            break;
        };
        match event {
            Event::Key {
                named,
                physical,
                modifiers,
                repeat,
                text,
                then_text,
            } => {
                let press = KeyPress {
                    named,
                    text: text.as_deref(),
                    physical,
                    modifiers,
                    repeat,
                };
                let _ = state.key_press(&press);
                let sent = state.take_input();
                swallow = !sent.is_empty() && press.typed().is_some();
                if then_text && let Some(typed) = press.typed() {
                    state.text_input(typed);
                    let again = state.take_input();
                    // The text a key press already encoded is not sent twice;
                    // text it did not encode is sent as typed.
                    let expected: &[u8] = if swallow { b"" } else { typed.as_bytes() };
                    assert_eq!(again, expected, "{press:?} sent {sent:?}");
                    swallow = false;
                }
            }
            Event::Text(text) => {
                state.text_input(&text);
                let expected: &[u8] = if swallow { b"" } else { text.as_bytes() };
                assert_eq!(state.take_input(), expected);
                swallow = false;
            }
            Event::Paste { text, allow_unsafe } => {
                let result = state.paste(&text, allow_unsafe);
                let sent = state.take_input();
                if result.is_err() {
                    assert!(!allow_unsafe, "an allowed paste was refused");
                    assert!(sent.is_empty(), "a refused paste sent {sent:?}");
                    continue;
                }
                if bracketed && !text.is_empty() {
                    let inner = sent
                        .strip_prefix(b"\x1b[200~")
                        .and_then(|s| s.strip_suffix(b"\x1b[201~"))
                        .unwrap_or_else(|| panic!("unbracketed paste {sent:?}"));
                    assert!(
                        !inner.windows(6).any(|w| w == b"\x1b[201~"),
                        "the paste ends early: {sent:?}"
                    );
                } else {
                    assert!(
                        !sent.contains(&b'\n'),
                        "a newline reached the shell: {sent:?}"
                    );
                }
            }
            Event::Focus(gained) => {
                state.focus_changed(gained);
                let expected: &[u8] = match (focus_reports, gained) {
                    (false, _) => b"",
                    (true, true) => b"\x1b[I",
                    (true, false) => b"\x1b[O",
                };
                assert_eq!(state.take_input(), expected);
            }
            Event::Mouse {
                action,
                button,
                at,
                modifiers,
                any_pressed,
            } => {
                let sent = vt.mouse(
                    action,
                    button,
                    at,
                    input::mods(modifiers),
                    geometry,
                    any_pressed,
                );
                let report = vt.take_effects().pty;
                assert_eq!(sent, !report.is_empty());
                match mouse {
                    None => assert!(report.is_empty(), "a report while tracking is off"),
                    Some(format) => check_mouse(&report, format),
                }
            }
        }
    }
});
