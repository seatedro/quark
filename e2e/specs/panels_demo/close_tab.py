"""Catches: a closable TabBar tab whose close button also selects it, that
Delete does not close, or whose removal strands focus. In the terminal
drawer's sessions, pressing "Close cargo watch" over AT-SPI removes that
inactive tab and leaves zsh selected; then Delete on the focused zsh tab
removes it, and the next session, dev server, is selected and focused."""

from quark_e2e import STATE_FOCUSED, Cua, app_pid, app_tree, main, wait_for

STATE_SELECTED = 23


def session(id):
    """The session tab `id`, or None once it is closed."""
    for node in app_tree().walk():
        if node.attributes.get("id") == f"tab:session:{id}":
            return node
    return None


def spec(cua: Cua):
    app_tree()
    pid = app_pid()
    cua.call("hotkey", pid=pid, keys=["ctrl", "j"], delivery_mode="foreground")
    wait_for("the drawer's sessions", lambda: session(1))

    cua.press(pid, "push button", "Close cargo watch")
    wait_for("cargo watch to close", lambda: session(2) is None)
    assert session(1).has_state(STATE_SELECTED), app_tree().dump()

    cua.press_id(pid, "tab:session:1")
    wait_for("zsh to take focus", lambda: session(1).has_state(STATE_FOCUSED))
    cua.call("press_key", pid=pid, key="delete", delivery_mode="foreground")
    wait_for("zsh to close", lambda: session(1) is None)
    wait_for(
        "dev server to be selected and focused",
        lambda: session(3).has_state(STATE_SELECTED) and session(3).has_state(STATE_FOCUSED),
    )


main(spec)
