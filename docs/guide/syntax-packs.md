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
[grammars.toml](../../tools/syntax-pack/grammars.toml): Bash, C, C++, CSS,
Go, HTML, JavaScript, JSON, Markdown (the `markdown` block grammar and the
`markdown_inline` grammar its injection query names), Python, Rust, TOML,
TSX, TypeScript, and YAML. Every grammar is MIT licensed; `syntax-pack list`
prints each with its license, repository, and commit.

```sh
cargo run -p syntax-pack -- build                 # every language, for this host
cargo run -p syntax-pack -- build rust            # one language
cargo run -p syntax-pack -- build --target x86_64-apple-darwin
cargo run -p syntax-pack -- build --sources DIR   # read DIR/<language>/ instead of git
cargo run -p syntax-pack -- check                 # re-check built packs for this host
```

Packs land in `target/syntax-packs/<target>/<language>/`, the root the
examples, quark-syntax's tests, and quark-ui's tests read
(`QUARK_SYNTAX_PACKS` points the examples elsewhere,
`QUARK_SYNTAX_TEST_PACKS` the tests). For each language the tool fetches
the pinned commit with git, hashes every C source, header, and query file
it reads and compares the result with the pinned `sha256`, compiles
`parser.c` and, when the entry says `scanner = "c"`, `scanner.c` with the
target's C compiler (MSVC on Windows) into a shared library that exports
only the language function, and writes the manifest. Grammars with C++
scanners are rejected; every pinned grammar uses a C scanner or none.
`--sources` serves offline and Nix builds; the hash still applies. Files
are hashed with LF line endings, so a Windows checkout that converted them
hashes the same, and query files ship with LF endings on every target.

For the host target the build then checks each pack: it loads the pack as
the store does, loads the library again on its own, parses the language's
sample from [samples](../../tools/syntax-pack/samples), and fails when the
sample has a parse error, when an expected capture from `captures` is
missing, or when the injection query does not name every language in
`injects`. The TypeScript and TSX samples hold a type assertion and a
generic arrow function beside an element, which each parse only under
their own grammar. Packs for another target are checked by a syntax-pack
built for that target, natively or, for Intel macOS on Apple silicon,
under Rosetta:

```sh
cargo run -p syntax-pack --target x86_64-apple-darwin -- check
```

Loading grammars.toml checks that every name, alias, and extension belongs
to one language and is lowercase, and that each expected injection is a
tag of a pinned language.

After bumping a `rev` in grammars.toml, run the build once: it fails and
prints the new source hash to pin. A changed grammar or query changes the
files' SHA-256 and so their published URLs (below).

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
`url` field or `<language>/<path>` next to the index. Published indexes
always set `url` to an immutable, content addressed location,
`<base>/<target>/<language>/<sha256>/<path>`, so a cached copy of an
older file can never be mistaken for its replacement.

## Publishing packs

Packs are served from `https://quark.seated.ro/v1`, the `v1/` prefix of
the `quark-packs` Cloudflare R2 bucket through its custom domain:
`<base>/<target>/index.json` is each target's signed
index, and the chat and diff demos fetch it, trusting the public key the
workflow's `PACK_INDEX_KEY` also names. The [Syntax packs](../../.github/workflows/syntax-packs.yml)
workflow runs on manual dispatch only and has four jobs:

| Job | Runs | Holds |
|---|---|---|
| `build` | Always, on a native runner per target: `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `aarch64-apple-darwin` (`macos-15`), `x86_64-apple-darwin` (`macos-15-intel`), `x86_64-pc-windows-msvc`. Builds every pack, loads it, checks its sample, and uploads `syntax-packs-unsigned-<target>`. It fails when the runner's host triple is not the target, so no pack goes unloaded. | No secrets |
| `sign` | Only when `languages` is empty. Signs each target's index with `--base-url`, then checks it with `syntax-pack verify` against the key the demos ship. Reads the build artifacts as data and restores no cache. Uploads `syntax-packs-signed`. | `QUARK_SYNTAX_SIGNING_KEY` |
| `publish` | Only with `publish` checked, `languages` empty, on `master`. Runs [publish.sh](../../tools/syntax-pack/publish.sh). One run at a time across refs. | `QUARK_PACKS_R2_*` bucket credentials |
| `smoke` | After `publish`, on every target's runner. `syntax-pack remote-check` downloads each pack through the public index with quark-syntax's downloader and highlights its sample. | No secrets |

publish.sh verifies every target's index first (signature, every pinned
language, local files, URLs) and uploads nothing unless all pass. It then
uploads each file to the key its URL names through R2's S3 API, skipping
keys that already exist so a published file is never replaced, downloads
every file through the public URL to check its SHA-256, and only then
replaces each `index.json` with a single PUT (readers see the old index or
the new one). Pack files are served with an immutable cache header and
indexes with `no-cache`. A subset dispatch cannot
reach the public index: `sign` and `publish` skip it, and `syntax-pack
index` refuses any language set other than the complete pinned one.

### One-time setup

- Create the `syntax-packs-signing` and `syntax-packs-publish`
  environments (Settings > Environments) with required reviewers and a
  deployment branch rule for `master`.
- Move `QUARK_SYNTAX_SIGNING_KEY` from the repository secrets into
  `syntax-packs-signing`, so only the sign job can read it. To make a new
  key, run `cargo run -p quark-update --example manifest_tool keygen`,
  compile the public half (`syntax-pack public-key` prints it) into apps,
  and update `PACK_INDEX_KEY` in the workflow. Use a key separate from the
  update key. To rotate, ship the new public key beside the old one before
  signing with it.
- In `syntax-packs-publish`, set `QUARK_PACKS_R2_ACCESS_KEY_ID` and
  `QUARK_PACKS_R2_SECRET_ACCESS_KEY`: the S3 credentials of a Cloudflare
  API token limited to object read and write on the `quark-packs` bucket.
  The bucket and its account endpoint are named in the workflow.
- Let non-browser clients through Cloudflare for `/v1/*`: apps download
  with ureq, and the publish job checks every URL with curl.

### Running it

1. Dispatch **Syntax packs** on `master` with `languages` empty and
   `publish` unchecked. Every target builds and checks, and the signed
   artifact is ready to inspect.
2. Dispatch again with `publish` checked, approve the two environments, and
   wait for `smoke` on all five targets.

`languages` set to, say, `yaml toml` builds and tests just those, on
every target, for trying a grammar bump.

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
