#!/usr/bin/env python3
"""restart_report.py — is the engine ready to listen again right after a turn?

Answers the "I finished dictating, it pasted, and then it went dead for a few seconds" question
from the app's own debug log (debug stats on). Three failure modes, one report:

  PHANTOM TURN — a turn that opens on the audio the previous turn already claimed rather than on
      anything new. Signature: `TOON_TURN_START restart_gap_ms=<small>` following a turn that
      produced text, and the new turn produces none. These occupy 1.5-3s, swallow the start of
      whatever you say next, and end as empty VAD turns.
  DISCARDED TURN — `TOON_TURN_TIMEOUT`, the empty-turn guard firing. `is_speech=true` on that line
      means it discarded a turn while you were still talking, taking those words with it.
  HELD START — `TOON_VAD_REARM action=hold`, the post-turn restart block refusing to open a turn.
      Bounded by the pre-roll window, so it costs start latency and not words; a long run of them
      still means the engine felt unresponsive.

Usage:
  python3 restart_report.py [stderr.log] [--since-restart] [--tail N]

Defaults to ~/Library/Logs/Azad/stderr.log and the whole file. `--since-restart` limits the report
to the current app run, which is what you want right after `just restart`.
"""
import argparse
import os
import sys

DEFAULT_LOG = os.path.expanduser("~/Library/Logs/Azad/stderr.log")
# A restart this soon after the previous turn ended is the engine re-triggering on its own tail,
# not a human drawing breath and starting a new thought.
PHANTOM_GAP_MS = 1200


def kv(line):
    return dict(p.split("=", 1) for p in line.split()[1:] if "=" in p)


def parse(path, since_restart):
    lines = open(path, errors="replace").read().splitlines()
    if since_restart:
        marks = [i for i, l in enumerate(lines) if "asr devices: controller startup" in l]
        if marks:
            lines = lines[marks[-1] :]

    turns, cur, timeouts, holds = [], None, [], 0
    for l in lines:
        tag = l.split(" ", 1)[0]
        if tag == "TOON_TURN_START":
            d = kv(l)
            cur = {
                "id": d.get("turn_id"),
                "reason": d.get("reason"),
                "gap": None if d.get("restart_gap_ms", "none") == "none" else int(d["restart_gap_ms"]),
                "text": False,
            }
            turns.append(cur)
        elif tag == "TOON_TURN_TIMEOUT":
            timeouts.append(kv(l))
        elif tag == "TOON_VAD_REARM" and kv(l).get("action") == "hold":
            holds += 1
        elif cur is not None and tag == "TOON_EOU_TEXT":
            cur["text"] = True
    return turns, timeouts, holds


def pct(n, d):
    return f"{100 * n / d:.0f}%" if d else "n/a"


def main():
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    ap.add_argument("log", nargs="?", default=DEFAULT_LOG)
    ap.add_argument("--since-restart", action="store_true", help="only the current app run")
    ap.add_argument("--tail", type=int, default=0, help="show the last N turn starts")
    args = ap.parse_args()
    if not os.path.exists(args.log):
        print(f"no log at {args.log}", file=sys.stderr)
        sys.exit(1)

    turns, timeouts, holds = parse(args.log, args.since_restart)
    if not turns:
        print("No turns logged yet (need debug stats on).")
        return

    # `restart_gap_ms` landed with the start-gate fix. Older turns cannot be judged, and silently
    # scoring them as clean would report a corpus full of phantoms as healthy.
    measurable = [t for t in turns if t["gap"] is not None]
    if len(measurable) < len(turns) - 1:
        print(
            f"note: {len(turns) - len(measurable)} of {len(turns)} turns predate `restart_gap_ms` "
            f"and are excluded; only {len(measurable)} are measurable.\n"
        )
    if not measurable:
        print("Nothing measurable in this log — it predates the `restart_gap_ms` field entirely.")
        return

    # A phantom is judged against the turn before it: only a restart hard on the heels of a turn
    # that actually captured speech is the engine re-triggering on its own tail.
    phantoms = [
        b
        for a, b in zip(turns, turns[1:])
        if a["text"] and not b["text"] and b["gap"] is not None and b["gap"] <= PHANTOM_GAP_MS
    ]
    spoken = [t for t in measurable if t["text"]]
    gaps = sorted(t["gap"] for t in measurable)

    print(f"turns: {len(measurable)}   with speech: {len(spoken)}   empty: {len(measurable) - len(spoken)}")
    print(
        f"phantom restarts (empty turn within {PHANTOM_GAP_MS}ms of a spoken turn): "
        f"{len(phantoms)}  ({pct(len(phantoms), len(spoken))} of spoken turns)"
    )
    print(f"turns discarded by the empty-turn timeout: {len(timeouts)}")
    mid_speech = [t for t in timeouts if t.get("is_speech") == "true"]
    if mid_speech:
        print(f"  ...of which fired while VAD still reported speech: {len(mid_speech)}  <-- lost words")
    print(f"chunks the restart block held back: {holds}  ({holds * 160}ms total)")
    if gaps:
        p = lambda q: gaps[min(len(gaps) - 1, int(len(gaps) * q))]
        print(f"restart gap after a turn (ms): p10={p(.1)} p50={p(.5)} p90={p(.9)}")

    healthy = not phantoms and not mid_speech
    print("\n" + ("OK — no phantom restarts, no turns discarded mid-speech." if healthy else
                  "REGRESSED — see the counts above; `grep TOON_TURN_START` for the raw gaps."))

    if args.tail:
        print()
        for t in turns[-args.tail :]:
            gap = "    first  " if t["gap"] is None else f"{t['gap']:>7}ms"
            flag = "  <-- phantom" if t in phantoms else ""
            print(f"  turn {t['id']:>5}  gap {gap}  {t['reason']:<14} "
                  f"{'speech' if t['text'] else 'EMPTY '}{flag}")


if __name__ == "__main__":
    main()
