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
Upstream's generated layout tests (`tests/generated`) are not published
either, so the tests here are the crate's own unit tests plus the
differential test the patches add.
