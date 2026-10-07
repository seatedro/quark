"""Catches: toasts appearing silently for screen reader users, or being
spoken again on every frame. Save shows a toast, a live status region;
AT-SPI must announce its message exactly once, also after later frames."""

from quark_e2e import STATE_CHECKED, Announcements, Cua, app_pid, app_tree, main, wait_for

MESSAGE = "Settings saved (1)"


def spec(cua: Cua):
    pid = app_pid()
    announcements = Announcements()
    try:
        cua.click_node(pid, app_tree().require("push button", "Save"))
        wait_for("the toast's status region", lambda: app_tree().find("status bar", MESSAGE))
        wait_for("the toast to be announced", lambda: announcements.count(MESSAGE) >= 1)

        # Repaint (check the checkbox) and make sure the toast stayed quiet.
        cua.click_node(pid, app_tree().require("check box", "Remember me"))
        wait_for(
            "the repaint",
            lambda: app_tree().require("check box", "Remember me").has_state(STATE_CHECKED),
        )
        assert announcements.count(MESSAGE) == 1, announcements.messages
    finally:
        announcements.close()


main(spec)
