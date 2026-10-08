"""Catches: @ mentions not working end to end. Typing "@comm" must open
suggestions at the caret, above the composer (which sits at the bottom of
the window) without moving the draft; Enter must replace the query with
the file's chip; Ctrl+Z must bring the typed query back. Saves
composer-suggestions.png."""

# quark-e2e-env: QUARK_WORKBENCH_THEME=dark
# quark-e2e-env: QUARK_WORKBENCH_TERMINAL=scripted

from quark_e2e import STATE_FOCUSED, Cua, app_pid, app_tree, main, resize_window, save_screenshot, wait_for

TITLE = "Quark Workbench"


def field():
    return app_tree().require("entry", "Message")


def draft():
    t = field().text
    return t.text if t else None


def option(name):
    return next((n for n in app_tree().walk() if n.name == name and n.role != 79), None)


def spec(cua: Cua):
    app_tree()
    pid = app_pid()
    resize_window(TITLE, 1240, 740)
    cua.press(pid, "entry", "Message")
    wait_for("the composer to take focus", lambda: field().has_state(STATE_FOCUSED))
    cua.call("type_text", pid=pid, text="see ", delivery_mode="foreground")
    wait_for("the typed text", lambda: draft() == "see ")
    before = field().extents

    cua.call("type_text", pid=pid, text="@comm", delivery_mode="foreground")
    found = wait_for("commands.ts to be suggested", lambda: option("commands.ts"))
    after = field().extents
    assert after == before, f"the draft moved from {before} to {after}"
    x, y, w, h = found.extents
    assert y + h <= after[1] + after[3], f"suggestion {found.extents} below the field {after}"
    assert y < after[1], f"suggestion {found.extents} not above the field {after}"
    save_screenshot(cua, "composer-suggestions")

    cua.call("press_key", pid=pid, key="Return", delivery_mode="foreground")
    wait_for("the chip in place of the query", lambda: draft() == "see @commands.ts ")
    wait_for("the suggestions to close", lambda: option("commands.ts") is None)

    cua.call("hotkey", pid=pid, keys=["ctrl", "z"], delivery_mode="foreground")
    wait_for("Undo to restore the query", lambda: draft() == "see @comm")


main(spec)
