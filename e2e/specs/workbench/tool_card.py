"""Catches: a tool card that cannot be collapsed and expanded, that keeps
its output while collapsed, or whose Retry does not start a run. (The
card publishes its expanded state, but accesskit_atspi_common 0.19 maps no
AT-SPI EXPANDED state, so the spec reads the output instead.) Works on the failed `npm test` card of "Add
keyboard shortcuts". Saves tool-card-failed.png (expanded, failed) and
tool-card-collapsed.png."""

import subprocess

from quark_e2e import Cua, app_pid, app_tree, center, main, resize_window, save_screenshot, wait_for, xdotool

TITLE = "Quark Workbench"
CARD = "Ran npm test, failed"
OUTPUT = "Tests  1 failed | 12 passed (13)"


def card():
    return app_tree().require("list", "Transcript").find("push button", CARD)


def output_shown():
    return any(OUTPUT in (n.name or "") for n in app_tree().require("list", "Transcript").walk())


def spec(cua: Cua):
    app_tree()
    pid = app_pid()
    resize_window(TITLE, 1240, 740)
    transcript = app_tree().require("list", "Transcript")
    xdotool("mousemove", "--sync", *center(transcript))
    # Wheel up until the card scrolls into view.
    for _ in range(40):
        node = card()
        if node and node.extents and node.extents[1] > transcript.extents[1]:
            break
        xdotool("click", "4")
    node = wait_for("the failed card to be shown", card)
    wait_for("the failed card to start expanded", output_shown)
    save_screenshot(cua, "tool-card-failed")

    cua.press(pid, "push button", CARD)
    wait_for("the card to collapse and drop its output", lambda: not output_shown())
    save_screenshot(cua, "tool-card-collapsed")

    cua.press(pid, "push button", CARD)
    wait_for("the card to expand again", output_shown)

    retry = next(
        n
        for n in app_tree().require("list", "Transcript").find_all("push button")
        if n.name == "Retry" and abs(n.extents[1] - card().extents[1]) < 30
    )
    xdotool("mousemove", "--sync", *center(retry))
    xdotool("click", "1")
    # The composer's Send turns into Stop while the run plays.
    wait_for("Retry to start a run", lambda: app_tree().find("push button", "Stop"))


main(spec)
