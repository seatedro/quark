# quark-macros

Procedural macros for [Quark](../../README.md), re-exported by the `quark`
crate:

- `view!` lowers JSX-like markup to the builder calls in scope, usually
  quark-ui's `div()`, `text()`, and their methods.
- `#[derive(Store)]` generates a store with one `quark::reactive::Signal`
  per field.

The syntax is summarized in [quark's ARCHITECTURE.md](../quark/ARCHITECTURE.md),
and [tests/view_macro.rs](tests/view_macro.rs) covers each lowering rule.
