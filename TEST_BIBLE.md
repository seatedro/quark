# Test bible

Every test in quark must earn its place. A test exists to catch a specific
regression in observable behavior. If you cannot name the regression it
catches, do not write it.

## Rules

1. **Tests live next to the code.** Put unit tests in a `#[cfg(test)] mod
   tests` at the bottom of the file under test. `tests/` directories hold
   only cross-crate pipeline tests (headless render, runner, accessibility
   tree end to end).
2. **The name states the behavior.** Use `unit_condition_outcome`, for
   example `block_order_prepend_keeps_existing_positions_ordered`. A failing
   test name should tell you what broke without opening the file.
3. **One behavior per test.** Set up through the public API and assert on
   observable output: strings, byte offsets, rects, tree dumps, pixels. Do
   not assert on private fields or call counts.
4. **Use domain test helpers.** Give types small `#[cfg(test)]` helpers that
   build state and dump it as text. A test should read as input, action,
   expected text.
5. **Every bug fix adds a regression test** that fails without the fix. Put
   the issue or commit in a comment above it.
6. **Data structures check their own invariants.** Non-trivial
   data structures (`BlockOrder`, `AnimationTable`, row height trees, text
   layout columns, the node store) get a `verify_integrity(&self) ->
   Result<(), IntegrityError>`, called through `debug_assert!` after
   mutations and compiled out of release builds. Tests then drive
   operations and let the integrity check do the deep asserting.
7. **Repeated cases become properties or tables.** When a test
   would repeat with different numbers, write a `proptest` property or a
   table-driven test instead of near-duplicate functions.
8. **Deterministic or nothing.** No sleeps and no wall clock (pass `now_ms`
   in). Use vendored fonts and an explicit scale factor. GPU tests run only
   when a device exists, and `QUARK_REQUIRE_GPU=1` turns a missing device
   into a failure.
9. **Fast.** A unit test runs in milliseconds. A crate's test suite runs in
   seconds. Run focused tests with a name filter while iterating.

## Verification beyond unit tests

| Tool | Use it for | Where |
|---|---|---|
| `proptest` | Invariants of pure logic: selection ordering and copy, text hit and caret round trips, spring settling, virtual list offsets | the module's test block |
| Kani | Small bounded cores that must hold for all inputs: Fenwick tree, `BlockOrder` index, selection repair | `#[cfg(kani)]` harnesses in the module |
| `cargo-fuzz` | Anything that parses or decodes untrusted input: Markdown, input event normalization, text layout of arbitrary strings | `fuzz/` with a committed, minimized corpus |
| Miri | Undefined behavior in quark's own `unsafe` and `bytemuck` casts | CI job over the crates that do not touch the GPU |
| Conformance suites | Behavior an external spec defines: CommonMark spec tests, AT-SPI tree through cua | CI job that reports without failing until the baseline passes |
| Headless render | Renderer behavior: draw order, borders, clipping | pixel probes at chosen coordinates, never whole-image goldens |

A fuzzer crash becomes a regression unit test with the minimized input.

## Banned

- Tests of derives, constructors, getters, `Default`, `Clone`, or `Debug`
  output.
- Tests that compute the expected value with the same formula as the code.
- Tests whose only assertion is `is_ok()` or "did not panic", except fuzz
  harnesses.
- Snapshots of whole structs, scenes, or images.
- Mocks of anything that can be constructed for real.
- Tests of third-party crates, or of code that does not exist yet.
- Tests written to raise coverage.

## Review checklist

Answer these for every new test. A "no" means rewrite or delete it.

- Would it fail if the behavior it names broke?
- Would it still pass after a refactor that keeps behavior the same?
- Does its name say what broke?
- Is it the only test covering this behavior?

A test that breaks on behavior-preserving refactors, or duplicates another
test, gets deleted.
