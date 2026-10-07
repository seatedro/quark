"""Catches: an AT-SPI Click on Greet not reaching the app's on_click, or
Greet losing its stable id. cua clicks the element it indexed for the
`hello.greet` id through the AT-SPI action, so the greeting label must
change without any pointer or keyboard focus."""

from quark_e2e import Cua, app_pid, app_tree, main, wait_for


def greeting(cua, pid):
    # The only label directly in the dialog; the heading's text sits one level down.
    dialog = cua.snapshot(pid).find("dialog", "Hello Quark")
    labels = [c.name for c in dialog.children if c.role == "label"]
    assert len(labels) == 1, f"expected one label in the dialog, found {labels}:\n{dialog.dump()}"
    return labels[0]


def spec(cua: Cua):
    app_tree()
    pid = app_pid()
    assert greeting(cua, pid) == "Type a name, then press Greet."
    cua.press_id(pid, "hello.greet")
    wait_for("the greeting to change", lambda: greeting(cua, pid) == "Hello, stranger!")


main(spec)
