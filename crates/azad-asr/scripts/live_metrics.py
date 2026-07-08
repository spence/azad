#!/usr/bin/env python3
"""live_metrics.py — on-device measurement for the dual-stream refinement rework.

Scores the three experience goals from the app's REAL daily-use artifacts (not
harness runs). The per-turn debug-recording sidecars are the primary evidence;
the app's input.log / metrics.log / stderr.log fill in latency and bailouts.

  G1  live-caption churn   — rollback tokens between successive displayed
                             captions (`live_display_events`). Gate: rollback
                             max <= 2 tokens, ZERO >4-token swaps.
  G2  finalize latency     — engine_speech_finalizing -> engine_final_text wall
                             time per turn (input.log); p50/p95 + bailout count.
                             Dual cross-check: sidecar `finalize_elapsed_ms`.
  G3  subtle in-place edits — refined-source corrections that sharpen the caption
                             in place, plus the draft->refined-final divergence.
                             Gate: >=1 refined edit on long turns, all <= 2.

Both sidecar generations parse: legacy captures lack `pipeline`/`draft_text`
(inferred / treated empty); dual captures carry them. Stdlib only.

Subcommands:
  report <sidecar-dir> [--input-log F] [--metrics-log F] [--stderr-log F]
                       [--since-ms T] [--min-long-s S] [--pipeline P]
                       [--md | --json]
  bank   <src-dir> <dest-dir>   rotation-proof copy of sidecars (+ wavs)
  events-churn                  stdin = `asr transcribe-file --events-jsonl`;
                                headless G1 cross-check (committed / merged)

Examples:
  python3 live_metrics.py report \\
      "$HOME/Library/Application Support/Azad/debug-recordings" \\
      --input-log ~/Library/Logs/Azad/input.log --since-ms 1783500000000 --md
  ./target/release/asr transcribe-file --refinement-mode dual_stream \\
      --events-jsonl X.wav | python3 live_metrics.py events-churn
"""
import argparse
import glob
import json
import os
import re
import shutil
import statistics
import sys

SR = 16000
DEFAULT_DIR = os.path.expanduser("~/Library/Application Support/Azad/debug-recordings")


def toks(s):
    return re.sub(r"[^\w\s]", " ", (s or "").lower()).split()


def lcs(a, b):
    if not a or not b:
        return 0
    prev = [0] * (len(b) + 1)
    for x in a:
        cur = [0] * (len(b) + 1)
        for j, y in enumerate(b):
            cur[j + 1] = prev[j] + 1 if x == y else max(prev[j + 1], cur[j])
        prev = cur
    return prev[len(b)]


def rollback(prev, cur):
    """Tokens of `prev` not preserved (in order) by `cur` — 0 for a pure append."""
    return len(prev) - lcs(prev, cur)


def is_append(prev, cur):
    return cur[: len(prev)] == prev


def pct(vals, p):
    if not vals:
        return None
    s = sorted(vals)
    if len(s) == 1:
        return s[0]
    k = (len(s) - 1) * (p / 100.0)
    lo, hi = int(k), min(int(k) + 1, len(s) - 1)
    return s[lo] + (s[hi] - s[lo]) * (k - lo)


# ---- sidecars ---------------------------------------------------------------

def pipeline_of(j):
    p = j.get("pipeline")
    if p:
        return p
    # Legacy captures predate the `pipeline` key; infer from emitted_kind.
    return "dual_stream" if j.get("emitted_kind") == "dual_final" else "legacy_stitch"


def load_sidecars(d, since_ms=None, pipeline=None):
    rows = []
    for f in sorted(glob.glob(os.path.join(d, "*.json"))):
        try:
            j = json.load(open(f))
        except Exception:
            continue
        if since_ms is not None and j.get("ts_ms", 0) < since_ms:
            continue
        if pipeline is not None and pipeline_of(j) != pipeline:
            continue
        j["_file"] = os.path.basename(f)
        j["_bailout"] = f.endswith("-bailout.json") or bool(j.get("bailout_reason"))
        rows.append(j)
    return rows


def turn_metrics(j):
    """Per-turn G1 (churn) + G3 (in-place refined edits, draft->final divergence)."""
    lde = j.get("live_display_events", [])
    rbs, refined_edits = [], []
    prev = None
    for e in lde:
        cur = toks(e.get("text") or "")
        if prev is not None:
            rb = rollback(prev, cur)
            rbs.append(rb)
            # An in-place refined correction: a refined-source emit that rewrites
            # already-shown tokens rather than just extending them.
            if e.get("source") == "refined" and not is_append(prev, cur) and rb > 0:
                refined_edits.append(rb)
        prev = cur
    draft = (j.get("draft_text") or "").strip()
    emitted = (j.get("emitted_text") or "").strip()
    draft_final_div = rollback(toks(draft), toks(emitted)) if draft else None
    return {
        "turn_id": j.get("turn_id"),
        "dur_s": j.get("num_samples", 0) / SR,
        "pipeline": pipeline_of(j),
        "bailout": j["_bailout"],
        "events": len(lde),
        "churn_max": max(rbs) if rbs else 0,
        "churn_gt4": sum(1 for r in rbs if r > 4),
        "churn_gt2": sum(1 for r in rbs if r > 2),
        "refined_edits": len(refined_edits),
        "refined_edit_max": max(refined_edits) if refined_edits else 0,
        "draft_final_div": draft_final_div,
        "finalize_ms_sidecar": j.get("finalize_elapsed_ms"),
        "ts_ms": j.get("ts_ms"),
    }


# ---- logs -------------------------------------------------------------------

def parse_jsonl(path, since_ms=None):
    if not path or not os.path.exists(path):
        return []
    out = []
    for line in open(path):
        line = line.strip()
        if not line.startswith("{"):
            continue
        try:
            d = json.loads(line)
        except Exception:
            continue
        if since_ms is not None and d.get("ts_ms", 0) < since_ms:
            continue
        out.append(d)
    return out


def finalize_events(input_log, since_ms=None):
    """Per finalized turn: {turn_id, latency_ms, draft_chars} from input.log.

    latency_ms = engine_final_text.ts - engine_speech_finalizing.ts. draft_chars (the live
    draft length at finalize) is a turn-size proxy so latency can be bucketed by size even
    for turns whose sidecars have rotated away — input.log is append-only and keeps them all.
    """
    start = {}
    out = []
    for d in parse_jsonl(input_log, since_ms):
        ev, tid, ts = d.get("event"), d.get("turn_id"), d.get("ts_ms")
        if tid is None or ts is None:
            continue
        if ev == "engine_speech_finalizing":
            start[tid] = (ts, d.get("draft_chars") or 0)
        elif ev == "engine_final_text" and tid in start:
            s_ts, dch = start.pop(tid)
            out.append({"turn_id": tid, "latency_ms": ts - s_ts, "draft_chars": dch})
    return out


def finalize_latencies(input_log, since_ms=None):
    """turn_id -> finalize wall time (ms)."""
    return {e["turn_id"]: e["latency_ms"] for e in finalize_events(input_log, since_ms)}


DRAFT_CHAR_BUCKETS = [(0, 50), (50, 200), (200, 500), (500, 10 ** 9)]


def latency_buckets(events):
    rows = []
    for lo, hi in DRAFT_CHAR_BUCKETS:
        vals = [e["latency_ms"] for e in events if lo <= e["draft_chars"] < hi]
        label = f"{lo}-{'+' if hi >= 10 ** 8 else hi} ch"
        rows.append([label, len(vals), pct(vals, 50), pct(vals, 95)])
    return rows


def metrics_bailouts(metrics_log, since_ms=None):
    n = 0
    for d in parse_jsonl(metrics_log, since_ms):
        if d.get("event") == "partial_finalize_outcome" and "bailout" in (d.get("outcome") or ""):
            n += 1
    return n


def stderr_stall_gap(stderr_log, since_ms=None):
    """GPU-contention risk observables (D-risk #1). since_ms unused: stderr has no ts."""
    if not stderr_log or not os.path.exists(stderr_log):
        return None
    stall = gap = dual_final = 0
    for line in open(stderr_log, errors="replace"):
        if "TOON_LIVE_STREAM_STALL" in line:
            stall += 1
        elif "TOON_LIVE_STREAM_GAP" in line:
            gap += 1
        elif "TOON_DUAL_STREAM_FINAL" in line:
            dual_final += 1
    return {"stall": stall, "gap": gap, "dual_final_lines": dual_final}


# ---- report -----------------------------------------------------------------

def build_report(args):
    rows = load_sidecars(args.dir, args.since_ms, args.pipeline)
    tms = [turn_metrics(j) for j in rows]
    lat_events = finalize_events(args.input_log, args.since_ms)
    lat = {e["turn_id"]: e["latency_ms"] for e in lat_events}
    for t in tms:
        t["finalize_ms"] = lat.get(t["turn_id"], t["finalize_ms_sidecar"])

    non_bailout = [t for t in tms if not t["bailout"]]
    long_turns = [t for t in non_bailout if t["dur_s"] >= args.min_long_s]
    lat_vals = [t["finalize_ms"] for t in tms if t["finalize_ms"] is not None]
    churn_all = [t["churn_max"] for t in non_bailout]

    agg = {
        "turns": len(tms),
        "non_bailout": len(non_bailout),
        "long_turns": len(long_turns),
        "bailouts_sidecar": sum(1 for t in tms if t["bailout"]),
        "bailouts_metrics_log": metrics_bailouts(args.metrics_log, args.since_ms),
        "churn_max": max(churn_all) if churn_all else 0,
        "churn_gt4_events": sum(t["churn_gt4"] for t in non_bailout),
        "churn_gt2_events": sum(t["churn_gt2"] for t in non_bailout),
        "finalize_ms_p50": pct(lat_vals, 50),
        "finalize_ms_p95": pct(lat_vals, 95),
        "refined_edit_turns": sum(1 for t in non_bailout if t["refined_edits"] > 0),
        "refined_edit_max": max((t["refined_edit_max"] for t in non_bailout), default=0),
        "long_turns_with_refined_edit": sum(1 for t in long_turns if t["refined_edits"] > 0),
        "latency_buckets": latency_buckets(lat_events),
        "stderr": stderr_stall_gap(args.stderr_log, args.since_ms),
    }
    # Gate verdicts (mirror plan Phase 1.4).
    agg["gate_G1_pass"] = agg["churn_max"] <= 2 and agg["churn_gt4_events"] == 0
    agg["gate_G2_bailouts_pass"] = agg["bailouts_sidecar"] == 0
    agg["gate_G3_pass"] = (
        agg["refined_edit_max"] <= 2
        and (not long_turns or agg["long_turns_with_refined_edit"] > 0)
    )
    return tms, agg


def emit_text(tms, agg, md=False):
    out = []
    h = "## " if md else ""
    out.append(f"{h}live_metrics — {agg['turns']} turns "
               f"({agg['non_bailout']} clean, {agg['long_turns']} long)")
    row = "| {tid:>6} | {pl:11} | {dur:>6} | {ev:>4} | {cmax:>4} | {cg4:>4} | " \
          "{re:>4} | {rmax:>4} | {dfd:>4} | {fin:>7} | {b} |"
    if md:
        out.append("")
        out.append("| turn | pipeline | dur_s | ev | cMax | c>4 | rEd | rMax | dfDiv | fin_ms | bail |")
        out.append("|-----:|----------|------:|---:|-----:|----:|----:|-----:|------:|-------:|:----:|")
    else:
        out.append(f"{'turn':>6} {'pipeline':11} {'dur_s':>6} {'ev':>4} {'cMax':>4} "
                   f"{'c>4':>4} {'rEd':>4} {'rMax':>4} {'dfDiv':>5} {'fin_ms':>7} bail")
    for t in sorted(tms, key=lambda x: -(x["dur_s"] or 0)):
        vals = dict(
            tid=t["turn_id"], pl=t["pipeline"], dur=f"{t['dur_s']:.0f}", ev=t["events"],
            cmax=t["churn_max"], cg4=t["churn_gt4"], re=t["refined_edits"],
            rmax=t["refined_edit_max"],
            dfd="-" if t["draft_final_div"] is None else t["draft_final_div"],
            fin="-" if t["finalize_ms"] is None else int(t["finalize_ms"]),
            b="Y" if t["bailout"] else " ",
        )
        if md:
            out.append(row.format(**vals))
        else:
            out.append(f"{vals['tid']:>6} {vals['pl']:11} {vals['dur']:>6} {vals['ev']:>4} "
                       f"{vals['cmax']:>4} {vals['cg4']:>4} {vals['re']:>4} {vals['rmax']:>4} "
                       f"{str(vals['dfd']):>5} {str(vals['fin']):>7} {vals['b']}")
    out.append("")
    out.append(f"{h}Gates (plan Phase 1.4)")
    p50 = agg["finalize_ms_p50"]
    p95 = agg["finalize_ms_p95"]
    out.append(f"  G1 churn      : max={agg['churn_max']} tok, >4-swaps={agg['churn_gt4_events']}, "
               f">2={agg['churn_gt2_events']}  -> {'PASS' if agg['gate_G1_pass'] else 'FAIL'}")
    out.append(f"  G2 latency    : p50={p50 and round(p50)} ms, p95={p95 and round(p95)} ms; "
               f"bailouts={agg['bailouts_sidecar']} (metrics.log={agg['bailouts_metrics_log']})"
               f"  -> {'PASS' if agg['gate_G2_bailouts_pass'] else 'FAIL'} (compare p50/p95 vs baseline)")
    if any(n for _, n, _, _ in agg["latency_buckets"]):
        out.append("  G2 by size    : finalize latency by draft chars (input.log, all turns in window)")
        for label, n, b50, b95 in agg["latency_buckets"]:
            if n:
                out.append(f"    {label:>10}: n={n:<4} p50={b50 and round(b50):>5} ms  "
                           f"p95={b95 and round(b95):>5} ms")
    out.append(f"  G3 corrections: refined-edit turns={agg['refined_edit_turns']} "
               f"(long: {agg['long_turns_with_refined_edit']}/{agg['long_turns']}), "
               f"max magnitude={agg['refined_edit_max']} tok"
               f"  -> {'PASS' if agg['gate_G3_pass'] else 'FAIL'}")
    if agg["stderr"]:
        s = agg["stderr"]
        out.append(f"  stderr risk   : live-stream stall={s['stall']}, gap={s['gap']}, "
                   f"dual_final lines={s['dual_final_lines']}")
    return "\n".join(out)


def cmd_report(args):
    tms, agg = build_report(args)
    if args.json:
        print(json.dumps({"turns": tms, "aggregate": agg}, indent=2, default=str))
    else:
        print(emit_text(tms, agg, md=args.md))


# ---- bank -------------------------------------------------------------------

def cmd_bank(args):
    os.makedirs(args.dest, exist_ok=True)
    n = 0
    for f in glob.glob(os.path.join(args.src, "*.json")) + glob.glob(os.path.join(args.src, "*.wav")):
        dst = os.path.join(args.dest, os.path.basename(f))
        if os.path.exists(dst) and os.path.getsize(dst) == os.path.getsize(f):
            continue
        shutil.copy2(f, dst)
        n += 1
    print(f"banked {n} new file(s) into {args.dest}")


# ---- events-churn (headless cross-check) ------------------------------------

def cmd_events_churn(_args):
    merged, committed = [], []
    for line in sys.stdin:
        line = line.strip()
        if not line.startswith("{"):
            continue
        try:
            e = json.loads(line)
        except Exception:
            continue
        if e.get("event") != "active":
            continue
        m = e.get("merged")
        if m is None:
            m = (e.get("committed", "") + e.get("live", ""))
        merged.append(m)
        committed.append(e.get("committed", ""))

    def series(texts):
        rbs, prev = [], None
        for t in texts:
            c = toks(t)
            if prev is not None:
                rbs.append(rollback(prev, c))
            prev = c
        return rbs

    for label, texts in (("merged (visible)", merged), ("committed", committed)):
        rbs = series(texts)
        nz = [r for r in rbs if r > 0]
        print(f"{label:18} events={len(texts)} rollbacks={len(nz)} "
              f"max={max(rbs) if rbs else 0} "
              f"mean={round(statistics.mean(nz), 2) if nz else 0} "
              f">2={sum(1 for r in rbs if r > 2)} >4={sum(1 for r in rbs if r > 4)}")


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest="cmd")

    r = sub.add_parser("report", help="score G1/G2/G3 from sidecars + logs")
    r.add_argument("dir", nargs="?", default=DEFAULT_DIR)
    r.add_argument("--input-log")
    r.add_argument("--metrics-log")
    r.add_argument("--stderr-log")
    r.add_argument("--since-ms", type=int)
    r.add_argument("--min-long-s", type=float, default=60.0)
    r.add_argument("--pipeline", choices=["dual_stream", "legacy_stitch"])
    r.add_argument("--md", action="store_true")
    r.add_argument("--json", action="store_true")
    r.set_defaults(func=cmd_report)

    b = sub.add_parser("bank", help="rotation-proof copy of sidecars + wavs")
    b.add_argument("src")
    b.add_argument("dest")
    b.set_defaults(func=cmd_bank)

    e = sub.add_parser("events-churn", help="headless G1 cross-check from --events-jsonl on stdin")
    e.set_defaults(func=cmd_events_churn)

    args = p.parse_args()
    if not args.cmd:
        p.print_help()
        sys.exit(2)
    args.func(args)


if __name__ == "__main__":
    main()
