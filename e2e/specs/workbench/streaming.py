"""Catches: a streamed answer that does not reach the screen step by step
under the manual clock: after the prompt, "Advance demo step" must play
the run one event at a time, showing the finished search card and the
start of the next prose block while Stop is still offered. Saves
streaming-dark.png at that midpoint."""

# quark-e2e-env: QUARK_WORKBENCH_THEME=dark
# quark-e2e-env: QUARK_WORKBENCH_MANUAL_CLOCK=1

from quark_e2e import STATE_FOCUSED, Cua, app_pid, app_tree, main, resize_window, save_screenshot, wait_for

TITLE = "Quark Workbench"


def field():
    return app_tree().require("entry", "Message")


def transcript_has(text):
    return any(text in (n.name or "") for n in app_tree().require("list", "Transcript").walk())


def search_card():
    return any(
        (n.name or "").startswith("Searched")
        for n in app_tree().require("list", "Transcript").walk()
    )


def spec(cua: Cua):
    app_tree()
    pid = app_pid()
    resize_window(TITLE, 1240, 740)
    cua.press(pid, "entry", "Message")
    wait_for("the composer to take focus", lambda: field().has_state(STATE_FOCUSED))
    cua.call("type_text", pid=pid, text="Make the shortcuts layout independent", delivery_mode="foreground")
    cua.call("press_key", pid=pid, key="Return", delivery_mode="foreground")
    wait_for("Send to turn into Stop", lambda: app_tree().find("push button", "Stop"))

    # One event per step: through the first answer and the search card,
    # into the second prose block.
    for _ in range(80):
        if search_card() and transcript_has("Modifier shortcuts"):
            break
        cua.call("hotkey", pid=pid, keys=["ctrl", "shift", "period"], delivery_mode="foreground")
    wait_for("the search card", search_card)
    wait_for("the second answer to start", lambda: transcript_has("Modifier shortcuts"))
    wait_for("the run to still be going", lambda: app_tree().find("push button", "Stop"))
    save_screenshot(cua, "streaming-dark")


main(spec)
