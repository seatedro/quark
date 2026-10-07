"""Catches: typed text not reaching a focused text field. A real pointer
click focuses the Name entry, real key events type into it, and Greet must
then greet that name. AccessKit exposes no AT-SPI text interface, so the
greeting is the only way to observe the field's value."""

from quark_e2e import STATE_FOCUSED, Cua, app_pid, app_tree, main, wait_for


def spec(cua: Cua):
    pid = app_pid()
    cua.click_node(pid, app_tree().find("entry", "Name"), delivery_mode="foreground")
    wait_for("the Name entry to take focus", lambda: app_tree().find("entry", "Name").has_state(STATE_FOCUSED))
    cua.call("type_text", pid=pid, text="Ada", delivery_mode="foreground")
    cua.click_node(pid, app_tree().find("push button", "Greet"))
    wait_for(
        "the greeting to name Ada",
        lambda: app_tree().find("label", "Hello, Ada!"),
    )


main(spec)
