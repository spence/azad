#!/usr/bin/env python3
"""lag_report.py — summarize live-decoder lag from the app's TOON_LIVE_LAG lines.

The live (streaming) ASR runs synchronously on the live thread, so if its decode is slower than
real time the live caption falls behind the speaker (words show up only at finalize). The engine
emits one `TOON_LIVE_LAG` line per turn at finalize (debug stats on):

  TOON_LIVE_LAG turn_id=N audio_ms=.. stream_decode_ms=.. behind_ms=.. chunks=.. max_chunk_ms=..

  behind_ms = stream_decode_ms - audio_ms  (clamped >=0) = how far behind real time the live
              caption was by turn end. 0 => decode kept up. Large => caption lagged (the
              "delay under load" case).
  rtf       = stream_decode_ms / audio_ms  = real-time factor (<1 keeps up, >1 falls behind).

Usage:
  python3 lag_report.py [stderr.log] [--tail N]
Defaults to ~/Library/Logs/Azad/stderr.log and the last 20 turns.
"""
import argparse
import os
import re
import sys

DEFAULT_LOG = os.path.expanduser("~/Library/Logs/Azad/stderr.log")


def parse(path):
    rows = []
    for line in open(path, errors="replace"):
        if "TOON_LIVE_LAG" not in line:
            continue
        d = dict(re.findall(r"(\w+)=(\d+)", line))
        if "audio_ms" not in d:
            continue
        rows.append({k: int(v) for k, v in d.items()})
    return rows


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("log", nargs="?", default=DEFAULT_LOG)
    ap.add_argument("--tail", type=int, default=20)
    args = ap.parse_args()
    if not os.path.exists(args.log):
        print(f"no log at {args.log}", file=sys.stderr)
        sys.exit(1)

    rows = parse(args.log)[-args.tail :]
    if not rows:
        print("No TOON_LIVE_LAG lines yet (need debug stats on + a finalized turn).")
        return

    print(f"{'turn':>5} {'audio_s':>7} {'decode_s':>8} {'rtf':>5} {'behind_s':>8} {'maxchunk_ms':>11}  {'':>6}")
    for r in rows:
        audio = r["audio_ms"] / 1000
        decode = r["stream_decode_ms"] / 1000
        behind = r["behind_ms"] / 1000
        rtf = decode / audio if audio else 0
        flag = "LAG" if behind >= 1.0 else ("" if behind < 0.3 else "~")
        print(
            f"{r.get('turn_id', 0):>5} {audio:7.1f} {decode:8.1f} {rtf:5.2f} {behind:8.1f} "
            f"{r.get('max_chunk_ms', 0):>11}  {flag:>6}"
        )
    worst = max(rows, key=lambda r: r["behind_ms"])
    print(
        f"\nworst: turn {worst.get('turn_id')} behind {worst['behind_ms']/1000:.1f}s "
        f"(decode {worst['stream_decode_ms']/1000:.1f}s for {worst['audio_ms']/1000:.1f}s audio)"
    )


if __name__ == "__main__":
    main()
