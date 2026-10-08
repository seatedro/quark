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
- Monospace fallback candidates (`src/font/fallback/mod.rs`,
  `src/font/system.rs`): `FontFallbackIter` collects a word's monospace
  candidates in a vector kept sorted largest first and pops from its end,
  where it used a `BTreeSet` it cleared per word. Clearing a `BTreeSet`
  frees its node, so every word of monospace text allocated one (19 per
  80-column terminal row of words); the vector keeps its storage. Order and
  the duplicate check are unchanged. Remove once upstream reuses the
  candidate storage; `mono_fallback_picks_nearest_weight_then_best_coverage`
  checks the order.
- Visual reorder scratch (`src/shape.rs`): `ShapeLine::reorder` fills a
  visual line's levels and reordered level runs into two vectors the
  `ShapeBuffer` keeps, where it collected two new vectors per visual line.
  Remove once upstream reuses reorder storage;
  `wrapped_bidi_lines_paint_their_runs_in_visual_order` checks the order.
- Fallback and feature scratch (`src/shape.rs`): `shape_run` and
  `shape_fallback` keep the missing-cluster lists, the fallback font's
  glyph vector, and the run's harfrust feature vector in `ShapeBuffer`
  instead of allocating them per run and per fallback font. Text that
  walks a long fallback chain allocated two vectors per font tried (148 of
  the bidi and emoji paragraph's 173 allocations after the plan cache),
  and with ligatures off every shaped word allocated a feature vector.
  Remove once upstream reuses that storage; the bidi and emoji budget
  checks it, and `layout_with_evicting_shape_plan_cache_matches_cold_layout`
  shapes fallback text after other fallback text.
