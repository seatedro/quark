# quark-macros

Procedural macros for [Quark](../../README.md), re-exported by the `quark`
crate:

- `view!` lowers HTML-like markup to the builder calls in scope, usually
  quark-ui's `div()`, `text()`, and their methods.
- `#[derive(Props)]` gives a component a typed builder for `<Component ..>`.
- `#[derive(Store)]` generates a store with one `quark::reactive::Signal`
  per field.

The syntax is documented in [Writing views](../../docs/guide/writing-views.md),
and [tests/view_macro.rs](tests/view_macro.rs) covers each lowering rule.
