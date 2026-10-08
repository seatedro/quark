"""Catches: a drag across two transcript messages not copying exactly
their text, and Select all missing rows that virtualization has not
materialized. The drag runs from the start of the user's question into
the assistant's answer below it; Select all must reach the thread's first
prompt, scrolled far out of view. Saves timeline-selection.png."""

from quark_e2e import Cua, app_pid, app_tree, main, pointer_drag, resize_window, save_screenshot, wait_for

TITLE = "Quark Workbench"
QUESTION = "Looks good. Does this work with non-Latin keyboard layouts, like ЙЦУКЕН or a Japanese IME?"
ANSWER = "Mostly. event.key reports the produced character"
FIRST_PROMPT = "Add keyboard shortcuts for the common actions"


def label_starting(prefix):
    transcript = app_tree().require("list", "Transcript")
    return next(n for n in transcript.walk() if n.role == 29 and n.name.startswith(prefix) and n.extents)


def hotkey(cua, pid, *keys):
    cua.call("hotkey", pid=pid, keys=list(keys), delivery_mode="foreground")


def clipboard(cua, what, check):
    return wait_for(what, lambda: (lambda t: t if check(t) else None)(cua.call("clipboard_read", include_text=True).get("text", "")))


def spec(cua: Cua):
    app_tree()
    pid = app_pid()
    resize_window(TITLE, 1240, 740)
    question = wait_for("the question to be on screen", lambda: label_starting(QUESTION[:20]))
    answer = label_starting(ANSWER)

    qx, qy, _, qh = question.extents
    ax, ay, aw, ah = answer.extents
    start = (qx + 1, qy + qh // 2 if qh < 30 else qy + 10)
    # Into the answer's first line, a few words in.
    end = (ax + min(aw, 200), ay + 10)
    path = [(start[0] + (end[0] - start[0]) * i // 8, start[1] + (end[1] - start[1]) * i // 8) for i in range(1, 9)]
    pointer_drag(start, path)
    save_screenshot(cua, "timeline-selection")
    hotkey(cua, pid, "ctrl", "c")
    text = clipboard(cua, "the dragged text on the clipboard", lambda t: t.startswith(QUESTION))
    assert "\n\nMostly." in text, f"copy stops before the answer: {text!r}"
    assert not text.endswith(QUESTION), f"copy ends inside the question: {text!r}"

    hotkey(cua, pid, "ctrl", "a")
    hotkey(cua, pid, "ctrl", "c")
    text = clipboard(cua, "the whole thread on the clipboard", lambda t: t.startswith(FIRST_PROMPT))
    assert "retrying continues from the last tool result." in text, f"select all stops early: {text[-200:]!r}"


main(spec)
