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
