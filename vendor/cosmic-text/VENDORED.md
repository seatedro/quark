# Vendored cosmic-text

Upstream: cosmic-text 0.15.0 from crates.io. Wired in through
`[patch.crates-io]` in the workspace `Cargo.toml`, and listed as a
workspace member so CI formats, lints, and tests the patch. glyphon
re-exports cosmic-text, so the patch reaches quark-text and quark-render
alike and `FontSystem` stays one type.

The first commit that added this directory is the crate as published,
minus `.cargo-ok`, `.cargo_vcs_info.json`, `.gitattributes`, `.github`,
`.gitignore`, `Cargo.lock`, `Cargo.toml.orig`, `CHANGELOG.md`, `deny.toml`,
the shell scripts, and the `benches`, `fonts`, `sample`, `screenshots`, and
`tests` directories. Its `Cargo.toml` drops the matching `[[bench]]`,
`[[test]]`, dev-dependency, and test-profile entries, and allows the four
clippy lints upstream's code trips under the workspace's `-D warnings`. It
also drops the `vi` feature with its `modit`, `syntect`, and
`cosmic_undo_2` dependencies and the `src/edit/vi.rs` and
`src/edit/syntect.rs` modules gated on it: quark does not use them, and a
workspace member's optional dependencies otherwise land in `Cargo.lock`.
`git diff` against that commit shows the whole patch.

## Patches

Each is a separate commit after the import and could go upstream as its
own pull request.

- Shape plan cache (`src/shape.rs`, `src/font/system.rs`): the cache of
  harfrust shape plans keeps the 128 most recently used plans instead of
  the 6 most recently added, and `FontSystem::set_shape_plan_capacity`
  changes the size. Font fallback shapes a run with each candidate font
  until its glyphs are found, so a script no font covers needs a plan per
  font in the fallback chain (69 for quark-text's bidi and emoji
  paragraph with vendored fonts only, about 2 KB each). Six plans rebuilt
  every one of them for every such run: 1,150 of the paragraph's 1,350
  allocations. Lookup searches from the most recently used end and moves
  a hit there, so the plans plain text keeps using survive a fallback
  walk. Upstream could keep its default of 6 and take the promotion and
  setter. Remove once upstream caches plans by recency with a capacity
  quark-text can raise; `layout_with_evicting_shape_plan_cache_matches_cold_layout`
  checks the cache, and the bidi and emoji budget in
  `quark-text/src/alloc_budget.rs` checks the size.
