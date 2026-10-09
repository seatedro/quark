"""Catches: Diff panel controls that do nothing in the real app. Find in
diff counts matches as typed and steps with Next; Expand all unchanged
lines shows App.tsx lines outside the hunks (the patch is backed by the
fixture sources); a wide dock offers Auto (chosen) and Split, which shows
unchanged lines on both sides. Saves diff-find.png and diff-split.png."""

# quark-e2e-env: QUARK_WORKBENCH_TERMINAL=scripted

from quark_e2e import (
    STATE_CHECKED,
    Cua,
    app_pid,
    app_tree,
    main,
    resize_window,
    save_screenshot,
    set_value,
    wait_for,
)

TITLE = "Quark Workbench"
OUTSIDE = "    </main>"


def named(name):
    return next((n for n in app_tree().walk() if n.name == name), None)


def spec(cua: Cua):
    app_tree()
    pid = app_pid()
    resize_window(TITLE, 1240, 740)
    wait_for("the proposed diff", lambda: named("Proposed"))

    cua.press(pid, "push button", "Find in diff")
    cua.call("type_text", pid=pid, text="onKeyDown", delivery_mode="foreground")
    wait_for("the match count", lambda: named("1 of 3"))
    cua.press(pid, "push button", "Next match")
    wait_for("the second match", lambda: named("2 of 3"))
    save_screenshot(cua, "diff-find")
    cua.press(pid, "push button", "Close find")

    assert named(OUTSIDE) is None, "context outside the hunks shows before expanding"
    cua.press(pid, "push button", "Expand all unchanged lines")
    wait_for("App.tsx's closing tag", lambda: named(OUTSIDE))

    set_value(named("Resize Right dock"), 700)
    auto = wait_for("Auto to be offered", lambda: app_tree().find("radio button", "Auto"))
    assert auto.has_state(STATE_CHECKED), "Auto is not the default layout"
    cua.press(pid, "radio button", "Split")
    # Split shows unchanged lines on both sides.
    wait_for("the split diff", lambda: sum(
        1 for n in app_tree().walk() if n.name == "import { TripList } from \"./trips\";") == 2)
    save_screenshot(cua, "diff-split")


main(spec)
