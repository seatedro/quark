"""Catches: shell navigation in the Codex recreation not reaching the
screen: a Recents row that does not open its thread in the transcript, the
sidebar toggle that does not hide and restore the sidebar card, or View
changes on a file change card that does not open the Changes tab. Saves
codex-thread.png and codex-changes.png for review."""

# quark-e2e-env: QUARK_CODEX_THEME=dark
# quark-e2e-env: QUARK_CODEX_TERMINAL=scripted
# quark-e2e-env: QUARK_CODEX_SCENE=home

from quark_e2e import Cua, app_pid, app_tree, main, resize_window, save_screenshot, wait_for

TITLE = "ChatGPT"


def named(name):
    """Any node of the app's window with this accessible name."""
    return next((n for n in app_tree().walk() if n.name == name), None)


def spec(cua: Cua):
    app_tree()
    pid = app_pid()
    resize_window(TITLE, 984, 738)
    wait_for("the home headline", lambda: named("codex-demo?") is not None)

    cua.press(pid, "list item", "Run tests and explain failures")
    wait_for("the thread's transcript", lambda: named("Transcript") is not None)
    wait_for("the file change card", lambda: named("View changes") is not None)
    save_screenshot(cua, "codex-thread")

    cua.press(pid, "push button", "Hide sidebar")
    wait_for("the sidebar to hide", lambda: named("Sidebar") is None)
    cua.press(pid, "push button", "Hide sidebar")
    wait_for("the sidebar to return", lambda: named("Sidebar") is not None)

    cua.press(pid, "push button", "View changes")
    wait_for("the Changes tab", lambda: app_tree().find("page tab", "Changes") is not None)
    save_screenshot(cua, "codex-changes")


main(spec)
