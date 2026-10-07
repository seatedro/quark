"""Catches: a checkbox or switch published with the wrong role or state.
The checkbox must be an AT-SPI check box that turns checked when clicked;
the switch must be a toggle button, pressed while on."""

from quark_e2e import STATE_CHECKED, STATE_PRESSED, Cua, app_pid, app_tree, main, wait_for


def spec(cua: Cua):
    frame = app_tree()
    remember = frame.require("check box", "Remember me")
    assert not remember.has_state(STATE_CHECKED), frame.dump()
    notify = frame.require("toggle button", "Notifications")
    assert notify.has_state(STATE_PRESSED), frame.dump()

    cua.click_node(app_pid(), remember)
    wait_for(
        "Remember me to be checked",
        lambda: app_tree().require("check box", "Remember me").has_state(STATE_CHECKED),
    )


main(spec)
