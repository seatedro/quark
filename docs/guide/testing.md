# Testing

Quark's tests drive real code through public APIs and assert on what a
user or screen reader would observe: text, offsets, rectangles, tree dumps,
and pixels at chosen points. [TEST_BIBLE.md](../../TEST_BIBLE.md) is the
rulebook; a test must name the regression it catches, run
deterministically in milliseconds, and survive behavior-preserving
refactors. It bans tests of derives and getters, snapshots of whole
structs or images, mocks of constructible types, and tests written for
coverage.

## Commands

```bash
cargo test --workspace --features quark-ui/integrity-checks   # what CI runs
cargo test -p quark-ui virtual_list                           # one module, by name filter
cargo test -p quark-app --example hello_ui                    # an example's test module
cargo test --doc -p quark-app                                 # doctests
```

`integrity-checks` makes data structures verify their whole state after
every mutation instead of the entries it touched; it is quadratic on large
documents, so local runs usually leave it off.

## Driving an app: `UiTestHarness`

`quark_app::testing::UiTestHarness` (feature `test-support`; examples and
tests in `quark-app` get it automatically) runs a `UiApp` through the same
adapter as a real window, with no window, event loop, or GPU:

- Time is a fake clock moved only by `advance(ms)`. A frame an animation
  asks for is drawn at the time it asked for.
- The clipboard is in memory.
- `UiSender` messages and wakes are delivered on the test's thread. Every
  input method runs until idle, so after any call the app has handled the
  input and painted the result.

Tests find nodes the way assistive tech does, with `By::name`, `By::role`,
`By::role_name`, `By::id` (accessibility id), or `By::test_id`, then
`click_node`, `type_text`, `key("mod+s")`, `drag`, `wheel`, `ime_preedit`,
and `ime_commit`. They read back `painted_text()`, `accessibility_tree()`,
`focused()`, a node's `bounds`, or `hit_test(point)`. `find` panics with
the whole accessibility tree when no node or several match.

This test is from [hello_ui.rs](../../crates/quark-app/examples/hello_ui.rs):

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

The second doctest in [crates/quark-app/src/lib.rs](../../crates/quark-app/src/lib.rs)
is a complete harness test that also delivers a `UiSender` message.

## Pixels

With the `headless-render` feature, `render_rgba()` draws the last frame on
a headless GPU device and returns `Pixels`; tests probe single pixels with
`pixel(x, y)`. On a host without a GPU adapter it returns
`RenderError::NoAdapter` and the test skips, unless `QUARK_REQUIRE_GPU=1`
turns that into a failure, as CI does (Linux runs on lavapipe, Windows on
WARP). The bible bans whole-image goldens.

## Allocation budgets

`quark_ui::test_alloc::Counting` counts allocations per thread; a test
binary installs it as its `#[global_allocator]`.
[Allocation budgets](#allocation-budgets) lists the budgets the adapter
is held to.

## Beyond unit tests

| Tool | Covers | Where |
|---|---|---|
| `proptest` | Invariants of pure logic: selection order and copy, caret round trips, spring settling, virtual list offsets | The module's test block; failing cases are kept in `proptest-regressions/` |
| `cargo-fuzz` | Parsers and decoders of untrusted input: markdown, text layout of arbitrary strings, unified diffs | [fuzz/](../../fuzz), with committed corpora; CI replays the `text_layout` and `markdown` corpora (`-runs=0`) |
| Miri | Undefined behavior in `quark`'s own `unsafe` and `bytemuck` casts | CI `miri` job, `cargo miri test -p quark` |
| Kani | Bounded proofs of small cores without `HashMap`: the Fenwick tree | `#[cfg(kani)]` harnesses in [fenwick.rs](../../crates/quark/src/fenwick.rs) |
| CommonMark examples | Every spec example through the markdown parser and its integrity check | CI `fuzz` job, report only |
| End-to-end specs | Real example apps through AT-SPI and cua on Xvfb | [e2e/](../../e2e); see [End-to-end specs with cua](#end-to-end-specs-with-cua) |

A fuzzer crash becomes a unit test with the minimized input. The Linux CI
job also runs clippy with `-D warnings` and a `cargo check` with every
optional feature enabled at once (except `profile-tracy`, which conflicts
with `profile-puffin`).

## Terminal sessions

A real shell prints whatever the machine's profile prints. Without a PTY,
`TerminalState::feed` writes bytes into the VT as program output, and
`take_input` returns the bytes queued for the program (typed keys,
pastes, replies to queries) while no PTY is attached. On those two, a
test backend can replay a captured transcript (the Codex
demo's `--terminal scripted`) or answer typed commands from fixtures (the
Workbench's `Shell`, in [dock/terminal.rs](../../examples/workbench/src/dock/terminal.rs)),
so tests and e2e specs see the same screen on every run. Both demos run
a real login shell by default.

The terminal_demo tests run a real shell on Linux and macOS. Its output arrives on the PTY
thread, outside the harness's fake clock, so the spawn callback also
signals a channel the test waits on.

## Allocation budgets

`quark_ui::test_alloc::Counting` is a global allocator that counts the
current thread's allocations; `test_alloc::count(f)` returns how many `f`
made, and `test_alloc::profile(f)` groups them by call site.
[frame_budget.rs](../../crates/quark-app/src/frame_budget.rs) runs whole
frames through the adapter and asserts:

| Frame | Budget |
|---|---|
| A cached list repeating the last frame, with or without a screen reader connected | 0 allocations |
| A tracked scroll container repeating the last frame | 0 |
| A list frame where one cached row changed | 40 per changed row |
| A markdown document repeating the last frame | 16 |
| A document frame after text streams into one message | 320 |

Examples with a test module can carry a budget of their own:
`hello_ui` rebuilds without a cache boundary and holds a repeated frame to
30 allocations. Each budget file also has an ignored test that prints the
call sites (`cargo test -p quark-app report_ -- --ignored --nocapture`).

An app measures its own frames the same way: an integration test installs
`Counting` as its global allocator and counts `UiTestHarness::frame`.
[examples/workbench/tests/perf.rs](../../examples/workbench/tests/perf.rs)
budgets the Workbench's whole window at 64 allocations for a repeated
frame and 512 for one streamed chunk of an answer with its frame. It
measures 45 (53 with a screen reader connected) and 337, and asserts
ceilings just above those, so a regression fails while still under
budget. Counts taken too early are noise:

- `Counting` counts only the calling thread. The block document measures
  rows, highlights code, and decodes images on worker threads, which the
  harness's fake clock does not drive, and a frame that picks up their
  results rebuilds rows. Before counting, wait for them with
  `MarkdownDocument::finish_highlights`, `finish_images`, and
  `finish_measures` (in that order: the first two rebuild rows that then
  need measuring), then draw a frame and advance the clock so the
  measured heights settle the scroll anchor.
- Draw a few frames first, and take the least of several counts, so a
  table growing once does not count:

  ```rust
  fn repeated_frame(ui: &mut UiTestHarness<Workbench>) -> u64 {
      (0..3)
          .map(|_| {
              test_alloc::count(|| {
                  ui.frame();
              })
              .1
          })
          .min()
          .unwrap_or(0)
  }
  ```

## Docking across windows

`UiTestHarness` drives several windows. `desktop_move`, `desktop_press`, and
`desktop_release` move the pointer across the virtual desktop with the
pressed window's grab, as X11, Windows, and macOS deliver a drag.
`DockWindows::window_stack` locates drags with a `ScriptedStack`
(`quark_app::platform::dock_drag`, feature `test-support`) instead of the
window system's; `UiTestHarness::set_capabilities` simulates a desktop
without window positions, and `UiTestHarness::with_monitor` starts on a
display so placements restore.
[crates/quark-app/src/dock_windows/tests.rs](../../crates/quark-app/src/dock_windows/tests.rs)
has the tear-off, drop, cancel, close, and restore tests.

The end-to-end specs `panels_demo/new_window` and `panels_demo/tear_off` in
[e2e/specs](../../e2e/specs/panels_demo) run the demo under Xvfb and
openbox: the first moves a tab to a new window from the keyboard and closes
it again, the second tears a tab off with real pointer events from
`xdotool` and docks it back. `quark_e2e.app_tree(window=...)` finds a window
by its name, and `app_frames()` lists them all.

## End-to-end specs with cua

[e2e/run.sh](../../e2e/run.sh) runs each spec against a real example app on
a private X11 desktop: Xvfb, openbox, a session D-Bus with the AT-SPI bus
enabled, and the [cua](https://github.com/trycua/cua) driver, pinned and
checksummed by [e2e/install-cua.sh](../../e2e/install-cua.sh). Each spec
gets a fresh desktop and a fresh copy of its app.

```bash
cargo build -p quark-app --examples --features ui,notifications
e2e/install-cua.sh
e2e/run.sh                              # every spec
e2e/run.sh e2e/specs/hello_ui/*.py      # some specs
```

A spec lives at `e2e/specs/<example>/<behavior>.py` and runs against the
binary of that name, from `QUARK_E2E_BIN_DIR` (default
`target/debug/examples`) or else from `target/debug`, where package
binaries such as `workbench` and `codex-demo` land:

```bash
cargo build -p quark-workbench --bin workbench
e2e/run.sh e2e/specs/workbench/*.py
```

The Workbench and Codex demos link `quark-terminal`, so building them
needs Zig 0.16 (see [Terminal](terminal.md#building)). The runner starts
apps without arguments; a spec sets its app's environment with header
lines, applied before launch:

```python
# quark-e2e-env: QUARK_WORKBENCH_SCENARIO=stress
```

It also exports `QUARK_SYNTAX_PACKS=target/syntax-packs` when that
directory exists, so code shows highlighted once the
[grammar packs](syntax-packs.md#building-packs) are built.
[theme_motion.py](../../e2e/specs/workbench/theme_motion.py) reads
screenshots with Pillow (`python3-pil`), which the runner does not check
for.

[e2e/quark_e2e.py](../../e2e/quark_e2e.py) gives a spec two clients:

- `Cua` drives the app as a computer-use agent does: snapshots of the
  AT-SPI tree, clicks by element or id (`press`, `press_id`), keys, the
  clipboard, and screenshots.
- `atspi_tree` reads the raw AT-SPI tree over D-Bus for what cua does not
  report: states such as focused, attributes such as `id`, and the Text
  interface.

Specs wait by polling observable state against a deadline (`wait_for`),
never by sleeping. Each starts with a docstring naming the regression it
catches; [greet_click.py](../../e2e/specs/hello_ui/greet_click.py) is a
short one. A failed spec leaves `screen.png`, `tree.txt`, `cua-tree.txt`,
and every log under `target/e2e/artifacts/<example>-<behavior>/`, with
`meta.txt` naming the spec, its environment, and the screen, and CI
uploads them. `QUARK_E2E_KEEP=1` keeps the artifacts of passing specs
too, for reviewing what the app looked like.

The specs run on Linux only, in the `e2e` workflow
([.github/workflows/e2e.yml](../../.github/workflows/e2e.yml)).

## Platform services

`crates/quark-app/tests/platform_smoke.rs` opens a real window on macOS
and Windows CI runners and walks the native menu bar and accelerators, a
menu pick, the badge, always on top, edit roles, and (macOS) a `kAEGetURL`
deep link. Linux has no native menu bar and its CI test job no display, so
it skips there; the `forward_url` end-to-end spec covers single instance
handoff on Linux.
