"""Catches: `UiContext::announce` not reaching screen readers, repeating,
or swallowing a repeated message. Each press of Check announces "All
checks passed" once, the second press included."""

from quark_e2e import STATE_CHECKED, Announcements, Cua, app_pid, app_tree, main, wait_for

MESSAGE = "All checks passed"


def spec(cua: Cua):
    pid = app_pid()
    announcements = Announcements()
    try:
        cua.click_node(pid, app_tree().require("push button", "Check"))
        wait_for("the first announcement", lambda: announcements.count(MESSAGE) == 1)

        # A repaint (checking the checkbox) must not repeat it.
        cua.click_node(pid, app_tree().require("check box", "Remember me"))
        wait_for(
            "the repaint",
            lambda: app_tree().require("check box", "Remember me").has_state(STATE_CHECKED),
        )
        assert announcements.count(MESSAGE) == 1, announcements.messages

        cua.click_node(pid, app_tree().require("push button", "Check"))
        wait_for("the second announcement", lambda: announcements.count(MESSAGE) == 2)
    finally:
        announcements.close()


main(spec)
