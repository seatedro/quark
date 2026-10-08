"""Catches: the stress scenario regressing past its launch and history
budgets, or the 2,000-thread sidebar not answering a search. The app
prints its first-frame and history-ready marks (QUARK_WORKBENCH_MARKS);
the 50,000-row history must be ready within 5 s of launch, and filtering
the sidebar must narrow it to matching threads. Writes marks.txt and
stress.png among the artifacts. Software rendering under Xvfb: these
timings smoke-test the loading path and certify no hardware frame rate."""

# quark-e2e-env: QUARK_WORKBENCH_SCENARIO=stress
# quark-e2e-env: QUARK_WORKBENCH_MARKS=1
# quark-e2e-env: QUARK_WORKBENCH_TERMINAL=scripted

import os

from quark_e2e import (
    Cua,
    app_marks,
    app_pid,
    app_tree,
    artifacts_dir,
    main,
    resize_window,
    save_screenshot,
    wait_for,
)

TITLE = "Quark Workbench"
HISTORY_READY_MS = 5_000


def listed():
    s = app_tree().find("list", "Threads")
    return [n.name for n in s.find_all("list item")] if s else []


def spec(cua: Cua):
    app_tree()
    pid = app_pid()
    resize_window(TITLE, 1240, 740)
    marks = wait_for("the history-ready mark", lambda: "history-ready" in app_marks() and app_marks())
    with open(os.path.join(artifacts_dir(), "marks.txt"), "w") as f:
        for name, ms in sorted(marks.items()):
            f.write(f"{name} {ms}\n")
    assert "first-frame" in marks, marks
    assert marks["history-ready"] <= HISTORY_READY_MS, f"history ready after {marks['history-ready']} ms"

    cua.press(pid, "entry", "Search threads")
    cua.call("type_text", pid=pid, text="geocoder", delivery_mode="foreground")
    wait_for(
        "the search to narrow the sidebar",
        lambda: listed() and all("geocoder" in name.lower() for name in listed()),
    )
    save_screenshot(cua, "stress")


main(spec)
