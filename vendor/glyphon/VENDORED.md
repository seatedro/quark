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
