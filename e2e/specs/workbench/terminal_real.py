"""Catches: a real terminal session that does not start the user's shell,
does not send it typed keys, or does not show its output. In the Terminal
panel, `echo quark-$((6*7))` typed into the shell prints quark-42, which
only the shell's arithmetic can produce (the typed line itself reads
`quark-$((6*7))`). Saves terminal-real.png."""

# quark-e2e-env: QUARK_WORKBENCH_TERMINAL=real
# The login shell the app finds in $SHELL, pinned so a runner whose shell
# is zsh does not start its first-run wizard in the empty home directory.
# quark-e2e-env: SHELL=/bin/sh

from quark_e2e import (
    Cua,
    app_pid,
    app_tree,
    center,
    main,
    resize_window,
    save_screenshot,
    wait_for,
    xdotool,
)

TITLE = "Quark Workbench"
ROLE_TERMINAL = 60


def terminal():
    return next((n for n in app_tree().walk() if n.role == ROLE_TERMINAL), None)


def terminal_text():
    term = terminal()
    return term.text.text if term is not None and term.text else ""


def spec(cua: Cua):
    app_tree()
    pid = app_pid()
    resize_window(TITLE, 1240, 740)

    cua.press(pid, "page tab", "Terminal")
    term = wait_for("the terminal", terminal)
    xdotool("mousemove", "--sync", *center(term))
    xdotool("click", 1)
    # The shell's first prompt, so the typed line is not lost to a shell
    # that is still starting (its rc files can turn echo off meanwhile).
    wait_for("a prompt", lambda: terminal_text().strip())
    xdotool("type", "--delay", "20", "echo quark-$((6*7))")
    xdotool("key", "Return")
    wait_for("the shell's answer", lambda: "quark-42" in terminal_text())
    save_screenshot(cua, "terminal-real")


main(spec)
