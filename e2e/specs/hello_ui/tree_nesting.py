"""Catches: the form's accessibility tree losing its structure. The named
dialog must sit in the window frame and hold the heading, the Name entry,
and both buttons, and cua must index both buttons as clickable."""

from quark_e2e import ROLE_NAME, Cua, app_pid, app_tree, main


def spec(cua: Cua):
    frame = app_tree()
    assert frame.name == "Hello Quark UI", frame.dump()
    dialog = frame.find("dialog", "Hello Quark")
    assert dialog in frame.children, f"no 'Hello Quark' dialog directly in the frame:\n{frame.dump()}"
    children = [(ROLE_NAME.get(c.role, c.role), c.name) for c in dialog.children]
    expected = [
        ("heading", "Hello from Quark"),
        ("entry", "Name"),
        ("label", "Type a name, then press Greet."),
        ("push button", "Greet"),
        ("push button", "Clear"),
    ]
    assert children == expected, f"dialog children {children}, expected {expected}"

    window = cua.window(app_pid())
    state = cua.call("get_window_state", pid=app_pid(), window_id=window["window_id"], include_screenshot=False)
    clickable = sorted(e["label"] for e in state["elements"] if "click" in e.get("actions", []))
    assert clickable == ["Clear", "Greet"], f"cua indexed {clickable} as clickable"


main(spec)
