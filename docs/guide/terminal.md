# Terminal

`quark-terminal` is a terminal element: libghostty-vt, Ghostty's VT
library, holds the screen and scrollback and encodes keys and mouse
input; a PTY runs the program; `terminal_view` paints the visible rows
with `quark-text`. The crate docs in
[lib.rs](../../crates/quark-terminal/src/lib.rs) list what it supports and
how input reaches it. [terminal_demo](../../crates/quark-terminal/examples/terminal_demo.rs)
is a window holding one terminal; the Workbench and Codex demos host
terminals in panels.

## Building

The build script builds libghostty-vt from a pinned Ghostty commit with
Zig 0.16, downloading the sources with curl, and caches the archive in
the target directory. Every crate that links `quark-terminal` needs Zig
to build, tests and e2e binaries included. For offline and Nix builds,
`QUARK_GHOSTTY_VT_SOURCE_DIR` names pre-fetched packages and
`QUARK_GHOSTTY_VT_LIB_DIR` a prebuilt archive;
[build.rs](../../crates/quark-terminal/build.rs) documents both.

## Wiring

The app owns a `TerminalState` and passes it to `terminal_view` each
frame. Program output arrives on the PTY's reader thread, which only
calls the callback given to `TerminalState::spawn`. Point that callback at
the window's `Waker`; `UiApp::wake` then runs on the UI thread, where
`read_pty` feeds the output through the VT. The adapter does not redraw
after a wake, so request one when `read_pty` returns true. This excerpt
of the Codex demo's [terminal.rs](../../examples/codex/src/terminal.rs)
does all of it, and starts the shell the first time the terminal is
shown:

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
```

- Spawning after `set_viewport` and `prepare` starts the program at the
  grid's real size. A shell started at a guessed size redraws its prompt
  when the first resize arrives.
- `UiApp::wake` runs after any wake, in the context of whichever window
  the wake reached. A terminal that can sit in another window (a docked
  panel) redraws every window with `request_redraw_all`.
- The id given to `TerminalState::new` is `'static` and names the
  terminal's cache entries and accessibility node, so each terminal open
  in a window needs its own.
- Dropping a `TerminalState` ends its program: SIGHUP on Unix,
  `TerminateProcess` on Windows.

## Appearance

`TerminalStyle` carries Ghostty's options with Ghostty's meanings.
`TerminalStyle::default()` uses the text system's monospace family at 13
points; `TerminalStyle::ghostty()` uses JetBrains Mono, Ghostty's default,
which quark-text bundles. `TerminalStyle::load_ghostty_config()` reads
the user's Ghostty config over those defaults and returns the style with
a message per line it did not understand; `None` means there is no
config. `from_ghostty_config(text)` parses given text. The Workbench
reads the user's config for real shells only, so its tests do not depend
on the machine.

- The cell is the font's advance by its line height, each rounded to
  whole device pixels, so the number of rows and columns that fit depends
  on the font, its size, and the scale factor. `prepare` remeasures when
  the scale factor or the fonts change, and resizes the grid and the
  PTY. `adjust_cell_height` grows or shrinks the cell, keeping the text
  centered.
- Box drawing, block elements, braille, powerline, and the legacy
  computing symbols are drawn to the cell rather than taken from the font
  (Ghostty's sprite face), so they join without seams in any font.
- Characters whose default presentation is text, such as `⏺` and `✔`,
  draw from a monochrome face that has them, and from the color emoji
  font only when no other face does. Emoji presentation characters, and
  sequences with the emoji variation selector (VS16), draw in color. This
  holds for all text quark draws.
- `alpha_blending` defaults to `LinearCorrected`, which weights glyph
  edges to look as Ghostty draws them; `Linear` draws dark text on light
  backgrounds thinner and light text on dark ones heavier.
