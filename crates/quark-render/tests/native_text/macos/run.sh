#!/usr/bin/env bash
# run.sh [OUT_DIR]: the macOS native text reference harness. On a Mac:
#   1. swash-dump shapes the reference lines with quark-text and writes the
#      manifest, the font bytes it used, and quark's swash sheets;
#   2. ct_ref.swift draws the same glyph ids at the same positions with
#      CTFontDrawGlyphs into DeviceGray bitmap contexts, one PNG per mode;
#   3. compare.py measures ink, stems, row profiles, and bounds of each
#      CoreText line against swash, and the smoothing sweep.
# The report lands in OUT_DIR/report.md. refresh.sh copies the CoreText
# sheets into refs/ as the checked-in references.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../../../../.." && pwd)
out=${1:-/tmp/quark-native-text}
mkdir -p "$out"

# The dumper is its own workspace; give it the root's exact versions.
cp "$root/Cargo.lock" "$here/swash_dump/Cargo.lock"
cargo build --release --quiet --manifest-path "$here/swash_dump/Cargo.toml"
rm -rf "$out/swash" "$out/fonts" "$out/ct"
"$here/swash_dump/target/release/swash-dump" "$out"

swiftc -O -o "$out/ct_ref" "$here/ct_ref.swift"
"$out/ct_ref" render "$out" "$out/ct"

python3 "$here/compare.py" "$out" --report "$out/report.md" > /dev/null
echo "report: $out/report.md"
