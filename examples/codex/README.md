# Codex desktop recreation

A recreation of the OpenAI Codex desktop app's UI built from
[quark](../../README.md)'s primitives, with fixed fake data and no
network. It follows ChatGPT 26.1007 in Codex mode (icon rail, inset
sidebar and main cards, Changes tab, agent turns with approvals). The
terminal, file viewer, search palette, keyboard shortcuts page, and
narrow widths follow Codex 26.623, the version captured before the app
updated itself.

```bash
cargo run -p quark-codex --bin codex-demo
cargo run -p quark-codex --bin codex-demo -- --scene approval --theme dark
cargo run -p quark-codex --bin codex-demo -- --list-scenes
```

| Flag | Environment | Default | Usage |
|---|---|---|---|
| `--theme system\|light\|dark` | `QUARK_CODEX_THEME` | `system` | Appearance. |
| `--scene NAME` | `QUARK_CODEX_SCENE` | none | Open in the state of one reference capture (`--list-scenes` names each scene's capture). |
| `--terminal real\|scripted` | `QUARK_CODEX_TERMINAL` | `real` | The Changes panel's Terminal tab runs your login shell on a PTY, or replays the captured `npm test` transcript. |

Sending a prompt plays a fake turn (starting, thinking, a streamed
preamble, a live command row, the answer). In the `approval` scene, Enter
or Allow once allows the command and Escape denies it.

## Tests

```bash
cargo test -p quark-codex
cargo build -p quark-codex --bin codex-demo
QUARK_E2E_BIN_DIR="$PWD/target/debug" e2e/run.sh e2e/specs/codex-demo/*.py
```

The e2e runner starts `codex-demo` from `QUARK_E2E_BIN_DIR`; under a Nix
devshell wrap the binary as the workbench README describes.
