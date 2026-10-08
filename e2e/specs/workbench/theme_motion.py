"""Catches: the workbench palette not reaching the window, a live edit to
the theme file not repainting it within a second, and a broken edit
changing or blanking the colors on screen.

The app watches QUARK_WORKBENCH_THEME_DIR (the runner points it at an
empty per-spec directory). The spec writes the shipped theme file there
with a new canvas color in both modes, times how long the window takes to
show it, then writes half a file and checks the colors stay. With
QUARK_E2E_SHOTS set, it also saves hover and keyboard focus screenshots
in the theme it found and in the edited one.

    QUARK_E2E_BIN_DIR=$PWD/target/debug e2e/run.sh e2e/specs/workbench/theme_motion.py
"""

import json
import os
import time

from PIL import Image

from quark_e2e import Cua, app_tree, center, main, wait_for, xdotool

HERE = os.path.dirname(os.path.abspath(__file__))
THEME_FILE = os.path.join(HERE, "../../../examples/workbench/assets/themes/workbench.json")
CANVAS = {"light": (0xFA, 0xF9, 0xF6), "dark": (0x15, 0x17, 0x1C)}
EDITED = (0x3A, 0x1F, 0x6E)
# The timeline canvas covers far more of the window than this.
MIN_SHARE = 0.08


def share(cua, rgb, path):
    """The fraction of the screen within 2 of `rgb` per channel."""
    assert cua.screenshot(path), "no screenshot"
    image = Image.open(path).convert("RGB")
    pixels = image.getdata()
    near = sum(1 for p in pixels if all(abs(a - b) <= 2 for a, b in zip(p, rgb)))
    return near / len(pixels)


def shots(cua, label, frame):
    """Hover a thread row, then Tab to a control, saving each."""
    out = os.environ.get("QUARK_E2E_SHOTS")
    if not out:
        return
    os.makedirs(out, exist_ok=True)
    rows = frame.find_all("list item")
    if rows:
        x, y = center(rows[min(1, len(rows) - 1)])
        xdotool("mousemove", x, y)
        time.sleep(0.3)  # the 100 ms hover fade, plus a frame
        cua.screenshot(os.path.join(out, f"workbench-{label}-hover.png"))
    xdotool("key", "Tab")
    time.sleep(0.2)
    cua.screenshot(os.path.join(out, f"workbench-{label}-focus.png"))


def spec(cua: Cua):
    theme_dir = os.environ.get("QUARK_WORKBENCH_THEME_DIR")
    assert theme_dir, "set QUARK_WORKBENCH_THEME_DIR to an empty directory for the app"
    os.makedirs(theme_dir, exist_ok=True)
    scratch = os.environ.get("TMPDIR", "/tmp")
    frame = app_tree()

    def mode_on_screen():
        for mode, rgb in CANVAS.items():
            if share(cua, rgb, os.path.join(scratch, "start.png")) >= MIN_SHARE:
                return mode
        return None

    mode = wait_for("the workbench canvas color", mode_on_screen)
    shots(cua, mode, frame)

    with open(THEME_FILE) as f:
        family = json.load(f)
    for variant in ("light", "dark"):
        family[variant]["palette"]["canvas"] = "#%02x%02x%02x" % EDITED
    edited = json.dumps(family, indent=2)
    target = os.path.join(theme_dir, "workbench.json")
    started = time.monotonic()
    with open(target, "w") as f:
        f.write(edited)
    probe = os.path.join(scratch, "edited.png")
    wait_for("the edited canvas", lambda: share(cua, EDITED, probe) >= MIN_SHARE, timeout=5.0, interval=0.02)
    elapsed_ms = (time.monotonic() - started) * 1000
    print(f"theme edit visible after {elapsed_ms:.0f} ms (includes screenshot time)")
    assert elapsed_ms < 1000, f"theme edit took {elapsed_ms:.0f} ms to show"
    shots(cua, f"{mode}-edited", frame)

    # A half-written file: the window keeps the edited colors.
    with open(target, "w") as f:
        f.write(edited[: len(edited) // 2])
    deadline = time.monotonic() + 1.0
    while time.monotonic() < deadline:
        kept = share(cua, EDITED, probe)
        assert kept >= MIN_SHARE, f"a broken theme edit changed the canvas (share {kept:.2f})"
        time.sleep(0.1)


main(spec)
