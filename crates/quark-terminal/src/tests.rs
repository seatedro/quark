//! Terminal behavior through the public state API: bytes in, grid, bytes
//! out, and signals.

use winit::keyboard::{KeyCode, ModifiersState, NamedKey};

use crate::Rgb;
use crate::grid::{Grid, GridRow};
use crate::input::KeyPress;
use crate::state::{PointerInput, TerminalEvent, TerminalOutcome, TerminalSignal, TerminalState};
use crate::vt::{Scroll, UnsafePaste};

const FG: Rgb = Rgb::new(0xdd, 0xdd, 0xdd);
const BG: Rgb = Rgb::new(0x11, 0x11, 0x11);

fn term(cols: u16, rows: u16) -> TerminalState {
    let mut t = TerminalState::headless(cols, rows);
    t.vt_mut().set_default_colors(FG, BG);
    t
}

/// Rows of the screen joined by `|`, trailing blank rows dropped.
fn screen(t: &mut TerminalState) -> String {
    t.refresh().text().replace('\n', "|")
}

/// A row's runs: `col+cols"text" attributes`, space separated.
fn runs(row: &GridRow) -> String {
    row.runs
        .iter()
        .map(|r| {
            let s = r.style;
            let mut out = format!("{}+{}{:?}", r.col, r.cols, row.run_text(r));
            if s.fg != FG {
                out.push_str(&format!(" fg={}", s.fg));
            }
            if let Some(bg) = s.bg {
                out.push_str(&format!(" bg={bg}"));
            }
            for (on, name) in [
                (s.bold, "bold"),
                (s.italic, "italic"),
                (s.faint, "faint"),
                (s.strikethrough, "strike"),
                (s.overline, "overline"),
                (s.hyperlink, "link"),
            ] {
                if on {
                    out.push(' ');
                    out.push_str(name);
                }
            }
            if s.underline != crate::Underline::None {
                out.push_str(&format!(" underline={:?}", s.underline));
            }
            out
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn first_row(t: &mut TerminalState) -> String {
    let grid: &Grid = t.refresh();
    runs(&grid.rows[0])
}

#[test]
fn sgr_sequences_style_runs() {
    let cases: &[(&str, &str)] = &[
        ("plain", r#"0+5"plain""#),
        ("\x1b[38;2;255;128;0mtc\x1b[0m", r#"0+2"tc" fg=#ff8000"#),
        (
            "\x1b[38;5;196mx\x1b[48;5;21my",
            r#"0+1"x" fg=#ff0000 1+1"y" fg=#ff0000 bg=#0000ff"#,
        ),
        ("\x1b[1;3mbi\x1b[22;23mn", r#"0+2"bi" bold italic 2+1"n""#),
        (
            "\x1b[4:3mc\x1b[4:2md\x1b[24;9;53ms",
            r#"0+1"c" underline=Curly 1+1"d" underline=Double 2+1"s" strike overline"#,
        ),
        ("\x1b[7minv", r#"0+3"inv" fg=#111111 bg=#dddddd"#),
        (
            "\x1b[2mdim\x1b[8mhid",
            r#"0+3"dim" faint 3+3"hid" fg=#111111 faint"#,
        ),
        ("\x1b[48;2;1;2;3m  \x1b[0m", r#"0+2"  " bg=#010203"#),
    ];
    for (input, expected) in cases {
        let mut t = term(20, 3);
        t.feed(input.as_bytes());
        assert_eq!(first_row(&mut t), *expected, "input {input:?}");
    }
}

#[test]
fn control_sequences_edit_the_screen() {
    let cases: &[(&str, &str)] = &[
        ("abc\x1b[2;5HX", "abc|    X"),
        ("hello\x1b[1;3H\x1b[K", "he"),
        ("hello\x1b[1;3H\x1b[1K", "   lo"),
        ("a\r\nb\x1b[2J", ""),
        ("abc\x1b[1;2H\x1b[P", "ac"),
        ("abc\x1b[1;2H\x1b[@", "a bc"),
        ("one\r\ntwo\x1b[1;1H\x1b[L", "|one|two"),
        ("a\tb", "a       b"),
        // 1049 saves the cursor and keeps its position on the new screen.
        ("main\x1b[?1049halt", "    alt"),
        ("main\x1b[?1049halt\x1b[?1049l", "main"),
        ("a\u{6f22}b\u{1f600}c", "a\u{6f22}b\u{1f600}c"),
    ];
    for (input, expected) in cases {
        let mut t = term(20, 4);
        t.feed(input.as_bytes());
        assert_eq!(screen(&mut t), *expected, "input {input:?}");
    }
}

#[test]
fn wide_characters_span_two_columns() {
    let mut t = term(10, 2);
    t.feed("a\u{6f22}b\u{1f600}c".as_bytes());
    assert_eq!(
        first_row(&mut t),
        "0+1\"a\" 1+2\"\u{6f22}\" 3+1\"b\" 4+2\"\u{1f600}\" 6+1\"c\""
    );
}

#[test]
fn output_past_the_bottom_goes_to_scrollback() {
    let mut t = term(10, 3);
    let lines: Vec<String> = (0..8).map(|i| format!("l{i}")).collect();
    t.feed(lines.join("\r\n").as_bytes());
    assert_eq!(screen(&mut t), "l5|l6|l7");
    let sb = t.scrollbar();
    assert_eq!((sb.total, sb.offset, sb.len), (8, 5, 3));
    t.vt_mut().scroll(Scroll::Top);
    assert_eq!(screen(&mut t), "l0|l1|l2");
    t.vt_mut().scroll(Scroll::Delta(2));
    assert_eq!(screen(&mut t), "l2|l3|l4");
}

#[test]
fn resize_reflows_soft_wrapped_lines() {
    let mut t = term(10, 4);
    t.feed(b"abcdefghijklmno");
    assert_eq!(screen(&mut t), "abcdefghij|klmno");
    for (cols, expected) in [
        (20, "abcdefghijklmno"),
        (5, "abcde|fghij|klmno"),
        (10, "abcdefghij|klmno"),
    ] {
        t.vt_mut().resize(cols, 4, 8, 16);
        assert_eq!(screen(&mut t), expected, "{cols} columns");
    }
}

/// Window point at the middle of cell `(col, row)`.
fn at(t: &TerminalState, col: u16, row: u16) -> (f32, f32) {
    let m = t.metrics();
    (
        (f32::from(col) + 0.5) * m.cell_w,
        (f32::from(row) + 0.5) * m.cell_h,
    )
}

fn press(t: &mut TerminalState, cell: (u16, u16), now_ms: u64) {
    let (x, y) = at(t, cell.0, cell.1);
    t.handle(TerminalEvent::Press { x, y }, now_ms);
}

#[test]
fn drags_select_cells_and_clicks_select_words_and_lines() {
    let mut t = term(20, 3);
    t.feed(b"hello world\r\nsecond line");
    press(&mut t, (6, 0), 0);
    let (x, y) = at(&t, 5, 1);
    t.handle(TerminalEvent::Drag { x, y }, 10);
    t.handle(TerminalEvent::Release, 20);
    assert_eq!(t.selection_text().as_deref(), Some("world\nsecond"));
    assert_eq!(t.refresh().rows[0].selection, Some((6, 19)));

    // A second click on the same cell selects the word, a third the line.
    press(&mut t, (8, 0), 1000);
    t.handle(TerminalEvent::Release, 1010);
    press(&mut t, (8, 0), 1100);
    assert_eq!(t.selection_text().as_deref(), Some("world"));
    t.handle(TerminalEvent::Release, 1110);
    press(&mut t, (8, 0), 1200);
    assert_eq!(t.selection_text().as_deref(), Some("hello world"));
    // Too slow for a double click: a plain click clears the selection.
    t.handle(TerminalEvent::Release, 1210);
    press(&mut t, (8, 0), 2000);
    assert_eq!(t.selection_text(), None);
}

#[test]
fn copy_shortcut_copies_the_selection() {
    let mut t = term(20, 2);
    t.feed(b"copy me");
    t.select((0, 0), (3, 0));
    let shortcut = if cfg!(target_os = "macos") {
        ModifiersState::SUPER
    } else {
        ModifiersState::CONTROL | ModifiersState::SHIFT
    };
    let outcome = t.key_press(&KeyPress {
        named: None,
        text: Some(if cfg!(target_os = "macos") { "c" } else { "C" }),
        physical: Some(KeyCode::KeyC),
        modifiers: shortcut,
        repeat: false,
    });
    assert_eq!(outcome, TerminalOutcome::Copy("copy".into()));
    assert!(t.take_input().is_empty());
}

fn key(named: Option<NamedKey>, text: Option<&str>, modifiers: ModifiersState) -> KeyPress<'_> {
    KeyPress {
        named,
        text,
        physical: None,
        modifiers,
        repeat: false,
    }
}

#[test]
fn keys_encode_for_the_active_keyboard_mode() {
    let none = ModifiersState::empty();
    let ctrl = ModifiersState::CONTROL;
    let alt = ModifiersState::ALT;
    let up = Some(NamedKey::ArrowUp);
    let cases: &[(&str, KeyPress, &str)] = &[
        ("", key(None, Some("a"), none), "a"),
        ("", key(None, Some("A"), ModifiersState::SHIFT), "A"),
        ("", key(None, Some("c"), ctrl), "\x03"),
        // Alt sends ESC first; on macOS Option composes text instead
        // (Ghostty's default, macos-option-as-alt off), so it arrives as is.
        if cfg!(target_os = "macos") {
            // A real Option+A carries its physical key; "å" alone maps to
            // no key.
            (
                "",
                KeyPress {
                    physical: Some(KeyCode::KeyA),
                    ..key(None, Some("å"), alt)
                },
                "å",
            )
        } else {
            ("", key(None, Some("a"), alt), "\x1ba")
        },
        ("", key(Some(NamedKey::Enter), None, none), "\r"),
        ("", key(Some(NamedKey::Backspace), None, none), "\x7f"),
        ("", key(Some(NamedKey::Tab), None, none), "\t"),
        ("", key(Some(NamedKey::Escape), None, none), "\x1b"),
        ("", key(up, None, none), "\x1b[A"),
        ("\x1b[?1h", key(up, None, none), "\x1bOA"),
        ("", key(Some(NamedKey::ArrowRight), None, ctrl), "\x1b[1;5C"),
        ("", key(Some(NamedKey::F5), None, none), "\x1b[15~"),
        ("", key(Some(NamedKey::Delete), None, none), "\x1b[3~"),
        // The Kitty keyboard protocol, once the program pushes flags.
        (
            "\x1b[>1u",
            key(Some(NamedKey::Escape), None, none),
            "\x1b[27u",
        ),
        ("\x1b[>1u", key(None, Some("c"), ctrl), "\x1b[99;5u"),
        ("\x1b[>1u", key(None, Some("a"), none), "a"),
    ];
    for (setup, press, expected) in cases {
        let mut t = term(20, 2);
        t.feed(setup.as_bytes());
        t.take_input();
        t.key_press(press);
        let sent = String::from_utf8(t.take_input()).unwrap();
        assert_eq!(sent, *expected, "{press:?} after {setup:?}");
    }
}

#[test]
fn text_after_an_encoded_key_is_not_sent_twice() {
    let mut t = term(20, 2);
    t.key_press(&key(None, Some("x"), ModifiersState::empty()));
    t.text_input("x");
    t.text_input("\u{3042}");
    assert_eq!(String::from_utf8(t.take_input()).unwrap(), "x\u{3042}");
}

#[test]
fn paste_is_bracketed_when_the_program_asks() {
    let mut t = term(20, 2);
    assert_eq!(t.paste("a\nb", false), Err(UnsafePaste));
    assert!(t.take_input().is_empty());
    t.paste("a\nb", true).unwrap();
    assert_eq!(t.take_input(), b"a\rb");
    t.feed(b"\x1b[?2004h");
    t.paste("a\nb", false).unwrap();
    assert_eq!(t.take_input(), b"\x1b[200~a\nb\x1b[201~");
}

#[test]
fn mouse_reports_follow_the_programs_mode() {
    let mut t = term(20, 4);
    let (x, y) = at(&t, 2, 1);
    t.pointer(PointerInput::Moved { x, y });
    let left = winit::event::MouseButton::Left;
    // No tracking: the press is left for selection.
    assert!(!t.pointer(PointerInput::Button {
        button: left,
        pressed: true
    }));
    t.pointer(PointerInput::Button {
        button: left,
        pressed: false,
    });
    t.feed(b"\x1b[?1000h\x1b[?1006h");
    assert!(t.pointer(PointerInput::Button {
        button: left,
        pressed: true
    }));
    assert!(t.pointer(PointerInput::Button {
        button: left,
        pressed: false
    }));
    assert!(t.pointer(PointerInput::Wheel { lines: 1.0 }));
    assert_eq!(
        String::from_utf8(t.take_input()).unwrap(),
        "\x1b[<0;3;2M\x1b[<0;3;2m\x1b[<64;3;2M"
    );
    // Shift hands the pointer back for selection.
    t.set_modifiers(ModifiersState::SHIFT);
    assert!(!t.pointer(PointerInput::Button {
        button: left,
        pressed: true
    }));
}

#[test]
fn title_bell_and_opt_in_clipboard_become_signals() {
    let mut t = term(20, 2);
    t.feed(b"\x1b]2;build\x07\x07\x1b]52;c;aGVsbG8=\x07");
    assert_eq!(
        t.take_signals(),
        vec![TerminalSignal::Title("build".into()), TerminalSignal::Bell]
    );
    t.allow_clipboard_write(true);
    t.feed(b"\x1b]52;c;aGVsbG8=\x07");
    assert_eq!(
        t.take_signals(),
        vec![TerminalSignal::Clipboard("hello".into())]
    );
}

#[test]
fn modified_click_on_a_hyperlink_opens_it() {
    let mut t = term(30, 2);
    t.feed(b"see \x1b]8;;https://example.com/\x1b\\docs\x1b]8;;\x1b\\ here");
    assert_eq!(first_row(&mut t), r#"0+4"see " 4+4"docs" link 8+5" here""#);
    let open = if cfg!(target_os = "macos") {
        ModifiersState::SUPER
    } else {
        ModifiersState::CONTROL
    };
    // A plain click only places the selection anchor.
    press(&mut t, (5, 0), 0);
    assert_eq!(
        t.handle(TerminalEvent::Release, 10),
        TerminalOutcome::Handled
    );
    t.set_modifiers(open);
    press(&mut t, (5, 0), 1000);
    assert_eq!(
        t.handle(TerminalEvent::Release, 1010),
        TerminalOutcome::OpenLink("https://example.com/".into())
    );
}

#[test]
fn query_replies_go_back_to_the_program() {
    let mut t = term(20, 4);
    t.feed(b"ab\x1b[6n");
    assert_eq!(t.take_input(), b"\x1b[1;3R");
}
