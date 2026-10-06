# Test bible

Every test in quark must earn its place. A test exists to catch a specific
regression in observable behavior. If you cannot name the regression it
catches, do not write it.

## Rules

1. **One behavior per test.** Set up through the public API and assert on
   observable output: strings, byte offsets, rects, tree dumps, pixels. Do
   not assert on private fields or call counts.
2. **Use domain test helpers.** Give types small `#[cfg(test)]` helpers that
   build state and dump it as text. A test should read as input, action,
   expected text.
3. **Data structures check their own invariants.** Non-trivial
   data structures (`BlockOrder`, `AnimationTable`, row height trees, text
   layout columns, the node store) get a `verify_integrity(&self) ->
   Result<(), IntegrityError>`, called through `debug_assert!` after
   mutations and compiled out of release builds. Tests then drive
   operations and let the integrity check do the deep asserting.
4. **Repeated cases become properties or tables.** When a test
   would repeat with different numbers, write a `proptest` property or a
   table-driven test instead of near-duplicate functions.
5. **Deterministic or nothing.** No sleeps and no wall clock (pass `now_ms`
   in). Use vendored fonts and an explicit scale factor. GPU tests run only
   when a device exists, and `QUARK_REQUIRE_GPU=1` turns a missing device
   into a failure.
6. **Fast.** A unit test runs in milliseconds. A crate's test suite runs in
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
- Is it the only test covering this behavior?

A test that breaks on behavior-preserving refactors, or duplicates another
test, gets deleted.
