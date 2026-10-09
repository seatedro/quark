"""Catches: the modal sign-in flow not reaching the page's getter, or the
demo leaking the credential. Sign in opens the webview on the fixture's
identity provider, which redirects to the app origin; once that page loads
the demo awaits its async getter and closes the window. The parent must
then report a redacted success, list the redirect and commit, and nowhere
publish the token itself."""

import os

from quark_e2e import Cua, app_pid, app_tree, atspi_tree, main, wait_for

# The fixture getter's synthetic token (webview_demo.rs TOKEN).
TOKEN = "demo-token-0123456789"


def names(node):
    return [n.name or "" for n in node.walk()]


def demo():
    return atspi_tree(os.environ["QUARK_E2E_APP"])


def status(prefix):
    return next((name for name in names(demo()) if name.startswith(prefix)), None)


def spec(cua: Cua):
    app_tree(window="Webview demo")
    pid = app_pid()
    assert status("Not signed in"), "\n".join(names(demo()))
    cua.press_id(pid, "webview_demo.sign_in")
    signed_in = wait_for("the getter result", lambda: status("Signed in"), timeout=30)
    assert signed_in == "Signed in: received a string of 21 characters", signed_in
    events = names(demo())
    for wanted in ("redirected: https://127.0.0.1", "committed: https://127.0.0.1", "page loaded (HTTP 200)"):
        assert any(e.startswith(wanted) for e in events), f"no {wanted!r} in {events}"
    wait_for("the modal to close", lambda: status("closed: Program"))
    assert not any(TOKEN in e for e in names(demo())), "the token reached the tree"


main(spec)
