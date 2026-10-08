"""Catches: the keyboard contract breaking or a modal letting shortcuts act
behind it. Ctrl+B hides and shows the sidebar; Ctrl+, opens Settings with
focus inside, where Ctrl+B and Ctrl+K do nothing and Tab never leaves the
dialog; Escape closes it; Ctrl+K opens the palette; and a run's tool and
run completions are each announced once."""

from quark_e2e import STATE_FOCUSED, Announcements, Cua, app_pid, app_tree, main, resize_window, wait_for

THREAD = "Add keyboard shortcuts"
TITLE = "Quark Workbench"


def keys(cua, pid, *names):
    cua.call("hotkey", pid=pid, keys=list(names), delivery_mode="foreground")


def sidebar_shown():
    return app_tree().find("list item", THREAD) is not None


def focused_names(node, inside=False):
    """Names of focused nodes, with whether each sits in the Settings dialog."""
    inside = inside or (node.role == 16 and node.name == "Settings")
    found = [(node.name, inside)] if node.has_state(STATE_FOCUSED) else []
    for child in node.children:
        found += focused_names(child, inside)
    return found


def spec(cua: Cua):
    pid = app_pid()
    # The default 1440x900 window overflows the 1280x800 desktop.
    resize_window(TITLE, 1240, 740)
    app_tree()
    # The first key can arrive before the window takes keyboard focus
    # under Xvfb; send it again rather than wait out the whole timeout.
    for attempt in range(3):
        keys(cua, pid, "ctrl", "b")
        try:
            wait_for("Ctrl+B to hide the sidebar", lambda: not sidebar_shown(), timeout=5.0)
            break
        except AssertionError:
            if attempt == 2:
                raise
    keys(cua, pid, "ctrl", "b")
    wait_for("Ctrl+B to show the sidebar", sidebar_shown)

    keys(cua, pid, "ctrl", "comma")
    wait_for("Settings to open", lambda: app_tree().find("dialog", "Settings"))
    keys(cua, pid, "ctrl", "b")
    keys(cua, pid, "ctrl", "k")
    for _ in range(6):
        cua.call("press_key", pid=pid, key="tab", delivery_mode="foreground")
        wait_for("focus inside Settings", lambda: [f for f in focused_names(app_tree()) if f[1]])
    tree = app_tree()
    assert tree.find("list item", THREAD) is not None, "Ctrl+B acted behind Settings"
    assert tree.find("dialog", "Command palette") is None, "Ctrl+K opened the palette over Settings"
    cua.call("press_key", pid=pid, key="escape", delivery_mode="foreground")
    wait_for("Settings to close", lambda: app_tree().find("dialog", "Settings") is None)

    keys(cua, pid, "ctrl", "k")
    wait_for("Ctrl+K to open the palette", lambda: app_tree().find("dialog", "Command palette"))
    cua.call("press_key", pid=pid, key="escape", delivery_mode="foreground")
    wait_for("the palette to close", lambda: app_tree().find("dialog", "Command palette") is None)

    announcements = Announcements()
    try:
        cua.press(pid, "entry", "Message")
        cua.call("type_text", pid=pid, text="Make it layout independent", delivery_mode="foreground")
        cua.call("press_key", pid=pid, key="return", delivery_mode="foreground")
        wait_for(
            "the run to complete",
            lambda: any("Run complete" in m for m in announcements.messages),
            timeout=30.0,
        )
        # Completions a frame plays together share one announcement.
        tools = sum(m.count("Tool finished") for m in announcements.messages)
        runs = sum(m.count("Run complete") for m in announcements.messages)
        assert (tools, runs) == (3, 1), announcements.messages
    finally:
        announcements.close()


main(spec)
