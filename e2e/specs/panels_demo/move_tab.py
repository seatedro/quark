"""Catches: a tab moved from the keyboard that lands in the wrong group,
leaves focus behind, is announced other than once, or moves although the
dock refuses. With the drawer shown and the sidebar's Threads tab focused,
Ctrl+Shift+Page Up changes nothing (no group comes before the sidebar);
Ctrl+Shift+Page Down moves Threads into the Chat group, focused, with one
"Moved Threads to Chat" announcement; and Shift+F10 opens the "Move to
group" menu, whose "Move to Drawer" moves it on the same way."""

from quark_e2e import STATE_FOCUSED, Announcements, Cua, app_pid, app_tree, main, wait_for

THREADS = "dock:tab:1"
DRAWER_TAB = "dock:tab:20"


def strip_of(tab_id):
    """The name of the dock tab list holding the tab `tab_id`."""
    for node in app_tree().walk():
        if node.attributes.get("id", "").endswith(":tabs") and any(
            child.attributes.get("id") == tab_id for child in node.children
        ):
            return node.name
    return None


def threads_focused_in(strip):
    return strip_of(THREADS) == strip and app_tree().require_id(THREADS).has_state(STATE_FOCUSED)


def keys(cua, pid, *names):
    cua.call("hotkey", pid=pid, keys=list(names), delivery_mode="foreground")


def spec(cua: Cua):
    app_tree()
    pid = app_pid()
    keys(cua, pid, "ctrl", "j")
    wait_for("the drawer to show", lambda: strip_of(DRAWER_TAB) == "Drawer")
    # Nothing has focus yet: the first Tab stop is the sidebar's tab.
    cua.call("press_key", pid=pid, key="tab", delivery_mode="foreground")
    wait_for("Threads to take focus", lambda: threads_focused_in("Sidebar"))

    announcements = Announcements()
    try:
        keys(cua, pid, "ctrl", "shift", "pageup")
        keys(cua, pid, "ctrl", "shift", "pagedown")
        wait_for("Threads to move into Chat, focused", lambda: threads_focused_in("Chat"))
        wait_for("the move to be announced", lambda: announcements.count("Moved Threads to Chat") == 1)
        # The refused Page Up came first and announced nothing.
        assert announcements.messages == ["Moved Threads to Chat"], announcements.messages

        keys(cua, pid, "shift", "f10")
        wait_for("the Move to group menu", lambda: cua.snapshot(pid).find("menu item", "Move to Drawer"))
        cua.press(pid, "menu item", "Move to Drawer")
        wait_for("Threads to move into the drawer, focused", lambda: threads_focused_in("Drawer"))
        wait_for("the second move to be announced", lambda: announcements.count("Moved Threads to Drawer") == 1)
    finally:
        announcements.close()


main(spec)
