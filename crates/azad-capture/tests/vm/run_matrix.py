#!/usr/bin/env python3
"""Runs the azad-capture access matrix inside a disposable, SIP-enabled macOS VM.

The host only builds, signs, copies, and reads results. Every input-producing fixture refuses to
run outside a VirtualMac guest. Nothing here touches the host desktop, its keyboards, or the
installed Azad app.

    run_matrix.py --vm <tart-vm-name> --out <evidence-dir> [--stage] [--only NAME ...]

The VM must already have the Karabiner DriverKit VirtualHIDDevice driver activated and the
`admin`/`admin` account of the Cirrus Labs base images. `--stage` rebuilds and installs the
helper and fixtures first. Results land in <evidence-dir>/<scenario>/ plus results.json.
"""

import argparse
import hashlib
import io
import json
import os
import subprocess
import sys
import tarfile
import time

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "../../../.."))
CRATE = os.path.join(ROOT, "crates/azad-capture")
FIXTURES = os.path.join(CRATE, "fixtures")
GUEST = "/tmp/azt"
HELPER_DIR = "/Library/Application Support/Azad"
HELPER_APP = f"{HELPER_DIR}/Azad Capture.app"
HELPER_LOG = "/var/log/azad-capture.log"
IDENTITY = os.environ.get("AZAD_CODESIGN_IDENTITY", "")

OPTION = 0x80000
SHIFT = 0x20000
# Mac virtual keycodes of keys the shortcut sequence claims; none may reach the foreground.
CLAIMED_KEYCODES = {49: "space", 126: "up", 125: "down", 123: "left", 124: "right", 53: "escape",
                    76: "keypad_enter"}
RETURN, KEY_A = 36, 0

OVERLAY_CONTEXT = {"listen_modifiers": 4, "escape": True, "enter": True, "arrows": True,
                   "arrow_left": True, "arrow_right": True, "search_input": False}
SHORTCUT_ACTIONS = [
    {"kind": "hotkey_pressed"},
    {"kind": "hotkey_released", "raw_requested": True},
    {"kind": "hotkey_pressed"},
    {"kind": "navigate", "direction": -1},
    {"kind": "hotkey_released", "raw_requested": False},
    {"kind": "finalize", "raw_requested": False},
    {"kind": "cancel"},
    {"kind": "navigate", "direction": -1},
    {"kind": "navigate", "direction": 1},
    {"kind": "history_collapse"},
    {"kind": "history_expand"},
    {"kind": "finalize", "raw_requested": False},
]

# Scenarios against the installed Azad.app (with its SMAppService helper) as the client. The
# guest needs Azad installed, onboarding complete, Accessibility granted, and the three history
# fixture entries ("qwerty fixture one" newest, "awerty fixture two", "zebra fixture three").
APP_SCENARIOS = {
    "app_hold_release": {
        "about": "Option+Space hold with listening off; Option released before Space.",
        "script": "app-hold.json",
    },
    "app_double_tap_on": {
        "about": "Two quick Option+Space taps turn always-listening on.",
        "script": "app-double-tap.json", "listen_after": True,
    },
    "app_double_tap_off": {
        "about": "Two quick Option+Space taps turn always-listening back off.",
        "script": "app-double-tap.json", "listen_after": False,
    },
    "app_history_search_us": {
        "about": "Hold+Up opens history; the key at HID 0x04 types 'a' on the US layout, "
                 "filters to 'awerty fixture two', and Enter pastes it into the foreground app.",
        "script": "app-history.json", "layout": "com.apple.keylayout.US",
        "pasted": "awerty fixture two",
    },
    "app_history_search_french": {
        "about": "The same keys on the French (AZERTY) layout type 'q', so history search is "
                 "resolved with the active layout and pastes 'qwerty fixture one'.",
        "script": "app-history.json", "layout": "com.apple.keylayout.French",
        "pasted": "qwerty fixture one",
    },
    "app_overlay_keys": {
        "about": "During a hold: Shift+Return reaches the foreground, Down navigates, keypad "
                 "Enter finalizes, Escape cancels; none of the claimed keys reach the foreground.",
        "script": "app-overlay-keys.json",
    },
    "app_caps_lock_and_repeat": {
        "about": "Caps Lock toggles through the virtual keyboard (the next 'a' is capitalized, "
                 "then not), and holding 'a' auto-repeats with a single release.",
        "script": "app-caps-repeat.json",
    },
    "app_ordinary_typing": {
        "about": "With the overlay hidden, a, Return, Escape, Up and Shift+A all reach the "
                 "foreground exactly once and Azad handles none of them.",
        "script": "app-typing.json",
    },
}

# Failures against the installed app. Each interrupts a claimed Option+Space hold; afterwards
# ordinary typing must reach the foreground with no stuck modifier, and the app must not be left
# in a held state.
FAILURE_SCENARIOS = {
    "app_quit_mid_hold": {
        "about": "The Azad app is killed mid-hold (helper loses its client).",
        "inject": "pkill -9 -f /Applications/Azad.app/Contents/MacOS/azad",
        "relaunch_app": True,
    },
    "helper_hang_mid_hold": {
        "about": "The helper's main loop hangs mid-hold; its watchdog exits so the kernel "
                 "releases the keyboards, launchd restarts it, and the app ends the hold.",
        "inject": "echo admin | sudo -S -p '' touch /var/run/ai.azad.capture.fault-hang",
        "cleanup": "echo admin | sudo -S -p '' rm -f /var/run/ai.azad.capture.fault-hang",
        "cleanup_after_s": 4,
    },
    "driver_daemon_killed_mid_hold": {
        "about": "The virtual keyboard daemon dies mid-hold (taking every driver keyboard, "
                 "including the test source, with it); the helper releases the keyboards and "
                 "the hold, restarts the daemon, and a new keyboard's typing is forwarded.",
        "inject": "echo admin | sudo -S -p '' pkill -9 -f Karabiner-VirtualHIDDevice-Daemon",
        "then_type_from_new_keyboard": True,
    },
    "forwarding_fails_mid_hold": {
        "about": "Posting to the virtual keyboard fails mid-hold while the keyboard stays "
                 "connected; the helper stops capturing, so the rest reaches the OS directly.",
        "inject": "echo admin | sudo -S -p '' touch /var/run/ai.azad.capture.fault-forward",
        "cleanup": "echo admin | sudo -S -p '' rm -f /var/run/ai.azad.capture.fault-forward",
        "cleanup_after_s": 8,
    },
    "keyboard_removed_mid_hold": {
        "about": "The keyboard disappears while Option+Space is held; the helper releases its "
                 "keys and the app ends the hold. A second keyboard then types.",
        "remove_after_report": 1,
    },
    "two_keyboards": {
        "about": "Option held on one keyboard, Space pressed on another, then typing on the "
                 "second: the chord is claimed and typing arrives once.",
        "two_keyboards": True,
    },
    "client_not_console_user": {
        "about": "A correctly signed client running as a user who does not own the console "
                 "(fast user switching): the helper must not capture for it.",
        "other_user_client": True,
    },
}

SCENARIOS = {
    "baseline": {
        "about": "Secure Input off, no competing tap.",
        "script": "shortcuts.json", "tap": None, "expect": "capture",
    },
    "secure_throughout": {
        "about": "Secure Input enabled for the entire sequence.",
        "script": "shortcuts-secure-throughout.json", "tap": None, "expect": "capture",
    },
    "consuming_tap": {
        "about": "An earlier consuming HID event tap swallows every key event it sees.",
        "script": "shortcuts.json", "tap": "consume", "expect": "capture",
    },
    "secure_transition_consuming_tap": {
        "about": "Secure Input turns on during the first Space hold and off before Return, "
                 "with a consuming HID tap installed.",
        "script": "shortcuts-secure-transition.json", "tap": "consume", "expect": "capture",
    },
    "cooperative_remapper": {
        "about": "A preexisting exclusive owner (Karabiner-style remapper) seizes the keyboard "
                 "and re-emits it through its own driver keyboard; Azad captures that output.",
        "script": "shortcuts.json", "tap": None, "owner": "remap", "expect": "capture",
    },
    "after_helper_restart": {
        "about": "The helper is restarted by launchd before the sequence; the existing Input "
                 "Monitoring grant must still yield per-device report flow.",
        "script": "shortcuts.json", "tap": None, "expect": "capture", "restart_helper": True,
    },
    "crash_during_hold": {
        "about": "The helper is killed (SIGKILL) while Space is held. The kernel must release the "
                 "keyboard: later ordinary typing reaches the foreground with no stuck modifier, "
                 "and launchd restarts the helper.",
        "script": "shortcuts.json", "tap": None, "expect": "fail_open", "kill_at_report": 4,
    },
    "app_hang_releases_context": {
        "about": "The app publishes a history-search context (all typing claimed) and then stops "
                 "heartbeating while staying connected. After the lease lapses the helper keeps "
                 "only the listen chord: typing and Return reach the foreground again.",
        "script": "hang.json", "tap": None, "expect": "hang_fail_open", "silent_after_ms": 1500,
        "context": {"listen_modifiers": 4, "escape": True, "enter": True, "arrows": True,
                    "arrow_left": True, "arrow_right": True, "search_input": True},
    },
    "karabiner_elements": {
        "about": "Karabiner-Elements 16.3.0 is installed, owns the VM keyboard and runs its own "
                 "driver daemon. Azad attaches to that daemon, yields the physical keyboard, "
                 "captures Karabiner's output keyboard, and Karabiner never grabs Azad's output. "
                 "Needs a VM with Karabiner-Elements set up.",
        "script": "shortcuts.json", "tap": "consume", "expect": "capture", "karabiner": True,
    },
    "negative_uncooperative_owner": {
        "about": "A preexisting exclusive owner discards input. Azad must report the device as "
                 "owned by another process and receive nothing.",
        "script": "shortcuts.json", "tap": None, "owner": "discard", "expect": "owned_by_other",
    },
    "negative_unauthorized_client": {
        "about": "A client not signed as Azad connects. It must be rejected and capture must not "
                 "start.",
        "script": "shortcuts.json", "tap": None, "client": "intruder", "expect": "rejected",
    },
    "negative_no_permission": {
        "about": "Input Monitoring is revoked. The helper must report permission_denied, seize "
                 "nothing, and claimed keys reach the foreground (the loss the grant prevents).",
        "script": "shortcuts.json", "tap": None, "expect": "permission_denied",
    },
}


def sh(cmd, **kw):
    return subprocess.run(cmd, check=True, text=True, capture_output=True, **kw).stdout


class Vm:
    def __init__(self, name):
        self.name = name
        self.ip = sh(["tart", "ip", name]).strip()
        self.known = os.path.join(os.environ.get("TMPDIR", "/tmp"), f"azt-known-{name}")

    def ssh(self, script, check=True, timeout=180):
        cmd = ["sshpass", "-p", "admin", "ssh", "-o", "StrictHostKeyChecking=no",
               "-o", f"UserKnownHostsFile={self.known}", "-o", "LogLevel=ERROR",
               f"admin@{self.ip}", "bash -s"]
        result = subprocess.run(cmd, input=script, text=True, capture_output=True, timeout=timeout)
        if check and result.returncode != 0:
            raise RuntimeError(f"guest command failed ({result.returncode}): {result.stderr}")
        return result

    def ssh_bytes(self, script, timeout=120):
        cmd = ["sshpass", "-p", "admin", "ssh", "-o", "StrictHostKeyChecking=no",
               "-o", f"UserKnownHostsFile={self.known}", "-o", "LogLevel=ERROR",
               f"admin@{self.ip}", "bash -s"]
        return subprocess.run(cmd, input=script.encode(), capture_output=True, timeout=timeout,
                              check=True).stdout

    def put(self, local, remote):
        subprocess.run(["sshpass", "-p", "admin", "scp", "-q", "-o", "StrictHostKeyChecking=no",
                        "-o", f"UserKnownHostsFile={self.known}", local, f"admin@{self.ip}:{remote}"],
                       check=True)


def sudo(line):
    return f"echo admin | sudo -S -p '' {line}"


def build_stage(stage):
    if not IDENTITY:
        sys.exit("AZAD_CODESIGN_IDENTITY must name the Developer ID used for the helper")
    os.makedirs(stage, exist_ok=True)
    sh([os.path.join(CRATE, "scripts/bundle-helper.sh"), stage, IDENTITY])
    sh(["cargo", "build", "--release", "-p", "azad-capture-fixtures"], cwd=ROOT)
    target = os.path.join(ROOT, "target/release")
    bins = os.path.join(stage, "bin")
    os.makedirs(bins, exist_ok=True)
    for name in ["fixture-keyboard", "fixture-owner"]:
        sh(["cp", os.path.join(target, name), bins])
        sh(["codesign", "--force", "--sign", "-", os.path.join(bins, name)])
    # The accepted client carries Azad's identity; the intruder differs only in identifier.
    for name, ident in [("fixture-app", "ai.azad"), ("fixture-intruder", "ai.azad.intruder")]:
        path = os.path.join(bins, name)
        sh(["cp", os.path.join(target, "fixture-app"), path])
        sh(["codesign", "--force", "--sign", IDENTITY, "--options", "runtime", "--identifier",
            ident, path])
    objc = os.path.join(FIXTURES, "objc")
    sink = os.path.join(bins, "Sink.app/Contents")
    os.makedirs(os.path.join(sink, "MacOS"), exist_ok=True)
    sh(["clang", "-fobjc-arc", "-framework", "AppKit", os.path.join(objc, "sink.m"), "-o",
        os.path.join(sink, "MacOS/sink")])
    sh(["cp", os.path.join(objc, "Sink-Info.plist"), os.path.join(sink, "Info.plist")])
    sh(["codesign", "--force", "--sign", "-", os.path.join(bins, "Sink.app")])
    sh(["clang", "-framework", "ApplicationServices", os.path.join(objc, "tap.m"), "-o",
        os.path.join(bins, "tap")])
    sh(["codesign", "--force", "--sign", "-", os.path.join(bins, "tap")])
    sh(["cp", "-R", os.path.join(FIXTURES, "scenarios"), stage])
    return stage


def plist():
    return f"""<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>ai.azad.capture</string>
  <key>ProgramArguments</key><array><string>{HELPER_APP}/Contents/MacOS/azad-capture</string></array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>AbandonProcessGroup</key><true/>
  <key>StandardOutPath</key><string>{HELPER_LOG}</string>
  <key>StandardErrorPath</key><string>{HELPER_LOG}</string>
</dict></plist>
"""


def install(vm, stage):
    with open(os.path.join(stage, "ai.azad.capture.plist"), "w") as f:
        f.write(plist())
    archive = os.path.join(stage, "..", "azt-stage.tgz")
    with tarfile.open(archive, "w:gz") as tar:
        for name in ["Azad Capture.app", "bin", "scenarios", "ai.azad.capture.plist"]:
            tar.add(os.path.join(stage, name), arcname=name)
    vm.put(archive, "/tmp/azt-stage.tgz")
    vm.ssh(f"""
set -e
rm -rf {GUEST} && mkdir -p {GUEST} && tar xzf /tmp/azt-stage.tgz -C {GUEST}
{sudo('launchctl bootout system/ai.azad.capture 2>/dev/null || true')}
sleep 1
{sudo(f'rm -rf "{HELPER_APP}"')}
{sudo(f'mkdir -p "{HELPER_DIR}"')}
{sudo(f'ditto "{GUEST}/Azad Capture.app" "{HELPER_APP}"')}
{sudo(f'chown -R root:wheel "{HELPER_DIR}"')}
{sudo(f'install -o root -g wheel -m 644 {GUEST}/ai.azad.capture.plist /Library/LaunchDaemons/ai.azad.capture.plist')}
{sudo(f'launchctl bootstrap system /Library/LaunchDaemons/ai.azad.capture.plist')}
sleep 3
""")


def scenario_script(name, spec):
    run = f"{GUEST}/run/{name}"
    client = "fixture-intruder" if spec.get("client") == "intruder" else "fixture-app"
    context = json.dumps(spec.get("context", OVERLAY_CONTEXT))
    if spec.get("silent_after_ms"):
        client += f" --silent-after-ms {spec['silent_after_ms']}"
    lines = [
        "set -u",
        f"D={run}; B={GUEST}/bin",
        "rm -rf $D && mkdir -p $D",
        sudo(f"wc -l < {HELPER_LOG}") + " > $D/logstart",
        *([sudo("launchctl kickstart -k system/ai.azad.capture"), "sleep 5",
           sudo("launchctl print system/ai.azad.capture") + " | grep -E '^\\s+pid' > $D/restart.txt"]
          if spec.get("restart_helper") else []),
        "open -n $B/Sink.app --args $D/sink.jsonl 22",
        "sleep 2",
    ]
    if spec.get("tap"):
        lines.append("$B/tap 18 --consume > $D/tap.jsonl 2>&1 &")
        lines.append("sleep 1")
    script = f"{GUEST}/scenarios/{spec['script']}"
    if spec.get("owner"):
        # The owner must hold the source before Azad's client activates capture.
        lines += [
            sudo(f"$B/fixture-keyboard --script {script} --settle-ms 7000") + " > $D/source.jsonl 2>&1 &",
            "SRC=$!",
            "sleep 1",
            sudo(f"$B/fixture-owner --mode {spec['owner']} --seconds 17") + " > $D/owner.jsonl 2>&1 &",
            "sleep 3",
            f"$B/{client} --seconds 12 --context '{context}' > $D/app.jsonl 2>&1 &",
        ]
    else:
        lines += [
            f"$B/{client} --seconds 12 --context '{context}' > $D/app.jsonl 2>&1 &",
            "sleep 1",
            sudo(f"$B/fixture-keyboard --script {script} --settle-ms 3000") + " > $D/source.jsonl 2>&1 &",
            "SRC=$!",
        ]
    if spec.get("kill_at_report"):
        lines += [
            # Kill once the source has posted the report that starts the claimed Space hold.
            f"for i in $(seq 200); do grep -q '\"index\":{spec['kill_at_report']},' $D/source.jsonl && break; sleep 0.05; done",
            sudo("launchctl print system/ai.azad.capture") + " | grep -E '^\\s+pid' > $D/pid-before.txt",
            sudo("pkill -9 -f 'Azad Capture.app/Contents/MacOS/azad-capture$'"),
            "sleep 2",
            sudo("launchctl print system/ai.azad.capture") + " | grep -E '^\\s+pid' > $D/pid-after.txt",
        ]
    lines += [
        "wait $SRC",
        "sleep 13",
        sudo(f"tail -n +$(( $(cat $D/logstart) + 1 )) {HELPER_LOG}") + " > $D/helper.jsonl",
        sudo("grep -hE 'hid device events monitor is started|is terminated' "
             "/var/log/karabiner/core_service.log 2>/dev/null | tail -40") + " > $D/karabiner.log || true",
        "pgrep -fl VirtualHIDDevice-Daemon > $D/driver-daemons.txt || true",
        "wait",
        "sleep 4",
    ]
    return "\n".join(lines) + "\n"


def app_scenario_script(name, spec):
    run = f"{GUEST}/run/{name}"
    script = f"{GUEST}/scenarios/{spec['script']}"
    input_log = '"$HOME/Library/Logs/Azad/input.log"'
    lines = [
        "set -u",
        f"D={run}; B={GUEST}/bin",
        "rm -rf $D && mkdir -p $D",
        f"wc -l < {input_log} > $D/applogstart",
        sudo(f"wc -l < {HELPER_LOG}") + " > $D/logstart",
        "echo '' | pbcopy",
        f"$B/layout {spec.get('layout', 'com.apple.keylayout.US')} > $D/layout.json",
        "open -n $B/Sink.app --args $D/sink.jsonl 20",
        "sleep 2",
        sudo(f"$B/fixture-keyboard --script {script} --settle-ms 3000") + " > $D/source.jsonl 2>&1",
        "sleep 3",
        f"tail -n +$(( $(cat $D/applogstart) + 1 )) {input_log} > $D/app.jsonl",
        sudo(f"tail -n +$(( $(cat $D/logstart) + 1 )) {HELPER_LOG}") + " > $D/helper.jsonl",
        "pbpaste > $D/pasteboard.txt",
        "defaults read ai.azad AzadAlwaysListeningEnabled > $D/listen.txt 2>/dev/null || echo 0 > $D/listen.txt",
        "$B/layout com.apple.keylayout.US > /dev/null",
        "sleep 14",
    ]
    return "\n".join(lines) + "\n"


def failure_scenario_script(name, spec):
    run = f"{GUEST}/run/{name}"
    input_log = '"$HOME/Library/Logs/Azad/input.log"'
    kb = f"{GUEST}/bin/fixture-keyboard"
    lines = [
        "set -u",
        f"D={run}; B={GUEST}/bin",
        "rm -rf $D && mkdir -p $D",
        f"wc -l < {input_log} > $D/applogstart",
        sudo(f"wc -l < {HELPER_LOG}") + " > $D/logstart",
        sudo("launchctl print system/ai.azad.capture") + " | grep -E '^\\s+pid' > $D/pid-before.txt",
        "open -n $B/Sink.app --args $D/sink.jsonl 26",
        "sleep 2",
    ]
    hold = f"{GUEST}/scenarios/fail-hold-then-type.json"
    if spec.get("two_keyboards"):
        lines += [
            sudo(f"{kb} --script {GUEST}/scenarios/multi-a.json --settle-ms 3000") + " > $D/source.jsonl 2>&1 &",
            sudo(f"{kb} --script {GUEST}/scenarios/multi-b.json --settle-ms 3000 --product 6033") + " > $D/source-b.jsonl 2>&1",
            "wait",
        ]
    elif spec.get("other_user_client"):
        lines += [
            "id azadother >/dev/null 2>&1 || " + sudo("sysadminctl -addUser azadother -password azadother") + " >/dev/null 2>&1",
            sudo("cp $B/fixture-app /Users/Shared/fixture-app && chmod 755 /Users/Shared/fixture-app"),
            "pkill -f /Applications/Azad.app/Contents/MacOS/azad; sleep 2",
            sudo("-u azadother /Users/Shared/fixture-app --seconds 12 --context '{\"listen_modifiers\":4}'") + " > $D/other-app.jsonl 2>&1 &",
            "sleep 2",
            sudo(f"{kb} --script {hold} --settle-ms 3000") + " > $D/source.jsonl 2>&1",
            "wait",
        ]
    elif spec.get("remove_after_report") is not None:
        lines += [
            sudo(f"{kb} --script {hold} --settle-ms 3000 --exit-after-report {spec['remove_after_report']}") + " > $D/source.jsonl 2>&1",
            "sleep 2",
            sudo(f"{kb} --script {GUEST}/scenarios/type-a.json --settle-ms 3000 --product 6033") + " > $D/source-b.jsonl 2>&1",
        ]
    else:
        lines += [
            sudo(f"{kb} --script {hold} --settle-ms 3000") + " > $D/source.jsonl 2>&1 &",
            "for i in $(seq 200); do grep -q '\"index\":1,' $D/source.jsonl && break; sleep 0.05; done",
            "sleep 0.5",
            spec["inject"],
        ]
        if spec.get("cleanup"):
            lines += [f"sleep {spec['cleanup_after_s']}", spec["cleanup"]]
        lines += ["wait"]
        if spec.get("then_type_from_new_keyboard"):
            lines += [
                "sleep 5",
                sudo(f"{kb} --script {GUEST}/scenarios/type-a.json --settle-ms 3000 --product 6033") + " > $D/source-b.jsonl 2>&1",
            ]
    lines += [
        "sleep 4",
        f"tail -n +$(( $(cat $D/applogstart) + 1 )) {input_log} > $D/app.jsonl",
        sudo(f"tail -n +$(( $(cat $D/logstart) + 1 )) {HELPER_LOG}") + " > $D/helper.jsonl",
        sudo("launchctl print system/ai.azad.capture") + " | grep -E '^\\s+pid' > $D/pid-after.txt",
    ]
    if spec.get("relaunch_app") or spec.get("other_user_client"):
        lines += ["open --stdout /tmp/azad.out --stderr /tmp/azad.err /Applications/Azad.app", "sleep 6"]
    lines += ["sleep 14"]
    return "\n".join(lines) + "\n"


def evaluate_failure(name, spec, dest):
    app = [r for r in jsonl(os.path.join(dest, "app.jsonl")) if r.get("event") != "keyboard_capture"]
    helper = jsonl(os.path.join(dest, "helper.jsonl"))
    sink = jsonl(os.path.join(dest, "sink.jsonl"))
    downs = [r for r in sink if r.get("kind") == "down"]
    a_downs = [r for r in downs if r["keycode"] == KEY_A]
    events = [r["event"] for r in app]
    helper_events = [r.get("event") for r in helper]
    source = [r for r in jsonl(os.path.join(dest, "source.jsonl")) if r.get("event") == "report"]
    checks = {
        "source_played": bool(source),
        "typing_after_failure_reaches_foreground_once": len(a_downs) == 1,
        "no_stuck_modifier": all(not r["flags"] & (OPTION | SHIFT) for r in a_downs),
    }
    if name == "app_quit_mid_hold":
        checks["helper_saw_client_leave"] = "client_disconnected" in helper_events
        checks["hold_had_started"] = events[:1] == ["hotkey_pressed"]
    elif name == "helper_hang_mid_hold":
        before = open(os.path.join(dest, "pid-before.txt")).read().split()
        after = open(os.path.join(dest, "pid-after.txt")).read().split()
        checks["hang_injected"] = "fault_hang" in helper_events
        checks["watchdog_exited"] = any("watchdog_exit" in json.dumps(r) for r in helper) or \
            "watchdog_exit" in open(os.path.join(dest, "helper.jsonl")).read()
        checks["launchd_restarted_helper"] = bool(before) and bool(after) and before != after
        checks["app_ended_hold"] = events[:2] == ["hotkey_pressed", "hotkey_released"]
    elif name == "forwarding_fails_mid_hold":
        checks["app_ended_hold"] = events[:2] == ["hotkey_pressed", "hotkey_released"]
        checks["forward_failure_injected"] = any(
            r.get("event") == "counters" and r.get("forward_errors", 0) > 0 for r in helper)
    elif name == "driver_daemon_killed_mid_hold":
        checks["app_ended_hold"] = events[:2] == ["hotkey_pressed", "hotkey_released"]
        statuses = [r["status"]["capture"] for r in helper if r.get("event") == "status"]
        checks["capture_dropped_then_recovered"] = (
            "driver_unavailable" in statuses and statuses[-1] == "capturing")
    elif name == "keyboard_removed_mid_hold":
        checks["app_ended_hold"] = events[:2] == ["hotkey_pressed", "hotkey_released"]
        checks["space_never_delivered"] = not any(r["keycode"] == 49 for r in downs)
    elif name == "two_keyboards":
        checks["chord_claimed_across_keyboards"] = events[:2] == ["hotkey_pressed", "hotkey_released"]
        checks["space_never_delivered"] = not any(r["keycode"] == 49 for r in downs)
    elif name == "client_not_console_user":
        other = jsonl(os.path.join(dest, "other-app.jsonl"))
        actions = [r for r in other if r.get("event") == "received"
                   and r["message"].get("type") == "action"]
        checks["no_actions_for_other_user"] = not actions
        connected = next((i for i, r in enumerate(helper) if r.get("event") == "client_connected"), None)
        after = [r["status"]["capture"] for r in helper[connected or 0:]
                 if r.get("event") == "status"] if connected is not None else []
        checks["client_connected"] = connected is not None
        checks["status_idle_not_capturing"] = bool(after) and "capturing" not in after
        checks["nothing_seized"] = not any(
            d["state"] == "seized" for r in helper if r.get("event") == "status"
            for d in r["status"]["devices"])
        checks["space_reached_foreground"] = any(r["keycode"] == 49 for r in downs)
    return {"scenario": name, "about": spec["about"], "passed": all(bool(v) for v in checks.values()),
            "checks": {k: bool(v) for k, v in checks.items()},
            "observed": {"app_events": events, "helper_events": helper_events,
                         "foreground_keys": [r["keycode"] for r in downs]}}


def evaluate_app(name, spec, dest):
    app = [r for r in jsonl(os.path.join(dest, "app.jsonl")) if r.get("event") != "keyboard_capture"]
    sink = jsonl(os.path.join(dest, "sink.jsonl"))
    helper = jsonl(os.path.join(dest, "helper.jsonl"))
    source = [r for r in jsonl(os.path.join(dest, "source.jsonl")) if r.get("event") == "report"]
    pasted = open(os.path.join(dest, "pasteboard.txt")).read().strip()
    listen = open(os.path.join(dest, "listen.txt")).read().strip() == "1"
    events = [r["event"] for r in app]
    downs = [r for r in sink if r.get("kind") == "down"]
    text = next((r["value"] for r in sink if r.get("kind") == "text"), "")
    keys = [r["keycode"] for r in downs]
    rejected = [r for r in helper if r.get("event") == "client_rejected"]
    script = json.load(open(os.path.join(FIXTURES, "scenarios", spec["script"])))
    checks = {"all_reports_posted": len(source) == len(script), "app_client_accepted": not rejected}
    if name == "app_hold_release":
        checks["events"] = events == ["hotkey_pressed", "hotkey_released"]
        checks["released_non_raw"] = app[-1:] and app[-1].get("raw_requested") is False
        checks["space_not_delivered"] = 49 not in keys
    elif name.startswith("app_double_tap"):
        checks["two_presses"] = events.count("hotkey_pressed") == 2
        checks["listen_state"] = listen == spec["listen_after"]
        checks["space_not_delivered"] = 49 not in keys
    elif name.startswith("app_history_search"):
        layout = json.load(open(os.path.join(dest, "layout.json")))
        checks["layout_selected"] = layout.get("current") == spec["layout"]
        checks["history_opened"] = any(
            r["event"] == "arrow_navigate" and r.get("direction") == -1 for r in app)
        checks["search_typed"] = any(
            r["event"] == "history_search_edit" and r.get("kind") == "append"
            and r.get("chars_appended") == 1 for r in app)
        checks["pasted_match"] = pasted == spec["pasted"]
        checks["foreground_received_paste"] = text.strip() == spec["pasted"]
        checks["paste_was_synthetic_cmd_v"] = any(
            r["keycode"] == 9 and r["flags"] & 0x100000 for r in downs)
        checks["search_key_not_delivered"] = 0 not in keys and 12 not in keys
        checks["claimed_keys_not_delivered"] = not ({49, 126, 36} & set(keys))
    elif name == "app_overlay_keys":
        checks["events"] = [e for e in events if e != "hotkey_released"] == [
            "hotkey_pressed", "arrow_navigate", "finalize_hotkey_pressed", "overlay_cancel"]
        checks["shift_return_delivered_once"] = [
            r for r in downs if r["keycode"] == 36] and all(
            r["flags"] & SHIFT for r in downs if r["keycode"] == 36) and keys.count(36) == 1
        checks["claimed_keys_not_delivered"] = not ({49, 125, 76, 53} & set(keys))
    elif name == "app_caps_lock_and_repeat":
        alpha_shift = 0x10000
        a_downs = [r for r in downs if r["keycode"] == KEY_A]
        a_ups = [r for r in sink if r.get("kind") == "up" and r.get("keycode") == KEY_A]
        checks["caps_lock_applied"] = bool(a_downs) and a_downs[0]["flags"] & alpha_shift != 0
        checks["caps_lock_released"] = len(a_downs) > 1 and a_downs[1]["flags"] & alpha_shift == 0
        checks["held_key_repeats"] = sum(1 for r in a_downs[1:] if r["repeat"]) >= 3
        checks["one_release_per_press"] = len(a_ups) == 2
        checks["text"] = text.startswith("Aa") and set(text[1:]) == {"a"}
        checks["no_app_events"] = events == []
    elif name == "app_ordinary_typing":
        # Every action the app receives is logged, so no events means no typing reached it.
        checks["no_app_events"] = events == []
        # Typing is neither logged by the helper nor sent to the app outside owned search input.
        helper_text = open(os.path.join(dest, "helper.jsonl")).read()
        checks["helper_log_has_no_keys"] = '"usage"' not in helper_text and '"keys"' not in helper_text
        checks["each_key_once"] = sorted(keys) == sorted([0, 36, 53, 126, 0])
    return {"scenario": name, "about": spec["about"], "passed": all(bool(v) for v in checks.values()),
            "checks": {k: bool(v) for k, v in checks.items()},
            "observed": {"events": events, "foreground_keys": keys, "foreground_text": text,
                         "pasteboard": pasted, "always_listening": listen}}


def collect(vm, name, out):
    data = vm.ssh_bytes(f"tar czf - -C {GUEST}/run/{name} .")
    dest = os.path.join(out, name)
    os.makedirs(dest, exist_ok=True)
    with tarfile.open(fileobj=io.BytesIO(data)) as tar:
        tar.extractall(dest, filter="data")
    return dest


def jsonl(path):
    if not os.path.exists(path):
        return []
    rows = []
    for line in open(path):
        line = line.strip()
        if line.startswith("{"):
            try:
                rows.append(json.loads(line))
            except json.JSONDecodeError:
                pass
    return rows


def evaluate(name, spec, dest):
    sink = jsonl(os.path.join(dest, "sink.jsonl"))
    tap = jsonl(os.path.join(dest, "tap.jsonl"))
    app = jsonl(os.path.join(dest, "app.jsonl"))
    source = jsonl(os.path.join(dest, "source.jsonl"))
    helper = jsonl(os.path.join(dest, "helper.jsonl"))
    owner = jsonl(os.path.join(dest, "owner.jsonl"))

    actions = [r["message"]["action"] for r in app
               if r.get("event") == "received" and r["message"].get("type") == "action"]
    stamps = [r["message"]["timestamp_ns"] for r in app
              if r.get("event") == "received" and r["message"].get("type") == "action"]
    statuses = [r["status"] for r in helper if r.get("event") == "status"]
    seized = [d for s in statuses for d in s["devices"] if d["state"] == "seized"]
    owned = [d for s in statuses for d in s["devices"] if d["state"] == "owned_by_other"]
    rejected = [r for r in helper if r.get("event") == "client_rejected"]
    reports = [r for r in source if r.get("event") == "report"]
    secure_steps = [r["index"] for r in reports if r.get("secure")]

    def downs(rows, key):
        return [r for r in rows if r.get("kind") == key]

    sink_down = downs(sink, "down")
    sink_up = downs(sink, "up")
    tap_keys = [r for r in tap if r.get("event") == "tap_key"]
    foreground = [(r["kind"], r["keycode"], r["flags"]) for r in sink_down + sink_up]
    tapped = [(r["type"], r["keycode"]) for r in tap_keys]

    def seen(kind, keycode):
        return (sum(1 for r in sink if r.get("kind") == kind and r.get("keycode") == keycode)
                + sum(1 for r in tap_keys if r["type"] == kind and r["keycode"] == keycode))

    claimed_leaks = sorted({CLAIMED_KEYCODES[k] for _, k, _ in foreground if k in CLAIMED_KEYCODES}
                           | {CLAIMED_KEYCODES[k] for _, k in tapped if k in CLAIMED_KEYCODES})
    bare_return = [r for r in sink_down if r["keycode"] == RETURN and not r["flags"] & SHIFT]
    shift_return_ok = all(r["flags"] & SHIFT for r in sink_down if r["keycode"] == RETURN)
    stuck_option = [r for r in sink_down if r["keycode"] == KEY_A and r["flags"] & OPTION]

    # Every scenario must have actually played its script; otherwise "nothing happened" checks
    # pass vacuously.
    script_steps = json.load(open(os.path.join(FIXTURES, "scenarios", spec["script"])))
    checks = {"all_reports_posted": len(reports) == len(script_steps)}
    expect = spec["expect"]
    if expect == "capture":
        checks["actions_exact"] = actions == SHORTCUT_ACTIONS
        checks["timestamps_monotonic"] = stamps == sorted(stamps) and len(stamps) == len(actions)
        checks["no_claimed_key_reaches_foreground_or_tap"] = not claimed_leaks and not bare_return
        checks["shift_return_once"] = seen("down", RETURN) == 1 and seen("up", RETURN) == 1
        checks["shift_return_keeps_shift"] = shift_return_ok
        checks["ordinary_a_once"] = seen("down", KEY_A) == 1 and seen("up", KEY_A) == 1
        checks["no_stuck_modifier_on_a"] = not stuck_option
        checks["seized_device_reported"] = bool(seized)
        if spec.get("tap"):
            checks["tap_consumed_forwarded_keys"] = not sink_down and not sink_up
        if "secure" in spec["script"]:
            checks["secure_input_observed"] = bool(secure_steps)
        if spec.get("karabiner"):
            karabiner = open(os.path.join(dest, "karabiner.log")).read()
            daemons = open(os.path.join(dest, "driver-daemons.txt")).read().strip().splitlines()
            checks["karabiner_output_seized_by_azad"] = any(
                d["kind"] == "remapper_output" and d["vendor_id"] == 0x05ac for d in seized)
            checks["physical_keyboard_left_to_karabiner"] = any(
                d["product"] == "Virtual USB Keyboard" and d["state"] == "yielded"
                for s in statuses for d in s["devices"])
            checks["karabiner_grabbed_physical_keyboard"] = "Virtual USB Keyboard" in karabiner and "(grabbed)" in karabiner
            checks["karabiner_never_grabbed_driver_keyboards"] = not any(
                "VirtualHIDKeyboard" in line and "(grabbed)" in line for line in karabiner.splitlines())
            checks["single_driver_daemon"] = len(daemons) == 1
        if spec.get("owner"):
            checks["remapper_output_seized"] = any(d["kind"] == "remapper_output" for d in seized)
            checks["source_owned_by_remapper"] = any(d["product_id"] == 0x1790 for d in owned)
    elif expect == "hang_fail_open":
        expired = [r for r in helper if r.get("event") == "context_lease_expired"]
        checks["lease_expired_logged"] = bool(expired)
        checks["typing_reaches_foreground_after_lease"] = (
            sum(1 for r in sink_down if r["keycode"] == KEY_A) == 1
            and sum(1 for r in sink_down if r["keycode"] == RETURN) == 1)
        checks["listen_chord_still_claimed"] = (
            actions == [{"kind": "hotkey_pressed"}, {"kind": "hotkey_released", "raw_requested": False}]
            and not any(r["keycode"] == 49 for r in sink_down))
        checks["no_search_keys_claimed_after_lease"] = not any(
            a.get("kind") in ("search_key", "finalize") for a in actions)
    elif expect == "fail_open":
        before = open(os.path.join(dest, "pid-before.txt")).read().split()
        after = open(os.path.join(dest, "pid-after.txt")).read().split()
        checks["hold_started_before_crash"] = actions[:1] == [{"kind": "hotkey_pressed"}]
        checks["ordinary_a_reaches_foreground_once"] = (
            sum(1 for r in sink_down if r["keycode"] == KEY_A) == 1
            and sum(1 for r in sink_up if r["keycode"] == KEY_A) == 1)
        checks["no_stuck_modifier_on_a"] = not stuck_option
        checks["launchd_restarted_helper"] = bool(before) and bool(after) and before != after
    elif expect == "owned_by_other":
        checks["no_actions_delivered"] = not actions
        checks["device_reported_owned_by_other"] = any(d["product_id"] == 0x1790 for d in owned)
        checks["source_never_seized_by_helper"] = not any(d["product_id"] == 0x1790 for d in seized)
        # While a client requests capture, every status listing the owned source must name it.
        during = [s for s in statuses if s["capture"] != "idle"
                  and any(d["product_id"] == 0x1790 for d in s["devices"])]
        checks["status_names_unavailable_source"] = bool(during) and all(
            s.get("unavailable_devices") for s in during)
    elif expect == "rejected":
        checks["client_rejected"] = bool(rejected)
        checks["no_actions_delivered"] = not actions
        checks["nothing_seized"] = not seized
        checks["claimed_keys_reach_foreground"] = bool(claimed_leaks)
    elif expect == "permission_denied":
        checks["no_actions_delivered"] = not actions
        checks["nothing_seized"] = not seized
        checks["status_permission_denied"] = bool(statuses) and all(
            s["capture"] == "permission_denied" for s in statuses) or not statuses
        checks["claimed_keys_reach_foreground"] = bool(claimed_leaks)

    return {
        "scenario": name,
        "about": spec["about"],
        "passed": all(checks.values()),
        "checks": checks,
        "observed": {
            "actions": actions,
            "claimed_leaks": claimed_leaks,
            "foreground_key_events": foreground,
            "tap_key_events": tapped,
            "secure_steps": secure_steps,
            "source_reports": len(reports),
            "helper_rejections": [r.get("reason") for r in rejected],
            "owner": owner,
            "final_status": statuses[-1] if statuses else None,
        },
    }


TCC_QUERY = ('sqlite3 "/Library/Application Support/com.apple.TCC/TCC.db" '
             '"select client,client_type,auth_value from access where service=\\"kTCCServiceListenEvent\\""')


def permission_step(vm, step):
    if step == "revoke":
        out = vm.ssh(f"""{sudo('tccutil reset ListenEvent ai.azad.capture')}
{sudo('launchctl kickstart -k system/ai.azad.capture')}
sleep 5
{sudo(TCC_QUERY)}
{sudo(f'tail -n 30 {HELPER_LOG}')} | grep '"event":"status"' | tail -1
""").stdout
        return {"step": step, "ok": '"permission":"denied"' in out and "ai.azad.capture|0|2" not in out,
                "guest": out.splitlines()}
    if step == "request":
        # Azad's onboarding runs this from the user's session: a root requester cannot be
        # prompted, but this registers the Input Monitoring entry for the switch.
        # The request blocks on the consent prompt; answering it registers the entry.
        out = vm.ssh(f"""open -a "{HELPER_APP}" --args --request-access
sleep 3
pgrep -fl -- "--request-access" | head -1
{sudo(TCC_QUERY)}
""").stdout
        # Either the prompt is pending or a prior answer already listed the (off) entry.
        return {"step": step, "ok": "--request-access" in out or "ai.azad.capture|0|0" in out,
                "guest": out.splitlines()}
    deadline = time.time() + 300
    while time.time() < deadline:
        tcc = vm.ssh(sudo(TCC_QUERY), check=False).stdout
        log = vm.ssh(sudo(f"tail -n 60 {HELPER_LOG}"), check=False).stdout
        events = [json.loads(line) for line in log.splitlines() if line.startswith("{")]
        restarted = [i for i, e in enumerate(events) if e.get("event") == "permission_granted_restart"]
        after = events[restarted[-1] + 1:] if restarted else events
        granted = any(e.get("event") == "status" and e["status"]["permission"] == "granted"
                      for e in after)
        if "ai.azad.capture|0|2" in tcc and granted:
            return {"step": step, "ok": True, "restarted_into_grant": bool(restarted),
                    "guest": tcc.splitlines() + [json.dumps(e) for e in after[:4]]}
        time.sleep(3)
    out = tcc + log
    return {"step": step, "ok": False, "guest": out.splitlines()}


def digest(path):
    return hashlib.sha256(open(path, "rb").read()).hexdigest()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--vm", required=True)
    parser.add_argument("--out", required=True)
    parser.add_argument("--stage", action="store_true", help="build, sign and install")
    parser.add_argument("--install", action="store_true",
                        help="install the existing stage directory without rebuilding")
    parser.add_argument("--installed-helper", action="store_true",
                        help="use the helper Azad.app registered; quit Azad during helper-only "
                             "scenarios so the fixture client is the helper's client")
    parser.add_argument("--stage-dir", default=os.path.join(ROOT, "target/azt-stage"))
    parser.add_argument("--only", nargs="*")
    parser.add_argument("--evaluate-only", action="store_true",
                        help="re-evaluate saved scenario outputs under --out without running")
    parser.add_argument("--permission", choices=["revoke", "request", "wait-granted"],
                        help="Input Monitoring lifecycle step instead of scenarios. The grant "
                             "itself is the user's switch in the guest's System Settings.")
    args = parser.parse_args()
    vm = Vm(args.vm)
    if args.permission:
        os.makedirs(args.out, exist_ok=True)
        record = permission_step(vm, args.permission)
        with open(os.path.join(args.out, f"permission-{args.permission}.json"), "w") as f:
            json.dump(record, f, indent=1)
        print(json.dumps(record))
        sys.exit(0 if record["ok"] else 1)
    if args.stage:
        build_stage(args.stage_dir)
    if args.stage or args.install:
        install(vm, args.stage_dir)
    os.makedirs(args.out, exist_ok=True)
    results = []
    for name, spec in FAILURE_SCENARIOS.items():
        if not args.only or name not in args.only:
            continue
        print(f"== {name}", flush=True)
        if not args.evaluate_only:
            vm.ssh(failure_scenario_script(name, spec), timeout=300)
            collect(vm, name, args.out)
        result = evaluate_failure(name, spec, os.path.join(args.out, name))
        results.append(result)
        print(json.dumps({"scenario": name, "passed": result["passed"], "checks": result["checks"]}),
              flush=True)
    for name, spec in APP_SCENARIOS.items():
        if not args.only or name not in args.only:
            continue
        print(f"== {name}", flush=True)
        if not args.evaluate_only:
            vm.ssh(app_scenario_script(name, spec), timeout=240)
            collect(vm, name, args.out)
        result = evaluate_app(name, spec, os.path.join(args.out, name))
        results.append(result)
        print(json.dumps({"scenario": name, "passed": result["passed"], "checks": result["checks"]}),
              flush=True)
    for name, spec in SCENARIOS.items():
        if args.only and name not in args.only:
            continue
        print(f"== {name}", flush=True)
        if args.evaluate_only:
            dest = os.path.join(args.out, name)
        else:
            script = scenario_script(name, spec)
            if args.installed_helper:
                script = ("pkill -f /Applications/Azad.app/Contents/MacOS/azad; sleep 2\n" + script
                          + "open --stdout /tmp/azad.out --stderr /tmp/azad.err /Applications/Azad.app\n"
                          + "sleep 6\n")
            vm.ssh(script, timeout=300)
            dest = collect(vm, name, args.out)
        result = evaluate(name, spec, dest)
        results.append(result)
        print(json.dumps({"scenario": name, "passed": result["passed"], "checks": result["checks"]}),
              flush=True)
        time.sleep(2)
    guest = vm.ssh(f"""sw_vers -productVersion; csrutil status; spctl --status
systemextensionsctl list | grep -i pqrs
codesign -dvvv "{HELPER_APP}" 2>&1 | grep -E '^(Identifier|TeamIdentifier|CDHash)='
shasum -a 256 "{HELPER_APP}/Contents/MacOS/azad-capture"
""", check=False).stdout
    path = os.path.join(args.out, "results.json")
    previous = json.load(open(path))["results"] if os.path.exists(path) else []
    ran = {r["scenario"] for r in results}
    merged = [r for r in previous if r["scenario"] not in ran] + results
    summary = {"vm": args.vm, "guest": guest.strip().splitlines(), "results": merged}
    with open(path, "w") as f:
        json.dump(summary, f, indent=1)
    failed = [r["scenario"] for r in results if not r["passed"]]
    print(json.dumps({"passed": not failed, "failed": failed}))
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
