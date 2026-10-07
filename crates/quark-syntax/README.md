# quark-syntax

Tree-sitter syntax highlighting for [Quark](../../README.md) code blocks.
Each grammar is a cargo feature, off by default, because each compiles a C
parser into the binary (about 30 KB for JSON, over 1 MB for TypeScript and
Bash):

`rust`, `javascript`, `typescript`, `python`, `bash`, `json`, `go`, or
`common` for all of them.

Without a language feature the crate has no dependencies and every lookup
falls back to plain text. `highlight` runs synchronously; `HighlightWorker`
runs on a background thread and drops requests a newer generation
superseded, which suits code that is still streaming.
