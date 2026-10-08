# Vendored taffy

Upstream: taffy 0.9.2 from crates.io (DioxusLabs/taffy commit
8f30e394106af09dbf131d184e3cff25d20cc207). Wired in through
`[patch.crates-io]` in the workspace `Cargo.toml`, and listed as a
workspace member so CI formats, lints, and tests the patch.

The first commit that added this directory is the crate as published,
minus `.cargo-ok`, `.cargo_vcs_info.json`, `Cargo.lock`, and
`Cargo.toml.orig`. Its `Cargo.toml` drops the `[profile.release]` section
(Cargo ignores a member's profiles and warns about it) and allows the
lints upstream's code trips under the workspace's pinned nightly with
`clippy -D warnings`. `rustfmt.toml` restores upstream's formatting
settings, which the published crate does not ship, so `cargo fmt --all`
leaves upstream's code alone. `git diff` against that commit shows the
whole patch.

The published crate ships no license file; its `Cargo.toml` declares MIT.
`LICENSE.md` is copied from upstream at commit `8f30e394` (the v0.9.2
release). Upstream's generated layout tests (`tests/generated`) are not
published either, so the tests here are the crate's own unit tests plus a
differential test. Upstream's full test suite, including all 2,060
generated layout tests, also passes against the patched `src` (checked by
copying it over a checkout of `8f30e394` and running `cargo test --tests`);
rerun that after changing a patch.

## Differential test

`src/compute/differential_tests.rs` lays out flex growth and shrinkage,
min and max clamping, wrapping (definite and under min-content), baseline
alignment, percentages, nested and reversed containers, auto margins,
block flow with collapsing margins, blocks sized by their content around
flex containers, aspect ratios, scroll containers, and absolutely
positioned and hidden children through `TaffyTree`, once with
`src/compute/upstream_flexbox.rs` and `upstream_block.rs` (byte for byte
the published `flexbox.rs` and `block.rs`, compiled only for tests) and
once with the patched code, and asserts every node's unrounded and
rounded layout is identical. The patched pass lays every case out in one
tree, twice, so each layout reuses storage other shapes left behind. A
test-only arm in `TaffyView::compute_child_layout` routes to the copies.
It is test scaffolding, not part of any patch.

## Patches

Each is a separate commit after the import and could go upstream as its
own pull request.

- Unfrozen items (`src/compute/flexbox.rs`): `resolve_flexible_lengths`
  filters the line's items for unfrozen ones in each step of its freeze
  loop, where it collected them into a `Vec<&mut FlexItem>` on every pass.
  No step before the freeze changes frozen state and the freeze changes an
  item's only after visiting it, so each step visits the same items in the
  same order and sums in the same order. Remove once upstream stops
  collecting there.
- Flexbox and block storage (`src/compute/flexbox.rs`,
  `src/compute/block.rs`, `src/tree/traits.rs`,
  `src/tree/taffy_tree.rs`): `compute_flexbox_layout` and
  `compute_block_layout` take their items (and flex lines) from
  `FlexboxScratch` and `BlockScratch` storage that a tree provides through
  new `LayoutFlexboxContainer::flexbox_scratch` and
  `LayoutBlockContainer::block_scratch` methods, where they collected new
  vectors per container per layout. The default methods return `None` and
  allocate as before; `TaffyTree` keeps one of each. A container's items
  stay in use while its children are laid out, so the storage is a stack
  of frames: a computation pops one, nested computations pop and push
  theirs, and it pushes its own back, so each nesting depth keeps reusing
  the same frame. Flex lines hold their items as index ranges into the
  frame's items, where they borrowed slices, so the line vector has no
  lifetime and can be kept. These were all of the terminal demo's taffy
  allocations: 16 per changed frame, 58 for a 30-row redraw. Remove once
  upstream retains that storage; the differential test checks the
  geometry, and quark-ui's `warmed_relayout_allocates_nothing` and the
  terminal demo budgets check the allocations.

- Measuring blocks (`src/compute/block.rs`, `src/compute/mod.rs`,
  `src/tree/taffy_tree.rs`, `src/lib.rs`): a block container measured in
  `RunMode::ComputeSize` measures its in-flow flex and grid containers
  where it laid them out, and leaves their layouts alone. Laying one out
  lays out all of its descendants at the inputs of the measurement, and
  leaves them there when a later final layout of an ancestor is answered
  from the cache. A flex or grid container's layout output carries no
  collapsible margins and never collapses through, so the block uses only
  its size, which measuring computes the same way. Blocks and leaves in a
  block are still laid out: their margins can collapse through them. New
  `compute_root_size` and `TaffyTree::compute_size_with_measure` compute
  the size `compute_root_layout` gives a root, in `ComputeSize` and with
  the same inputs, without writing any layout. quark-ui sizes cache
  boundaries with it (`Subtree::size` in
  `crates/quark-ui/src/element/layout.rs`): a boundary's root is a block
  around its content, so measuring it no longer lays the content out.
  Remove once upstream offers both; the differential test checks the
  geometry, `root_size_is_the_size_layout_gives_the_root` the new entry
  point, and quark-ui's `measured_subtrees_lay_out_as_laid_out_ones`
  the boundaries.

## Dropping the vendor

Once an upstream release lets a tree keep flexbox and block storage
across layouts (and stops collecting unfrozen items):

1. Delete `vendor/taffy`, its `members` entry, and its line in the
   `[patch.crates-io]` section of the workspace `Cargo.toml`.
2. Bump `taffy` in `[workspace.dependencies]` to that release.
3. Run quark-ui's `element::layout::tests` and the terminal demo's
   `a_changed_frame_stays_within_its_allocation_budget`; both fail if
   layout allocates again.

quark-ui calls `TaffyTree::compute_size_with_measure`, which the published
crate lacks. Until a release has an equivalent, keep that patch, or size
boundaries by layout again: `Subtree::size` falls back to `Subtree::layout`
when `measures` is false.
