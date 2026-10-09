#!/usr/bin/env bash
# panel.sh OUT_DIR SHEET COLUMNS CAPTURE: shows `ct_ref panel` as a minimal
# .app (GUI binaries started over ssh get no window otherwise), captures
# the display with cua-driver, quits the app, and matches every block
# against the offscreen sheets. Run after run.sh, on the Mac's display.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
out=$1 sheet=$2 columns=$3 capture=$4
app="$out/CTRefPanel.app"
rm -rf "$app" && mkdir -p "$app/Contents/MacOS"
cp "$out/ct_ref" "$app/Contents/MacOS/ct_ref"
echo "panel $out $sheet $columns" > "$app/Contents/MacOS/args.txt"
cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>ct_ref</string>
<key>CFBundleIdentifier</key><string>dev.quark.ct-ref-panel</string>
<key>CFBundleName</key><string>CTRefPanel</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
rm -f /tmp/ct_ref_panel_layout.txt
open -n "$app"
for _ in $(seq 50); do [ -s /tmp/ct_ref_panel_layout.txt ] && break; sleep 0.2; done
sleep 1.5
cua-driver call get_desktop_state "{\"screenshot_out_file\": \"$capture\"}" > /dev/null
# The window's frame (points = pixels at 1x) locates the content view: its
# bottom edge is the frame's, its height the view's.
frame=$(cua-driver call list_windows '{}' | python3 -c '
import json, sys
d = json.load(sys.stdin)
ws = d.get("windows", d) if isinstance(d, dict) else d
for w in ws:
    if "ct_ref" in str(w.get("title", "")) or "CTRefPanel" in str(w.get("app_name", w.get("owner_name", ""))):
        b = w.get("bounds", w)
        print(int(b["x"]), int(b["y"]), int(b["width"]), int(b["height"]))
        break
')
osascript -e 'quit app "CTRefPanel"' || pkill -f CTRefPanel.app || true
read -r wx wy ww wh <<< "$frame"
vh=$(python3 -c "import sys; print(max(int(l.split()[3]) + int(l.split()[5]) for l in open('/tmp/ct_ref_panel_layout.txt')) + 8)")
cp /tmp/ct_ref_panel_layout.txt "$out/panel_layout.txt"
python3 "$here/compare.py" "$out" --panel "$capture" --panel-layout "$out/panel_layout.txt" \
  --panel-origin "$wx,$((wy + wh - vh))" --panel-sheet "$sheet"
