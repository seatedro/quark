"""Catches: single instance handoff breaking. A second launch with a URL
must exit after forwarding it, and the running instance must receive it as
AppEvent::OpenUrls."""

import os
import subprocess

from quark_e2e import Cua, example_binary, main, wait_for

URL = "quark-e2e://open/item?id=42"


def spec(cua: Cua):
    # run.sh waited for the window; the primary is listening once it logs.
    log = os.environ["QUARK_E2E_APP_LOG"]
    wait_for("the primary instance to start", lambda: "platform_demo: started" in open(log).read())
    second = subprocess.run(
        [example_binary("platform_demo"), URL], capture_output=True, text=True, timeout=30
    )
    assert second.returncode == 0, second
    assert f'forwarded ["{URL}"]' in second.stdout, second.stdout
    wait_for(
        "the primary instance to log OpenUrls",
        lambda: f'OpenUrls(["{URL}"])' in open(log).read(),
    )


main(spec)
