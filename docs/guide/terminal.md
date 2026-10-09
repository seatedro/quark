# Terminal

`quark-terminal`: a terminal element backed by libghostty-vt (Ghostty's VT
library) and a PTY.

- libghostty-vt holds screen and scrollback and encodes key and mouse input.
- A PTY runs the program.
- `terminal_view` paints the visible rows with `quark-text`.
- Supported features and input paths: crate docs in
  [lib.rs](../../crates/quark-terminal/src/lib.rs).
- Examples:
  [terminal_demo](../../crates/quark-terminal/examples/terminal_demo.rs)
  (one terminal in a window); the Workbench and Codex demos (terminals in
  panels).

## Building

- Every crate linking `quark-terminal` needs Zig 0.16 to build, tests
  included.
- The build downloads Ghostty's sources with curl and caches the built
  library in the target directory.
- Offline and Nix builds: `QUARK_GHOSTTY_VT_SOURCE_DIR` names pre-fetched
  packages; `QUARK_GHOSTTY_VT_LIB_DIR` names a prebuilt archive.
- `ZIG` and `CURL` override the binaries on `PATH`.
- All variables: [build.rs](../../crates/quark-terminal/build.rs).

## Wiring

1. The app owns a `TerminalState` and passes it to `terminal_view` each
   frame.
2. `TerminalState::spawn` takes a callback that the PTY reader thread calls
   on output. Point it at the window's `Waker`.
3. `UiApp::wake` then runs on the UI thread; call `read_pty` there to feed
   output through the VT.
4. No redraw follows a wake: request one when `read_pty` returns true.

Excerpt of the Codex demo's
[terminal.rs](../../examples/codex/src/terminal.rs), which starts the shell
the first time the terminal is shown:

```rust
pub fn init(&mut self, cx: &mut UiContext) {
    self.waker = Some(cx.window.waker().clone());
}

pub fn wake(&mut self, cx: &mut UiContext) {
    if self.term.read_pty() {
        self.term.take_signals();
        cx.window.request_redraw_all();
    }
}

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
        // ... pick the command (a shell in the current directory)
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
```

- Spawn after `set_viewport` and `prepare`, so the program starts at the
  grid's real size. A shell started at a guessed size redraws its prompt on
  the first resize.
- `UiApp::wake` runs in whichever window the wake reached. A terminal that
  can sit in another window (a docked panel) needs `request_redraw_all`.
- The `'static` id passed to `TerminalState::new` names the terminal's cache
  entries and accessibility node: each terminal in a window needs its own.
- Dropping a `TerminalState` ends its program: SIGHUP on Unix,
  `TerminateProcess` on Windows.
- Scripted sessions without a PTY: [Testing](testing.md#terminals).

## Appearance

`TerminalStyle` carries Ghostty's options with Ghostty's meanings.

| Constructor | Gives |
|---|---|
| `TerminalStyle::default()` | The text system's monospace family at 13 points |
| `TerminalStyle::ghostty()` | JetBrains Mono (Ghostty's default, bundled by quark-text) |
| `TerminalStyle::load_ghostty_config()` | The user's Ghostty config over those defaults, plus a message per line it did not understand. `None`: no config |
| `from_ghostty_config(text)` | Parses given text |

- Cell size: the font's advance by its line height, each rounded to whole
  device pixels. Rows and columns that fit depend on font, size, and scale
  factor.
- `prepare` remeasures when the scale factor or fonts change, and resizes
  the grid and the PTY.
- `adjust_cell_height` grows or shrinks the cell, keeping text centered.
- Box drawing, block elements, braille, powerline, and legacy computing
  symbols are drawn to the cell (Ghostty's sprite face), so they join
  without seams in any font.
- Characters with text default presentation (`⏺`, `✔`) draw from a
  monochrome face that has them; color emoji only when no other face does.
- Emoji presentation characters, and sequences with VS16, draw in color.
  This holds for all text quark draws.
- `alpha_blending` defaults to `LinearCorrected`, weighting glyph edges as
  Ghostty draws them. `Linear` draws dark text on light backgrounds thinner
  and light text on dark ones heavier.
