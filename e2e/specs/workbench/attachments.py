"""Catches: attachment chips that cannot be added, removed from the
keyboard path, or restored. The attach button must add the fixture image
as a named chip; its Remove button must take it away and return focus to
the composer, and Ctrl+Z must put it back. Saves composer-attachment.png."""

# quark-e2e-env: QUARK_WORKBENCH_THEME=light
# quark-e2e-env: QUARK_WORKBENCH_TERMINAL=scripted

from quark_e2e import STATE_FOCUSED, Cua, app_pid, app_tree, main, resize_window, save_screenshot, wait_for

TITLE = "Quark Workbench"


def named(name):
    return next((n for n in app_tree().walk() if n.name == name), None)


def spec(cua: Cua):
    app_tree()
    pid = app_pid()
    resize_window(TITLE, 1240, 740)
    cua.press(pid, "entry", "Message")
    cua.call("type_text", pid=pid, text="Compare with this screenshot", delivery_mode="foreground")

    cua.press(pid, "push button", "Attach fixture image")
    wait_for("the attachment chip", lambda: named("attachment.png"))
    save_screenshot(cua, "composer-attachment")

    cua.press(pid, "push button", "Remove attachment.png")
    wait_for("the chip to go", lambda: named("attachment.png") is None)
    wait_for(
        "focus back in the composer",
        lambda: app_tree().require("entry", "Message").has_state(STATE_FOCUSED),
    )
    cua.call("hotkey", pid=pid, keys=["ctrl", "z"], delivery_mode="foreground")
    wait_for("Undo to restore the chip", lambda: named("attachment.png"))
    text = app_tree().require("entry", "Message").text
    assert text and text.text == "Compare with this screenshot", f"draft changed: {text}"


main(spec)
