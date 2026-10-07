# AGENTS.md

Guidance for agents working in `crates/quark-macros`. The root `AGENTS.md`
still applies; this node covers the proc-macro boundary that is easy to break
with plausible-looking changes.

## Purpose And Scope

`quark-macros` owns compile-time parsing and lowering for
`#[derive(Store)]` and `view!`.

It does not own runtime reactivity, scene primitives, layout behavior, app
actions, design tokens, or renderer behavior. It emits Rust method chains that
the `halogen` crate and Diffy's UI builders must provide.

## Related Context

- Root Rust and change-hygiene rules: `../../AGENTS.md`
- Runtime UI/reactivity contracts: `../halogen/AGENTS.md`
- Macro syntax examples and reactive notes: `../halogen/ARCHITECTURE.md`

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
- `view!` supports optional `scale,`, built-in tags (`div`, `text`, `icon`,
  `spacer`, `fragment`), component tags, `if` / `else if` / `else`, `for`,
  `match`, raw expressions, optional expressions, and spread expressions.
  An `if` chain with no final `else` yields no child when nothing matches.
- Reactive attributes use `name={@signal}` and lower to `cx.read(signal)`.
  Call sites must provide a `cx` with the expected `read` method.
- `class="..."` lowers to builder method calls. It is not CSS and must stay
  aligned with Diffy's builder methods.
- Multi-child `if` branches and fragments must spread children into the parent,
  not wrap them in a bare `div()`. Wrapping changes layout and percentage-size
  resolution.
- Components know nothing about specific types. `<Button(a, b)>` lowers to
  `Button::new(a, b)`; every attribute becomes a builder call.
- `<.method>` inside a component is a slot: each child becomes
  `.method(child)`. Slots take no attributes and need at least one child.
- Input the macro cannot lower faithfully (non-identifier classes, extra
  `<text>` children, children of `icon`/`spacer`, attributes on
  `spacer`/`fragment`/slots) is a spanned compile error. Errors replace the
  whole expansion.
- Auto-scaling applies only to known spatial attributes when the optional
  `scale` identifier is supplied.

## Usage Patterns

- Prefer extending the parser AST and code generation deliberately over ad hoc
  token-string manipulation.
- Preserve hygienic internal names like `__quark_children` and `__w`; avoid
  names that can collide with user bindings unless they are already part of the
  macro convention.
- Cover lowering changes with runtime tests in `tests/view_macro.rs` and
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
- Store derive integration: `cargo test -p halogen --test store_derive`
- UI lowering fallout: run `cargo test -p halogen` when macro output changes
  exported contracts.

## Maintenance

Keep syntax facts here and runtime facts in `../halogen/AGENTS.md`. If a macro
change requires a new builder method in Diffy, update the app-side API and tests
in the same change.
