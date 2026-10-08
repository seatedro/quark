"""Catches: a terminal panel that does not run its scripted commands, a
file source that is not selectable, and a preview that is not labelled
as a snapshot or ignores its zoom. Typed into the Terminal panel,
`cat package.json` prints the fixture file and `cargo test` the scripted
run. In Files, README.md opens in the source view, and Ctrl+A there
selects all of it. The Snapshot preview names itself so, fits the image
inside the panel, and at 100% shows it at 960 points. Saves terminal.png,
files.png, and preview-actual.png, in the light theme (the other dock
specs run dark)."""

# quark-e2e-env: QUARK_WORKBENCH_THEME=light
# quark-e2e-env: QUARK_WORKBENCH_TERMINAL=scripted

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
ROLE_IMAGE = 27


def named(name):
    return next((n for n in app_tree().walk() if n.name == name), None)


def by_role(role):
    return next((n for n in app_tree().walk() if n.role == role), None)


def snapshot():
    """The preview's image; the transcript has images of its own."""
    return next(
        (n for n in app_tree().walk() if n.role == ROLE_IMAGE and n.attributes.get("id") == "preview.image"),
        None,
    )


def terminal_text():
    term = by_role(ROLE_TERMINAL)
    return term.text.text if term is not None and term.text else ""


def run(command):
    xdotool("type", "--delay", "20", command)
    xdotool("key", "Return")


def spec(cua: Cua):
    app_tree()
    pid = app_pid()
    resize_window(TITLE, 1240, 740)

    cua.press(pid, "page tab", "Terminal")
    term = wait_for("the terminal", lambda: by_role(ROLE_TERMINAL))
    xdotool("mousemove", "--sync", *center(term))
    xdotool("click", 1)
    run("cat package.json")
    wait_for("package.json printed", lambda: '"name": "atlas"' in terminal_text())
    run("cargo test")
    wait_for("the scripted cargo run", lambda: "4 passed; 0 failed" in terminal_text())
    save_screenshot(cua, "terminal")

    cua.press(pid, "page tab", "Files")
    cua.press(pid, "tree item", "README.md")
    entry = wait_for(
        "README.md in the source view",
        lambda: (e := app_tree().find("entry", "File source")) and e.text.text.startswith("# Atlas") and e,
    )
    xdotool("mousemove", "--sync", *center(entry))
    xdotool("click", 1)
    xdotool("key", "ctrl+a")
    length = len(entry.text.text)
    wait_for(
        "the whole file selected",
        lambda: app_tree().find("entry", "File source").text.selection == (0, length),
    )
    save_screenshot(cua, "files")

    cua.press(pid, "page tab", "Snapshot preview")
    wait_for("the snapshot label", lambda: named("Snapshot preview"))
    panel = next(n for n in app_tree().walk() if n.name == "Snapshot preview" and n.attributes.get("id", "").endswith(":panel"))
    image = wait_for("the snapshot", snapshot)
    px, py, pw, ph = panel.extents
    x, y, w, h = image.extents
    assert px <= x and x + w <= px + pw and py <= y and y + h <= py + ph, f"fit image {image.extents} outside {panel.extents}"
    cua.press(pid, "radio button", "100%")
    wait_for("the snapshot at 100%", lambda: snapshot().extents[2] == 960)
    save_screenshot(cua, "preview-actual")


main(spec)
