"""Catches: a checkbox or switch published with the wrong role or state.
The checkbox must be an AT-SPI check box that turns checked when clicked;
the switch must be a toggle button, pressed while on."""

from quark_e2e import STATE_PRESSED, Cua, app_pid, app_tree, main, wait_for


def spec(cua: Cua):
    frame = app_tree()
    pid = app_pid()
    snapshot = cua.snapshot(pid)
    remember = snapshot.element("check box", "Remember me")
    assert not remember.get("selected"), remember
    snapshot.element("toggle button", "Notifications")
    # cua reports checked but not pressed; read that over D-Bus.
    assert frame.require("toggle button", "Notifications").has_state(STATE_PRESSED), frame.dump()

    cua.click_element(pid, remember)
    wait_for(
        "Remember me to be checked",
        lambda: cua.snapshot(pid).element("check box", "Remember me").get("selected"),
    )


main(spec)
