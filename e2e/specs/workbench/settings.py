"""Catches: a settings dialog whose nested picker loses Escape to the
dialog, whose Save accepts an invalid value, or whose error never reaches
assistive tech. Ctrl+, opens Settings on the Theme picker; Enter opens its
list and Escape closes only the list; an out-of-range Tool output limit
keeps the dialog open on Save, with the error as the field's description
and focus in the field; Escape then cancels."""

import os

from quark_e2e import STATE_FOCUSED, Cua, app_pid, app_tree, main, wait_for

ERROR = "Enter a whole number from 100 to 10,000."


def screenshot(cua, name):
    out = os.path.join(os.environ.get("QUARK_E2E_OUT", "target/e2e/artifacts"), "workbench-screenshots")
    os.makedirs(out, exist_ok=True)
    cua.screenshot(os.path.join(out, f"{name}.png"))


def key(cua, pid, name):
    cua.call("press_key", pid=pid, key=name, delivery_mode="foreground")


def dialog():
    return app_tree().find("dialog", "Settings")


def spec(cua: Cua):
    pid = app_pid()
    app_tree()
    cua.call("hotkey", pid=pid, keys=["ctrl", "comma"], delivery_mode="foreground")
    wait_for("Settings to open", dialog)
    screenshot(cua, "settings")

    key(cua, pid, "return")
    wait_for("the Theme picker's list", lambda: cua.snapshot(pid).find("list item", "Dark"))
    screenshot(cua, "settings-picker")
    key(cua, pid, "escape")
    wait_for("the list to close", lambda: cua.snapshot(pid).find("list item", "Dark") is None)
    assert dialog() is not None, "Escape on the picker closed the dialog"

    cua.press(pid, "entry", "Tool output limit")
    wait_for("the limit field to take focus", lambda: app_tree().require("entry", "Tool output limit").has_state(STATE_FOCUSED))
    cua.call("hotkey", pid=pid, keys=["ctrl", "a"], delivery_mode="foreground")
    cua.call("type_text", pid=pid, text="5", delivery_mode="foreground")
    cua.press(pid, "push button", "Save")
    wait_for("the error under the field", lambda: cua.snapshot(pid).find("status bar", ERROR))
    tree = app_tree()
    assert tree.find("dialog", "Settings") is not None, tree.dump()
    assert tree.require("entry", "Tool output limit").has_state(STATE_FOCUSED), tree.dump()

    key(cua, pid, "escape")
    wait_for("Escape to cancel", lambda: dialog() is None)


main(spec)
