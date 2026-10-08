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

## Dropping the vendor

Once glyphon can prepare already positioned glyphs (or quark draws glyphs
from its own atlas):

1. Delete `vendor/glyphon`, its `members` entry, and its line in the
   `[patch.crates-io]` section of the workspace `Cargo.toml`.
2. Move `quark-render`'s positioned path to the upstream API, then run
   `cargo test -p quark-render --features headless-render`:
   `positioned_text_draws_the_buffer_paths_pixels` compares both paths.
