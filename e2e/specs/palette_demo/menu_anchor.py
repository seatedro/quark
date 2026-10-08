"""Catches: a menu opened from the keyboard appearing at a fixed point
instead of at its trigger. Ctrl+Shift+O must open the Options menu with its
left edge on the Options button's and its top just below the button, as
AT-SPI reports both."""

from quark_e2e import Cua, app_pid, app_tree, main, wait_for


def spec(cua: Cua):
    pid = app_pid()
    options = app_tree().require("push button", "Options").extents
    cua.call("hotkey", pid=pid, keys=["ctrl", "shift", "o"], delivery_mode="foreground")
    wait_for("Ctrl+Shift+O to open a menu", lambda: app_tree().find("menu") is not None)
    tree = app_tree()
    menu = tree.require("menu").extents
    x, y, w, h = options
    # Extents are in screen pixels; the gap below the button is a few
    # points at any scale.
    assert abs(menu[0] - x) <= 1, f"menu at {menu}, Options at {options}:\n{tree.dump()}"
    assert y + h <= menu[1] <= y + h + h // 2, f"menu at {menu}, Options at {options}:\n{tree.dump()}"


main(spec)
