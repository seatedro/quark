"""Catches: Ctrl+A then Ctrl+C failing to put the transcript on the system
clipboard. Keys go in as real key events; the clipboard is read back through
the X selection by cua."""

from quark_e2e import Cua, app_pid, app_tree, main, wait_for


def spec(cua: Cua):
    app_tree()
    pid = app_pid()
    cua.call("hotkey", pid=pid, keys=["ctrl", "a"], delivery_mode="foreground")
    cua.call("hotkey", pid=pid, keys=["ctrl", "c"], delivery_mode="foreground")
    text = wait_for(
        "transcript text on the clipboard",
        lambda: cua.call("clipboard_read", include_text=True).get("text", ""),
    )
    # Select all spans from the first message, which the demo numbers #0.
    assert text.startswith("#0:"), f"clipboard starts with {text[:80]!r}"


main(spec)
