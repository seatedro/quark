#!/usr/bin/env bash
# Runs end-to-end specs against real quark example apps on a private X11
# desktop: Xvfb, openbox, a session D-Bus with the AT-SPI bus, and the cua
# driver. Each spec gets a fresh desktop and a fresh copy of its app.
#
#   e2e/run.sh                          # every spec under e2e/specs
#   e2e/run.sh e2e/specs/hello_ui/*.py  # some specs
#
# A spec lives at e2e/specs/<example>/<behavior>.py and runs against
# <example>. Build the examples first:
#   cargo build -p quark-app --examples --features ui,notifications
#
# Environment:
#   QUARK_E2E_BIN_DIR  example binaries (default target/debug/examples); a
#                      package binary such as workbench lives in target/debug
#   CUA_DRIVER         cua-driver binary (default: PATH, then e2e/install-cua.sh's)
#   QUARK_E2E_OUT      artifacts root (default target/e2e/artifacts)
#   QUARK_E2E_KEEP=1   keep the artifacts of passing specs too, with a final
#                      screen.png and tree.txt, for visual review
#
# A spec can set its app's environment with header lines, applied before
# the app launches (the runner starts binaries without arguments):
#   # quark-e2e-env: QUARK_WORKBENCH_SCENARIO=empty
#
# A failed spec leaves screen.png, tree.txt, cua-tree.txt, and every log in
# $QUARK_E2E_OUT/<example>-<behavior>/, plus meta.txt naming the spec, its
# environment, and the screen. Specs save named screenshots there with
# quark_e2e.save_screenshot.
set -euo pipefail

E2E_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
ROOT=$(dirname "$E2E_DIR")
export QUARK_E2E_BIN_DIR=${QUARK_E2E_BIN_DIR:-$ROOT/target/debug/examples}
export QUARK_E2E_OUT=${QUARK_E2E_OUT:-$ROOT/target/e2e/artifacts}
if [[ -z ${CUA_DRIVER:-} ]]; then
  CUA_DRIVER=$(command -v cua-driver || echo "$ROOT/target/e2e/cua-driver/cua-driver")
fi
export CUA_DRIVER
export PYTHONPATH=$E2E_DIR${PYTHONPATH:+:$PYTHONPATH}
export PYTHONDONTWRITEBYTECODE=1

# Polls a command until it succeeds or the deadline passes.
wait_until() {
  local what=$1 deadline=$((SECONDS + 20))
  shift
  until "$@" >/dev/null 2>&1; do
    if ((SECONDS >= deadline)); then
      echo "timed out waiting for $what" >&2
      return 1
    fi
    sleep 0.1
  done
}

wm_ready() {
  [[ $(xprop -root _NET_SUPPORTING_WM_CHECK 2>/dev/null) == *"window id"* ]]
}

# Runs one spec on its own desktop. Called in a new session by the loop
# below, so the loop can kill the whole process group afterwards.
run_one() {
  local spec=$1
  local example behavior
  example=$(basename "$(dirname "$spec")")
  behavior=$(basename "$spec" .py)
  local out=$QUARK_E2E_OUT/$example-$behavior
  rm -rf "$out"
  mkdir -p "$out/home" "$out/runtime" "$out/tmp"
  chmod 700 "$out/runtime"

  # Keep the apps, the driver, and the single instance socket away from the
  # user's real session and from each other. A private TMPDIR keeps state an
  # example saves there (panels_demo's layout) from leaking into later runs.
  export HOME=$out/home XDG_RUNTIME_DIR=$out/runtime TMPDIR=$out/tmp
  export XDG_CONFIG_HOME=$HOME/.config XDG_CACHE_HOME=$HOME/.cache XDG_DATA_HOME=$HOME/.local/share
  unset WAYLAND_DISPLAY
  export QUARK_E2E_APP=$example QUARK_E2E_APP_LOG=$out/app.log QUARK_E2E_ARTIFACTS=$out

  # A private live theme directory the workbench watches; specs write
  # theme files there to test reloading. Harmless for other apps.
  mkdir -p "$out/themes"
  export QUARK_WORKBENCH_THEME_DIR=$out/themes

  # The spec's own app environment (`# quark-e2e-env: KEY=VALUE`).
  local line name
  local env_line='^# quark-e2e-env: ([A-Za-z_][A-Za-z0-9_]*=.*)$'
  while IFS= read -r line; do
    if [[ $line =~ $env_line ]]; then
      export "${BASH_REMATCH[1]}"
    fi
  done <"$spec"
  {
    echo "spec: $spec"
    echo "screen: 1280x800x24, scale 1"
    for name in $(compgen -e | sort); do
      case $name in
        QUARK_E2E_PID | QUARK_E2E_OUT | QUARK_E2E_ARTIFACTS | QUARK_E2E_APP_LOG) ;;
        QUARK_*) echo "$name=${!name}" ;;
      esac
    done
  } >"$out/meta.txt"

  exec 3>"$out/display"
  Xvfb -displayfd 3 -screen 0 1280x800x24 -nolisten tcp >"$out/xvfb.log" 2>&1 &
  exec 3>&-
  wait_until "Xvfb" test -s "$out/display"
  DISPLAY=:$(<"$out/display")
  export DISPLAY

  dbus-daemon --session --nofork --print-address=4 4>"$out/dbus-address" >"$out/dbus.log" 2>&1 &
  wait_until "the session bus" test -s "$out/dbus-address"
  DBUS_SESSION_BUS_ADDRESS=$(head -n1 "$out/dbus-address")
  export DBUS_SESSION_BUS_ADDRESS

  # The session bus starts the AT-SPI bus on demand (at-spi2-core's
  # org.a11y.Bus service file). AccessKit publishes its tree only while
  # assistive technology is enabled.
  dbus-send --session --print-reply --dest=org.a11y.Bus /org/a11y/bus \
    org.freedesktop.DBus.Properties.Set string:org.a11y.Status string:IsEnabled variant:boolean:true >/dev/null

  openbox --sm-disable >"$out/openbox.log" 2>&1 &
  wait_until "openbox" wm_ready

  "$CUA_DRIVER" telemetry disable >/dev/null 2>&1 || true

  "$QUARK_E2E_BIN_DIR/$example" >"$out/app.log" 2>&1 &
  export QUARK_E2E_PID=$!

  local status=0
  if ! timeout 60 python3 "$E2E_DIR/quark_e2e.py" wait >"$out/spec.log" 2>&1; then
    status=1
  elif ! timeout 120 python3 "$spec" >>"$out/spec.log" 2>&1; then
    status=1
  fi
  if ((status != 0)) || [[ ${QUARK_E2E_KEEP:-0} != 0 ]]; then
    timeout 60 python3 "$E2E_DIR/quark_e2e.py" dump "$out" >>"$out/spec.log" 2>&1 || true
  fi
  if ((status != 0)); then
    sed 's/^/    /' "$out/spec.log" >&2
  fi
  return $status
}

if [[ ${1:-} == --one ]]; then
  run_one "$2"
  exit
fi

for tool in Xvfb openbox dbus-daemon dbus-send xprop python3; do
  command -v "$tool" >/dev/null || { echo "missing $tool" >&2; exit 2; }
done
[[ -f /usr/share/dbus-1/services/org.a11y.Bus.service ]] || { echo "missing at-spi2-core" >&2; exit 2; }
[[ -x $CUA_DRIVER ]] || command -v "$CUA_DRIVER" >/dev/null || {
  echo "missing cua-driver; run e2e/install-cua.sh" >&2
  exit 2
}
python3 -c 'import dbus' 2>/dev/null || { echo "missing python3-dbus" >&2; exit 2; }

if (($# == 0)); then
  set -- "$E2E_DIR"/specs/*/*.py
fi

failed=0
for spec in "$@"; do
  name=$(basename "$(dirname "$spec")")/$(basename "$spec" .py)
  setsid "${BASH_SOURCE[0]}" --one "$spec" &
  leader=$!
  if wait "$leader"; then
    if [[ ${QUARK_E2E_KEEP:-0} != 0 ]]; then
      echo "PASS $name (artifacts kept in $QUARK_E2E_OUT/${name/\//-})"
    else
      echo "PASS $name"
      rm -rf "${QUARK_E2E_OUT:?}/${name/\//-}"
    fi
  else
    echo "FAIL $name (artifacts in $QUARK_E2E_OUT/${name/\//-})"
    failed=$((failed + 1))
  fi
  # Whatever the spec left running (app, Xvfb, buses, window manager).
  kill -TERM -- "-$leader" 2>/dev/null || true
done

echo "$(($# - failed))/$# specs passed"
((failed == 0))
