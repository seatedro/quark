"""Catches: the form's accessibility tree losing its structure or its role
names. As cua reads it, the named dialog must sit in the window frame and
hold the heading, the Name entry, the hint, and both buttons, with the
entry and both buttons indexed as clickable."""

from quark_e2e import Cua, app_pid, app_tree, main


def spec(cua: Cua):
    app_tree()
    snapshot = cua.snapshot(app_pid())
    frame = snapshot.root.children[0] if snapshot.root.children else None
    assert frame and (frame.role, frame.name) == ("frame", "Hello Quark UI"), snapshot.root.dump()
    dialog = frame.find("dialog", "Hello Quark")
    assert dialog in frame.children, f"'Hello Quark' dialog is not directly in the frame:\n{frame.dump()}"
    children = [(c.role, c.name) for c in dialog.children]
    expected = [
        ("heading", "Hello from Quark"),
        ("entry", "Name"),
        ("label", "Type a name, then press Greet."),
        ("push button", "Greet"),
        ("push button", "Clear"),
    ]
    assert children == expected, f"dialog children {children}, expected {expected}"
    clickable = sorted(e["label"] for e in snapshot.elements if "click" in e.get("actions", []))
    assert clickable == ["Clear", "Greet", "Name"], f"cua indexed {clickable} as clickable"


main(spec)
