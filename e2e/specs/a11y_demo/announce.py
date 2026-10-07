"""Catches: `UiContext::announce` not reaching screen readers, repeating,
or swallowing a repeated message. Each press of Check announces "All
checks passed" once, the second press included."""

from quark_e2e import Announcements, Cua, app_pid, app_tree, main, wait_for

MESSAGE = "All checks passed"


def spec(cua: Cua):
    app_tree()
    pid = app_pid()
    announcements = Announcements()
    try:
        cua.press_id(pid, "a11y.check")
        wait_for("the first announcement", lambda: announcements.count(MESSAGE) == 1)

        # A repaint (checking the checkbox) must not repeat it.
        cua.press(pid, "check box", "Remember me")
        wait_for("the repaint", lambda: cua.snapshot(pid).element("check box", "Remember me").get("selected"))
        assert announcements.count(MESSAGE) == 1, announcements.messages

        cua.press_id(pid, "a11y.check")
        wait_for("the second announcement", lambda: announcements.count(MESSAGE) == 2)
    finally:
        announcements.close()


main(spec)
