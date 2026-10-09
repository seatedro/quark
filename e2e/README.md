# End-to-end specs

Run quark's example apps on a private X11 desktop and drive them through
AT-SPI with the [cua](https://github.com/trycua/cua) driver. Linux only.

## Running

```bash
cargo build -p quark-app --examples --features ui,notifications
e2e/install-cua.sh
e2e/run.sh                              # every spec
e2e/run.sh e2e/specs/hello_ui/*.py      # some specs
```

- Each spec gets a fresh desktop (Xvfb, openbox, a session D-Bus with the
  AT-SPI bus) and a fresh copy of its app.
- [install-cua.sh](install-cua.sh) pins and checksums the cua driver.
- CI runs them in [.github/workflows/e2e.yml](../.github/workflows/e2e.yml)
  and uploads failure artifacts.

## Specs

- Path: `specs/<example>/<behavior>.py`, run against the binary named
  `<example>`.
- Binary lookup: `QUARK_E2E_BIN_DIR` (default `target/debug/examples`), else
  `target/debug`, where package binaries such as `workbench` and
  `codex-demo` land:

```bash
cargo build -p quark-workbench --bin workbench
e2e/run.sh e2e/specs/workbench/*.py
```

- The Workbench and Codex demos link `quark-terminal`, so they need Zig 0.16
  to build ([Terminal](../docs/guide/terminal.md#building)).
- The runner starts apps without arguments. Set the app's environment with
  header lines, applied before launch:

```python
# quark-e2e-env: QUARK_WORKBENCH_SCENARIO=stress
```

- `QUARK_SYNTAX_PACKS=target/syntax-packs` is exported when that directory
  exists, so code is highlighted once the [grammar
  packs](../docs/guide/syntax-packs.md#building-packs) are built.
- [theme_motion.py](specs/workbench/theme_motion.py) reads screenshots with
  Pillow (`python3-pil`); the runner does not check for it.
- Each spec starts with a docstring naming the regression it catches;
  [greet_click.py](specs/hello_ui/greet_click.py) is a short one.
- Wait by polling observable state against a deadline (`wait_for`), never by
  sleeping.

## Clients

[quark_e2e.py](quark_e2e.py) gives a spec two clients:

| Client | Use |
|---|---|
| `Cua` | Drives the app as a computer-use agent: AT-SPI snapshots, clicks by element or id (`press`, `press_id`), keys, clipboard, screenshots |
| `atspi_tree` | Raw AT-SPI tree over D-Bus, for what cua does not report: states such as focused, attributes such as `id`, the Text interface |

- `app_tree(window=...)` finds a window by name; `app_frames()` lists all of
  them.
- `panels_demo/new_window` moves a tab to a new window from the keyboard and
  closes it.
- `panels_demo/tear_off` tears a tab off with real `xdotool` pointer events
  and docks it back.
- `platform_demo/forward_url` covers single instance handoff on Linux.

## Artifacts

- A failed spec leaves `screen.png`, `tree.txt`, `cua-tree.txt`, and every
  log under `target/e2e/artifacts/<example>-<behavior>/`.
- `meta.txt` there names the spec, its environment, and the screen.
- `QUARK_E2E_KEEP=1` keeps artifacts of passing specs too.
