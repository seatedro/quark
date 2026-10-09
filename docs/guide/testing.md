# Testing

Drive a `UiApp` from `cargo test` and assert on what a user or screen reader
sees.

## `UiTestHarness`

`quark_app::testing::UiTestHarness`, feature `test-support` (examples and
tests in `quark-app` get it automatically).

- Runs the app through the real adapter with no window, event loop, or GPU.
- Time is a fake clock moved only by `advance(ms)`; an animation's frame
  draws at the time it asked for.
- The clipboard is in memory.
- `UiSender` messages and wakes arrive on the test's thread.
- Every input call runs until idle: after it returns, the app has handled
  the input and painted.

| Step | API |
|---|---|
| Find a node | `By::name`, `By::role`, `By::role_name`, `By::id` (accessibility id), `By::test_id` |
| Act | `click_node`, `type_text`, `key("mod+s")`, `drag`, `wheel`, `ime_preedit`, `ime_commit` |
| Read | `painted_text()`, `accessibility_tree()`, `focused()`, a node's `bounds`, `hit_test(point)` |

- `find` panics with the whole accessibility tree when no node, or several,
  match.

From [hello_ui.rs](../../crates/quark-app/examples/hello_ui.rs):

```rust
#[test]
fn greet_shows_the_typed_name() {
    let mut ui = UiTestHarness::new(HelloUi::new(), (640.0, 400.0), 2.0);

    ui.click_node(By::role_name(Role::TextInput, "Name"));
    ui.type_text("Ada");
    ui.click_node(By::role_name(Role::Button, "Greet"));

    let greeting = ui.find(By::name("Hello, Ada!"));
    assert_eq!(greeting.role, Some(Role::Label));
    assert!(
        ui.painted_text().lines().any(|line| line == "Hello, Ada!"),
        "painted:\n{}",
        ui.painted_text()
    );
}
```

- The second doctest in
  [crates/quark-app/src/lib.rs](../../crates/quark-app/src/lib.rs) also
  delivers a `UiSender` message.

## Pixels

- Feature `headless-render`: `render_rgba()` draws the last frame on a
  headless GPU and returns `Pixels`.
- Probe single pixels with `pixel(x, y)`.
- No GPU adapter: returns `RenderError::NoAdapter` and the test skips.
- `QUARK_REQUIRE_GPU=1` turns a missing adapter into a failure.

## Allocation counts

- `quark_ui::test_alloc::Counting`: a global allocator counting the current
  thread's allocations. Install it as the test binary's
  `#[global_allocator]`.
- `test_alloc::count(f)` returns how many allocations `f` made;
  `test_alloc::profile(f)` groups them by call site.
- Count `UiTestHarness::frame` to budget a frame. Example:
  [examples/workbench/tests/perf.rs](../../examples/workbench/tests/perf.rs).
- Only the calling thread is counted. The block document measures rows,
  highlights code, and decodes images on workers the fake clock does not
  drive; a frame picking up their results rebuilds rows.
- Before counting a document, call `MarkdownDocument::finish_highlights`,
  `finish_images`, then `finish_measures` (the first two rebuild rows that
  then need measuring). Then draw a frame and advance the clock so measured
  heights settle the scroll anchor.
- Draw a few frames first and take the least of several counts, so a table
  growing once does not count:

```rust
fn repeated_frame(ui: &mut UiTestHarness<Workbench>) -> u64 {
    (0..3)
        .map(|_| test_alloc::count(|| { ui.frame(); }).1)
        .min()
        .unwrap_or(0)
}
```

## Several windows

- `UiTestHarness` drives several windows.
- `desktop_move`, `desktop_press`, `desktop_release` move the pointer across
  the virtual desktop with the pressed window's grab, as X11, Windows, and
  macOS deliver a drag.
- `DockWindows::window_stack` takes a `ScriptedStack`
  (`quark_app::platform::dock_drag`, feature `test-support`) in place of the
  window system's stacking order.
- `UiTestHarness::set_capabilities` simulates a desktop without window
  positions.
- `UiTestHarness::with_monitor` starts on a display, so placements restore.
- Examples:
  [dock_windows/tests.rs](../../crates/quark-app/src/dock_windows/tests.rs).

## Terminals

- A real shell prints whatever the machine's profile prints; script the
  session instead.
- With no PTY attached, `TerminalState::feed` writes bytes into the VT as
  program output.
- `take_input` returns bytes queued for the program (keys, pastes, query
  replies).
- Test backends built on those two:
  - Replay a captured transcript: the Codex demo's `--terminal scripted`.
  - Answer typed commands from fixtures: the Workbench's `Shell` in
    [dock/terminal.rs](../../examples/workbench/src/dock/terminal.rs).
- Read the user's Ghostty config only for real shells, so tests do not
  depend on the machine.
- Real PTY output arrives outside the fake clock. Have the spawn callback
  also signal a channel the test waits on (as terminal_demo's tests do).

## Smoke runs

- Debug builds exit after N presented frames when
  `QUARK_EXIT_AFTER_FRAMES=N` is set.

Testing quark itself (CI, budgets, fuzzing, end-to-end specs) is in
[docs/maintainers/testing.md](../maintainers/testing.md).
