# Vendored accesskit_unix

Upstream: accesskit_unix 0.22.1 from crates.io (AccessKit commit
c88605b96d04431f9c3c792464a0f2f253480e94, `platforms/unix`). Wired in
through `[patch.crates-io]` in the workspace `Cargo.toml`, and listed as a
workspace member so CI formats, lints, and tests the patch.

The first commit that added this directory is the crate exactly as
published, minus `.cargo-ok`, `.cargo_vcs_info.json`, `Cargo.lock`, and
`Cargo.toml.orig`. `git diff` against that commit shows the whole patch.

## Patch

All in `src/atspi/interfaces/accessible.rs`:

- `GetRoleName` on nodes and on the application root. AccessKit 0.22.1
  serves `GetRole` but not `GetRoleName`, and cua-driver (0.34) reads roles
  only through `GetRoleName`, so every role came back blank and its element
  tokens never matched. Names follow libatspi's `atspi_role_get_name`; the
  atspi crate's table differs from it only for `PushButton` ("button"
  there, "push button" in libatspi).
- `GetLocalizedRoleName` falls back to the role name when the node has no
  role description (it returned an empty string), and the root serves it.
- `GetAttributes` adds `id` with the node's author id (quark sets it from
  `accessibility_id`, else the semantic id or key, else `test_id`), so
  tools that read attributes can find elements by stable id. The root
  serves `GetAttributes` with no attributes instead of failing.
- Tests for the three behaviors, through `accesskit_atspi_common`.

Outside that file, `Cargo.toml` drops `resolver = "2"`: Cargo ignores a
member's resolver in a workspace and warns about it on every build.

Being a workspace member also adds the crate's optional tokio dependencies
to `Cargo.lock`; they are not built.

## Dropping the vendor

Once an AccessKit release serves `GetRoleName` and an `id` attribute (or
cua reads `AccessibleId`):

1. Delete `vendor/accesskit_unix`, its `members` entry, and the
   `[patch.crates-io]` section in the workspace `Cargo.toml`.
2. Bump `accesskit_winit` to the release that pulls in the fixed
   accesskit_unix and run `cargo update -p accesskit_unix`.
3. Run `e2e/run.sh`. The specs that find nodes by role or by `id` through
   cua fail if either piece is missing.
