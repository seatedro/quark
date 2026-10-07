# AGENTS.md

Guidance for agents working in `crates/quark`, the core crate of the quark
UI framework. Repo-wide rules (including `TEST_BIBLE.md`) still apply.

## Purpose And Scope

This crate provides the renderer-free foundation: fine-grained reactive
signals, geometry, the hit table, the semantic frame, focus, document
selection, the animation table, the Fenwick tree, style data, scene
primitives, and re-exports for the `view!` and `Store` macros.

It does not own design tokens, elements, the renderer, text shaping, the OS
event loop, or app state. Those live in `quark-ui`, `quark-render`,
`quark-text`, `quark-app`, and the app itself.

## Related Context

- Test rules: `../../TEST_BIBLE.md`
- Core architecture overview: `ARCHITECTURE.md`
- Macro parser and lowering contracts: `../quark-macros/AGENTS.md`
- Builder/style methods that `view!` calls: `../quark-ui/src/style.rs`

## Core Contracts

- Read `ARCHITECTURE.md` before changing signals, scene primitives, style data,
  hit-testing, or the `view!` contract.
- `Signal<T>` is a small `Copy` handle into `SignalStore`; values live in arena
  slots indexed by `{ index, generation }`.
- Reactive propagation has three states: `Clean`, `Check`, and `Dirty`. Writes
  mark the source dirty and transitive subscribers for lazy checking; memo reads
  settle the graph.
- `create_memo` relies on `PartialEq` to stop invalidation cascades. Avoid memos
  whose values churn every frame.
- `any_dirty()` is the frame-loop signal that something was written since the
  last `clear_dirty()`. Clear it after rendering, not before consumers observe
  the redraw need.
- Re-entrant signal access is a bug. The store intentionally panics on nested
  mutable/immutable borrow conflicts instead of hiding feedback-loop writes.
- `scene::Scene` is pure data. This crate defines primitives; `quark-render`
  decides how to batch, cache, clip, and draw them.
- `hit` is generic over the host click-result payload. Keep action routing in
  the app; Quark should only resolve geometry, z-order, blocking, cursor, and
  identity data.
- `style::ElementStyle` is pure layout/visual data. The fluent `Styled`
  helpers and design tokens live in `quark-ui`.
- `class="..."` in `view!` is not CSS. It lowers to Rust builder method calls
  such as `.flex_row()` or mapped aliases from `quark-macros`.

## Usage Patterns

- Fix shared UI contract bugs in Quark when the bug is in the primitive
  semantics, not at one app call site.
- Keep primitive structs compact, cloneable when useful, and renderer-agnostic.
- Prefer generic payloads and identities at the Quark boundary so the app can
  own policy and actions.
- Add tests around behavior that affects layout, clipping, hit ordering,
  reactivity propagation, or macro-generated store access.

## Anti-Patterns

- Do not reason from browser CSS behavior. Verify the actual Taffy/style/paint
  path and the macro lowering before claiming semantics.
- Do not add wgpu, glyphon, winit, app `Action`, or theme-token dependencies to
  Quark.
- Do not patch only a component in `quark-components` or `quark-ui` if the
  underlying primitive contract here is wrong.
- Do not introduce reactive effects that write during their own execution
  without a deferred-write design.
- Do not let scene or hit-test construction allocate unbounded data per frame
  without a clear app-level reuse story.

## Validation

- Focused crate tests: `cargo test -p quark`
- Macro contract fallout: `cargo test -p quark-macros` and then
  `cargo test -p quark`
- Changes that reach elements or the app: also run the relevant `quark-ui` or
  `quark-app` tests, or an example under Xvfb.

## Maintenance

Keep cross-cutting UI rules at the root or in `ARCHITECTURE.md` when they apply
beyond this crate. Keep this node focused on the contracts an agent needs before
editing Quark itself.
