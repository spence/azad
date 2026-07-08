#!/usr/bin/env python3
"""Phase-0 measurement harness for the dual-stream refinement rework.

Two modes:

  baseline  — GPU-free. Reads the captured debug-recording sidecars and reports,
              per recording, the word error rate of (a) the live streaming caption
              and (b) the pasted/emitted text, each vs the whole-turn full pass
              (`full_text`). Answers "is the stitcher actually buying quality over
              the raw stream?" without touching the model or the running app.

  events    — Parse a `asr transcribe-file --events-jsonl` stream on stdin and
              print the final live caption (last Active/ReplaceLine before the
              FinalLine) and the FinalLine. Lets us score a continuous-Nms stream
              (`--streaming-chunk-ms N`) against `full_text` without code changes.

Usage:
  python3 wer_corpus.py baseline [<debug-recordings-dir>]
  asr transcribe-file --streaming-chunk-ms 560 --events-jsonl X.wav | python3 wer_corpus.py events
"""
import glob
import json
import os
import re
import statistics
import sys

DEFAULT_DIR = os.path.expanduser(
    "~/Library/Application Support/Azad/debug-recordings"
)


def norm(s):
    return re.sub(r"[^\w\s]", " ", (s or "").lower()).split()


def wer(ref, hyp):
    r, h = norm(ref), norm(hyp)
    if not r:
        return None
    prev = list(range(len(h) + 1))
    for i in range(1, len(r) + 1):
        cur = [i] + [0] * len(h)
        for j in range(1, len(h) + 1):
            cur[j] = min(prev[j] + 1, cur[j - 1] + 1, prev[j - 1] + (r[i - 1] != h[j - 1]))
        prev = cur
    return prev[len(h)] / len(r)


def live_final(j):
    for e in reversed(j.get("live_display_events", [])):
        t = e.get("text") or e.get("candidate_text")
        if t and t.strip():
            return t
    return ""


def cmd_baseline(d):
    rows = []
    for f in sorted(glob.glob(os.path.join(d, "*.json"))):
        if f.endswith("-bailout.json"):
            continue
        try:
            j = json.load(open(f))
        except Exception:
            continue
        full = j.get("full_text", "").strip()
        if not full:
            continue
        rows.append(
            (
                os.path.basename(f)[:24],
                j.get("num_samples", 0) / 16000,
                len(norm(full)),
                wer(full, live_final(j)),
                wer(full, j.get("emitted_text", "").strip()),
            )
        )
    print(f"{'recording':24} {'dur_s':>6} {'words':>6} {'live/full':>10} {'emitted/full':>13}")
    lv, ev = [], []
    for name, dur, w, l, e in sorted(rows, key=lambda r: -r[1]):
        lv.append(l)
        ev.append(e)
        print(f"{name:24} {dur:6.0f} {w:6} {l*100:9.1f}% {e*100:12.1f}%")
    print("-" * 64)
    print(f"{'MEAN WER':24} {'':6} {'':6} {statistics.mean(lv)*100:9.1f}% {statistics.mean(ev)*100:12.1f}%")
    print(f"{'MEDIAN WER':24} {'':6} {'':6} {statistics.median(lv)*100:9.1f}% {statistics.median(ev)*100:12.1f}%")


def cmd_events():
    last_live, final_line = "", ""
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            ev = json.loads(line)
        except Exception:
            continue
        kind = ev.get("kind") or ev.get("type") or next(iter(ev), "")
        text = ev.get("text", "")
        blob = json.dumps(ev).lower()
        if '"finalline"' in blob or '"final_line"' in blob:
            final_line = text or final_line
        elif '"active"' in blob or '"replaceline"' in blob or '"replace_line"' in blob:
            if text.strip():
                last_live = text
    print(json.dumps({"live_final": last_live, "final_line": final_line}))


if __name__ == "__main__":
    cmd = sys.argv[1] if len(sys.argv) > 1 else "baseline"
    if cmd == "baseline":
        cmd_baseline(sys.argv[2] if len(sys.argv) > 2 else DEFAULT_DIR)
    elif cmd == "events":
        cmd_events()
    else:
        print(__doc__)
        sys.exit(2)
