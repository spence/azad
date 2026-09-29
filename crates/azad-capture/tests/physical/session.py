#!/usr/bin/env python3
"""Owner-attended physical keyboard session for Azad's device-level capture.

Run by the owner, on the Mac whose keyboards are under test, after installing the build to
verify. It never posts input: the owner presses every key. It shows a test window that records
the keys it receives, reads Azad's input log and the capture helper's log, and can briefly turn
on Secure Input or a consuming event tap (its own processes, stopped at the end of the step).

    python3 crates/azad-capture/tests/physical/session.py --out <evidence-dir> [--rerun]

Each automated check compares what Azad received (input.log), what reached the focused window
(the test window) and the helper's status. Visual items (brightness, LED, emoji picker) are
confirmed by the owner. The helper's content-free edge counters give a check per step that does
not depend on the test window. Results are written to <evidence-dir>/report.json. Only the
prescribed test keys are recorded. --rerun repeats only the steps that need the test window, on
the built-in keyboard and the Keychron.

If typing ever stops working, quit Azad from its menu bar item with the mouse: the capture
helper releases every keyboard as soon as Azad disconnects.
"""

import argparse
import json
import os
import re
import subprocess
import sys
import time

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "../../../.."))
OBJC = os.path.join(ROOT, "crates/azad-capture/fixtures/objc")
BUILD = os.path.join(ROOT, "target/azad-capture-session")
INPUT_LOG = os.path.expanduser("~/Library/Logs/Azad/input.log")
HELPER_LOG = "/var/log/azad-capture.log"
APP = os.path.expanduser("~/Applications/Azad.app")
HELPER = "Contents/Library/Helpers/Azad Capture.app"
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


def latest_counters():
    try:
        lines = open(HELPER_LOG).read().splitlines()
    except OSError:
        return None
    for line in reversed(lines):
        if '"event":"counters"' in line:
            return json.loads(line)
    return None


def kernel_time(name):
    out = subprocess.run(["sysctl", "-n", name], capture_output=True, text=True).stdout
    match = re.search(r"sec = (\d+)", out)
    return int(match.group(1)) if match else 0


def installed_identities():
    identities = {}
    for name, path in [("app", APP), ("helper", os.path.join(APP, HELPER))]:
        out = subprocess.run(["codesign", "-dvvv", path], capture_output=True, text=True).stderr
        identities[name] = [line for line in out.splitlines()
                            if line.startswith(("Identifier=", "CDHash=", "TeamIdentifier="))]
    return identities


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
        self.tap_available = None

    def start_sink(self):
        subprocess.run(["open", "-n", os.path.join(BUILD, "Sink.app"), "--env", "AZAD_OWNER_SESSION=1",
                        "--args", self.sink_path, "7200"], check=True)
        for _ in range(50):
            time.sleep(0.1)
            if os.path.exists(self.sink_path) and '"ready"' in open(self.sink_path).read():
                break
        else:
            sys.exit("The test window did not start; nothing was tested.")
        self.sink = Stream(self.sink_path)

    def check_tap(self):
        out = subprocess.run([os.path.join(BUILD, "tap"), "0", "--consume"], env=OWNER_ENV,
                             capture_output=True, text=True).stdout
        self.tap_available = '"tap_ready"' in out
        if not self.tap_available:
            print("This terminal may not install an event tap, so the consuming-tap step is skipped.")

    def step(self, name, keyboard, prompt, check, helper_process=None, timed=False, claimed=None):
        print(f"\n[{keyboard}] {name}\n  {prompt}")
        before = latest_counters() or {}
        self.app.mark()
        self.helper.mark()
        self.sink.mark()
        process = None
        if helper_process:
            process = subprocess.Popen(helper_process, env=OWNER_ENV, stdout=subprocess.PIPE, text=True)
            time.sleep(0.5)
        if timed:
            # The terminal cannot receive Return while the step swallows typing; it ends by itself.
            extra = process.communicate(timeout=60)[0]
            print("  The step has ended; typing works again.")
        else:
            input("  Click the test window if asked, do it, then press Return here... ")
            time.sleep(0.8)
            extra = None
            if process:
                process.terminate()
                extra = process.communicate(timeout=10)[0]
        # The helper writes its counters every 5 s.
        time.sleep(5.5)
        after = latest_counters() or {}
        edges = {k: after.get(k, 0) - before.get(k, 0)
                 for k in ("claimed_edges", "forwarded_edges", "forward_errors")}
        app = app_events(self.app.since_mark())
        sink = self.sink.since_mark()
        helper = self.helper.since_mark()
        checks = check(app, sink, helper, extra)
        checks["sink_running"] = self.sink_alive()
        checks["no_forward_errors"] = bool(after) and edges["forward_errors"] == 0
        if claimed == "none":
            checks["helper_claimed_nothing"] = bool(after) and edges["claimed_edges"] == 0
        elif claimed == "some":
            checks["helper_claimed_keys"] = edges["claimed_edges"] > 0
        passed = all(v for v in checks.values() if v is not None)
        print(f"  -> {'PASS' if passed else 'FAIL'} {checks}")
        self.results.append({"keyboard": keyboard, "step": name, "passed": passed, "checks": checks,
                             "app_events": [r["event"] for r in app],
                             "foreground_keys": [(r["kind"], r["keycode"], r["flags"]) for r in sink
                                                 if r.get("kind") in ("down", "up")],
                             "helper_edges": edges, "tap": extra})

    def sink_alive(self):
        return subprocess.run(["pgrep", "-f", os.path.join(BUILD, "Sink.app")],
                              capture_output=True).returncode == 0

    def confirm(self, name, keyboard, question):
        print(f"\n[{keyboard}] {name}")
        answer = ask(question)
        self.results.append({"keyboard": keyboard, "step": name, "passed": answer,
                             "checks": {"owner_confirmed": answer}})

    def shortcut_steps(self, keyboard):
        keys = lambda sink: [r["keycode"] for r in sink if r.get("kind") == "down"]
        self.step("listen hold", keyboard, "Hold Option+Space for about a second, then release.",
                  lambda app, sink, helper, _: {
                      "hotkey_pressed_and_released": [r["event"] for r in app] ==
                      ["hotkey_pressed", "hotkey_released"],
                      "space_not_delivered": KEY["space"] not in keys(sink)}, claimed="some")
        before = listen_enabled()
        self.step("double tap", keyboard, "Double-tap Option+Space quickly.",
                  lambda app, sink, helper, _: {"listen_toggled": listen_enabled() != before,
                                                "space_not_delivered": KEY["space"] not in keys(sink)},
                  claimed="some")
        self.step("double tap back", keyboard, "Double-tap Option+Space again to restore it.",
                  lambda app, sink, helper, _: {"listen_restored": listen_enabled() == before},
                  claimed="some")
        if listen_enabled() != before:
            print("  Always Listening is not back where it was; set it from Azad's menu.")
        self.step("history search", keyboard,
                  "Hold Option+Space, press Up, release both, type 'ab', then press Escape.",
                  lambda app, sink, helper, _: {
                      "history_opened": any(r["event"] == "arrow_navigate" and r.get("direction") == -1
                                            for r in app),
                      "search_typed": sum(1 for r in app if r["event"] == "history_search_edit") == 2,
                      "closed": any(r["event"] == "overlay_cancel" for r in app),
                      "search_keys_not_delivered": not ({KEY["a"], KEY["b"]} & set(keys(sink)))},
                  claimed="some")

    def window_steps(self, keyboard, interference):
        downs = lambda sink: [r for r in sink if r.get("kind") == "down"]
        keys = lambda sink: [r["keycode"] for r in downs(sink)]
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
                                                         & set(keys(sink)))}, claimed="some")
        self.step("ordinary typing", keyboard,
                  "Click the test window and type: azad then Return.",
                  lambda app, sink, helper, _: {
                      "each_key_once": keys(sink) == [KEY["a"], KEY["z"], KEY["a"], KEY["d"], KEY["return"]],
                      "no_azad_events": app == [],
                      "no_stuck_modifier": all(not r["flags"] & (OPTION | SHIFT) for r in downs(sink))},
                  claimed="none")
        if not interference:
            return
        self.step("secure input", keyboard,
                  "Secure Input is on now. Hold Option+Space about a second and release, then click "
                  "the test window and type: xy",
                  lambda app, sink, helper, extra: {
                      "secure_input_was_on": '"enabled":1' in (extra or ""),
                      "hotkey_received": [r["event"] for r in app] == ["hotkey_pressed", "hotkey_released"],
                      "typing_delivered_once": keys(sink) == [KEY["x"], KEY["y"]]},
                  helper_process=[os.path.join(BUILD, "secure"), "120"], claimed="some")
        if not self.tap_available:
            self.results.append({"keyboard": keyboard, "step": "consuming tap", "passed": None,
                                 "checks": {"tap_installed": None},
                                 "reason": "this terminal may not install an event tap"})
            return
        self.step("consuming tap", keyboard,
                  "For the next 15 seconds another program's event tap swallows every key, so only "
                  "Azad's shortcut works and this terminal will not respond. Hold Option+Space about a "
                  "second and release, then type: q. The step ends by itself.",
                  lambda app, sink, helper, extra: {
                      "tap_installed": '"tap_ready"' in (extra or ""),
                      "hotkey_received": [r["event"] for r in app] == ["hotkey_pressed", "hotkey_released"],
                      "space_never_seen_by_tap": '"keycode":49' not in (extra or ""),
                      "q_seen_once_by_tap": (extra or "").count('"type":"down","keycode":12') == 1},
                  helper_process=[os.path.join(BUILD, "tap"), "15", "--consume"], timed=True,
                  claimed="some")

    def fidelity_confirmations(self):
        kb = "built-in"
        self.confirm("F-row brightness", kb, "Press F1 then F2: did the display dim then brighten?")
        self.confirm("F-row volume", kb, "Press F11 then F12: did the volume go down then up?")
        self.confirm("Globe key", kb, "Press and release fn/Globe alone: did the emoji or "
                     "input-source picker behave as it normally does for you?")
        self.confirm("Touch ID / power button", kb, "Press the Touch ID (power) button briefly: did the "
                     "Mac lock or sleep the display as it normally does?")

    def fidelity_window_steps(self):
        keys = lambda sink: [r["keycode"] for r in sink if r.get("kind") == "down"]
        kb = "built-in"
        self.step("fn+Left", kb, "Click the test window, then press fn+Left arrow.",
                  lambda app, sink, helper, _: {"home_delivered": KEY["home"] in keys(sink)},
                  claimed="none")
        self.step("Caps Lock", kb,
                  "Click the test window. Press Caps Lock, type a, press Caps Lock, type a.",
                  lambda app, sink, helper, _: {
                      "caps_applied_then_released": [r["flags"] & ALPHA_SHIFT != 0 for r in sink
                                                     if r.get("kind") == "down" and r["keycode"] == KEY["a"]]
                      == [True, False]}, claimed="none")
        self.confirm("Caps Lock LED", kb, "Did the Caps Lock light turn on and then off?")
        self.step("key repeat", kb, "Click the test window and hold k for about two seconds.",
                  lambda app, sink, helper, _: {
                      "repeats": sum(1 for r in sink if r.get("kind") == "down" and r["keycode"] == KEY["k"]
                                     and r.get("repeat")) >= 5}, claimed="none")

    def sleep_wake(self):
        print("\n[system] sleep and wake")
        started = int(time.time())
        input("  Choose Apple menu > Sleep (closing the lid does not sleep a Mac with an external "
              "display), wait about 15 seconds, wake it, unlock, then press Return here... ")
        time.sleep(3)
        status = latest_status() or {}
        checks = {"system_slept": kernel_time("kern.sleeptime") >= started,
                  "system_woke": kernel_time("kern.waketime") >= started,
                  "capturing_after_wake": status.get("capture") == "capturing"}
        self.results.append({"keyboard": "system", "step": "after wake", "passed": all(checks.values()),
                             "checks": checks, "status": status})
        self.step("hotkey after wake", "any", "Hold Option+Space for about a second, then release.",
                  lambda app, sink, helper, _: {
                      "hotkey_pressed_and_released": [r["event"] for r in app] ==
                      ["hotkey_pressed", "hotkey_released"],
                      "space_not_delivered": KEY["space"] not in
                      [r["keycode"] for r in sink if r.get("kind") == "down"]}, claimed="some")
        self.step("typing after wake", "any", "Click the test window and type: w",
                  lambda app, sink, helper, _: {
                      "w_delivered_once": [r["keycode"] for r in sink if r.get("kind") == "down"] == [KEY["w"]]},
                  claimed="none")

    def write(self):
        report = {"host": subprocess.run(["sysctl", "-n", "hw.model"], capture_output=True, text=True)
                  .stdout.strip(),
                  "os": subprocess.run(["sw_vers", "-productVersion"], capture_output=True, text=True)
                  .stdout.strip(),
                  "installed": installed_identities(),
                  "helper_status": latest_status(),
                  "results": self.results,
                  "passed": all(r["passed"] is not False for r in self.results)}
        with open(os.path.join(self.out, "report.json"), "w") as f:
            json.dump(report, f, indent=1)
        print(f"\nReport: {os.path.join(self.out, 'report.json')}  passed={report['passed']}")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--out", required=True)
    parser.add_argument("--rerun", action="store_true",
                        help="only the steps that need the test window, on the built-in keyboard and "
                             "the Keychron")
    args = parser.parse_args()
    if not sys.stdin.isatty():
        sys.exit("The physical session is interactive; run it from a terminal.")
    print(__doc__)
    if input("Type 'yes' to start the owner-attended session: ").strip() != "yes":
        sys.exit("Not started.")
    status = latest_status()
    if not status or status.get("capture") != "capturing":
        sys.exit(f"The capture helper is not capturing: {status}")
    out = os.path.abspath(args.out)
    os.makedirs(out, exist_ok=True)
    build()
    session = Session(out)
    session.start_sink()
    session.check_tap()
    captured = sorted({d["product"] for d in status["devices"] if d["state"] == "seized"})
    print(f"Captured devices: {captured}")
    if args.rerun:
        keyboards = [name for name in captured if "Internal" in name or "Keychron" in name]
    else:
        # Some mice expose a keyboard interface; only test what the owner types on.
        keyboards = [name for name in captured if ask(f"Test typing on '{name}'?")]
    internal = [k for k in keyboards if "Internal" in k]
    for keyboard in internal + [k for k in keyboards if k not in internal]:
        input(f"\nUse only the {keyboard} for the next steps. Press Return to begin... ")
        if not args.rerun:
            session.shortcut_steps(keyboard)
        session.window_steps(keyboard, interference=not args.rerun or keyboard in internal)
        if keyboard in internal:
            if not args.rerun:
                session.fidelity_confirmations()
            session.fidelity_window_steps()
    if not args.rerun:
        if any("Keychron" in k for k in keyboards):
            session.confirm("Keychron media keys", "Keychron Q6 HE",
                            "Press the Keychron's volume or brightness keys: do they work as before?")
            session.confirm("Keychron mouse keys", "Keychron Q6 HE",
                            "If you use the Keychron's mouse keys, do they still move and click?")
        session.sleep_wake()
    session.write()
    subprocess.run(["pkill", "-f", os.path.join(BUILD, "Sink.app")])


if __name__ == "__main__":
    main()
