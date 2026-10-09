# quark-fmt

Formats Rust sources and the quark `view!` templates in them: rustfmt first,
then the view printer. Template edits change only whitespace; attribute
order, class strings, literals, and comments are kept as written.

## Install

- `cargo install --path tools/quark-fmt --locked` installs `quark-fmt` and
  `cargo-quark` (which provides `cargo quark fmt`).
- rustfmt is the toolchain's, picked in each file's directory, so
  `rust-toolchain.toml` applies.
- `QUARK_FMT_RUSTFMT=<path>` runs a different rustfmt.

## Usage

| Command | Does |
|---|---|
| `cargo quark fmt --all` | Format every workspace member |
| `cargo quark fmt --all --check` | Print a diff, write nothing |
| `quark-fmt src/a.rs src/ui/` | Format files; directories are walked |
| `quark-fmt --all --view-only --check` | Views only, after `cargo fmt` (CI) |
| `quark-fmt --stdin --stdin-filepath src/a.rs` | Editor mode: stdin to stdout |

Other flags: `--manifest-path`, `--config-path`, `--edition`,
`--style-edition`, `--emit files|stdout`, `-v`, `--help`, `--version`.
Unknown flags are errors.

| Exit | Meaning |
|---|---|
| 0 | Formatted, or nothing to change |
| 1 | `--check` found changes |
| 2 | Bad arguments or config, a file failed (it is left unchanged), or a `--check` warning |

## Behavior

- Per file: rustfmt over stdin (no file access, no `mod` traversal), each
  view body put back as it was, then the view printer.
- That round repeats until it changes nothing, at most 3 times; no fixed
  point means the file is left unchanged and exit 2.
- `--view-only`: the view printer alone; a second pass must change nothing.
- Any failure (Rust or view parse error, rustfmt error, a failed check)
  leaves the whole file byte-for-byte unchanged. Other files are still
  formatted, and the exit status is 2.
- Only changed files are written: a temporary in the same directory, then a
  rename. The file mode is kept. A file edited during formatting is not
  overwritten.
- `--check` writes nothing and prints a unified diff on stdout.
- `--check` is strict: a warning (a view hidden in another macro, embedded
  Rust kept as written) also exits 2.
- Files without views get rustfmt alone, exactly as `cargo fmt` would.
- Views inside `vec![..]` (any `expr_macros` entry) are formatted when the
  body parses as Rust expressions (`a, b` or `x; n`); a view inside any
  other macro is left as written with a warning.
- `// quark-fmt: skip` on the line before a view, statement, or item keeps
  it as written; so does `#[rustfmt::skip]`.
- `--stdin` prints only the formatted source. On failure stdout is empty and
  the error goes to stderr, so editors keep the buffer.

## Files

- `--all`: members from `cargo metadata --no-deps --offline --locked`; each
  target's root file plus the `src`, `examples`, `tests`, and `benches`
  trees.
- Walks skip hidden directories, symlinks, and build output (directories
  holding `CACHEDIR.TAG`).
- `exclude` globs apply to explicit paths too, so pre-commit honors them.
  Excluded files get neither pass; `cargo fmt` still covers them.

## Configuration

- `rustfmt.toml` / `.rustfmt.toml`: found as rustfmt finds it (from
  `--config-path`, else the file's directory upward) and passed whole to
  rustfmt.
- The view printer follows its `max_width` (100), `tab_spaces` (4),
  `hard_tabs`, and `newline_style`.
- Edition: `--edition`, else rustfmt.toml `edition`, else the package's
  (following `edition.workspace = true`), else 2024.
- `quark-fmt.toml`, the nearest one above the file; unknown keys are errors:

| Key | Default | Use |
|---|---|---|
| `style_version` | `1` | Pins the layout policy; only `1` exists |
| `macro_names` | `["view", "quark::view"]` | Macro paths formatted as views |
| `expr_macros` | `["vec"]` | Macros whose bodies are searched for views as Rust |
| `exclude` | `[]` | Globs relative to this file, e.g. `"vendor/**"` |

## Editors

rust-analyzer pipes the buffer through `rustfmt.overrideCommand`.

VS Code, `.vscode/settings.json`:

```json
{
  "rust-analyzer.rustfmt.overrideCommand": [
    "quark-fmt", "--stdin", "--edition", "2024", "--config-path", "."
  ],
  "[rust]": { "editor.formatOnSave": true }
}
```

Zed, `.zed/settings.json`:

```json
{
  "lsp": {
    "rust-analyzer": {
      "initialization_options": {
        "rustfmt": {
          "overrideCommand": [
            "quark-fmt", "--stdin", "--edition", "2024", "--config-path", "."
          ]
        }
      }
    }
  },
  "languages": { "Rust": { "formatter": "language_server" } }
}
```

- `--config-path .` resolves against the language server's working
  directory, the project root.
- Editors that can pass the file name should add `--stdin-filepath <file>`
  so nested configs apply.

## Pre-commit

`.pre-commit-config.yaml`:

```yaml
repos:
  - repo: local
    hooks:
      - id: quark-fmt
        name: quark-fmt
        entry: quark-fmt
        language: system
        types: [rust]
        require_serial: true
```

- Formats only the staged file names pre-commit passes; never runs
  `git add`.
- `entry: quark-fmt --check` fails the commit instead of rewriting.
