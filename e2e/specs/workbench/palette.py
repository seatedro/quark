"""Catches: a command palette that runs commands its own way or forgets
who opened it. Ctrl+K from the composer opens the palette with its search
field focused; typing "Settings" and Enter opens the Settings dialog, the
same one Ctrl+, opens; Escape closes that, and a second Ctrl+K then Escape
gives focus back to the composer."""

from quark_e2e import STATE_FOCUSED, Cua, app_pid, app_tree, main, resize_window, save_screenshot, save_tree, wait_for

TITLE = "Quark Workbench"


def keys(cua, pid, *names):
    cua.call("hotkey", pid=pid, keys=list(names), delivery_mode="foreground")


def composer_focused():
    return app_tree().require("entry", "Message").has_state(STATE_FOCUSED)


def spec(cua: Cua):
    pid = app_pid()
    # The default 1440x900 window overflows the 1280x800 desktop.
    resize_window(TITLE, 1240, 740)
    cua.press(pid, "entry", "Message")
    wait_for("the composer to take focus", composer_focused)

    keys(cua, pid, "ctrl", "k")
    wait_for("the palette, focused", lambda: app_tree().require("entry", "Command palette").has_state(STATE_FOCUSED))
    save_screenshot(cua, "palette")
    save_tree("palette")
    cua.call("type_text", pid=pid, text="Settings", delivery_mode="foreground")
    cua.call("press_key", pid=pid, key="return", delivery_mode="foreground")
    wait_for("Settings to open from the palette", lambda: app_tree().find("dialog", "Settings"))
    assert app_tree().find("dialog", "Command palette") is None, app_tree().dump()

    cua.call("press_key", pid=pid, key="escape", delivery_mode="foreground")
    wait_for("Settings to close", lambda: app_tree().find("dialog", "Settings") is None)
    wait_for("focus back in the composer", composer_focused)

    keys(cua, pid, "ctrl", "k")
    wait_for("the palette to open", lambda: app_tree().find("dialog", "Command palette"))
    cua.call("press_key", pid=pid, key="escape", delivery_mode="foreground")
    wait_for("the palette to close", lambda: app_tree().find("dialog", "Command palette") is None)
    wait_for("focus back in the composer", composer_focused)


main(spec)
