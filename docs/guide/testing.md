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
[Performance model](performance.md#allocation-budgets) lists the budgets
the adapter is held to.

## Beyond unit tests

| Tool | Covers | Where |
|---|---|---|
| `proptest` | Invariants of pure logic: selection order and copy, caret round trips, spring settling, virtual list offsets | The module's test block; failing cases are kept in `proptest-regressions/` |
| `cargo-fuzz` | Parsers and decoders of untrusted input: markdown, text layout of arbitrary strings, unified diffs | [fuzz/](../../fuzz), with committed corpora; CI replays the `text_layout` and `markdown` corpora (`-runs=0`) |
| Miri | Undefined behavior in `quark`'s own `unsafe` and `bytemuck` casts | CI `miri` job, `cargo miri test -p quark` |
| Kani | Bounded proofs of small cores without `HashMap`: the Fenwick tree | `#[cfg(kani)]` harnesses in [fenwick.rs](../../crates/quark/src/fenwick.rs) |
| CommonMark examples | Every spec example through the markdown parser and its integrity check | CI `fuzz` job, report only |
| End-to-end specs | Real example apps through AT-SPI and cua on Xvfb | [e2e/](../../e2e); see [Accessibility and automation](accessibility-and-automation.md#end-to-end-specs-with-cua) |

A fuzzer crash becomes a unit test with the minimized input. The Linux CI
job also runs clippy with `-D warnings` and a `cargo check` with every
optional feature enabled at once (except `profile-tracy`, which conflicts
with `profile-puffin`).
