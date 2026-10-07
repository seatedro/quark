"""Catches: typed text not reaching a focused text field. An AT-SPI click
focuses the Name entry, real key events type into it, and Greet must then
greet that name. (a11y_demo/field_text reads a field's text over AT-SPI
directly.)"""

from quark_e2e import STATE_FOCUSED, Cua, app_pid, app_tree, main, wait_for


def spec(cua: Cua):
    pid = app_pid()
    cua.press(pid, "entry", "Name")
    wait_for("the Name entry to take focus", lambda: app_tree().require("entry", "Name").has_state(STATE_FOCUSED))
    cua.call("type_text", pid=pid, text="Ada", delivery_mode="foreground")
    cua.press_id(pid, "hello.greet")
    wait_for("the greeting to name Ada", lambda: cua.snapshot(pid).find("label", "Hello, Ada!"))


main(spec)
