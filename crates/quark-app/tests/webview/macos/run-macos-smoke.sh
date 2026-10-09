#!/bin/bash
# Run the macOS webview smoke checks. AppKit gives a process windows only
# when LaunchServices starts it, which a plain ssh or CI shell does not, so
# this wraps the test binary in a minimal bundle and opens it.
#
#   crates/quark-app/tests/webview/macos/run-macos-smoke.sh [cargo args...]
#
# Exits 0 only if the checks print their pass line.
set -euo pipefail
cd "$(dirname "$0")/../../../../.."

bin=$(cargo test -p quark-app --features webview --test webview_smoke --no-run \
    --message-format json "$@" |
    python3 -c 'import json,sys
for line in sys.stdin:
    m = json.loads(line)
    if m.get("reason") == "compiler-artifact" and m.get("executable") and m["target"]["name"] == "webview_smoke":
        print(m["executable"])' | tail -1)

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
app="$work/WebviewSmoke.app"
mkdir -p "$app/Contents/MacOS"
cp "$bin" "$app/Contents/MacOS/webview_smoke"
cat > "$app/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>webview_smoke</string>
<key>CFBundleIdentifier</key><string>dev.quark.webview-smoke</string>
<key>CFBundleName</key><string>WebviewSmoke</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
log="$work/log"
: > "$log"
open -n -W --stdout "$log" --stderr "$log" "$app" --args --macos
cat "$log"
grep -q '^webview_smoke macos: passed$' "$log"
