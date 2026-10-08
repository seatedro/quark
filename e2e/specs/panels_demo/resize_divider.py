"""Catches: a divider between split dock groups that the keyboard and
assistive tech cannot move, or that moves without its groups following.
Splitting the right panel from Shift+F10's "Split right" puts a divider
between Terminal's group and Preview's. Focused over AT-SPI, it publishes
its position in points with the range the groups' minimums allow; the
Right arrow moves it 10 points, and Value.SetCurrentValue past its maximum
stops it there. Each time Terminal's panel takes exactly what Preview's
gives up."""

from quark_e2e import STATE_FOCUSED, Cua, app_pid, app_tree, grab_focus, main, set_value, value_of, wait_for

PREVIEW_TAB = "dock:tab:10"


def divider():
    """The divider inside the right panel, once it is split."""
    for node in app_tree().walk():
        if node.attributes.get("id", "").startswith("dock:split:"):
            return node
    return None


def panel(name):
    """The tab panel showing `name`."""
    for node in app_tree().walk():
        if node.attributes.get("id", "").endswith(":panel") and node.name == name:
            return node
    raise AssertionError(f"no {name} panel:\n{app_tree().dump()}")


def widths():
    return panel("Terminal").extents[2], panel("Preview").extents[2]


def spec(cua: Cua):
    app_tree()
    pid = app_pid()
    cua.press_id(pid, PREVIEW_TAB)
    wait_for("Preview to take focus", lambda: app_tree().require_id(PREVIEW_TAB).has_state(STATE_FOCUSED))
    cua.call("hotkey", pid=pid, keys=["shift", "f10"], delivery_mode="foreground")
    wait_for("the Shift+F10 menu", lambda: cua.snapshot(pid).find("menu item", "Split right"))
    cua.press(pid, "menu item", "Split right")
    wait_for("the right panel to split", divider)

    grab_focus(divider())
    wait_for("the divider to take focus", lambda: divider().has_state(STATE_FOCUSED))
    at, least, most = value_of(divider())
    assert least < at < most, (at, least, most)
    terminal, preview = widths()

    cua.call("press_key", pid=pid, key="right", delivery_mode="foreground")
    wait_for("Right to move the divider 10 points", lambda: value_of(divider())[0] == at + 10)
    # Extents are in screen pixels; the window paints at scale 1 here.
    wait_for("the groups to follow", lambda: widths() == (terminal + 10, preview - 10))

    set_value(divider(), most + 500)
    wait_for("the divider to stop at its maximum", lambda: value_of(divider())[0] == most)
    wait_for("the groups to follow", lambda: widths() == (terminal + most - at, preview - (most - at)))


main(spec)
