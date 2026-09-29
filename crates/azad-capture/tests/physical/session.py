#!/usr/bin/env python3
"""Owner-attended physical keyboard session for Azad's device-level capture.

Run by the owner, on the Mac whose keyboards are under test, after installing the build to
verify. It never posts input: the owner presses every key. It shows a test window that records
the keys it receives, reads Azad's input log and the capture helper's log, and can briefly turn
on Secure Input or a consuming event tap (its own processes, stopped at the end of the step).

    python3 crates/azad-capture/tests/physical/session.py --out <evidence-dir>

Each automated check compares what Azad received (input.log), what reached the focused window
(the test window) and the helper's status. Visual items (brightness, LED, emoji picker) are
confirmed by the owner. Results are written to <evidence-dir>/report.json with the raw log
excerpts. Only the prescribed test keys are recorded.
"""

import argparse
import json
import os
import subprocess
import sys
import time

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "../../../.."))
OBJC = os.path.join(ROOT, "crates/azad-capture/fixtures/objc")
BUILD = os.path.join(ROOT, "target/azad-capture-session")
INPUT_LOG = os.path.expanduser("~/Library/Logs/Azad/input.log")
HELPER_LOG = "/var/log/azad-capture.log"
OWNER_ENV = {**os.environ, "AZAD_OWNER_SESSION": "1"}

KEY = {"a": 0, "z": 6, "d": 2, "b": 11, "x": 7, "y": 16, "q": 12, "k": 40, "w": 13,
       "return": 36, "space": 49, "home": 115, "down": 125, "escape": 53, "up": 126}
SHIFT = 0x20000
OPTION = 0x80000
ALPHA_SHIFT = 0x10000


def build():
    os.makedirs(os.path.join(BUILD, "Sink.app/Contents/MacOS"), exist_ok=True)
    run = lambda *cmd: subprocess.run(cmd, check=True)
    run("clang", "-DAZAD_OWNER_SESSION", "-fobjc-arc", "-framework", "AppKit",
        os.path.join(OBJC, "sink.m"), "-o", os.path.join(BUILD, "Sink.app/Contents/MacOS/sink"))
    run("cp", os.path.join(OBJC, "Sink-Info.plist"), os.path.join(BUILD, "Sink.app/Contents/Info.plist"))
    run("codesign", "--force", "--sign", "-", os.path.join(BUILD, "Sink.app"))
    run("clang", "-DAZAD_OWNER_SESSION", "-framework", "ApplicationServices",
        os.path.join(OBJC, "tap.m"), "-o", os.path.join(BUILD, "tap"))
    run("clang", "-DAZAD_OWNER_SESSION", "-framework", "Carbon",
        os.path.join(OBJC, "secure.m"), "-o", os.path.join(BUILD, "secure"))


class Stream:
    """Reads lines appended to a JSONL file since the last mark."""

    def __init__(self, path):
        self.path = path
        self.offset = self.size()

    def size(self):
        try:
            return os.path.getsize(self.path)
        except OSError:
            return 0

    def mark(self):
        self.offset = self.size()

    def since_mark(self):
        try:
            with open(self.path) as f:
                f.seek(self.offset)
                text = f.read()
        except OSError:
            return []
        rows = []
        for line in text.splitlines():
            if line.startswith("{"):
                try:
                    rows.append(json.loads(line))
                except json.JSONDecodeError:
                    pass
        return rows


def listen_enabled():
    out = subprocess.run(["defaults", "read", "ai.azad", "AzadAlwaysListeningEnabled"],
                         capture_output=True, text=True).stdout.strip()
    return out == "1"


def latest_status():
    try:
        lines = open(HELPER_LOG).read().splitlines()
    except OSError:
        return None
    for line in reversed(lines):
        if '"event":"status"' in line:
            return json.loads(line)["status"]
    return None


def ask(question):
    while True:
        answer = input(f"  {question} [y/n/s=skip] ").strip().lower()
        if answer in ("y", "n", "s"):
            return {"y": True, "n": False, "s": None}[answer]


def app_events(rows):
    return [r for r in rows if r.get("event") != "keyboard_capture"]


class Session:
    def __init__(self, out):
        self.out = out
        self.sink_path = os.path.join(out, "sink.jsonl")
        self.results = []
        self.app = Stream(INPUT_LOG)
        self.helper = Stream(HELPER_LOG)
        self.sink = None

    def start_sink(self):
        subprocess.run(["open", "-n", os.path.join(BUILD, "Sink.app"), "--env", "AZAD_OWNER_SESSION=1",
                        "--args", self.sink_path, "7200"], check=True)
        time.sleep(2)
        self.sink = Stream(self.sink_path)

    def step(self, name, keyboard, prompt, check, helper_process=None):
        print(f"\n[{keyboard}] {name}\n  {prompt}")
        self.app.mark()
        self.helper.mark()
        self.sink.mark()
        process = None
        if helper_process:
            process = subprocess.Popen(helper_process, env=OWNER_ENV, stdout=subprocess.PIPE, text=True)
            time.sleep(0.5)
        input("  Click the test window if asked, do it, then press Return here... ")
        time.sleep(0.8)
        extra = None
        if process:
            process.terminate()
            extra = process.communicate(timeout=10)[0]
        app = app_events(self.app.since_mark())
        sink = self.sink.since_mark()
        helper = self.helper.since_mark()
        checks = check(app, sink, helper, extra)
        passed = all(v for v in checks.values() if v is not None)
        print(f"  -> {'PASS' if passed else 'FAIL'} {checks}")
        self.results.append({"keyboard": keyboard, "step": name, "passed": passed, "checks": checks,
                             "app_events": [r["event"] for r in app],
                             "foreground_keys": [(r["kind"], r["keycode"], r["flags"]) for r in sink
                                                 if r.get("kind") in ("down", "up")],
                             "tap": extra})

    def confirm(self, name, keyboard, question):
        print(f"\n[{keyboard}] {name}")
        answer = ask(question)
        self.results.append({"keyboard": keyboard, "step": name, "passed": answer,
                             "checks": {"owner_confirmed": answer}})

    def keyboard_steps(self, keyboard):
        downs = lambda sink: [r for r in sink if r.get("kind") == "down"]
        keys = lambda sink: [r["keycode"] for r in downs(sink)]
        self.step("listen hold", keyboard, "Hold Option+Space for about a second, then release.",
                  lambda app, sink, helper, _: {
                      "hotkey_pressed_and_released": [r["event"] for r in app] ==
                      ["hotkey_pressed", "hotkey_released"],
                      "space_not_delivered": KEY["space"] not in keys(sink)})
        before = listen_enabled()
        self.step("double tap", keyboard, "Double-tap Option+Space quickly.",
                  lambda app, sink, helper, _: {"listen_toggled": listen_enabled() != before,
                                                "space_not_delivered": KEY["space"] not in keys(sink)})
        self.step("double tap back", keyboard, "Double-tap Option+Space again to restore it.",
                  lambda app, sink, helper, _: {"listen_restored": listen_enabled() == before})
        self.step("history search", keyboard,
                  "Hold Option+Space, press Up, release both, type 'ab', then press Escape.",
                  lambda app, sink, helper, _: {
                      "history_opened": any(r["event"] == "arrow_navigate" and r.get("direction") == -1
                                            for r in app),
                      "search_typed": sum(1 for r in app if r["event"] == "history_search_edit") == 2,
                      "closed": any(r["event"] == "overlay_cancel" for r in app),
                      "search_keys_not_delivered": not ({KEY["a"], KEY["b"]} & set(keys(sink)))})
        self.step("overlay keys", keyboard,
                  "Click the test window. Hold Option+Space; while holding, press Shift+Return, "
                  "then Down, then Escape; release.",
                  lambda app, sink, helper, _: {
                      "shift_return_delivered_once": [r["flags"] & SHIFT != 0 for r in downs(sink)
                                                      if r["keycode"] == KEY["return"]] == [True],
                      "down_claimed": any(r["event"] == "arrow_navigate" and r.get("direction") == 1
                                          for r in app),
                      "escape_claimed": any(r["event"] == "overlay_cancel" for r in app),
                      "claimed_keys_not_delivered": not ({KEY["space"], KEY["down"], KEY["escape"]}
                                                         & set(keys(sink)))})
        self.step("ordinary typing", keyboard,
                  "Click the test window and type: azad then Return.",
                  lambda app, sink, helper, _: {
                      "each_key_once": keys(sink) == [KEY["a"], KEY["z"], KEY["a"], KEY["d"], KEY["return"]],
                      "no_azad_events": app == [],
                      "no_stuck_modifier": all(not r["flags"] & (OPTION | SHIFT) for r in downs(sink))})
        self.step("secure input", keyboard,
                  "Secure Input is on now. Hold Option+Space about a second and release, then click "
                  "the test window and type: xy",
                  lambda app, sink, helper, extra: {
                      "secure_input_was_on": '"enabled":1' in (extra or ""),
                      "hotkey_received": [r["event"] for r in app] == ["hotkey_pressed", "hotkey_released"],
                      "typing_delivered_once": keys(sink) == [KEY["x"], KEY["y"]]},
                  helper_process=[os.path.join(BUILD, "secure"), "120"])
        self.step("consuming tap", keyboard,
                  "Another program's event tap now swallows all keys. Hold Option+Space about a second "
                  "and release, then type: q",
                  lambda app, sink, helper, extra: {
                      "tap_installed": None if "tap_failed" in (extra or "") else '"tap_ready"' in (extra or ""),
                      "hotkey_received": [r["event"] for r in app] == ["hotkey_pressed", "hotkey_released"],
                      "space_never_seen_by_tap": '"keycode":49' not in (extra or ""),
                      "q_seen_once_by_tap": (extra or "").count('"type":"down","keycode":12') == 1},
                  helper_process=[os.path.join(BUILD, "tap"), "120", "--consume"])

    def fidelity_steps(self):
        keys = lambda sink: [r["keycode"] for r in sink if r.get("kind") == "down"]
        kb = "built-in"
        self.confirm("F-row brightness", kb, "Press F1 then F2: did the display dim then brighten?")
        self.confirm("F-row volume", kb, "Press F11 then F12: did the volume go down then up?")
        self.step("fn+Left", kb, "Click the test window, then press fn+Left arrow.",
                  lambda app, sink, helper, _: {"home_delivered": KEY["home"] in keys(sink)})
        self.confirm("Globe key", kb, "Press and release fn/Globe alone: did the emoji or "
                     "input-source picker behave as it normally does for you?")
        self.step("Caps Lock", kb,
                  "Click the test window. Press Caps Lock, type a, press Caps Lock, type a.",
                  lambda app, sink, helper, _: {
                      "caps_applied_then_released": [r["flags"] & ALPHA_SHIFT != 0 for r in sink
                                                     if r.get("kind") == "down" and r["keycode"] == KEY["a"]]
                      == [True, False]})
        self.confirm("Caps Lock LED", kb, "Did the Caps Lock light turn on and then off?")
        self.step("key repeat", kb, "Click the test window and hold k for about two seconds.",
                  lambda app, sink, helper, _: {
                      "repeats": sum(1 for r in sink if r.get("kind") == "down" and r["keycode"] == KEY["k"]
                                     and r.get("repeat")) >= 5})

    def sleep_wake(self):
        print("\n[system] sleep and wake")
        input("  Close the lid (or sleep the Mac) for about 15 seconds, wake it, unlock, then press "
              "Return here... ")
        time.sleep(3)
        status = latest_status() or {}
        self.results.append({"keyboard": "system", "step": "after wake", "passed": status.get("capture") ==
                             "capturing", "checks": {"capturing_after_wake": status.get("capture") ==
                                                     "capturing"}, "status": status})
        self.step("typing after wake", "any", "Click the test window and type: w",
                  lambda app, sink, helper, _: {
                      "w_delivered_once": [r["keycode"] for r in sink if r.get("kind") == "down"] == [KEY["w"]]})

    def write(self):
        report = {"host": subprocess.run(["sysctl", "-n", "hw.model"], capture_output=True, text=True)
                  .stdout.strip(),
                  "os": subprocess.run(["sw_vers", "-productVersion"], capture_output=True, text=True)
                  .stdout.strip(),
                  "helper_status": latest_status(),
                  "results": self.results,
                  "passed": all(r["passed"] is not False for r in self.results)}
        with open(os.path.join(self.out, "report.json"), "w") as f:
            json.dump(report, f, indent=1)
        print(f"\nReport: {os.path.join(self.out, 'report.json')}  passed={report['passed']}")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--out", required=True)
    args = parser.parse_args()
    if not sys.stdin.isatty():
        sys.exit("The physical session is interactive; run it from a terminal.")
    print(__doc__)
    if input("Type 'yes' to start the owner-attended session: ").strip() != "yes":
        sys.exit("Not started.")
    status = latest_status()
    if not status or status.get("capture") != "capturing":
        sys.exit(f"The capture helper is not capturing: {status}")
    os.makedirs(args.out, exist_ok=True)
    build()
    session = Session(args.out)
    session.start_sink()
    captured = sorted({d["product"] for d in status["devices"] if d["state"] == "seized"})
    print(f"Captured devices: {captured}")
    # Some mice expose a keyboard interface; only test what the owner types on.
    keyboards = [name for name in captured if ask(f"Test typing on '{name}'?")]
    for keyboard in keyboards:
        input(f"\nUse only the {keyboard} for the next steps. Press Return to begin... ")
        session.keyboard_steps(keyboard)
    if any("Internal" in k for k in keyboards):
        session.fidelity_steps()
    if any("Keychron" in k for k in keyboards):
        session.confirm("Keychron mouse keys", "Keychron Q6 HE",
                        "If you use the Keychron's mouse keys, do they still move and click?")
    session.sleep_wake()
    session.write()
    subprocess.run(["pkill", "-f", os.path.join(BUILD, "Sink.app")])


if __name__ == "__main__":
    main()
