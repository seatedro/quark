"""Catches: a dock tab that cannot leave the main window, a torn-off
window that loses the panel's content or its keyboard input, a drop back
into the main window that leaves the floating window open, and a keyboard
"Move to new window" whose window does not dock its panel back when
closed. With the main window at 1200x700, the Terminal tab dragged with
the real pointer onto the desktop opens "Terminal - Quark Workbench"
showing the fixture test run; `help` typed there answers in that window;
dragged back onto the Diff panel, Terminal rejoins the right dock and its
window closes. Then Shift+F10 on the Files tab and "Move to new window"
open "Files - Quark Workbench", and closing it (Alt+F4) docks Files back.
Saves dock-floating.png with the floating terminal over the main window."""

# quark-e2e-env: QUARK_WORKBENCH_TERMINAL=scripted

from quark_e2e import (
    STATE_FOCUSED,
    Cua,
    app_frames,
    app_pid,
    app_tree,
    center,
    main,
    pointer_drag,
    resize_window,
    save_screenshot,
    wait_for,
    window_closed,
    xdotool,
)

MAIN = "Quark Workbench"
TERMINAL_WINDOW = "Terminal - Quark Workbench"
FILES_WINDOW = "Files - Quark Workbench"
TERMINAL_TAB = "dock:tab:11"
FILES_TAB = "dock:tab:12"
ROLE_TERMINAL = 60
# Right of the 1200-point main window, on the 1280x800 Xvfb screen.
DESKTOP = (1245, 420)


def frame(name):
    return next((f for f in app_frames() if f.name == name), None)


def node_by_id(window, node_id):
    root = frame(window)
    return root and next((n for n in root.walk() if n.attributes.get("id") == node_id), None)


def strip_of(window, tab_id):
    """The name of the tab list holding `tab_id` in the frame `window`."""
    root = frame(window)
    if root is None:
        return None
    for node in root.walk():
        if node.attributes.get("id", "").endswith(":tabs") and any(
            child.attributes.get("id") == tab_id for child in node.children
        ):
            return node.name
    return None


def terminal_text(window):
    root = frame(window)
    term = root and next((n for n in root.walk() if n.role == ROLE_TERMINAL), None)
    return term.text.text if term is not None and term.text else ""


def spec(cua: Cua):
    app_tree(window=MAIN)
    pid = app_pid()
    resize_window(MAIN, 1200, 700)
    wait_for("the main window to shrink", lambda: frame(MAIN).extents[2] <= 1200)
    tab = wait_for("the Terminal tab", lambda: node_by_id(MAIN, TERMINAL_TAB))
    start = center(tab)

    # Past the drag threshold, then out over the desktop, button held.
    path = [(start[0] + 12, start[1]), (start[0] + 40, start[1] + 30), (1190, 420), DESKTOP]
    pointer_drag(start, path, release=False)
    wait_for("Terminal torn off into its own window", lambda: frame(TERMINAL_WINDOW))
    xdotool("mouseup", 1)
    wait_for("Terminal to leave the main window", lambda: strip_of(MAIN, TERMINAL_TAB) is None)
    wait_for("the fixture run in the floating terminal", lambda: "13 passed" in terminal_text(TERMINAL_WINDOW))

    # Keyboard input reaches the panel in its new window.
    term = next(n for n in frame(TERMINAL_WINDOW).walk() if n.role == ROLE_TERMINAL)
    xdotool("mousemove", "--sync", *center(term))
    xdotool("click", 1)
    xdotool("type", "--delay", "20", "help")
    xdotool("key", "Return")
    wait_for("help to answer in the floating window", lambda: "Commands: help" in terminal_text(TERMINAL_WINDOW))
    # Over the main window's thread, so the review screenshot shows both.
    xdotool("search", "--sync", "--name", f"^{TERMINAL_WINDOW}$", "windowmove", "--sync", 300, 160)
    wait_for("the floating window to move", lambda: frame(TERMINAL_WINDOW).extents[0] < 400)
    save_screenshot(cua, "dock-floating")

    # Back onto the right dock's panel in the main window.
    right = max(
        (n for n in frame(MAIN).walk() if n.attributes.get("id", "").endswith(":panel")),
        key=lambda n: n.extents[0],
    )
    target = center(right)
    grabbed = center(node_by_id(TERMINAL_WINDOW, TERMINAL_TAB))
    pointer_drag(grabbed, [(grabbed[0] - 12, grabbed[1]), (900, 300), (target[0] + 20, target[1]), target])
    window_closed(TERMINAL_WINDOW)
    wait_for("Terminal back in the right dock", lambda: strip_of(MAIN, TERMINAL_TAB) == "Right dock")

    # Keyboard: Shift+F10 on the Files tab, "Move to new window".
    cua.press(pid, "page tab", "Files")
    wait_for("the Files tab focused", lambda: node_by_id(MAIN, FILES_TAB).has_state(STATE_FOCUSED))
    cua.call("hotkey", pid=pid, keys=["shift", "f10"], delivery_mode="foreground")
    wait_for("the move menu", lambda: cua.snapshot(pid).find("menu item", "Move to new window"))
    cua.press(pid, "menu item", "Move to new window")
    app_tree(window=FILES_WINDOW)
    assert strip_of(MAIN, FILES_TAB) is None, "Files is still in the main window"

    xdotool("search", "--sync", "--name", f"^{FILES_WINDOW}$", "windowactivate", "--sync")
    xdotool("key", "alt+F4")
    window_closed(FILES_WINDOW)
    wait_for("Files back in the right dock", lambda: strip_of(MAIN, FILES_TAB) == "Right dock")


main(spec)
