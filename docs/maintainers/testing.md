# Testing quark

How quark's own tests and CI run. Rules for writing tests:
[TEST_BIBLE.md](../../TEST_BIBLE.md). Testing an app:
[guide](../guide/testing.md). End-to-end specs:
[e2e/README.md](../../e2e/README.md).

## Commands

```bash
cargo test --workspace --features quark-ui/integrity-checks   # what CI runs
cargo test -p quark-ui virtual_list                           # one module, by name filter
cargo test -p quark-app --example hello_ui                    # an example's test module
cargo test --doc -p quark-app                                 # doctests
```

- `integrity-checks` makes data structures verify their whole state after
  every mutation, not just the touched entries. Quadratic on large
  documents; local runs usually leave it off.
- Without it, `BlockOrder`, the animation table, the row table, and the
  block document check only the entries a mutation touched.
- Syntax tests read packs from `target/syntax-packs`, or
  `$QUARK_SYNTAX_TEST_PACKS`.

## CI

- Linux: clippy with `-D warnings`, and `cargo check` with every optional
  feature at once (except `profile-tracy`, which conflicts with
  `profile-puffin`).
- GPU tests run with `QUARK_REQUIRE_GPU=1`: Linux on lavapipe, Windows on
  WARP.
- Debug `hello_ui` launches with `QUARK_EXIT_AFTER_FRAMES` as a smoke test.
- [platform_smoke.rs](../../crates/quark-app/tests/platform_smoke.rs) opens
  a real window on macOS and Windows runners: native menu bar and
  accelerators, a menu pick, the badge, always on top, edit roles, and
  (macOS) a `kAEGetURL` deep link. It skips on Linux (no native menu bar, no
  display).

| Tool | Covers | Where |
|---|---|---|
| `proptest` | Invariants of pure logic: selection order and copy, caret round trips, spring settling, virtual list offsets | The module's test block; failing cases kept in `proptest-regressions/` |
| `cargo-fuzz` | Parsers and decoders of untrusted input: markdown, text layout of arbitrary strings, unified diffs | [fuzz/](../../fuzz) with committed corpora; CI replays `text_layout` and `markdown` (`-runs=0`) |
| Miri | UB in `quark`'s own `unsafe` and `bytemuck` casts | CI `miri` job: `cargo miri test -p quark` |
| Kani | Bounded proofs of small cores without `HashMap`: the Fenwick tree | `#[cfg(kani)]` harnesses in [fenwick.rs](../../crates/quark/src/fenwick.rs) |
| CommonMark examples | Every spec example through the markdown parser and its integrity check | CI `fuzz` job, report only |

- A fuzzer crash becomes a unit test with the minimized input.

## Frame allocation budgets

[frame_budget.rs](../../crates/quark-app/src/frame_budget.rs) runs whole
frames through the adapter:

| Frame | Budget |
|---|---|
| Cached list repeating the last frame, with or without a screen reader | 0 allocations |
| Tracked scroll container repeating the last frame | 0 |
| List frame where one cached row changed | 40 per changed row |
| Markdown document repeating the last frame | 16 |
| Document frame after text streams into one message | 320 |

- `hello_ui` rebuilds without a cache boundary; its repeated frame is held
  to 30.
- The Workbench ([perf.rs](../../examples/workbench/tests/perf.rs)): budgets
  64 for a repeated frame, 512 for one streamed answer chunk with its frame.
  Measures 45 (53 with a screen reader) and 337; ceilings sit just above, so
  a regression fails while still under budget.
- Print call sites: `cargo test -p quark-app report_ -- --ignored
  --nocapture`.

## Coverage gaps

- Docking: headless tests and the X11 e2e specs cover X11-style desktops.
  The Windows, macOS, and Wayland drag paths have no automated coverage.
