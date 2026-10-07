# quark-syntax

Tree-sitter syntax highlighting for [Quark](../../README.md) code blocks.
No grammar is compiled in: each language is a pack (a shared library, its
queries, and a manifest) that a `GrammarStore` loads from local pack
directories the app trusts or, with the `download` feature, from a signed
index. Code renders plain until its grammar is available.

Features: `engine` (the tree-sitter runtime and local packs) and `download`
(fetching packs over HTTPS with Ed25519-verified indexes). Without `engine`
the crate has no dependencies and every lookup falls back to plain text.

`highlight` runs synchronously; `HighlightWorker` runs on a background
thread and drops requests a newer generation superseded, which suits code
that is still streaming.

Build packs with `cargo run -p syntax-pack -- build`. The
[guide](../../docs/guide/syntax-packs.md) covers configuration, the pack
format, publishing an index, and the threat model.
