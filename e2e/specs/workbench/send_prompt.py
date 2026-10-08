"""Catches: the composer failing its core loop on a real desktop. Typed
text with Shift+Enter must grow the field to two lines without sending;
Enter must send it (the field empties and Send turns into Stop), and Stop
must end the run and bring Send back. Saves composer-multiline.png."""

# quark-e2e-env: QUARK_WORKBENCH_THEME=light
# quark-e2e-env: QUARK_WORKBENCH_TERMINAL=scripted

from quark_e2e import STATE_FOCUSED, Cua, app_pid, app_tree, main, resize_window, save_screenshot, wait_for

TITLE = "Quark Workbench"


def field():
    return app_tree().require("entry", "Message")


def draft():
    t = field().text
    return t.text if t else None


def hotkey(cua, pid, *keys):
    cua.call("hotkey", pid=pid, keys=list(keys), delivery_mode="foreground")


def spec(cua: Cua):
    app_tree()
    pid = app_pid()
    resize_window(TITLE, 1240, 740)
    cua.press(pid, "entry", "Message")
    wait_for("the composer to take focus", lambda: field().has_state(STATE_FOCUSED))
    one_line = field().extents[3]

    cua.call("type_text", pid=pid, text="Make the shortcuts layout independent", delivery_mode="foreground")
    hotkey(cua, pid, "shift", "Return")
    cua.call("type_text", pid=pid, text="and keep the tests green", delivery_mode="foreground")
    wait_for(
        "two lines in the draft",
        lambda: draft() == "Make the shortcuts layout independent\nand keep the tests green",
    )
    wait_for("the field to grow a line", lambda: field().extents[3] > one_line)
    save_screenshot(cua, "composer-multiline")

    cua.call("press_key", pid=pid, key="Return", delivery_mode="foreground")
    wait_for("the draft to be sent", lambda: draft() == "")
    wait_for("Send to turn into Stop", lambda: app_tree().find("push button", "Stop"))
    wait_for("the prompt in the transcript", lambda: any(
        "and keep the tests green" in (n.name or "") or "and keep the tests green" in ((n.text.text if n.text else "") or "")
        for n in app_tree().walk()
    ))

    cua.press(pid, "push button", "Stop")
    wait_for("Stop to turn back into Send", lambda: app_tree().find("push button", "Send"))


main(spec)
