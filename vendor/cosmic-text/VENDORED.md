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
- Borrowed span attributes (`src/attrs.rs`, `src/shape.rs`): shaping reads
  a span's attributes through `AttrsList::get_span_ref` and
  `defaults_ref`, which borrow the stored `AttrsOwned`, where
  `get_span` and `defaults` built an `Attrs` that clones the font
  features. Font matching gets an `Attrs` without features, which it does
  not read. With any font features set (quark-text's ligatures off), the
  clones allocated per glyph, per word, and per shaped run: 139 of the 146
  allocations of an 80-column row of words. An unused `defaults()` copy
  per ASCII word is gone. Remove once upstream stops cloning features in
  its shaping loops (or makes `Attrs` borrow them);
  `ligatures_off_reach_every_span_and_paragraph` checks the features still
  apply, and the ligatures-off budgets check the copies.
- Adjusted bidi levels (`src/shape.rs`): `ShapeLine::adjust_levels` writes
  a paragraph's whitespace-adjusted levels into a vector `ShapeBuffer`
  keeps, where it cloned `BidiInfo::levels` per paragraph. Remove once
  upstream reuses that storage; `wrapped_bidi_lines_paint_their_runs_in_visual_order`
  and `layout_bidi_paragraph_separators_start_new_lines` check the levels.
- Left-to-right bidi fast path (`src/shape.rs`): `ShapeLine::build` skips
  `unicode_bidi::BidiInfo` for a non-empty line with no character of
  class R, AL, AN, LRE, RLE, LRO, RLO, LRI, RLI, FSI, or B, and builds the
  one level-0 span the full pass would produce. Those are the classes that
  clear unicode-bidi's own pure-LTR flag (which makes it return the
  paragraph level everywhere), plus paragraph separators, which would split
  the line into several paragraphs. With no right-to-left character the
  paragraph level is 0, so the output is identical; the check is one
  table lookup per char. This removes the five unicode-bidi allocations
  below from every left-to-right line, the whole residual of a plain
  terminal row. Remove once upstream skips the pass the same way;
  `shape::tests::bidi_fast_path_matches_full_pass` compares both paths over
  each class, and the stream budget in `quark-text/src/alloc_budget.rs`
  checks the skip.

- Spare shape words (`src/shape.rs`): `ShapeSpan::build` keeps the words
  earlier spans left over in `ShapeBuffer`, up to 256, beneath the span's
  own, where it cleared them at the start of every span. A line reshaped
  with more words than it had (a rebuilt layout taking longer text, a
  `TextBlock` edit) took a new word and glyph vector for each extra word:
  20 allocations for a twelve-word line refilled after a one-word one,
  now 2. It takes the words shorter lines gave up instead. Remove once upstream
  keeps spare words across spans; quark-text's
  `block_edits_within_warmed_capacity_copy_no_text` budget checks it.
- Layout glyph reserve (`src/shape.rs`): `ShapeLine::layout_to_buffer`
  reserves each visual line's glyphs before pushing them (rounded up to
  the capacity pushing would have reached), where a new line's glyph
  vector started at one and grew push by push: six reallocations for an
  80-column row, four on every fresh terminal row. A debug assertion
  checks the count against the glyphs pushed. Remove once upstream
  reserves the line's glyphs; the terminal demo's allocation budgets
  check it.
- Storage measurement (`src/attrs.rs`, `src/shape.rs`,
  `src/buffer_line.rs`): `BufferLine::storage_bytes`,
  `ShapeLine::storage_bytes`, and `AttrsList::storage_bytes` report the
  heap bytes a line keeps at capacity, including shaping and layout kept
  unused for reuse, and attribute sets' features and heap family names
  (approximate; `AttrsList::storage_bytes` documents what it leaves out).
  quark-text's layout cache bounds its memory in bytes and cannot see
  those capacities otherwise. Remove once upstream reports retained
  storage; the cache's byte-limit tests in
  `quark-text/src/cache.rs` use it.

- Deferred monospace candidates (`src/font/fallback/mod.rs`): for a
  monospace default family, `FontFallbackIter` returns the default
  monospace font first and collects the other monospace candidates only
  when a second font is asked for, where it collected them for every word.
  The default font sorts before every candidate (it has no weight
  difference), so it came first whatever they were, and without the
  `monospace_fallback` feature every font's codepoint list is empty, so
  every word of monospace text looked up every monospace face's font and
  codepoint counts: about 700 ns a word, over half the time of laying out
  a terminal row.
  The candidates and their order are unchanged. Remove once upstream
  collects candidates lazily;
  `fallback::tests::deferred_monospace_candidates_keep_the_fallback_order`
  compares the whole fallback order against collecting them first.
- ASCII table skips (`src/shape.rs`): `shape_run`'s script scan returns at
  once for an ASCII run (ASCII is all Common or Latin, which it skips),
  and the left-to-right bidi check classes ASCII chars without the table
  (only the paragraph separators among them send a line through the full
  pass). Each table search cost about as much as shaping the char: 17% of
  a terminal row. Remove once upstream skips them;
  `shape::tests::ascii_runs_collect_the_scripts_a_full_scan_does` and
  `bidi_fast_path_matches_full_pass` check every ASCII char.
- Shaped run memo (`src/run_memo.rs`, `src/shape.rs`,
  `src/font/system.rs`): advanced shaping answers runs of up to 16 bytes
  from a 256-slot direct-mapped memo of runs it shaped, keyed by what
  decides their glyphs (text, direction, and the family, stretch, style,
  weight, and features at the run's start) and recomputing what each glyph
  copies from its own attributes (letter spacing, color, weight, metadata,
  flags, metrics). A terminal row's words are mostly blanks and words it
  repeats, and each cost a fallback walk and a harfrust shape. The memo is
  allocated once (about 140 KB), on first use, so runs kept later allocate
  nothing; `FontSystem::db_mut` clears it, and
  `FontSystem::set_shape_run_memo` turns it off. Runs whose glyphs carry
  letter spacing are not kept, so the advance kept is the bare one. With
  the `shape-run-cache` feature, upstream's cache replaces it. Remove once
  upstream caches shaped words without allocating per lookup (its
  `shape-run-cache` copies the text and attributes into a new key for
  every run); quark-text's `memoized_shaping_*` and
  `memoized_runs_take_each_glyphs_own_attributes` tests compare layouts
  and shaped lines with the memo on and off.

## Not patched: bidi analysis

For lines the fast path above does not take, `ShapeLine::build` runs
`unicode_bidi::BidiInfo::new`, and unicode-bidi 0.3.18 has no way to reuse
storage: `BidiInfo` owns its vectors and every constructor builds new ones.
That costs five allocations per paragraph (`original_classes`, `levels`,
the `processing_classes` copy, `paragraphs`, and the per-paragraph flags)
and more for text with RTL runs or isolates (the explicit-embedding status
stack, level runs, isolating run sequences, and bracket pairs).

Removing them needs an upstream unicode-bidi API rather than a quark copy
of the algorithm: a `BidiInfo` (or `ParagraphBidiInfo`) that can be
recomputed in place for new text, keeping its vectors, such as
`BidiInfo::reset(&mut self, text, default_para_level)` or a
`BidiStorage` value passed to `new_with_storage`, with the internal
scratch (status stack, level runs, run sequences, bracket pairs) kept in
the same storage. The `BidiInfo<'text>` borrow of the text would have to
move to the call (or the storage be separate from the borrowing view) so
one value outlives each line's text. cosmic-text would then keep that
storage in `ShapeBuffer`. unicode-bidi's `smallvec` feature only moves
small level-run and run-sequence lists inline, so it would not remove the
five vectors.

## Dropping the vendor

Once upstream carries the patches above (or quark-text accepts the
allocations they remove):

1. Delete `vendor/cosmic-text`, its `members` entry, and its line in the
   `[patch.crates-io]` section of the workspace `Cargo.toml`.
2. Point `layout_with_evicting_shape_plan_cache_matches_cold_layout`'s
   `set_shape_plan_capacity` call at upstream's equivalent, then run
   `cargo test -p quark-text`: the allocation budgets in
   `src/alloc_budget.rs` show which patch upstream lacks.
- Thickening (`src/glyph_cache.rs`, `src/swash.rs`): a `THICKEN` cache key
  flag rasterizes outlines with swash's embolden at a fiftieth of the
  font size, half a pixel at 26 pixels per em. quark-text sets it for
  `TextStyle::thicken`, which a terminal uses for Ghostty's
  `font-thicken`. Upstream has no equivalent; it could go up as a
  general faux-bold strength option.
- Linear-corrected blending flag (`src/glyph_cache.rs`): a
  `LINEAR_CORRECTED` cache key flag with the background's luminance in
  bits 8 to 15 (`CacheKeyFlags::linear_corrected`, `blend_background`).
  Rasterization ignores it; it travels with the glyph to the vendored
  glyphon, which blends its edges as Ghostty's `linear-corrected` does.
  quark-text sets it for `TextStyle::linear_correction`.
- Rendering without a font system (`src/swash.rs`): `SwashFace` holds a
  face's shared bytes and swash key, and `SwashCache::render_face_into`
  draws a glyph of it into a reused `SwashImage` from explicit size,
  hinting, variation values, offset, embolden, skew, and color sources.
  quark-render's swash rasterizer draws the exact font instance quark-text
  prepared (every axis, not only `wght`) through it, with no
  `&mut FontSystem`; its differential test checks the bitmaps equal
  `get_image_uncached`'s. Upstream could take it as a lower-level entry
  beside `get_image`.
