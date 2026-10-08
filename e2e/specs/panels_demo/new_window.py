"""Catches: "Move to new window" that leaves the tab behind, opens a window
without it or without focus on it, announces other than once, or a
floating window whose close loses its tab. From the keyboard, Shift+F10 on
the sidebar's Threads tab and "Move to new window" open a window titled
"Threads - Quark Panels" holding Threads, focused, with one "Moved Threads
to a new window" announcement; closing that window (Alt+F4) docks Threads
back into the sidebar, focused, announced once."""

from quark_e2e import (
    STATE_FOCUSED,
    Announcements,
    Cua,
    app_frames,
    app_pid,
    app_tree,
    main,
    wait_for,
    window_closed,
    xdotool,
)

MAIN = "Quark Panels"
FLOATING = "Threads - Quark Panels"
THREADS = "dock:tab:1"


def frame(name):
    return next((f for f in app_frames() if f.name == name), None)


def strip_of(window, tab_id):
    """The name of the tab list holding `tab_id` in the frame `window`."""
    for node in frame(window).walk():
        if node.attributes.get("id", "").endswith(":tabs") and any(
            child.attributes.get("id") == tab_id for child in node.children
        ):
            return node.name
    return None


def threads_focused_in(window, strip):
    tab = next(n for n in frame(window).walk() if n.attributes.get("id") == THREADS)
    return strip_of(window, THREADS) == strip and tab.has_state(STATE_FOCUSED)


def spec(cua: Cua):
    app_tree(window=MAIN)
    pid = app_pid()
    # The first key can arrive before the window takes keyboard focus under
    # Xvfb; send it again rather than wait out the whole timeout.
    for attempt in range(3):
        cua.call("press_key", pid=pid, key="tab", delivery_mode="foreground")
        try:
            wait_for("Threads to take focus", lambda: threads_focused_in(MAIN, "Sidebar"), timeout=5.0)
            break
        except AssertionError:
            if attempt == 2:
                raise

    announcements = Announcements()
    try:
        cua.call("hotkey", pid=pid, keys=["shift", "f10"], delivery_mode="foreground")
        wait_for("the move menu", lambda: cua.snapshot(pid).find("menu item", "Move to new window"))
        cua.press(pid, "menu item", "Move to new window")
        app_tree(window=FLOATING)
        wait_for("Threads to be focused in its own window", lambda: threads_focused_in(FLOATING, "Threads window"))
        assert strip_of(MAIN, THREADS) is None, "Threads is still in the main window"
        wait_for("the move to be announced", lambda: announcements.count("Moved Threads to a new window") == 1)

        xdotool("search", "--sync", "--name", f"^{FLOATING}$", "windowactivate", "--sync")
        xdotool("key", "alt+F4")
        window_closed(FLOATING)
        wait_for("Threads back in the sidebar, focused", lambda: threads_focused_in(MAIN, "Sidebar"))
        wait_for("the return to be announced", lambda: announcements.count("Moved Threads to Sidebar") == 1)
    finally:
        announcements.close()


main(spec)
