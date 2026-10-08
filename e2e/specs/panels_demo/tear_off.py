"""Catches: a tab dragged off the window that stays inside it, a torn-off
window that does not follow the pointer or drops the grab point, a release
on the desktop that snaps back, and a drop into another window that is
ignored or leaves the emptied window open. With the main window shrunk to
leave desktop free beside it, Threads dragged from the sidebar onto the
desktop opens "Threads - Quark Panels" holding it under the pointer, which
follows the pointer and stays where it is released; dragged from there onto
the Chat panel, Threads joins the Chat group and its window closes."""

from quark_e2e import (
    Cua,
    app_frames,
    app_tree,
    center,
    main,
    pointer_drag,
    wait_for,
    window_closed,
    xdotool,
)

MAIN = "Quark Panels"
FLOATING = "Threads - Quark Panels"
THREADS = "dock:tab:1"
# Right of the shrunk main window, on the 1280x800 Xvfb screen.
DESKTOP = (1000, 400)


def frame(name):
    return next((f for f in app_frames() if f.name == name), None)


def node_by_id(window, node_id):
    root = frame(window)
    return root and next((n for n in root.walk() if n.attributes.get("id") == node_id), None)


def strip_of(window, tab_id):
    for node in frame(window).walk():
        if node.attributes.get("id", "").endswith(":tabs") and any(
            child.attributes.get("id") == tab_id for child in node.children
        ):
            return node.name
    return None


def near(a, b, slack=4):
    return abs(a[0] - b[0]) <= slack and abs(a[1] - b[1]) <= slack


def threads_under(window, point):
    tab = node_by_id(window, THREADS)
    return tab is not None and near(center(tab), point)


def spec(cua: Cua):
    app_tree(window=MAIN)
    wid = xdotool("search", "--sync", "--name", f"^{MAIN}$").splitlines()[0]
    xdotool("windowsize", "--sync", wid, 760, 560)
    xdotool("windowmove", "--sync", wid, 0, 0)
    wait_for("the main window to shrink", lambda: frame(MAIN).extents[2] <= 760)
    threads = wait_for("the Threads tab", lambda: node_by_id(MAIN, THREADS))
    start = center(threads)

    # Past the drag threshold, then out over the desktop, button held.
    path = [(start[0] + 10, start[1]), (700, start[1]), (820, 300), DESKTOP]
    pointer_drag(start, path, release=False)
    wait_for("Threads torn off under the pointer", lambda: threads_under(FLOATING, DESKTOP))
    later = (DESKTOP[0] + 60, DESKTOP[1] + 40)
    xdotool("mousemove", "--sync", *later)
    wait_for("the torn-off window to follow", lambda: threads_under(FLOATING, later))
    xdotool("mouseup", 1)
    wait_for("Threads to leave the main window", lambda: strip_of(MAIN, THREADS) is None)
    assert threads_under(FLOATING, later), "the window moved on release"

    chat = next(
        n for n in frame(MAIN).walk() if n.name == "Chat" and n.attributes.get("id", "").endswith(":panel")
    )
    target = center(chat)
    grabbed = center(node_by_id(FLOATING, THREADS))
    pointer_drag(grabbed, [(grabbed[0] - 10, grabbed[1]), (900, 300), (700, 300), target])
    window_closed(FLOATING)
    wait_for("Threads in the Chat group", lambda: strip_of(MAIN, THREADS) == "Chat")


main(spec)
