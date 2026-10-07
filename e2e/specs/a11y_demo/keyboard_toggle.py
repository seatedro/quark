"""Catches: a control a pointer can press being unreachable from the
keyboard. Tab from the Name entry must move focus to the Remember me
checkbox, with AT-SPI reporting it focused, and Space must check it."""

from quark_e2e import STATE_FOCUSED, Cua, app_pid, app_tree, main, wait_for


def checkbox():
    return app_tree().require("check box", "Remember me")


def spec(cua: Cua):
    pid = app_pid()
    # cua reports no focus, so focus is read over D-Bus.
    cua.press(pid, "entry", "Name")
    wait_for("the Name entry to take focus", lambda: app_tree().require("entry", "Name").has_state(STATE_FOCUSED))
    cua.call("press_key", pid=pid, key="tab", delivery_mode="foreground")
    wait_for("Tab to focus the checkbox", lambda: checkbox().has_state(STATE_FOCUSED))
    cua.call("press_key", pid=pid, key="space", delivery_mode="foreground")
    wait_for(
        "Space to check the checkbox",
        lambda: cua.snapshot(pid).element("check box", "Remember me").get("selected"),
    )


main(spec)
