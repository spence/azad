#!/usr/bin/env python3
"""Records content-free sleep/wake evidence for Azad's device-level capture during normal use.

A Tart VM cannot sleep (`pmset sleepnow` fails with 0xe00002e2), so sleep and wake are observed on
the owner's Mac instead, without driving it: the script only reads the kernel's sleep and wake
times, the capture helper's status and edge counters, Azad's input-log event names and the
combined modifier state. It waits for the next sleep and wake, then for Option+Space to reach Azad
and ordinary typing to be forwarded after the wake, and writes <evidence-dir>/report.json.

    python3 crates/azad-capture/tests/physical/sleep_wake_watch.py --out <evidence-dir>
"""

import argparse
import ctypes
import json
import os
import re
import subprocess
import time

INPUT_LOG = os.path.expanduser("~/Library/Logs/Azad/input.log")
HELPER_LOG = "/var/log/azad-capture.log"
# Shift, Control, Option and Command in CGEventFlags; Caps Lock is a lock, not a held key.
HELD_MODIFIERS = 0x20000 | 0x40000 | 0x80000 | 0x100000
EDGES = ("claimed_edges", "forwarded_edges", "forward_errors", "actions")


def kernel_time(name):
    out = subprocess.run(["sysctl", "-n", name], capture_output=True, text=True).stdout
    match = re.search(r"sec = (\d+)", out)
    return int(match.group(1)) if match else 0


def latest(event):
    try:
        lines = open(HELPER_LOG).read().splitlines()
    except OSError:
        return None
    for line in reversed(lines):
        if f'"event":"{event}"' in line:
            return json.loads(line)
    return None


def modifier_flags():
    graphics = ctypes.CDLL("/System/Library/Frameworks/CoreGraphics.framework/CoreGraphics")
    graphics.CGEventSourceFlagsState.restype = ctypes.c_uint64
    graphics.CGEventSourceFlagsState.argtypes = [ctypes.c_int32]
    # kCGEventSourceStateHIDSystemState: the combined state of every keyboard.
    return graphics.CGEventSourceFlagsState(1)


def app_events_since(ms):
    rows = []
    try:
        lines = open(INPUT_LOG).read().splitlines()
    except OSError:
        return rows
    for line in lines:
        if not line.startswith("{"):
            continue
        try:
            row = json.loads(line)
        except json.JSONDecodeError:
            continue
        if row.get("ts_ms", 0) >= ms:
            entry = {"ts_ms": row["ts_ms"], "event": row.get("event")}
            if row.get("event") == "keyboard_capture":
                entry.update(connected=row.get("connected"), capturing=row.get("capturing"))
            rows.append(entry)
    return rows


def sample():
    counters = latest("counters") or {}
    status = (latest("status") or {}).get("status") or {}
    return {"t": int(time.time()), "counters": {k: counters.get(k) for k in EDGES},
            "capture": status.get("capture"), "seized": status.get("seized_devices"),
            "modifiers_held": bool(modifier_flags() & HELD_MODIFIERS)}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--out", required=True)
    parser.add_argument("--poll-seconds", type=int, default=15)
    parser.add_argument("--give-up-hours", type=float, default=24)
    args = parser.parse_args()
    out = os.path.abspath(args.out)
    os.makedirs(out, exist_ok=True)
    started = int(time.time())
    deadline = started + int(args.give_up_hours * 3600)
    samples = []
    report = {"started": started}
    while time.time() < deadline:
        samples.append(sample())
        slept, woke = kernel_time("kern.sleeptime"), kernel_time("kern.waketime")
        if slept >= started and woke > slept:
            after = [s for s in samples if s["t"] > woke + 5]
            events = app_events_since(woke * 1000)
            names = [e["event"] for e in events]
            before = [s for s in samples if s["t"] < slept]
            if after and before and "hotkey_pressed" in names and "hotkey_released" in names:
                first, last = before[-1]["counters"], after[-1]["counters"]
                if (last["forwarded_edges"] or 0) > (first["forwarded_edges"] or 0):
                    report.update(slept=slept, woke=woke, before_sleep=before[-1], after_wake=after,
                                  app_events_after_wake=events)
                    break
        time.sleep(args.poll_seconds)
    after = report.get("after_wake") or []
    before = report.get("before_sleep") or {}
    checks = {
        "system_slept_and_woke": "woke" in report,
        "capturing_after_wake": bool(after) and after[-1]["capture"] == "capturing",
        "hotkey_after_wake": "hotkey_pressed" in [e["event"] for e in report.get("app_events_after_wake", [])],
        "typing_forwarded_after_wake": bool(after) and after[-1]["counters"]["forwarded_edges"]
        > before.get("counters", {}).get("forwarded_edges", 0),
        "no_forward_errors": bool(after) and after[-1]["counters"]["forward_errors"]
        == before.get("counters", {}).get("forward_errors"),
        "no_modifier_stuck_after_wake": bool(after) and any(not s["modifiers_held"] for s in after),
    }
    report.update(checks=checks, passed=all(checks.values()),
                  host=subprocess.run(["sysctl", "-n", "hw.model"], capture_output=True, text=True)
                  .stdout.strip())
    with open(os.path.join(out, "report.json"), "w") as f:
        json.dump(report, f, indent=1)
    print(json.dumps({"passed": report["passed"], "checks": checks}))


if __name__ == "__main__":
    main()
