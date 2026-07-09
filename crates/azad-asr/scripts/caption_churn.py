#!/usr/bin/env python3
"""Measure live-caption churn from `asr transcribe-file --events-jsonl` output.

Churn = a token that was already shown (not the growing last word) changing value
between consecutive caption frames. This is exactly the mid-speech "swapping
versions" the user sees. Case/punctuation toggles count — they are visible swaps.

Reads events JSONL on stdin (or a file arg) and prints per-turn + total flip counts
plus the finalized paste text (so a sweep can confirm finalize stays byte-identical).
"""
import json
import sys
from collections import defaultdict


def tokens(s):
  return s.split()


def turn_flips(frames):
  """frames: list of caption strings in emit order. Returns (flips, frames_with_flip)."""
  flips = 0
  changed_frames = 0
  for prev, cur in zip(frames, frames[1:]):
    p, c = tokens(prev), tokens(cur)
    # Only the last token of prev is the growing edge; everything before it was
    # "shown and settled". A change there is churn.
    n = min(len(p) - 1, len(c))
    frame_flips = sum(1 for i in range(max(n, 0)) if p[i] != c[i])
    if frame_flips:
      changed_frames += 1
    flips += frame_flips
  return flips, changed_frames


def main():
  src = open(sys.argv[1]) if len(sys.argv) > 1 else sys.stdin
  active = defaultdict(list)
  finals = {}
  for line in src:
    line = line.strip()
    if not line:
      continue
    try:
      ev = json.loads(line)
    except json.JSONDecodeError:
      continue
    if ev.get("event") == "active":
      active[ev["turn_id"]].append(ev["merged"])
    elif ev.get("event") == "final_line":
      finals[ev["turn_id"]] = ev.get("text", "")

  total_flips = 0
  total_frames = 0
  print(f"{'turn':>5} {'frames':>7} {'flips':>6} {'chg_fr':>7}  final_preview")
  for tid in sorted(active):
    frames = active[tid]
    flips, chg = turn_flips(frames)
    total_flips += flips
    total_frames += len(frames)
    preview = (finals.get(tid, "") or "")[:48].replace("\n", " ")
    print(f"{tid:>5} {len(frames):>7} {flips:>6} {chg:>7}  {preview}")
  print(f"\nTOTAL  frames={total_frames}  flips={total_flips}  turns={len(active)}")
  # Emit a machine-readable summary line for sweep aggregation.
  print(f"SUMMARY flips={total_flips} frames={total_frames} turns={len(active)}")


if __name__ == "__main__":
  main()
