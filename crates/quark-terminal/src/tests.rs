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

/// The alternate screen does not reflow, so narrowing it can leave a wide
/// character in the last column with no room for its second half. Its run
/// stays inside the grid. (Found by the terminal_vt fuzz target.)
#[test]
fn a_wide_character_cut_off_by_a_resize_stays_in_the_grid() {
    let mut t = term(4, 2);
    t.feed("\x1b[?1049hab\u{ff16}".as_bytes());
    t.vt_mut().resize(3, 2, 8, 16);
    assert_eq!(first_row(&mut t), "0+2\"ab\" 2+1\"\u{ff16}\"");
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

// Regression: a cancelled drag fell back to its release, so focus loss
// during a Ctrl+click (Cmd+click) on a link opened it.
#[test]
fn a_cancelled_modified_click_opens_no_link() {
    let mut t = term(30, 2);
    t.feed(b"see \x1b]8;;https://example.com/\x1b\\docs\x1b]8;;\x1b\\ here");
    t.set_modifiers(if cfg!(target_os = "macos") {
        ModifiersState::SUPER
    } else {
        ModifiersState::CONTROL
    });
    press(&mut t, (5, 0), 0);
    assert_eq!(
        t.handle(TerminalEvent::Cancel, 10),
        TerminalOutcome::Handled
    );
    assert_eq!(
        t.handle(TerminalEvent::Release, 20),
        TerminalOutcome::Ignored
    );
}

#[test]
fn query_replies_go_back_to_the_program() {
    let mut t = term(20, 4);
    t.feed(b"ab\x1b[6n");
    assert_eq!(t.take_input(), b"\x1b[1;3R");
}

/// A paste larger than the PTY's input queue reaches the program whole:
/// the state keeps what the queue turned away and sends it as room frees.
#[cfg(unix)]
#[test]
fn a_paste_larger_than_the_input_queue_arrives_whole() {
    let len = 3 * crate::pty::INPUT_QUEUE;
    let mut t = term(40, 4);
    let ui = std::thread::current();
    let command = crate::PtyCommand::new("sh")
        .arg("-c")
        .arg(format!("stty raw -echo; printf R; head -c {len} | wc -c"));
    t.spawn(&command, move || ui.unpark()).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    let mut pasted = false;
    while !t.has_exited() {
        assert!(std::time::Instant::now() < deadline, "{}", screen(&mut t));
        std::thread::park_timeout(std::time::Duration::from_millis(100));
        t.read_pty();
        // Once the terminal is raw.
        if !pasted && screen(&mut t).starts_with('R') {
            let text: String = (0..len)
                .map(|i| char::from(b'a' + (i % 26) as u8))
                .collect();
            t.paste(&text, false).unwrap();
            pasted = true;
        }
    }
    // wc pads its count on some platforms.
    let shown: String = screen(&mut t).split_whitespace().collect();
    assert_eq!(shown, format!("R{len}"));
}

/// Input that would pass the backlog a program that stopped reading leaves
/// behind is refused whole and reported; input that still fits is taken.
#[cfg(unix)]
#[test]
fn input_past_the_backlog_is_refused_whole_and_reported() {
    use crate::pty::INPUT_QUEUE;
    use crate::state::INPUT_BACKLOG;

    let mut t = term(40, 4);
    let ui = std::thread::current();
    let command = crate::PtyCommand::new("sh")
        .arg("-c")
        .arg("stty raw -echo; printf R; exec sleep 30");
    t.spawn(&command, move || ui.unpark()).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !screen(&mut t).starts_with('R') {
        assert!(std::time::Instant::now() < deadline, "{}", screen(&mut t));
        std::thread::park_timeout(std::time::Duration::from_millis(10));
        t.read_pty();
    }
    let text = |len: usize| -> String {
        (0..len)
            .map(|i| char::from(b'a' + (i % 26) as u8))
            .collect()
    };
    // The PTY's queue takes the first INPUT_QUEUE bytes; the rest waits.
    t.paste(&text(INPUT_BACKLOG), false).unwrap();
    assert_eq!(t.take_signals(), vec![]);
    t.paste(&text(2 * INPUT_QUEUE), false).unwrap();
    assert_eq!(
        t.take_signals(),
        vec![TerminalSignal::InputRefused(2 * INPUT_QUEUE)]
    );
    t.text_input("x");
    assert_eq!(t.take_signals(), vec![]);
}

/// A terminal that was sized before the monospace family changed matches
/// one made after it: columns, the cell under a point, and the pixel size
/// it reports to the program.
#[test]
fn a_font_change_resizes_an_existing_terminal_like_a_fresh_one() {
    use quark_text::{FontSettings, LayoutCache, TextSystem};
    use quark_ui::theme::Theme;

    let theme = Theme::default_dark();
    let mut text = TextSystem::vendored_only(&FontSettings::default());
    let mut layouts = LayoutCache::new(1);
    let mut sized = |t: &mut TerminalState, text: &mut TextSystem| {
        t.set_viewport(640.0, 400.0);
        t.prepare(text, &mut layouts, 1.0, &theme);
        t.feed(b"\x1b[14t");
        (t.size(), t.cell_at(300.0, 100.0), t.take_input())
    };
    let new = || TerminalState::new("test", quark_ui::FocusId::from_key("test.terminal"));
    let mut existing = new();
    let before = sized(&mut existing, &mut text);
    // Any family can be the monospace one; Inter's digits are narrower.
    text.set_font_settings(&FontSettings {
        mono_family: "Inter".to_owned(),
        ..FontSettings::default()
    });
    let after = sized(&mut existing, &mut text);
    let fresh = sized(&mut new(), &mut text);
    assert_ne!(before.0, fresh.0, "the fonts have the same advance");
    assert_eq!(after, fresh);
}

// ---- IME -----------------------------------------------------------------

/// Composition updates, and keys pressed while composing (an IME that
/// lets them through), send the program nothing.
#[test]
fn composing_sends_nothing_to_the_program() {
    let mut t = term(20, 2);
    let none = ModifiersState::empty();
    t.set_preedit("n", Some((1, 1)));
    t.key_press(&key(None, Some("i"), none));
    t.set_preedit("\u{306b}", Some((3, 3)));
    t.key_press(&key(Some(NamedKey::Space), None, none));
    t.set_preedit("\u{65e5}\u{672c}", Some((0, 6)));
    assert_eq!(t.take_input(), b"");
    assert_eq!(
        t.preedit().map(|p| p.text.as_str()),
        Some("\u{65e5}\u{672c}")
    );
}

/// A commit reaches the program exactly once whichever way it ends the
/// composition, including right after a shortcut key whose own text the
/// terminal was waiting to skip.
#[test]
fn a_commit_is_sent_exactly_once() {
    let ctrl_c = key(None, Some("c"), ModifiersState::CONTROL);
    type Step = fn(&mut TerminalState);
    let cases: [(&str, &[Step], &[u8]); 3] = [
        (
            "winit order: the preedit clears, then the text",
            &[
                |t| t.set_preedit("\u{65e5}\u{672c}", Some((6, 6))),
                |t| t.set_preedit("", None),
                |t| t.text_input("\u{65e5}\u{672c}"),
            ],
            "\u{65e5}\u{672c}".as_bytes(),
        ),
        (
            "text while the preedit still shows",
            &[
                |t| t.set_preedit("\u{65e5}\u{672c}", Some((6, 6))),
                |t| t.text_input("\u{65e5}\u{672c}"),
            ],
            "\u{65e5}\u{672c}".as_bytes(),
        ),
        (
            "an explicit commit",
            &[
                |t| t.set_preedit("\u{e9}", None),
                |t| t.commit_preedit("\u{e9}"),
            ],
            "\u{e9}".as_bytes(),
        ),
    ];
    for (name, steps, expected) in cases {
        let mut t = term(20, 2);
        t.key_press(&ctrl_c);
        assert_eq!(t.take_input(), b"\x03", "{name}");
        for step in steps {
            step(&mut t);
        }
        assert_eq!(t.take_input(), expected, "{name}");
        assert_eq!(t.preedit(), None, "{name}");
    }
}

/// An IME that cancels, a window that loses focus, and a terminal built
/// without focus all drop the composition and send nothing.
#[test]
fn cancelling_a_composition_sends_nothing() {
    use crate::view::{TerminalEnv, terminal_view};
    use quark_ui::theme::Theme;

    let theme = Theme::default_dark();
    type Cancel = fn(&mut TerminalState, &Theme);
    let cases: [(&str, Cancel); 3] = [
        ("the IME cancels", |t, _| t.set_preedit("", None)),
        ("the window loses focus", |t, _| t.focus_changed(false)),
        ("another element has focus", |t, theme| {
            let env = TerminalEnv {
                focused: false,
                accessible: false,
            };
            terminal_view(t, theme, env, |_| quark_ui::Action::new(()));
        }),
    ];
    for (name, cancel) in cases {
        let mut t = term(20, 2);
        t.set_preedit("\u{306b}\u{307b}", Some((6, 6)));
        cancel(&mut t, &theme);
        assert_eq!(t.preedit(), None, "{name}");
        assert_eq!(t.take_input(), b"", "{name}");
    }
}

/// The IME's cursor bytes land on grapheme boundaries of the composition,
/// in order, whatever the IME sends.
#[test]
fn composition_cursors_snap_onto_graphemes() {
    let cases = [
        // 日本語: three 3-byte characters.
        (Some((4, 7)), Some(3..6)),
        (Some((9, 0)), Some(0..9)),
        (Some((40, 40)), Some(9..9)),
        (None, None),
    ];
    for (cursor, expected) in cases {
        let mut t = term(20, 2);
        t.set_preedit("\u{65e5}\u{672c}\u{8a9e}", cursor);
        let selection = t.preedit().and_then(|p| p.selection.clone());
        assert_eq!(selection, expected, "{cursor:?}");
    }
}

proptest::proptest! {
    #[test]
    fn composition_cursors_are_ordered_char_boundaries(
        text in "\\PC{1,12}",
        start in 0usize..64,
        end in 0usize..64,
    ) {
        let mut t = term(20, 2);
        t.set_preedit(&text, Some((start, end)));
        let range = t.preedit().and_then(|p| p.selection.clone()).expect("selection");
        proptest::prop_assert!(range.start <= range.end && range.end <= text.len());
        proptest::prop_assert!(text.is_char_boundary(range.start));
        proptest::prop_assert!(text.is_char_boundary(range.end));
    }
}

/// Release-mode time per phase of a terminal frame for each kind of
/// change, on the fixture of terminal_demo's allocation budget tests (an
/// 80x22 terminal after three screens of output and a prompt). Prints
/// medians over fresh harnesses, then the per-cell cost of reading a full
/// screen. Run with `cargo test --release -p quark-terminal --lib --
/// --ignored --nocapture report_frame_timing`.
#[test]
#[ignore = "measurement, prints a report"]
fn report_frame_timing() {
    use std::time::{Duration, Instant};

    use quark_app::testing::UiTestHarness;
    use quark_app::{UiApp, UiContext, ViewContext};
    use quark_ui::Action;
    use quark_ui::element::AnyElement;

    use crate::view::{TerminalEnv, terminal_view};
    use crate::vt::timing;

    struct Probe(TerminalState);

    impl UiApp for Probe {
        type Action = TerminalEvent;
        type Message = ();

        fn view(&mut self, cx: &mut ViewContext) -> AnyElement {
            let (width, height) = cx.frame.size();
            self.0.set_viewport(width, height);
            let scale = cx.frame.scale_factor();
            let text = cx.frame.text();
            self.0
                .prepare(&mut text.system, &mut text.layouts, scale, cx.theme);
            let env = TerminalEnv {
                focused: true,
                accessible: false,
            };
            terminal_view(&mut self.0, cx.theme, env, Action::new)
        }

        fn update(&mut self, _event: TerminalEvent, _cx: &mut UiContext) {}
    }

    fn lines(seed: usize, count: usize) -> String {
        (0..count)
            .map(|n| {
                let text = format!("output {seed}.{n} of a build step");
                format!("\x1b[32m{n:04}\x1b[0m {text:<74}\r\n")
            })
            .collect()
    }

    fn median(mut v: Vec<Duration>) -> Duration {
        v.sort();
        v[v.len() / 2]
    }

    const RUNS: usize = 41;
    let cases: [(&str, String); 4] = [
        ("typed char", "o".into()),
        ("scrolled line", "\r\nfresh output line".into()),
        ("30-line redraw", format!("\r\n{}", lines(9, 30))),
        ("cursor move", "\x1b[22;2H".into()),
    ];
    for (name, input) in &cases {
        let mut frames = Vec::new();
        let mut scopes: Vec<Vec<(Duration, u32)>> = vec![Vec::new(); timing::SCOPES.len()];
        let mut costs = None;
        for _ in 0..RUNS {
            let mut ui = UiTestHarness::new(
                Probe(TerminalState::new(
                    "probe",
                    quark_ui::FocusId::from_key("probe"),
                )),
                (640.0, 400.0),
                1.0,
            );
            ui.set_accessibility_active(false);
            for seed in 0..3 {
                ui.app_mut().0.feed(lines(seed, 30).as_bytes());
                ui.frame();
            }
            ui.app_mut().0.feed(b"$ ech");
            ui.frame();
            assert_eq!(ui.app().0.size(), (80, 22));

            ui.app_mut().0.feed(input.as_bytes());
            timing::take();
            let started = Instant::now();
            ui.frame();
            frames.push(started.elapsed());
            for (i, (_, d, n)) in timing::take().into_iter().enumerate() {
                scopes[i].push((d, n));
            }
            if costs.is_none() && *name == "30-line redraw" {
                costs = Some(ui.app_mut().0.vt_mut().cell_read_costs(2000));
            }
        }
        eprintln!("{name}: frame {:?}", median(frames));
        for (scope, samples) in timing::SCOPES.iter().zip(scopes) {
            let n = samples[0].1;
            let d = median(samples.into_iter().map(|(d, _)| d).collect());
            if n > 0 {
                eprintln!("  {scope:?} x{n}: {d:?}");
            }
        }
        if let Some(costs) = costs {
            eprintln!("  per full screen of cells (22 rows):");
            for (step, d) in costs {
                eprintln!("    {step}: {d:?}");
            }
        }
    }
}
