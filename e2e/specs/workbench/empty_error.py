"""Catches: the error and empty states not offering their way forward. The
error scenario opens "Fix flaky geocoder test", whose last run failed:
the sidebar marks it failed, the transcript explains the failure, and
Retry starts a new run that completes. Mod+N then opens an empty thread
whose "Review changes" starter sends a real prompt. Saves shell-dark.png
(the error scene in the dark theme) and empty-dark.png."""

# quark-e2e-env: QUARK_WORKBENCH_SCENARIO=error
# quark-e2e-env: QUARK_WORKBENCH_THEME=dark
# quark-e2e-env: QUARK_WORKBENCH_TERMINAL=scripted

from quark_e2e import Cua, app_pid, app_tree, main, resize_window, save_screenshot, wait_for

TITLE = "Quark Workbench"
STATE_SELECTED = 23


def selected():
    s = app_tree().find("list", "Threads")
    return [n.name for n in s.find_all("list item") if n.has_state(STATE_SELECTED)] if s else []


def shows_text(fragment):
    return any(fragment in n.name for n in app_tree().walk())


def hotkey(cua, pid, *keys):
    cua.call("hotkey", pid=pid, keys=list(keys), delivery_mode="foreground")


def spec(cua: Cua):
    app_tree()
    pid = app_pid()
    resize_window(TITLE, 1240, 740)
    wait_for("the failed thread to be selected", lambda: selected() == ["Fix flaky geocoder test, failed"])
    wait_for("the failure to be explained", lambda: shows_text("The test run timed out"))
    save_screenshot(cua, "shell-dark")

    cua.press(pid, "push button", "Retry")
    wait_for("the retry to run", lambda: shows_text("Retry the last step."))
    wait_for("the retried run to finish", lambda: app_tree().find("push button", "Send"), timeout=30.0)

    for attempt in range(3):
        hotkey(cua, pid, "ctrl", "n")
        try:
            wait_for("Mod+N to open an empty thread", lambda: app_tree().find("push button", "Review changes"), timeout=5.0)
            break
        except AssertionError:
            if attempt == 2:
                raise
    wait_for("the new thread to be selected", lambda: selected() == ["New thread"])
    save_screenshot(cua, "empty-dark")
    cua.press(pid, "push button", "Review changes")
    wait_for("the starter prompt to be sent", lambda: shows_text("Review the uncommitted changes"))


main(spec)
