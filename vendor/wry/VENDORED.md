# Vendored wry

Upstream: wry 0.57.0 from crates.io (tauri-apps/wry). Wired in through
`[patch.crates-io]` in the workspace `Cargo.toml` for quark-webview's
macOS backend. Listed in the workspace's `exclude`, not its members, so
workspace builds, `cargo fmt --all`, and clippy never touch it or need
WebKitGTK.

The first commit that added this directory is the crate as published,
minus `.cargo-ok`, `.cargo_vcs_info.json`, `Cargo.lock`, and
`Cargo.toml.orig`. `git diff` against that commit shows the whole patch.

## Patch

macOS only; every other platform builds upstream's code unchanged.

- `NavigationHooks` (`src/wkwebview/navigation_hooks.rs`), re-exported at
  the crate root with `AuthChallengeCompletion`, set with
  `WebViewBuilderExtMacos::with_navigation_hooks`. Methods default to
  no-ops and add to wry's navigation delegate; wry's own callbacks keep
  running, and the delegate is not replaced or swizzled.
  - `decide_action`, `decide_response`: run before wry's policy logic;
    `false` cancels.
  - `provisional_started`, `server_redirect`, `provisional_failed`,
    `failed`: new delegate selectors.
  - `committed`, `finished`: called after wry's existing handlers.
  - `authentication_challenge`: `false` keeps default TLS handling;
    quark uses it only for the test-only fixture trust.
- `didCommitNavigation` and `didFinishNavigation` take
  `Option<&WKNavigation>`: WebKit passes nil for some loads.
- `objc2-foundation` gains `NSError`, `NSURLAuthenticationChallenge`,
  `NSURLCredential`, `NSURLProtectionSpace`, and `NSURLSession`.
- `Cargo.toml` allows rustc warnings: upstream's code warns under the
  workspace's pinned nightly.

Checked with `cargo check` for `aarch64-apple-darwin` and
`aarch64-apple-ios`.

## Updating

Copy the new published crate over this directory in one commit, then
reapply the patch and rerun quark-webview's macOS checks on a Mac.
