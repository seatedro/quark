# Publish readiness

Quark is not ready for crates.io. Four problems block a publish outright:
the name `quark` is taken, `quark-text` is over the upload size limit, the
internal dependencies have no version requirements, and the vendored
`accesskit_unix` patch does not travel with published crates. Nothing has
been published. The results below come from `cargo package --list` and
`cargo package --no-verify`, run offline on this tree; verification, which
compiles each package, was skipped.

## Blockers

| # | Blocker | Affects | Evidence | Fix |
|---|---|---|---|---|
| 1 | The crate name `quark` is taken on crates.io (version 1.1.0, "Types for manipulating numeric primitives at the bit level", repository `ryanq/quark`, last updated 2019) | `quark`, and every crate that depends on it | `GET https://crates.io/api/v1/crates/quark` returns 200; the other eight names return 404 | Rename the core crate (for example `quark-core`) or obtain the name from its owner. The other eight names are free |
| 2 | `quark-text` packages to 15.1 MiB compressed; crates.io rejects uploads over 10 MB by default | `quark-text` | `cargo package --workspace --no-verify` in a copy with versions added: "Packaged 46 files, 19.6MiB (15.1MiB compressed)". `NotoColorEmoji.ttf` alone is 10.7 MB, `QuarkCJKFallback-Regular.otf` 3.7 MB | Move the emoji and CJK fonts into separate crates (each still near or over the limit for emoji), fetch them at build time, or request a size limit increase from the crates.io team |
| 3 | Workspace dependencies on Quark crates are `path` only | Every crate except `quark-macros`, `quark-syntax`, and `quark-diff` | `cargo package -p quark`: "all dependencies must have a version requirement specified when packaging. dependency `quark-macros` does not specify a version" (the same for `quark-text` on `quark`) | Add `version = "0.1.0"` beside each `path` in `[workspace.dependencies]` of the root `Cargo.toml` |
| 4 | `[patch.crates-io]` does not apply to dependents | `quark-app` (through `accesskit_winit`), and every app on Linux | Cargo applies `[patch]` only in the top-level workspace. A published `quark-app` resolves `accesskit_unix` 0.22.x from crates.io, without `GetRoleName` or the `id` attribute | See [The vendored accesskit_unix](#the-vendored-accesskit_unix) |

## Path dependencies that need versions

| Crate | Quark dependencies without a version |
|---|---|
| `quark` | `quark-macros` |
| `quark-text` | `quark` |
| `quark-render` | `quark`, `quark-text` |
| `quark-ui` | `quark`, `quark-render`, `quark-text`, `quark-syntax` (optional) |
| `quark-components` | `quark`, `quark-render`, `quark-ui`, `quark-diff`, `quark-syntax`, `quark-text` |
| `quark-app` | `quark`, `quark-render`, `quark-text`, `quark-ui` (optional) |

All come from `[workspace.dependencies]`, so one edit there fixes them.
With versions added, every crate packages; this is the publish order
cargo chose, which follows the dependency graph:

| Order | Crate | Files | Size (compressed) |
|---|---|---|---|
| 1 | `quark-macros` | 23 | 68.4 KiB (18.1 KiB) |
| 2 | `quark` | 24 | 180.4 KiB (48.9 KiB) |
| 3 | `quark-diff` | 14 | 107.6 KiB (29.4 KiB) |
| 4 | `quark-text` | 46 | 19.6 MiB (15.1 MiB) |
| 5 | `quark-render` | 10 | 254.5 KiB (61.8 KiB) |
| 6 | `quark-syntax` | 9 | 34.1 KiB (10.2 KiB) |
| 7 | `quark-ui` | 75 | 1.1 MiB (289.9 KiB) |
| 8 | `quark-components` | 44 | 545.8 KiB (135.9 KiB) |
| 9 | `quark-app` | 56 | 687.3 KiB (177.9 KiB) |

Dev-dependencies given only a `path` are dropped from the packaged
manifest. Two of them matter to anyone who runs the packaged tests:

- `quark-app`'s dev-dependency on itself with `headless-render` is dropped,
  so its examples' test modules and the `UiTestHarness` doctest in
  `src/lib.rs` need `--features test-support` (or `headless-render`) to
  compile from the package.
- `quark-macros`' dev-dependency on `quark` is dropped, so
  `tests/view_macro.rs` does not compile from the package.

Neither affects building or using the libraries.

## The vendored accesskit_unix

`vendor/accesskit_unix` is accesskit_unix 0.22.1 plus `GetRoleName`, a
`GetLocalizedRoleName` fallback, and an `id` attribute
([VENDORED.md](../vendor/accesskit_unix/VENDORED.md)). It reaches the build
only through `[patch.crates-io]` in the root `Cargo.toml`. For publishing,
that means:

- **Published crates do not carry it.** An app that depends on a published
  `quark-app` gets upstream accesskit_unix. Screen readers keep working;
  cua-driver sees blank roles and cannot find nodes by `id`, so cua-based
  automation of that app breaks unless the app adds the same patch to its
  own workspace.
- **It cannot be published under its name.** `accesskit_unix` belongs to
  the AccessKit project. It is also a workspace member, so
  `cargo publish --workspace` would try to upload it. Before any workspace
  publish, give `vendor/accesskit_unix/Cargo.toml` `publish = false` or
  publish crates one by one with `-p`.
- **The lasting fix is upstream.** Once an AccessKit release serves
  `GetRoleName` and an `id` attribute (or cua reads `AccessibleId`), the
  vendor directory and the patch go away; VENDORED.md lists the steps.
  Publishing a renamed fork would require forking `accesskit_winit` too,
  since that is what depends on `accesskit_unix`.

## Licenses

- Every crate declares `license = "MIT"` (through `[workspace.package]`),
  except `quark-text`, which now declares `MIT AND OFL-1.1` because it
  ships fonts under the SIL Open Font License 1.1 (Geist, Inter, IBM Plex,
  Source Sans 3, JetBrains Mono, Fira Code, Noto Color Emoji, Noto Sans
  CJK). The font license files are in `assets/fonts` and are packaged.
- **No crate packages a `LICENSE` file.** The MIT license requires its
  notice to accompany copies; the only copy is the workspace root
  `LICENSE`, which `cargo package --list` does not include for any crate.
  Copy or symlink `LICENSE` into each crate directory (cargo follows
  symlinks when packaging).
- `vendor/accesskit_unix` is `MIT OR Apache-2.0`, and stays unpublished.

## Metadata

| Field | Before | Now |
|---|---|---|
| `description` | Set on all nine crates | Unchanged |
| `license`, `repository` | Inherited from `[workspace.package]` | Unchanged, except `quark-text`'s license |
| `readme` | Only `quark` had a `README.md` | Every crate has a `README.md` and names it |
| `documentation` | Not set | `https://docs.rs/<crate>` |
| `keywords`, `categories` | Not set | Set on every crate |
| `homepage` | Not set | Not set; optional |

`repository` points at `https://github.com/seatedro/quark`. The crate
READMEs link to the root README and the guide with relative paths; check
how crates.io renders them before publishing.

## Other risks

- **docs.rs features.** docs.rs builds with default features, so
  `quark_app::testing`, `platform::notification`, `platform::tray`, and
  the dialog API will not appear in the docs. `--all-features` cannot be
  used instead: `profile-puffin` and `profile-tracy` together hit a
  `compile_error!`. A `[package.metadata.docs.rs]` table with an explicit
  feature list would fix it.
- **Toolchain.** Every crate declares `rust-version = "1.92"`, but CI
  builds only with the pinned `nightly-2026-10-07`. A stable 1.92 build is
  untested.
- **Rustdoc warnings.** `cargo doc --workspace --no-deps` reports seven
  warnings outside the crate root files: an unresolved link to
  `register_url_scheme` in `quark-app/src/platform/deep_link.rs` (the
  function exists only on Windows), links to private items in
  `quark-ui` (`element/cache/mod.rs`, `text_input/editor.rs`,
  `text_input/text_edit.rs`, `transcript/find.rs`) and `quark-diff`
  (`projection.rs`), and a redundant link target in
  `quark-ui/src/transcript/element.rs`.
- **Version 0.1.0 everywhere.** All crates share the workspace version, so
  a fix to one crate republishes or skews the others.

## Commands used

```bash
cargo package --list --offline --allow-dirty -p <crate>     # every crate
cargo package --no-verify --offline --allow-dirty -p quark  # blocker 3
# In a copy with `version = "0.1.0"` added to each Quark workspace dependency:
cargo package --workspace --exclude accesskit_unix --no-verify --offline
```
