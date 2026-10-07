"""Catches: a radio group whose arrow keys do not move the choice, or move
it without moving focus, so a screen reader stays on the old option.
Clicking Medium and pressing Down must check Large and report it focused
over AT-SPI, with Medium no longer checked."""

from quark_e2e import STATE_CHECKED, STATE_FOCUSED, Cua, app_pid, app_tree, main, wait_for


def radio(name):
    return app_tree().require("radio button", name)


def spec(cua: Cua):
    pid = app_pid()
    cua.click_node(pid, radio("Medium"), delivery_mode="foreground")
    wait_for("Medium to take focus", lambda: radio("Medium").has_state(STATE_FOCUSED))
    cua.call("press_key", pid=pid, key="down", delivery_mode="foreground")
    wait_for(
        "Down to check and focus Large",
        lambda: radio("Large").has_state(STATE_CHECKED) and radio("Large").has_state(STATE_FOCUSED),
    )
    assert not radio("Medium").has_state(STATE_CHECKED), app_tree().dump()


main(spec)
