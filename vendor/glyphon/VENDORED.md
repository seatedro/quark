# Vendored glyphon

Upstream: glyphon 0.10.0 from crates.io. Wired in through
`[patch.crates-io]` in the workspace `Cargo.toml`, and listed as a
workspace member so CI formats, lints, and tests the patch.

The first commit that added this directory is the crate as published,
minus `.cargo-ok`, `.cargo_vcs_info.json`, `.github`, `.gitignore`,
`Cargo.lock`, `Cargo.toml.orig`, and the `benches`, `examples`, and
`samples` directories. Its `Cargo.toml` drops the matching `[[bench]]`,
`[[example]]`, and dev-dependency entries, and allows the two clippy lints
upstream's code trips under the workspace's `-D warnings`. `git diff`
against that commit shows the whole patch.

## Patch

- `PositionedGlyph` (`src/lib.rs`): a glyph's rasterization key, pixel
  position, color, and clip.
- `TextRenderer::prepare_glyphs` (`src/text_render.rs`): prepares
  positioned glyphs through the same atlas lookup, rasterization, and
  clipping as `prepare`, without reading a `Buffer`. quark-render uses it
  to color each glyph by its span, so a multi-colored text no longer needs
  a buffer copy shaped with those colors.
- `TextRenderer::upload` is public, and `prepare_glyphs` leaves the upload
  to the caller. The renderer's allocation budget covers preparation;
  wgpu's `write_buffer` staging allocates on every call.
- `#![recursion_limit = "256"]` (`src/lib.rs`): newer nightlies need it
  for wgpu's `Send` chain, as in quark-render.
- `prepare` and `prepare_glyphs` share the bounds clipping and glyph
  rasterization helpers they used to inline.
- Atlas growth copies (`src/text_atlas.rs`): atlas textures also have
  `COPY_SRC`, and `InnerAtlas::grow` copies the old texture into the
  corner of the larger one with `copy_texture_to_texture`, submitted on
  the queue at once. etagere's `grow` already keeps every allocation in
  place, so cached glyphs keep their atlas coordinates and nothing is
  rasterized or uploaded again. Upstream's re-rasterizing loop remains as
  the fallback for an old texture without `COPY_SRC`, which no atlas
  made here lacks.
- `TextAtlas::clear` (`src/text_atlas.rs`): drops every cached glyph
  and frees its space, keeping the textures. quark-render calls it when
  the text system changes, since a new font database reuses face ids.
- `TextAtlas::epoch` (`src/text_atlas.rs`): counts evictions and
  clears, the events that can move a cached glyph. quark-render draws a
  text run's vertices again without preparing them while it is unchanged.
- Draw offsets (`src/shader.wgsl`, `src/viewport.rs`, `src/cache.rs`,
  `src/text_render.rs`): a viewport has a second uniform of
  `MAX_DRAW_OFFSETS` pixel offsets, set with `Viewport::set_draw_offsets`
  (one buffer write, only when an offset changed).
  `TextRenderer::render_at` draws with offset `slot`, passing the slot as
  the first vertex (`4 * slot`), and the vertex shader moves every glyph
  by the offset its vertex index selects; `render` draws at slot 0.
  quark-render gives each run's renderer a slot and, when a run moved by
  whole pixels with its clip and places exactly the glyphs prepared moved
  alike, draws the prepared vertices at the new offset instead of
  preparing and uploading them again. `PositionedGlyph` also derives
  `PartialEq` and `Eq` for that comparison.
- `AtlasStats` and `TextAtlas::stats` (`src/text_atlas.rs`, counted in
  `src/text_render.rs`): glyph misses, evictions, growths, glyphs
  re-rasterized by a growth, and glyph bytes uploaded, for tests and
  devtools. quark-render exposes them as `Renderer::text_atlas_stats`.

## Dropping the vendor

Once glyphon can prepare already positioned glyphs (or quark draws glyphs
from its own atlas):

1. Delete `vendor/glyphon`, its `members` entry, and its line in the
   `[patch.crates-io]` section of the workspace `Cargo.toml`.
2. Move `quark-render`'s positioned path to the upstream API, then run
   `cargo test -p quark-render --features headless-render`:
   `positioned_text_draws_the_buffer_paths_pixels` compares both paths.
3. Keep the draw offsets patch, or have quark-render move kept glyph
   vertices on the CPU and upload them again:
   `scrolled_text_draws_the_pixels_of_text_prepared_again` covers it.
4. Check that the upstream atlas still grows without re-rasterizing, or
   keep that patch: `atlas_growth_rasterizes_no_cached_glyph_again` and
   `atlas_growth_keeps_cached_glyph_pixels` cover it, and the counters
   they read would need another source.
- Linear-corrected blending (`src/text_render.rs`, `src/shader.wgsl`): a
  glyph whose cache key has the vendored cosmic-text's `LINEAR_CORRECTED`
  flag passes it and the background luminance in the upper half of the
  vertex's content type (bit 1, bits 8 to 15). The fragment shader then
  maps mask coverage the way Ghostty's `linear-corrected` alpha blending
  does: it blends the foreground and background luminances in sRGB space
  and solves for the coverage that gives that luminance when the target
  blends linearly. A terminal uses it so text has Ghostty's weight.
- Encoded targets (`src/viewport.rs`, `src/shader.wgsl`,
  `src/text_render.rs`): `Viewport::set_encoded` marks a target that holds
  sRGB-encoded values (quark's web-compatible compositing). Glyph colors
  then stay encoded, color glyphs are encoded after sampling, and no
  coverage correction applies. `TextRenderer::set_target_format` swaps the
  renderer's pipeline for one targeting another format (the non-sRGB view
  of the same texture).
- Draw-time paint (`src/lib.rs`, `src/text_render.rs`, `src/cache.rs`,
  `src/shader.wgsl`): `PositionedGlyph::backdrop` asks for perceptual
  (linear-corrected) coverage over a backdrop known when drawing, overriding
  the cache key's flag, so a hover background change needs no new raster or
  layout. `PositionedGlyph::fill` points a monochrome glyph at one of its
  renderer's `GlyphFill`s (`TextRenderer::set_fills`, a uniform at bind
  group 2): a linear gradient or a shimmer band evaluated per pixel and
  multiplied by coverage. Both travel in a new `paint` vertex attribute.
