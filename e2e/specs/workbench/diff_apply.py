"""Catches: Apply or Undo that do not reach the demo file store, badges
that do not follow it, and a diff that never offers side by side. In the
Diff panel (unified at the dock's 400 points), Apply turns the
"Proposed" badge into "Applied", the Files panel shows App.tsx with the
new keydown listener and a "modified" header, and the Snapshot preview
reads "Updated from proposed changes"; Undo restores all three. Widening
the right dock past 600 points offers Split, which selects. Saves
diff-unified.png and diff-split.png."""

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
ADDED = 'window.addEventListener("keydown", onKeyDown);'


def named(name):
    return next((n for n in app_tree().walk() if n.name == name), None)


def source():
    entry = app_tree().find("entry", "File source")
    return entry.text.text if entry is not None and entry.text else ""


def spec(cua: Cua):
    app_tree()
    pid = app_pid()
    resize_window(TITLE, 1240, 740)
    wait_for("the proposed diff", lambda: named("Proposed"))
    assert app_tree().find("radio button", "Split") is None, "Split offered at 400 points"
    save_screenshot(cua, "diff-unified")

    cua.press(pid, "push button", "Apply")
    wait_for("the Applied badge", lambda: named("Applied"))
    cua.press(pid, "page tab", "Files")
    wait_for("App.tsx with the listener", lambda: ADDED in source())
    wait_for("the modified header", lambda: named("src/App.tsx, modified"))
    cua.press(pid, "page tab", "Snapshot preview")
    wait_for("the preview to follow", lambda: named("Updated from proposed changes"))

    cua.press(pid, "page tab", "Diff")
    cua.press(pid, "push button", "Undo")
    wait_for("the Proposed badge again", lambda: named("Proposed"))
    cua.press(pid, "page tab", "Files")
    wait_for("App.tsx restored", lambda: source() and ADDED not in source())
    wait_for("the header unmarked", lambda: named("src/App.tsx"))
    cua.press(pid, "page tab", "Snapshot preview")
    wait_for("the preview restored", lambda: named("Snapshot of the current build"))

    cua.press(pid, "page tab", "Diff")
    set_value(named("Resize Right dock"), 640)
    split = wait_for("Split to be offered", lambda: app_tree().find("radio button", "Split"))
    assert not split.has_state(STATE_CHECKED), "Split chosen before it was picked"
    cua.press(pid, "radio button", "Split")
    wait_for("Split selected", lambda: app_tree().find("radio button", "Split").has_state(STATE_CHECKED))
    save_screenshot(cua, "diff-split")


main(spec)
