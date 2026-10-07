"""Helpers for the end-to-end specs.

Two clients, both talking to the real desktop session run.sh set up:

- `Cua` drives the app the way a computer-use agent does, through the cua
  driver's MCP server (clicks, keys, clipboard, screenshots).
- `atspi_tree` reads the raw AT-SPI tree over D-Bus. The specs assert on it
  because cua 0.34 cannot report AccessKit roles: it asks for
  `GetRoleName`, which accesskit_unix does not implement, so every role comes
  back blank. The same gap makes cua's element_token clicks fail, so actions
  go through cua's pixel route, which hit-tests to the AT-SPI action.

Waiting is by polling observable state against a deadline, never a fixed
sleep.
"""

import base64
import json
import os
import re
import subprocess
import sys
import threading
import time

import dbus

# AT-SPI role numbers (AtspiRole) that the specs name.
ROLE = {
    "frame": 23,
    "dialog": 16,
    "heading": 83,
    "label": 29,
    "entry": 79,
    "push button": 43,
    "list": 31,
    "list item": 32,
    "check box": 7,
    "radio button": 44,
    "toggle button": 62,
    "status bar": 54,
    "notification": 101,
}
ROLE_NAME = {v: k for k, v in ROLE.items()}
STATE_CHECKED = 4
STATE_FOCUSED = 12
STATE_PRESSED = 20


class Text:
    """What AT-SPI's Text interface reports. Offsets count characters
    (Unicode scalar values); `selection` is None when nothing is selected."""

    def __init__(self, text, caret, selection):
        self.text = text
        self.caret = caret
        self.selection = selection

    def __repr__(self):
        return f"Text({self.text!r}, caret={self.caret}, selection={self.selection})"


class Node:
    def __init__(self, role, name, extents, attributes, states, children, text=None, path=None):
        self.role = role
        self.name = name
        self.extents = extents  # (x, y, w, h) in screen pixels, or None
        self.attributes = attributes
        self.states = states
        self.children = children
        self.text = text  # Text, for nodes with the Text interface
        self.path = path  # (bus name, object path), for calling the node

    def walk(self):
        yield self
        for child in self.children:
            yield from child.walk()

    def find(self, role, name=None):
        for node in self.walk():
            if node.role == ROLE[role] and (name is None or node.name == name):
                return node
        return None

    def require(self, role, name=None):
        """Like `find`, but a missing node fails the spec with the tree."""
        node = self.find(role, name)
        if node is None:
            what = f"{role} {name!r}" if name is not None else role
            raise AssertionError(f"no {what} under {ROLE_NAME.get(self.role, self.role)} {self.name!r}:\n{self.dump()}")
        return node

    def find_all(self, role):
        return [n for n in self.walk() if n.role == ROLE[role]]

    def has_state(self, state):
        return bool(self.states[state // 32] & (1 << (state % 32)))

    def center(self):
        x, y, w, h = self.extents
        return x + w // 2, y + h // 2

    def dump(self, depth=0):
        role = ROLE_NAME.get(self.role, f"role{self.role}")
        attrs = "".join(f" {k}={v}" for k, v in sorted(self.attributes.items()))
        text = f" {self.text!r}" if self.text else ""
        line = f"{'  ' * depth}{role} {self.name!r} {self.extents}{attrs}{text}\n"
        return line + "".join(c.dump(depth + 1) for c in self.children)


def _a11y_bus():
    session = dbus.SessionBus()
    bus = session.get_object("org.a11y.Bus", "/org/a11y/bus")
    return dbus.bus.BusConnection(bus.GetAddress(dbus_interface="org.a11y.Bus"))


def _read(bus, name, path, depth):
    obj = bus.get_object(name, path)
    acc = dbus.Interface(obj, "org.a11y.atspi.Accessible")
    props = dbus.Interface(obj, "org.freedesktop.DBus.Properties")
    interfaces = acc.GetInterfaces()
    extents = None
    if "org.a11y.atspi.Component" in interfaces:
        comp = dbus.Interface(obj, "org.a11y.atspi.Component")
        extents = tuple(int(v) for v in comp.GetExtents(dbus.UInt32(0)))
    text = None
    if "org.a11y.atspi.Text" in interfaces:
        iface = dbus.Interface(obj, "org.a11y.atspi.Text")
        count = int(props.Get("org.a11y.atspi.Text", "CharacterCount"))
        start, end = (int(v) for v in iface.GetSelection(0))
        text = Text(
            str(iface.GetText(0, count)),
            int(props.Get("org.a11y.atspi.Text", "CaretOffset")),
            (start, end) if start >= 0 else None,
        )
    try:
        attributes = {str(k): str(v) for k, v in acc.GetAttributes().items()}
    except dbus.DBusException:
        attributes = {}  # AccessKit's application root has no GetAttributes
    children = []
    if depth < 64:
        children = [_read(bus, n, p, depth + 1) for n, p in acc.GetChildren()]
    return Node(
        role=int(acc.GetRole()),
        name=str(props.Get("org.a11y.atspi.Accessible", "Name")),
        extents=extents,
        attributes=attributes,
        states=[int(s) for s in acc.GetState()],
        children=children,
        text=text,
        path=(name, path),
    )


def atspi_tree(app_name):
    """The application node named `app_name`, or None if it is not registered."""
    bus = _a11y_bus()
    root = dbus.Interface(
        bus.get_object("org.a11y.atspi.Registry", "/org/a11y/atspi/accessible/root"),
        "org.a11y.atspi.Accessible",
    )
    for name, path in root.GetChildren():
        props = dbus.Interface(
            bus.get_object(name, path), "org.freedesktop.DBus.Properties"
        )
        try:
            app = str(props.Get("org.a11y.atspi.Accessible", "Name"))
        except dbus.DBusException:
            continue  # an application that exited mid-walk
        if app == app_name:
            return _read(bus, name, path, 0)
    return None


def set_text_selection(node, start, end):
    """Select characters `start..end` of `node` through AT-SPI, as a screen
    reader's "select text" command does."""
    name, path = node.path
    iface = dbus.Interface(_a11y_bus().get_object(name, path), "org.a11y.atspi.Text")
    return bool(iface.SetSelection(0, start, end))


class Announcements:
    """Records the AT-SPI `Announcement` events apps emit (live regions and
    explicit announcements) by running dbus-monitor on the AT-SPI bus.
    Start it before the action that should announce."""

    def __init__(self):
        address = dbus.SessionBus().get_object("org.a11y.Bus", "/org/a11y/bus").GetAddress(
            dbus_interface="org.a11y.Bus"
        )
        self.proc = subprocess.Popen(
            [
                "dbus-monitor",
                "--address",
                str(address),
                "type='signal',interface='org.a11y.atspi.Event.Object',member='Announcement'",
            ],
            stdout=subprocess.PIPE,
            text=True,
        )
        self.messages = []
        self._lock = threading.Lock()
        self._ready = threading.Event()
        threading.Thread(target=self._read, daemon=True).start()
        # dbus-monitor prints its own NameAcquired/NameLost signals once the
        # match is installed; wait for that so no announcement is missed.
        if not self._ready.wait(10):
            raise AssertionError("dbus-monitor did not start on the AT-SPI bus")

    def _read(self):
        in_announcement = False
        for line in self.proc.stdout:
            self._ready.set()
            if line.startswith("signal "):
                in_announcement = "member=Announcement" in line
                continue
            match = re.match(r'\s*variant\s+string "(.*)"$', line)
            if in_announcement and match:
                with self._lock:
                    self.messages.append(match.group(1))
                in_announcement = False

    def count(self, message):
        with self._lock:
            return self.messages.count(message)

    def close(self):
        self.proc.terminate()
        self.proc.wait(timeout=10)


def wait_for(what, probe, timeout=15.0, interval=0.1):
    """Poll `probe` until it returns a truthy value; fail with `what` on timeout.

    Any exception from `probe` counts as "not yet": the tree is rebuilt
    between polls, so a node can be missing or half-published mid-update.
    The last one is reported if the deadline passes."""
    deadline = time.monotonic() + timeout
    last_error = None
    while time.monotonic() < deadline:
        try:
            value = probe()
            if value:
                return value
        except Exception as error:
            last_error = error
        time.sleep(interval)
    detail = f": {type(last_error).__name__}: {last_error}" if last_error else ""
    raise AssertionError(f"timed out waiting for {what}{detail}")


def app_tree(content=True):
    """The first window frame of the app run.sh launched. With `content`,
    waits until the frame has children; a raw `App` publishes none."""
    name = os.environ["QUARK_E2E_APP"]

    def probe():
        app = atspi_tree(name)
        frame = app and app.find("frame")
        return frame if frame and (frame.children or not content) else None

    return wait_for(f"{name}'s accessibility tree", probe)


class CuaError(AssertionError):
    pass


class Cua:
    """One MCP session with `cua-driver mcp`, kept open so snapshots and
    captures persist between calls."""

    def __init__(self):
        driver = os.environ.get("CUA_DRIVER", "cua-driver")
        self.proc = subprocess.Popen(
            [driver, "mcp", "--direct", "--no-overlay"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            text=True,
        )
        self.next_id = 0
        self._rpc(
            "initialize",
            {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "quark-e2e", "version": "0"},
            },
        )
        self._send({"jsonrpc": "2.0", "method": "notifications/initialized"})

    def _send(self, message):
        self.proc.stdin.write(json.dumps(message) + "\n")
        self.proc.stdin.flush()

    def _rpc(self, method, params):
        self.next_id += 1
        self._send({"jsonrpc": "2.0", "id": self.next_id, "method": method, "params": params})
        while True:
            line = self.proc.stdout.readline()
            if not line:
                raise CuaError(f"cua-driver exited during {method}")
            reply = json.loads(line)
            if reply.get("id") == self.next_id:
                if "error" in reply:
                    raise CuaError(f"{method}: {reply['error']}")
                return reply["result"]

    def call(self, tool, **args):
        result = self._rpc("tools/call", {"name": tool, "arguments": args})
        text = " ".join(c.get("text", "") for c in result.get("content", []) if c.get("type") == "text")
        if result.get("isError"):
            raise CuaError(f"{tool} failed: {text}")
        return result.get("structuredContent") or {}

    def window(self, pid):
        windows = self.call("list_windows", pid=pid, on_screen_only=True)["windows"]
        if not windows:
            raise CuaError(f"pid {pid} has no on-screen window")
        return max(windows, key=lambda w: w.get("z_index") or 0)

    def click_node(self, pid, node, delivery_mode="background"):
        """Click the center of an AT-SPI node in window-local pixels.

        A pixel click needs a screenshot from this session first; the fresh
        get_window_state also supplies the window origin for the conversion.
        """
        window = self.window(pid)
        state = self.call(
            "get_window_state", pid=pid, window_id=window["window_id"], max_image_dimension=0
        )
        origin = state["window_bounds"]
        x, y = node.center()
        return self.call(
            "click",
            pid=pid,
            window_id=window["window_id"],
            x=x - origin["x"],
            y=y - origin["y"],
            delivery_mode=delivery_mode,
        )

    def screenshot(self, path):
        """Save the full display as a PNG."""
        result = self._rpc("tools/call", {"name": "get_desktop_state", "arguments": {}})
        for content in result.get("content", []):
            if content.get("type") == "image":
                with open(path, "wb") as f:
                    f.write(base64.b64decode(content["data"]))
                return True
        return False

    def close(self):
        self.proc.stdin.close()
        self.proc.wait(timeout=10)


def app_pid():
    return int(os.environ["QUARK_E2E_PID"])


def example_binary(name):
    return os.path.join(os.environ["QUARK_E2E_BIN_DIR"], name)


def main(spec):
    """Run `spec(cua)`; exit 0 on success, 1 with a one-line FAIL on any error."""
    cua = None
    try:
        cua = Cua()
        spec(cua)
    except Exception as error:
        kind = "" if isinstance(error, AssertionError) else f"{type(error).__name__}: "
        print(f"FAIL: {kind}{error}", file=sys.stderr)
        sys.exit(1)
    finally:
        if cua:
            try:
                cua.close()
            except Exception as error:
                print(f"cua-driver did not exit cleanly: {error}", file=sys.stderr)
    print("PASS")


def _cli():
    """`wait`: block until the launched app's tree is up.
    `dump DIR`: save a screenshot and the AT-SPI tree for a failed spec."""
    command = sys.argv[1]
    if command == "wait":
        app_tree(content=False)
    elif command == "dump":
        out = sys.argv[2]
        app = atspi_tree(os.environ["QUARK_E2E_APP"])
        with open(os.path.join(out, "tree.txt"), "w") as f:
            f.write(app.dump() if app else "application not registered on the AT-SPI bus\n")
        cua = Cua()
        try:
            cua.screenshot(os.path.join(out, "screen.png"))
        finally:
            cua.close()
    else:
        sys.exit(f"unknown command {command}")


if __name__ == "__main__":
    _cli()
