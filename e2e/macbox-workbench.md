# Checking the workbench on macbox

macbox is an EC2 Mac (macOS 27, arm64) with `cua-driver` installed and its
Accessibility and Screen Recording grants in place. Its virtual display is
1024x768 at scale 1, so Retina rendering cannot be checked there, and cua
pointer drags reach no window: check drag features elsewhere. There is no
e2e runner for macOS; these commands launch the workbench and capture it
by hand.

## Build

Copy the worktree (sources only) and build with the box's Zig 0.16, which
quark-terminal needs (the `zig` on `PATH` is newer):

```bash
rsync -az --delete --exclude target --exclude .git ./ macbox:/tmp/wb-src/
ssh macbox 'cd /tmp/wb-src && ZIG=$HOME/.local/share/zig-0.16.0/zig CARGO_BUILD_JOBS=4 \
  CARGO_TARGET_DIR=/tmp/wb-target cargo build -p quark-workbench --bin workbench'
```

## Launch

A GUI binary started over plain ssh gets no window, so wrap it in a
minimal app bundle and `open` it. The launcher reads extra environment
(any `QUARK_WORKBENCH_*` variable) from `/tmp/wb/env`:

```bash
ssh macbox 'set -e; A=/tmp/wb/Workbench.app; mkdir -p $A/Contents/MacOS
cat > $A/Contents/Info.plist <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>launch</string>
<key>CFBundleIdentifier</key><string>dev.quark.workbench.check</string>
<key>CFBundleName</key><string>Quark Workbench</string>
<key>CFBundlePackageType</key><string>APPL</string>
</dict></plist>
EOF
printf "#!/bin/bash\n[ -f /tmp/wb/env ] && . /tmp/wb/env\nexec /tmp/wb-target/debug/workbench >/tmp/wb/app.log 2>&1\n" > $A/Contents/MacOS/launch
chmod +x $A/Contents/MacOS/launch
printf "export QUARK_WORKBENCH_THEME=light\nexport QUARK_WORKBENCH_MARKS=1\n" > /tmp/wb/env
open $A'
```

## Capture and drive

[macbox_drive.py](macbox_drive.py) runs `cua-driver call` with one session
label, so element tokens from a window snapshot stay valid for the next
click. Screenshots come from `get_window_state`, which captures the window
as it is now:

```bash
scp e2e/macbox_drive.py macbox:/tmp/wb/
ssh macbox 'cd /tmp/wb && python3 macbox_drive.py zoom wait:2 shot:zoomed'
scp macbox:/tmp/wb/zoomed.png target/e2e/artifacts/
ssh macbox 'pkill -x workbench'
```

## Known behavior on this box

- The window asks for 1440x900 and macOS shrinks it to the 1024x768
  display, but the first frames keep the 1440-point layout, scaled down,
  until a real resize arrives (the `zoom` step forces one). In the
  windowed state the traffic lights sit inside the top bar's reserved
  leading zone; full screen hides them.
- After `zoom` the window fills the display at 1024x768: the sidebar
  leaves the dock and the right dock collapses, per the width policy.
- `key:cmd+b` reports the key as pressed, but the sidebar overlay did not
  appear in the full-screen window. Whether the key reached the app is
  unverified; the same toggle works under Linux e2e
  (`e2e/specs/workbench/navigation.py`).
