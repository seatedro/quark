"""Catches: the Codex diff surfaces not being the working shared viewer.
The inline card under the edit row lists the change's lines; its file
link opens the Changes tab, whose lines match the card's; Find in changes
counts matches as typed; a fold bar reveals the line it hid; the full
view switches the diff to split. Saves codex-diff-inline.png,
codex-diff-changes.png, and codex-diff-full.png for review."""

# quark-e2e-env: QUARK_CODEX_THEME=dark
# quark-e2e-env: QUARK_CODEX_TERMINAL=scripted
# quark-e2e-env: QUARK_CODEX_SCENE=diff-open

from quark_e2e import Cua, app_pid, app_tree, main, resize_window, save_screenshot, wait_for

TITLE = "ChatGPT"
EDITED = "  return total - total * percent / 100;"
HIDDEN = "// Shopping cart helpers."


def named(name):
    """Any node of the app's window with this accessible name."""
    return next((n for n in app_tree().walk() if n.name == name), None)


def count(name):
    return sum(1 for n in app_tree().walk() if n.name == name)


def spec(cua: Cua):
    app_tree()
    pid = app_pid()
    resize_window(TITLE, 984, 738)
    wait_for("the inline diff's edited line", lambda: count(EDITED) == 1)
    save_screenshot(cua, "codex-diff-inline")

    cua.press(pid, "link", "Open cart.js in Changes")
    wait_for("the Changes view beside the card", lambda: count(EDITED) == 2)
    save_screenshot(cua, "codex-diff-changes")

    cua.press(pid, "push button", "Find in changes")
    cua.call("type_text", pid=pid, text="total", delivery_mode="foreground")
    wait_for("the match count", lambda: named("1 of 4") is not None)
    cua.call("press_key", pid=pid, key="escape", delivery_mode="foreground")
    wait_for("the find bar to close", lambda: named("Find in changes") is not None
             and named("1 of 4") is None)

    assert named(HIDDEN) is None, "the folded line shows before it is revealed"
    folds = [n for n in app_tree().walk() if n.name == "Show 1 unmodified line"]
    cua.click_element(pid, folds[0])
    wait_for("the folded line", lambda: named(HIDDEN) is not None)

    cua.press(pid, "push button", "Enter full view")
    cua.press(pid, "push button", "Toggle split diff")
    # Split shows unchanged lines on both sides.
    wait_for("the split diff", lambda: count(HIDDEN) == 2)
    save_screenshot(cua, "codex-diff-full")


main(spec)
