# Quark Workbench

A deterministic coding-agent client built from [Quark](../../README.md)'s
document, composer, dock, and window primitives. It opens on the thread
"Add keyboard shortcuts" in a fictional project, Atlas; sending a prompt
plays a scripted run (text, tool calls, a file edit, tests) from
[fixtures/run.json](fixtures/run.json). Nothing touches the network, a
shell, or your files: the backend, the terminal, and the file store are
simulated in memory, and the title bar says "Demo workspace".

## Launch

From the repository root:

```bash
cargo run -p quark-workbench --release --bin workbench
cargo run -p quark-workbench --bin workbench --features devtools -- --scenario review --theme dark
```

| Flag | Environment | Default | Usage |
|---|---|---|---|
| `--scenario review\|empty\|error\|stress` | `QUARK_WORKBENCH_SCENARIO` | `review` | Opening scene. `stress` adds 50,000 history rows and 2,000 sidebar threads. |
| `--theme system\|light\|dark` | `QUARK_WORKBENCH_THEME` | `system` | Appearance. |
| `--seed N` | `QUARK_WORKBENCH_SEED` | `7` | Seed of the stress generator. |
| `--manual-clock` | `QUARK_WORKBENCH_MANUAL_CLOCK=1` | off | Time moves only through "Advance demo step" (`mod+shift+.`), for screenshots of exact animation endpoints. |
| `--state-dir DIR` | `QUARK_WORKBENCH_STATE_DIR` | none | Restore and save the dock layout in `DIR`. Without it the session keeps nothing. |
| `--perf FILE` | `QUARK_WORKBENCH_PERF` | none | Record frame timings (see [Profiling](#profiling)) and exit. |
| | `QUARK_WORKBENCH_MARKS=1` | off | Print the first-frame and history-ready marks to stderr. |

Flags win over the environment. The environment forms exist because the
e2e runner starts binaries without arguments.

Code blocks are highlighted from local grammar packs only: `$QUARK_SYNTAX_PACKS`,
else `assets/syntax-packs/<arch>-<os>/` beside the executable, else the
workspace's `target/syntax-packs` (`cargo run -p syntax-pack -- build`).
Without packs code renders as plain text.

## Layout

The app is split so parallel work does not collide. Each surface module
(`shell`, `timeline`, `composer`, `dock`, `overlays`, `settings`) exports a
`State`, an `Action`, and `view`, `update`, `event`, `edit_text`, and
`command` functions. Surfaces read the model through the read-only
`SurfaceCx` and ask for changes by returning `Effect`s, which
[src/app.rs](src/app.rs) applies. [src/contracts.rs](src/contracts.rs)
holds the shared IDs, commands, effects, panel registry, and these
signatures.

| Path | Holds |
|---|---|
| `src/model.rs` | Projects, threads, transcripts with a change log, runs, the in-memory file store |
| `src/scenario.rs` | Scripted run playback and the demo clock |
| `src/fixtures.rs` | Fixture loading (compiled in) and the seeded stress generator |
| `fixtures/` | `threads.json` (3 projects, 24 threads, 120 rows), `run.json`, `diff.patch`, `terminal.ansi`, `files/` |
| `assets/` | Bundled images: a snapshot of the fixture app and an attachment |

Stress history loads in batches of 2,000 rows, newest first, one batch per
frame, so the opening scene draws before the history is complete.

## Tests

```bash
cargo test -p quark-workbench                         # unit and integration tests
cargo test -p quark-workbench --test shell shell_     # one surface
cargo test -p quark-workbench --test perf -- --ignored --nocapture   # allocation report
```

Integration tests drive the real app headlessly through
`quark_app::testing`; [tests/common/mod.rs](tests/common/mod.rs) builds the
harness. [tests/perf.rs](tests/perf.rs) states the design's allocation
budgets beside the ceilings the app currently meets; the ignored report
prints the counts and the top allocation sites.

End-to-end specs run the binary on Xvfb with AT-SPI and cua (see
[e2e/run.sh](../../e2e/run.sh)):

```bash
cargo build -p quark-workbench --bin workbench
QUARK_E2E_BIN_DIR="$PWD/target/debug" QUARK_E2E_KEEP=1 e2e/run.sh e2e/specs/workbench/*.py
```

A spec sets the app's environment with header lines such as
`# quark-e2e-env: QUARK_WORKBENCH_SCENARIO=stress`. With `QUARK_E2E_KEEP=1`
screenshots, trees, logs, and `meta.txt` stay in
`target/e2e/artifacts/workbench-<spec>/` for review. Under a Nix devshell
the binary may need the shell's `LD_LIBRARY_PATH` (for the dlopened
xkbcommon and Vulkan loader) and, without a GPU, Mesa's lavapipe through
`VK_DRIVER_FILES`; wrap it in a script in `QUARK_E2E_BIN_DIR` that sets
both and `exec`s the real binary, so the runner's recorded pid stays the
app's.

## Profiling

`--perf FILE` sends a prompt on launch and writes one JSON line per
main-window frame until the run and any queued history finish:

```json
{"frame":12,"t_ms":702,"build_us":110,"render_cpu_us":96,"acquire_us":11,"present_us":8400,"rows":18,"running":true}
```

`build_us` is the app's own view build; the render fields are that same
frame's renderer CPU, swapchain acquire, and present times. Layout and
paint phases are not in the file: the adapter measures them only with the
`devtools` feature, in its HUD. The file ends with the marks
(`first-frame`, `history-ready`). Timings from software rendering under
Xvfb say nothing about hardware frame rates.

For attribution use one profiler feature at a time: `--features
profile-puffin` (connect `puffin_viewer` to 127.0.0.1:8585) or `--features
profile-tracy`. The `devtools` feature adds the frame HUD and inspector
(`ctrl+shift+h`, `ctrl+shift+i`).

## Capability limits

- The preview panel shows a bundled image labeled "Snapshot preview"; there
  is no web engine.
- The terminal never starts a shell or PTY: it shows fixture output
  ([fixtures/terminal.ansi](fixtures/terminal.ansi)) and scripted commands.
- Applying the diff changes only the in-memory file store, with Undo.
- On macOS the window uses custom chrome with native traffic lights; on
  Linux and Windows it keeps system decorations above the in-app top bar.
