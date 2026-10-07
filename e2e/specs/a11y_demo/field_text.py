"""Catches: a text field's contents, caret, or selection not reaching
screen readers, or a screen reader's selection not reaching the field.
Typed text must show up through AT-SPI's Text interface with the caret
after it; selecting the first word over AT-SPI must select it in the app,
which publishes the selection back."""

from quark_e2e import STATE_FOCUSED, Cua, app_pid, app_tree, main, set_text_selection, wait_for


def entry():
    return app_tree().require("entry", "Name")


def spec(cua: Cua):
    pid = app_pid()
    cua.click_node(pid, entry(), delivery_mode="foreground")
    wait_for("the Name entry to take focus", lambda: entry().has_state(STATE_FOCUSED))
    cua.call("type_text", pid=pid, text="Ada Lovelace", delivery_mode="foreground")
    wait_for(
        "the typed text with the caret after it",
        lambda: (lambda t: t and t.text == "Ada Lovelace" and t.caret == 12 and t.selection is None)(entry().text),
    )

    assert set_text_selection(entry(), 0, 3), "SetSelection refused"
    wait_for(
        "the first word selected",
        lambda: (lambda t: t.selection == (0, 3) and t.caret == 3)(entry().text),
    )


main(spec)
