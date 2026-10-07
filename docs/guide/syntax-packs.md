# Syntax highlighting and grammar packs

`quark-syntax` compiles no grammar into the app. Each language arrives at
runtime as a pack: a shared library that exports the tree-sitter language
function, its highlight (and optional injection) query, and a manifest. An
app hands a `GrammarStore` to the markdown document and the diff view;
the store loads packs from directories the app trusts and, with the
`download` feature, from a signed index. A code block whose grammar is
missing or still downloading renders plain, and is recolored when the
grammar arrives.

## Configuring a store

| Source | Configure with | Cargo feature | Trust |
|---|---|---|---|
| Local pack roots | `StoreConfig::local_packs(root)` | `quark-ui/syntax` (`quark-syntax/engine`) | The app's opt-in: unsigned |
| Signed index | `StoreConfig::downloads(Downloads::new(app_id, url, &keys))` | `quark-ui/syntax-download` (`quark-syntax/download`) | Ed25519 signature over the index |

Local roots are searched first, in the order added; then the download
cache; then the network. There is no default root and no default index
URL. Wiring, from the examples
([chat_demo.rs](../../crates/quark-app/examples/chat_demo.rs),
[diff_demo.rs](../../crates/quark-app/examples/diff_demo.rs)):

```rust
use quark_app::quark_ui::quark_syntax::{GrammarStore, StoreConfig};

let store = GrammarStore::new(StoreConfig::new().local_packs(root));
chat.set_grammar_store(store.clone()); // MarkdownDocument
diff.enable_syntax(store);             // DiffViewState
```

With downloads, `index_url` may contain `{target}`, which becomes the
build's target triple, and `keys` are `quark_syntax::PublicKey` values (the
same type and hex format as quark-update's). The cache defaults to
`<data dir>/<app id>/syntax-packs` (`$XDG_DATA_HOME` or `~/.local/share` on
Linux, `~/Library/Application Support` on macOS, `%LOCALAPPDATA%` on
Windows); `Downloads::cache_dir` overrides it.

`GrammarStore::subscribe` calls back on the fetch thread each time a
pending language resolves; use it to wake the event loop so the recolored
blocks are polled.

### Lookup behavior

A fence tag or file extension resolves once per store and the result is
kept, so an unknown language costs one map lookup after the first time.

1. A local pack whose name, alias, or extension matches loads at once.
2. A pack listed in the cached signed index, with every file present and
   matching its SHA-256, loads at once.
3. Otherwise, when downloads are configured, the language is pending: the
   fetch thread downloads the index (at most once per store, saving it to
   the cache) and then the pack. A language the fresh index lacks, or
   whose download fails, is unavailable for the rest of the session.

`HighlightWorker` answers a request for a pending language with a plain
result marked `pending`, then sends the real result for the same slot and
generation once the language resolves. `SyntaxHighlighter::finish_pending`
does not wait for downloads.

## Building packs

[tools/syntax-pack](../../tools/syntax-pack/src/main.rs) builds packs from
the pinned sources in
[grammars.toml](../../tools/syntax-pack/grammars.toml): Bash, Go,
JavaScript, JSON, Python, Rust, and TypeScript.

```sh
cargo run -p syntax-pack -- build                 # every language, for this host
cargo run -p syntax-pack -- build rust            # one language
cargo run -p syntax-pack -- build --target x86_64-apple-darwin
cargo run -p syntax-pack -- build --sources DIR   # read DIR/<language>/ instead of git
```

Packs land in `target/syntax-packs/<target>/<language>/`, the root the
examples, quark-syntax's tests, and quark-ui's tests read
(`QUARK_SYNTAX_PACKS` points the examples elsewhere,
`QUARK_SYNTAX_TEST_PACKS` the tests). For each language the tool fetches
the pinned commit with git, hashes every C source, header, and query file
it reads and compares the result with the pinned `sha256`, compiles
`parser.c` and `scanner.c` with the target's C compiler (MSVC on Windows)
into a shared library that exports only the language function, writes the
manifest, and, for the host target, loads the pack and compiles its query.
`--sources` serves offline and Nix builds; the hash check still applies.

After bumping a `rev` in grammars.toml, run the build once: it fails and
prints the new source hash to pin.

### Pack format

`manifest.json` holds `schema` (1), `language`, `aliases`, `extensions`,
`version`, `target`, `abi` (the grammar's tree-sitter `LANGUAGE_VERSION`),
`symbol`, a `library`, `highlights`, and optional `injections` entry each
with `path`, `sha256`, and `size`, and the `source` repository, commit,
and hash. [pack.rs](../../crates/quark-syntax/src/pack.rs) documents each
field and the checks.

The index is `{"payload": {"schema": 1, "target": ..., "packs": [manifest,
...]}, "signature": "<hex>"}`, signed with Ed25519 over the payload's
canonical JSON exactly like quark-update's manifests. A file's URL is its
`url` field or `<language>/<path>` next to the index.

## Publishing an index

The [Syntax packs](../../.github/workflows/syntax-packs.yml) workflow runs
on manual dispatch only. It builds packs for `x86_64-unknown-linux-gnu`,
`aarch64-unknown-linux-gnu`, `aarch64-apple-darwin`,
`x86_64-apple-darwin`, and `x86_64-pc-windows-msvc`, signs one
`index.json` per target, and uploads `syntax-packs-<target>` artifacts. It
publishes nothing.

A host needs:

- **A signing key.** Generate a seed with
  `cargo run -p quark-update --example manifest_tool keygen`, store the
  private half as the `QUARK_SYNTAX_SIGNING_KEY` repository secret, and
  compile the public half (`syntax-pack public-key` prints it) into the
  app. Use a key separate from the update key. To rotate, ship the new
  public key beside the old one before signing with it.
- **A static file host.** Copy each artifact's contents to
  `<base>/<target>/` so the index sits at `<base>/<target>/index.json`
  with the packs beside it, and configure the app with
  `<base>/{target}/index.json`. Serve over HTTPS. Pack URLs do not carry a
  version, so publish each run under a fresh `<base>` (or keep cache
  lifetimes short): a stale cached file fails its SHA-256 check and its
  language stays plain until the cache expires.

## Threat model

Loading a pack runs its native code with the app's privileges. The
crate docs ([lib.rs](../../crates/quark-syntax/src/lib.rs)) state the
model; in short:

- **Downloaded packs** load only through an index that verifies against
  a compiled-in key, and only when every file matches the SHA-256 that
  index lists, checked at download and again at each load from the cache.
  There is no switch to accept unsigned indexes. The index names its
  target; manifests must use plain file names, a C identifier for the
  symbol, this platform's library extension, and a supported ABI. A
  hostile network or host can withhold packs or replay an older signed
  index, never get unsigned code loaded.
- **Local packs** are not signed. They load only from roots the app names;
  name only directories that the app's installer or the developer
  controls.
- **The cache** is not defended against someone who can already write the
  user's files, including a library swapped between its hash check and
  `dlopen`.
- **Parsers** see untrusted text. A crash in a grammar's C code takes the
  process down; a panic in highlighting is caught by the worker.
