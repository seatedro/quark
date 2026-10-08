"""Catches: the end of a wide code line staying out of keyboard reach, a selection
landing on the wrong characters once the line has scrolled sideways, Copy
code copying less than the whole block, and a table published as loose
text or copied as something other than a markdown table. Saves
wide-code-scrolled.png, wide-code-wrapped.png, and table.png."""

# quark-e2e-env: QUARK_WORKBENCH_TERMINAL=scripted

from quark_e2e import Cua, app_pid, app_tree, center, main, pointer_drag, resize_window, save_screenshot, wait_for, xdotool

TITLE = "Quark Workbench"
CODE_START = "/** Resolves a keydown to a command"
TAIL = "finalIdentifierOnWideLine"
# AT-SPI roles the shared helper does not name.
ROLE_TABLE, ROLE_CELL, ROLE_COLUMN_HEADER = 55, 56, 10


def transcript():
    return app_tree().require("list", "Transcript")


def code_label():
    return next((n for n in transcript().walk() if n.role == 29 and (n.name or "").startswith(CODE_START) and n.extents), None)


def scroll_to(probe):
    xdotool("mousemove", "--sync", *center(transcript()))
    for _ in range(60):
        node = probe()
        top = transcript().extents[1]
        if node is not None and node.extents and node.extents[1] > top:
            return node
        xdotool("click", "4")
    return wait_for("the block to scroll into view", probe)


def hotkey(cua, pid, *keys):
    cua.call("hotkey", pid=pid, keys=list(keys), delivery_mode="foreground")


def clipboard(cua, what, check):
    return wait_for(what, lambda: (lambda t: t if check(t) else None)(cua.call("clipboard_read", include_text=True).get("text", "")))


def spec(cua: Cua):
    app_tree()
    pid = app_pid()
    resize_window(TITLE, 1240, 740)

    code = scroll_to(code_label)
    x, y, w, h = code.extents
    # The wide line is the block's last. A press there focuses the block's
    # sideways scroll; arrow keys then bring the line's end in.
    xdotool("mousemove", "--sync", max(x, 0) + 40, y + h - 30)
    xdotool("click", "1")
    for _ in range(12):
        xdotool("key", "--delay", "40", *(["Right"] * 10))
    save_screenshot(cua, "wide-code-scrolled")
    # From the blank line above down to the wide line's visible end: the
    # copy must stop inside the identifier now showing at the right edge.
    # The label spans the scrolled content, wider than the panel; clamp to
    # the transcript's column.
    tx, _, tw, _ = transcript().extents
    right = min(x + w, tx + tw - 16) - 40
    start, end = (right, y + h - 30), (right, y + h - 13)
    pointer_drag(start, [((start[0] + end[0]) // 2, (start[1] + end[1]) // 2), end])
    save_screenshot(cua, "wide-code-selected")
    hotkey(cua, pid, "ctrl", "c")
    text = clipboard(cua, "the line's end selected after scrolling", lambda t: "DEFAULT_SHORTCUTS" in t)
    assert TAIL[:8] in text, f"the selection stops short of the revealed end: {text[-60:]!r}"

    cua.press(pid, "push button", "Copy code")
    text = clipboard(cua, "the whole block from Copy code", lambda t: t.startswith(CODE_START))
    assert TAIL in text and "export function commandForKey" in text, f"Copy code copied {text!r}"
    cua.press(pid, "toggle button", "Wrap lines")
    wait_for("the block to wrap", lambda: code_label() and code_label().extents[3] > h)
    save_screenshot(cua, "wide-code-wrapped")

    table = scroll_to(lambda: next((n for n in transcript().walk() if n.role == ROLE_TABLE and n.extents), None))
    headers = [n for n in table.walk() if n.role == ROLE_COLUMN_HEADER]
    cells = [n for n in table.walk() if n.role == ROLE_CELL]
    assert len(headers) == 3 and len(cells) == 9, f"table has {len(headers)} headers and {len(cells)} cells"
    save_screenshot(cua, "table")
    first, last = headers[0], cells[-1]
    fx, fy, _, fh = first.extents
    lx, ly, lw, lh = last.extents
    pointer_drag((fx + 2, fy + fh // 2), [(lx + lw - 2, ly + lh // 2)])
    hotkey(cua, pid, "ctrl", "c")
    text = clipboard(cua, "the table on the clipboard", lambda t: t.startswith("Command"))
    lines = text.splitlines()
    assert len(lines) == 5 and set(lines[1].replace(" ", "")) <= set("|-"), f"not a markdown table: {text!r}"


main(spec)
