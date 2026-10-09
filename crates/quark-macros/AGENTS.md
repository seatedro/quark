# AGENTS.md

Guidance for agents working in `crates/quark-macros`. The root `AGENTS.md`
still applies; this node covers the proc-macro boundary that is easy to break
with plausible-looking changes.

## Purpose And Scope

`quark-macros` owns compile-time parsing and lowering for
`#[derive(Store)]` and `view!`.

It does not own runtime reactivity, scene primitives, layout behavior, app
actions, design tokens, or renderer behavior. It emits Rust method chains that
the `quark` crate and quark-ui's element builders must provide.

## Related Context

- Root Rust and change-hygiene rules: `../../AGENTS.md`
- Runtime UI/reactivity contracts: `../quark/AGENTS.md`
- Macro syntax examples and reactive notes: `../quark/ARCHITECTURE.md`

## Core Contracts

- Keep dependencies limited to proc-macro tooling unless there is a strong
  reason: `syn`, `quote`, and `proc-macro2` are the intended dependency set.
- `#[derive(Store)]` only supports non-generic structs with named fields.
  Tuple structs, unit structs, enums, unions, and generics should fail with
  clear compile errors.
- Store leaves become `::quark::reactive::Signal<T>`. `#[store(flatten)]`
  maps a named `Foo` field to `FooStore`; `#[store(skip)]` omits the field from
  the generated store.
- Generated stores derive `Clone`, `Copy`, and `Debug` and expose `new`.
  `new_default` exists only with struct-level `#[store(default)]`, so
  non-`Default` structs derive cleanly. `snapshot()` is generated only when
  no field is skipped.
- `view!` syntax is documented in `docs/guide/writing-views.md`; keep it
  in step with the macro. Every attribute lowers to a method call whose
  name carries the attribute's span, so the builder API stays the single
  list of attributes; do not add attribute allow-lists. `on:`, `aria-`,
  and `role` are the only mapped names (tables in `src/view/emit.rs`).
- `class="..."` lowers through the tables in `src/classes.rs`. It is not
  CSS. `__class_vocabulary!` compiles every entry against the builders in
  quark-components' `tests/view_equivalence.rs`, and a unit test keeps the
  generated reference in the guide current (`QUARK_BLESS=1` rewrites it).
- `<Name ..>` lowers to `#[derive(Props)]`'s `Name::builder()...build()`;
  `<Name(args) ..>` to `Name::new(args)` plus builder calls. Required props
  are type-state markers whose bounds sit on `build`, so a missing one is
  E0277 with the `on_unimplemented` message.
- Multi-child `if`/`match` branches and fragments spread children into the
  parent, never into a wrapper `div()`: wrapping changes layout.
- Control flow among an element's children lowers to statements that call
  `.child(..)` on the parent builder in place (`Sink` in `emit.rs`). Do not
  collect branches or loop bodies into a `Vec`: views run every frame and
  the frame budget tests count allocations.
- `<name(args)>` (lowercase, with arguments) calls the function `name` in
  scope and lowers like a builder component.
- Reactive attributes use `name={@signal}` and lower to `cx.read(signal)`.
- Every element lowers to its builder chain plus `.into_any()`, except the
  root of `view! { -> Type, .. }`, which stays the builder (ascribed to
  `Type`) so helpers can return a `Div` for callers to extend.
- Input the macro cannot lower faithfully is a spanned compile error, and
  any error replaces the whole expansion.
- Emitted paths are limited to `::core`, `::std`, and `::quark`; everything
  else is a method call or a name the call site has in scope.

## Usage Patterns

- Prefer extending the parser AST and code generation deliberately over ad hoc
  token-string manipulation.
- Preserve hygienic internal names like `__quark_children` and `__w`; avoid
  names that can collide with user bindings unless they are already part of the
  macro convention.
- Cover lowering changes with runtime tests in `tests/view_macro.rs`, the
  builder equivalence in quark-components' `tests/view_equivalence.rs`, and
  rejected inputs with trybuild cases in `tests/ui/`.

## Anti-Patterns

- Do not add runtime behavior to this crate. Proc macros should parse, validate,
  and emit code only.
- Do not assume class names have browser semantics.
- Do not reintroduce wrapper nodes for multi-child control flow.
- Do not broaden macro syntax without compile-error tests or representative
  emission tests.
- Do not hide unsupported Rust shapes behind partial generated code; fail
  clearly at compile time.

## Validation

- Focused macro tests: `cargo test -p quark-macros`
- Store derive integration: `cargo test -p quark --test store_derive`
- UI lowering fallout: run `cargo test -p quark` when macro output changes
  exported contracts.

## Maintenance

Keep syntax facts here and runtime facts in `../quark/AGENTS.md`. If a macro
change requires a new builder method in quark-ui, update its API and tests
in the same change.
