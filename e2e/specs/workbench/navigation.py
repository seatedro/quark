"""Catches: shell navigation that does not reach the screen: a sidebar
click that does not switch the selected thread and the title bar, Mod+B
that does not hide and restore the docked sidebar, or a narrow window that
keeps the sidebar docked instead of offering it as a dismissible overlay
while the composer stays on screen. Saves shell-light.png at 1240x740 and
shell-narrow.png at 1000x700 for review."""

# quark-e2e-env: QUARK_WORKBENCH_THEME=light

from quark_e2e import Cua, app_pid, app_tree, main, resize_window, save_screenshot, wait_for

TITLE = "Quark Workbench"
STATE_SELECTED = 23


def sidebar():
    return app_tree().find("list", "Threads")


def selected():
    s = sidebar()
    return [n.name for n in s.find_all("list item") if n.has_state(STATE_SELECTED)] if s else []


def title_shows(name):
    bar = app_tree().find("tool bar", "Workbench toolbar")
    return bar is not None and any(n.name == name for n in bar.walk())


def composer_inside(width, height):
    field = app_tree().find("entry", "Message")
    if field is None or field.extents is None:
        return False
    x, y, w, h = field.extents
    return x >= 0 and y >= 0 and x + w <= width and y + h <= height and w > 300


def hotkey(cua, pid, *keys):
    cua.call("hotkey", pid=pid, keys=list(keys), delivery_mode="foreground")


def spec(cua: Cua):
    app_tree()
    pid = app_pid()
    resize_window(TITLE, 1240, 740)
    wait_for("the review thread to be selected", lambda: selected() == ["Add keyboard shortcuts"])
    wait_for("the composer on screen", lambda: composer_inside(1240, 740))
    save_screenshot(cua, "shell-light")

    cua.press(pid, "list item", "Share trips as read-only links, unread")
    wait_for("Share trips to be selected", lambda: selected() == ["Share trips as read-only links"])
    wait_for("the title bar to follow", lambda: title_shows("Share trips as read-only links"))

    # The first key can arrive before the window has keyboard focus under
    # Xvfb; send it again rather than wait out the whole timeout.
    for attempt in range(3):
        hotkey(cua, pid, "ctrl", "b")
        try:
            wait_for("Mod+B to hide the sidebar", lambda: sidebar() is None, timeout=5.0)
            break
        except AssertionError:
            if attempt == 2:
                raise
    hotkey(cua, pid, "ctrl", "b")
    wait_for("Mod+B to restore the sidebar", lambda: selected() == ["Share trips as read-only links"])

    resize_window(TITLE, 1000, 700)
    wait_for("the sidebar to leave the dock", lambda: sidebar() is None)
    wait_for("the composer on screen when narrow", lambda: composer_inside(1000, 700))
    hotkey(cua, pid, "ctrl", "b")
    wait_for("the sidebar overlay", lambda: selected() == ["Share trips as read-only links"])
    save_screenshot(cua, "shell-narrow")
    cua.call("press_key", pid=pid, key="escape", delivery_mode="foreground")
    wait_for("Escape to dismiss the overlay", lambda: sidebar() is None)


main(spec)
