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
    context = json.dumps(OVERLAY_CONTEXT)
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

    checks = {}
    expect = spec["expect"]
    if expect == "capture":
        checks["all_reports_posted"] = len(reports) == 28
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
    elif expect == "fail_open":
        before = open(os.path.join(dest, "pid-before.txt")).read().split()
        after = open(os.path.join(dest, "pid-after.txt")).read().split()
        checks["all_reports_posted"] = len(reports) == 28
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
    for name, spec in SCENARIOS.items():
        if args.only and name not in args.only:
            continue
        print(f"== {name}", flush=True)
        if args.evaluate_only:
            dest = os.path.join(args.out, name)
        else:
            vm.ssh(scenario_script(name, spec), timeout=240)
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
