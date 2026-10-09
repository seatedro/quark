#!/usr/bin/env bash
# Text render report: every fixture at scales 1, 1.5, and 2 through every
# rasterizer this platform offers. Writes report.tsv and one PNG per row
# into the output directory, and compares with a baseline report when one
# is given. The harness is crates/quark-render/tests/native_text.
#
# Usage: tools/text-render/report.sh [OUT_DIR] [BASELINE_DIR]
#
#   OUT_DIR        default target/text-render/<short sha>[+dirty]
#   BASELINE_DIR   a previous OUT_DIR; prints per-row deltas, pixel
#                  differences, and gate failures (warm frame slower by
#                  more than 5% or 0.1 ms, more text allocations, any
#                  raster or upload in a repeated frame)
#
# Environment:
#   QUARK_TEXT_FRAMES   repeated frames per row (default 1000)
#   QUARK_TEXT_PROFILE  cargo profile flag (default --release; "" for test)
#   QUARK_TEXT_GATE=1   fail when a gate fails
#   QUARK_CARGO         cargo command, e.g. "capped -m 8G cargo"
#
# Compare like with like: same machine, adapter, and build profile;
# alternate baseline and candidate runs.
set -euo pipefail

root=$(git rev-parse --show-toplevel)
revision=$(git -C "$root" rev-parse --short HEAD)
git -C "$root" diff --quiet HEAD || revision="$revision+dirty"
out=$(realpath -m "${1:-$root/target/text-render/$revision}")
baseline=${2:+$(realpath "$2")}

export QUARK_TEXT_REPORT_DIR="$out"
export QUARK_TEXT_REVISION="$revision"
export QUARK_TEXT_FRAMES="${QUARK_TEXT_FRAMES:-1000}"
export QUARK_REQUIRE_GPU=1
if [ -n "$baseline" ]; then
  export QUARK_TEXT_BASELINE_DIR="$baseline"
fi

profile=${QUARK_TEXT_PROFILE---release}
cd "$root"
# shellcheck disable=SC2086 # QUARK_CARGO and the profile flag split on purpose.
${QUARK_CARGO:-cargo} test $profile -p quark-render --features headless-render \
  --test native_text report_text_render -- --ignored --nocapture --exact
echo "report: $out/report.tsv"
