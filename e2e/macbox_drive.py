"""Drives a running workbench on macOS through cua-driver CLI calls that
share one daemon session label, so element tokens survive between calls.
See e2e/macbox-workbench.md.

usage: python3 macbox_drive.py STEP...
  zoom              press the window's zoom button (forces a real resize)
  shot:NAME         save the window as $OUT/NAME.png (OUT defaults to /tmp/wb)
  key:cmd+b         press a key combination with the window in front
  press:ROLE:LABEL  press the first element of ROLE whose JSON contains LABEL
  wait:SECONDS      sleep
  elements          print the window's indexed elements
"""
import json, os, subprocess, sys, time

SESSION = "workbench-check"
OUT = os.environ.get("OUT", "/tmp/wb")

def call(tool, **args):
    args.setdefault("session", SESSION)
    out = subprocess.run(["cua-driver", "call", tool, json.dumps(args)], capture_output=True, text=True)
    try:
        return json.loads(out.stdout)
    except ValueError:
        raise SystemExit(f"{tool}: {out.stdout[:400]} {out.stderr[:400]}")

pid = int(subprocess.run(["pgrep", "-x", "workbench"], capture_output=True, text=True).stdout.split()[0])

def wid():
    ws = [w for w in call("list_windows")["windows"] if w.get("pid") == pid and w.get("title") == "Quark Workbench"]
    return ws[0]["window_id"]

def state(**extra):
    return call("get_window_state", pid=pid, window_id=wid(), **extra)

for step in sys.argv[1:]:
    kind, _, arg = step.partition(":")
    if kind == "zoom":
        z = [e for e in state()["elements"] if "zoom" in json.dumps(e).lower()][0]
        print(json.dumps(call("click", pid=pid, window_id=wid(), element_token=z["element_token"]))[:300])
    elif kind == "shot":
        st = state(screenshot_out_file=os.path.join(OUT, f"{arg}.png"))
        print(arg, st["window_bounds"])
    elif kind == "key":
        print(json.dumps(call("hotkey", pid=pid, window_id=wid(), keys=arg.split("+"), delivery_mode="foreground"))[:200])
    elif kind == "press":
        role, _, label = arg.partition(":")
        e = [e for e in state()["elements"] if e.get("role") == role and label in json.dumps(e)][0]
        print(json.dumps(call("click", pid=pid, window_id=wid(), element_token=e["element_token"]))[:300])
    elif kind == "wait":
        time.sleep(float(arg))
    elif kind == "elements":
        for e in state()["elements"]:
            print(json.dumps(e)[:200])
