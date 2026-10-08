#!/usr/bin/env bash
# The workbench development loop.
#
#   examples/workbench/scripts/dev.sh [workbench flags, e.g. --theme light]
#
# - Theme edits: save examples/workbench/assets/themes/workbench.json and the
#   window repaints within about a tenth of a second (the app polls the
#   directory every 100 ms). A file that fails to parse is reported on
#   stderr and in a toast, and the window keeps the theme it had.
# - View edits: with the Dioxus CLI (`dx`) installed, the app runs under
#   `dx serve --hotpatch`, which patches changed view code into the running
#   process; threads, drafts, and dock layout stay. Changes to the model,
#   state types, or startup need a restart: quit and rerun, or set
#   QUARK_DEV_RESTART=1 to rebuild and relaunch on every save.
#   Without dx, the script runs the app once with devtools; rerun it after
#   code changes.
# - Devtools: ctrl+shift+i opens the inspector (edit padding, gap, colors,
#   radius, then "copy patch" for view! attributes), ctrl+shift+h the frame
#   HUD, ctrl+shift+l layout outlines.
#
# Environment: CARGO (default cargo; on shared machines a wrapper such as
# `capped cargo`), QUARK_WORKBENCH_THEME_DIR (default the assets/themes
# directory), and the app's own QUARK_WORKBENCH_* options.
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)
cd "$ROOT"
if command -v direnv >/dev/null; then
  eval "$(direnv export bash 2>/dev/null)" || true
fi

export QUARK_WORKBENCH_THEME_DIR=${QUARK_WORKBENCH_THEME_DIR:-$ROOT/examples/workbench/assets/themes}
read -r -a cargo <<<"${CARGO:-cargo}"
features=devtools

if command -v dx >/dev/null && [[ ${QUARK_DEV_RESTART:-0} != 1 ]]; then
  exec dx serve --hotpatch --package quark-workbench --bin workbench \
    --features "$features,hot-reload" -- "$@"
fi

if [[ ${QUARK_DEV_RESTART:-0} == 1 ]]; then
  # Rebuild and relaunch whenever a workbench source file changes; stop
  # when the app quits on its own.
  changed() { find "$ROOT/examples/workbench/src" -name '*.rs' -newer "$marker" -print -quit; }
  marker=$(mktemp)
  trap 'rm -f "$marker"; kill "${app:-}" 2>/dev/null || true' EXIT
  while true; do
    touch "$marker"
    "${cargo[@]}" run -p quark-workbench --bin workbench --features "$features" -- "$@" &
    app=$!
    while kill -0 "$app" 2>/dev/null; do
      if [[ -n $(changed) ]]; then
        kill "$app" 2>/dev/null || true
        wait "$app" 2>/dev/null || true
        continue 2
      fi
      sleep 0.5
    done
    wait "$app" 2>/dev/null || true
    exit
  done
fi

exec "${cargo[@]}" run -p quark-workbench --bin workbench --features "$features" -- "$@"
