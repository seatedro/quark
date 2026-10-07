"""Catches: an AT-SPI Click on Greet not reaching the app's on_click. The
click goes through cua's accessibility route, so the greeting label must
change without any pointer or keyboard focus."""

from quark_e2e import ROLE, Cua, app_pid, app_tree, main, wait_for


def greeting(frame):
    # The only label directly in the dialog; the heading's text sits one level down.
    (label,) = [c for c in frame.find("dialog").children if c.role == ROLE["label"]]
    return label.name


def spec(cua: Cua):
    frame = app_tree()
    assert greeting(frame) == "Type a name, then press Greet.", frame.dump()
    result = cua.click_node(app_pid(), frame.find("push button", "Greet"))
    assert result.get("route") == "accessibility", f"click did not use the AT-SPI action: {result}"
    wait_for("the greeting to change", lambda: greeting(app_tree()) == "Hello, stranger!")


main(spec)
