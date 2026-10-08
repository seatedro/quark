"""Catches: keyboard focus that cannot reach the controls inside the
workbench's clipping containers. Tab moves focus through the Settings
body's fields, from the sidebar search into the thread list, from the
composer into its pickers, and from a dock tab into its panel; each stop
is saved as a screenshot (focus-*.png) for checking that the focus ring
is drawn whole, not cut by the container's edge."""

from quark_e2e import STATE_FOCUSED, Cua, app_pid, app_tree, main, resize_window, save_screenshot, wait_for

TITLE = "Quark Workbench"


def keys(cua, pid, *names):
    cua.call("hotkey", pid=pid, keys=list(names), delivery_mode="foreground")


def tab(cua, pid):
    cua.call("press_key", pid=pid, key="tab", delivery_mode="foreground")


def focused():
    """Names of the focused nodes, innermost last."""
    return [n.name for n in app_tree().walk() if n.has_state(STATE_FOCUSED)]


def focus_moves(cua, pid, what):
    """Press Tab and wait for focus to land somewhere new."""
    before = focused()
    tab(cua, pid)
    return wait_for(what, lambda: (now := focused()) and now != before and now)


def spec(cua: Cua):
    pid = app_pid()
    # The default 1440x900 window overflows the 1280x800 desktop.
    resize_window(TITLE, 1240, 740)
    app_tree()

    # The first key can arrive before the window takes keyboard focus
    # under Xvfb; send it again rather than wait out the whole timeout.
    for attempt in range(3):
        keys(cua, pid, "ctrl", "comma")
        try:
            wait_for("Settings to open", lambda: app_tree().find("dialog", "Settings"), timeout=5.0)
            break
        except AssertionError:
            if attempt == 2:
                raise
    wait_for("focus inside Settings", focused)
    save_screenshot(cua, "focus-settings")
    print("settings:", focus_moves(cua, pid, "focus to the next Settings field"))
    save_screenshot(cua, "focus-settings-next")
    cua.call("press_key", pid=pid, key="escape", delivery_mode="foreground")
    wait_for("Settings to close", lambda: app_tree().find("dialog", "Settings") is None)

    cua.press(pid, "entry", "Search threads")
    wait_for("the sidebar search to take focus", lambda: "Search threads" in focused())
    save_screenshot(cua, "focus-sidebar-search")
    print("sidebar:", focus_moves(cua, pid, "focus to leave the search field"))
    save_screenshot(cua, "focus-sidebar-list")

    cua.press(pid, "entry", "Message")
    wait_for("the composer to take focus", lambda: "Message" in focused())
    print("composer:", focus_moves(cua, pid, "focus to leave the composer text"))
    save_screenshot(cua, "focus-composer")

    cua.press(pid, "page tab", "Files")
    wait_for("the Files tab to take focus", lambda: "Files" in focused())
    save_screenshot(cua, "focus-dock-tab")
    # Past the tab strip's close buttons, into the panel.
    for _ in range(6):
        now = focus_moves(cua, pid, "focus to move along the dock")
        print("dock:", now)
        if not now[-1].startswith("Close"):
            break
    save_screenshot(cua, "focus-dock-panel")


main(spec)
