"""Catches: toasts appearing silently for screen reader users, or being
spoken again on every frame. Save shows a toast, a live status region;
AT-SPI must announce its message exactly once, also after later frames."""

from quark_e2e import Announcements, Cua, app_pid, app_tree, main, wait_for

MESSAGE = "Settings saved (1)"


def spec(cua: Cua):
    app_tree()
    pid = app_pid()
    announcements = Announcements()
    try:
        cua.press_id(pid, "a11y.save")
        wait_for("the toast's status region", lambda: cua.snapshot(pid).find("status bar", MESSAGE))
        wait_for("the toast to be announced", lambda: announcements.count(MESSAGE) >= 1)

        # Repaint (check the checkbox) and make sure the toast stayed quiet.
        cua.press(pid, "check box", "Remember me")
        wait_for("the repaint", lambda: cua.snapshot(pid).element("check box", "Remember me").get("selected"))
        assert announcements.count(MESSAGE) == 1, announcements.messages
    finally:
        announcements.close()


main(spec)
