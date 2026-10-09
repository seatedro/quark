#!/usr/bin/env bash
# refresh.sh OUT_DIR: copies the CoreText sheets the calibration and parity
# tests read from run.sh's output into refs/, with the manifest (glyph ids
# and positions) and env.json (OS build, smoothing preferences, resolved
# variation instances). Dark plain sheets are left out: plain coverage
# does not depend on the colors, so they repeat the light ones.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
out=$1 refs="$here/refs"
rm -rf "$refs" && mkdir -p "$refs"
cp "$out/manifest.json" "$out/ct/env.json" "$refs/"
for f in "$out"/ct/*.png; do
  name=$(basename "$f")
  case "$name" in
    *.dark-plain.png) continue ;;
    *@1x.* | sweep@2x.* | panel@2x.* | sf-pro@2x.* | inter@2x.*) cp "$f" "$refs/" ;;
  esac
done
du -sh "$refs"
