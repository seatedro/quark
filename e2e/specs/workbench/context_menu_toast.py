"""Catches: a context menu that opens away from the pointer or acts on its
own instead of through the shared commands, toasts spoken more than once,
and an Undo that does nothing. A real right-click on a thread row opens a
menu at the pointer; Copy title copies it and shows one toast, announced
once; the palette's "Apply proposed changes" shows a toast whose Undo
button reverts the files ("Changes undone")."""

# quark-e2e-env: QUARK_WORKBENCH_TERMINAL=scripted

from quark_e2e import Announcements, Cua, app_pid, app_tree, center, main, resize_window, wait_for, xdotool

THREAD = "Offline tile cache"
TITLE = "Quark Workbench"
COPIED = f"Copied “{THREAD}”"


def spec(cua: Cua):
    pid = app_pid()
    # The default 1440x900 window overflows the 1280x800 desktop.
    resize_window(TITLE, 1240, 740)
    row = app_tree().require("list item", THREAD)
    x, y = center(row)
    xdotool("mousemove", "--sync", x, y)
    xdotool("click", 3)
    wait_for("the thread's menu", lambda: app_tree().find("menu"))
    menu = app_tree().require("menu").extents
    assert abs(menu[0] - x) <= 2 and abs(menu[1] - y) <= 2, f"menu at {menu}, pointer at {(x, y)}"

    announcements = Announcements()
    try:
        cua.press(pid, "menu item", "Copy title")
        wait_for("the menu to close", lambda: app_tree().find("menu") is None)
        wait_for("the Copied toast", lambda: cua.snapshot(pid).find("status bar", COPIED))
        wait_for("the toast to be announced", lambda: announcements.count(COPIED) >= 1)
        wait_for(
            "the title on the clipboard",
            lambda: cua.call("clipboard_read", include_text=True).get("text") == THREAD,
        )

        cua.call("hotkey", pid=pid, keys=["ctrl", "k"], delivery_mode="foreground")
        wait_for("the palette", lambda: app_tree().find("dialog", "Command palette"))
        cua.call("type_text", pid=pid, text="Apply proposed", delivery_mode="foreground")
        cua.call("press_key", pid=pid, key="return", delivery_mode="foreground")
        wait_for("the Applied toast", lambda: cua.snapshot(pid).find("status bar", "Applied changes to 2 files"))
        cua.press(pid, "push button", "Undo")
        wait_for("Undo to revert the files", lambda: cua.snapshot(pid).find("status bar", "Changes undone"))
        assert announcements.count(COPIED) == 1, announcements.messages
    finally:
        announcements.close()


main(spec)
