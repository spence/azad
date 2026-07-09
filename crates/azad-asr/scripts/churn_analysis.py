#!/usr/bin/env python3
"""
Measure live-caption churn across dual_stream debug-recording sidecars.

For each dual_stream turn, walks live_display_events in order. For each
consecutive pair (prev_text -> text) where `text` is NOT a pure append of
prev_text (i.e. prev_text is not a token-prefix of text), we compute:
  - the token-level common prefix length
  - rollback_tokens = len(prev_tokens) - common_prefix_len
    (how many trailing tokens of the previously-shown caption were discarded)
  - a classification of the change type

This is measurement only. No pipeline code is modified.
"""
import json
import glob
import os
import re
import difflib
from collections import Counter, defaultdict

DEBUG_DIR = os.path.expanduser("~/Library/Application Support/Azad/debug-recordings")

WORD_RE = re.compile(r"[A-Za-z0-9']+|[.,!?;:]")


def tokenize(text):
    if not text:
        return []
    return WORD_RE.findall(text)


def norm_word(w):
    # Lowercase, strip trailing punctuation-only tokens are already separate,
    # strip possessive/apostrophes for homophone-ish comparison base.
    return w.lower()


def strip_punct(w):
    return w.lower().strip(".,!?;:")


PRONOUN_HOMOPHONE_PAIRS = {
    frozenset({"they're", "their", "there"}),
    frozenset({"its", "it's"}),
    frozenset({"your", "you're"}),
    frozenset({"to", "too", "two"}),
    frozenset({"then", "than"}),
    frozenset({"were", "we're", "where"}),
    frozenset({"whose", "who's"}),
}


def is_pronoun_homophone(a, b):
    a_, b_ = strip_punct(a), strip_punct(b)
    if a_ == b_:
        return False
    for pair_set in PRONOUN_HOMOPHONE_PAIRS:
        if a_ in pair_set and b_ in pair_set:
            return True
    return False


def is_word_boundary_flip(prev_tail, new_tail):
    """
    e.g. ["sub", "agents"] vs ["subagents"] -- same characters when joined
    (ignoring spaces/case/punct), different tokenization.
    """
    prev_joined = "".join(strip_punct(w) for w in prev_tail)
    new_joined = "".join(strip_punct(w) for w in new_tail)
    return bool(prev_joined) and prev_joined == new_joined and len(prev_tail) != len(new_tail)


def is_punctuation_only_diff(prev_tail, new_tail):
    prev_stripped = [strip_punct(w) for w in prev_tail if strip_punct(w)]
    new_stripped = [strip_punct(w) for w in new_tail if strip_punct(w)]
    return prev_stripped == new_stripped and prev_tail != new_tail


def is_capitalization_only_diff(prev_tail, new_tail):
    if len(prev_tail) != len(new_tail):
        return False
    if prev_tail == new_tail:
        return False
    return [w.lower() for w in prev_tail] == [w.lower() for w in new_tail]


def diff_tails(prev_tokens, tokens):
    """
    Use SequenceMatcher over the full token sequences to find the true
    changed span (not just a fixed-position rollback from the common
    prefix), since a mid-caption re-decode can shift alignment by more
    than a simple word-for-word substitution at the tail.
    Returns (prev_span_tokens, new_span_tokens, rollback_tokens) where
    rollback_tokens = tokens dropped from prev's suffix onward (i.e. the
    count used for "how far back did the visible caption change").
    """
    sm = difflib.SequenceMatcher(a=prev_tokens, b=tokens, autojunk=False)
    ops = sm.get_opcodes()
    # find first non-equal op
    first_diff = next((op for op in ops if op[0] != "equal"), None)
    if first_diff is None:
        return [], [], 0
    _, i1, _, j1, _ = first_diff
    prev_span = prev_tokens[i1:]
    new_span = tokens[j1:]
    rollback_tokens = len(prev_tokens) - i1
    return prev_span, new_span, rollback_tokens


def classify_subspan(prev_sub, new_sub):
    """
    Classify a single (replace/delete/insert) opcode's token spans.
    Order matters: check the narrowest/most specific explanation first.
    """
    if not prev_sub and not new_sub:
        return "no-op"
    if not prev_sub or not new_sub:
        # pure insertion or deletion of tokens -- most often filler words
        # (uh, um, like) getting added/removed, or a genuine word add/drop.
        content = [w for w in (prev_sub or new_sub) if strip_punct(w) not in ("", "uh", "um", "like")]
        if not content:
            return "filler-insert-delete"
        return "genuine-correction"
    if is_capitalization_only_diff(prev_sub, new_sub):
        return "capitalization"
    if is_punctuation_only_diff(prev_sub, new_sub):
        return "punctuation-swap"
    if is_word_boundary_flip(prev_sub, new_sub):
        return "word-boundary-flip"
    if len(prev_sub) == len(new_sub):
        diffs = [(a, b) for a, b in zip(prev_sub, new_sub) if strip_punct(a) != strip_punct(b)]
        if len(diffs) == 1 and is_pronoun_homophone(*diffs[0]):
            return "pronoun/homophone"
        if len(diffs) == 0:
            return "capitalization"
        if len(diffs) == 1:
            a, b = diffs[0]
            a_, b_ = strip_punct(a), strip_punct(b)
            # single-word growth, e.g. "gen" -> "generate": a prefix of b,
            # from the ASR completing a partial token -- low-severity churn
            # distinct from a full word swap.
            if a_ and b_.startswith(a_) and a_ != b_:
                return "single-word-completion"
            return "single-word-correction"
    return "genuine-correction"


def classify_change(prev_tail, new_tail):
    """
    prev_tail / new_tail are the full changed span (from the first diff
    opcode onward) between two consecutive caption renders. A single span
    can bundle multiple distinct edits (case flip + rejoin + pronoun swap
    all at once), so we re-run SequenceMatcher at word granularity inside
    the span, classify each opcode's sub-edit, and report the tag set.
    Returns (primary_class, all_classes_set) where primary_class is the
    most specific single label if the whole span reduces to one edit type,
    else "compound:<a>+<b>+..." listing the distinct sub-edit classes
    (still informative for the taxonomy, and never silently mislabeled as
    a single genuine-correction when it wasn't one).
    """
    if not prev_tail and not new_tail:
        return "no-op"
    sm = difflib.SequenceMatcher(a=prev_tail, b=new_tail, autojunk=False)
    sub_classes = []
    for tag, i1, i2, j1, j2 in sm.get_opcodes():
        if tag == "equal":
            continue
        sub_classes.append(classify_subspan(prev_tail[i1:i2], new_tail[j1:j2]))
    distinct = []
    for c in sub_classes:
        if c not in distinct:
            distinct.append(c)
    if not distinct:
        return "no-op"
    if len(distinct) == 1:
        return distinct[0]
    # meaningful edit exists alongside cosmetic ones -> still surface it,
    # but tag as compound so we don't hide the cosmetic churn inside a
    # "genuine-correction" bucket.
    return "compound:" + "+".join(sorted(distinct))


def common_prefix_len(a_tokens, b_tokens):
    n = 0
    for a, b in zip(a_tokens, b_tokens):
        if a == b:
            n += 1
        else:
            break
    return n


def analyze_turn(turn_id, events, sample_rate):
    """
    Returns list of churn records for this turn, plus summary counters.
    A churn event = an emit/hold_rollback whose text is not a pure
    token-append of the immediately preceding *shown* text.
    """
    churns = []
    prev_text = ""
    prev_tokens = []
    streaming_emits = 0
    refined_emits = 0
    rollback_actions = 0
    max_rollback = 0

    for idx, ev in enumerate(events):
        text = ev.get("text") or ""
        source = ev.get("source", "?")
        action = ev.get("action", "?")
        samples = ev.get("audio_samples")

        if source == "streaming":
            streaming_emits += 1
        elif source == "refined":
            refined_emits += 1

        if action == "hold_rollback":
            rollback_actions += 1

        tokens = tokenize(text)

        if not prev_tokens:
            prev_text, prev_tokens = text, tokens
            continue

        cpl = common_prefix_len(prev_tokens, tokens)
        is_pure_append = cpl == len(prev_tokens)

        if not is_pure_append:
            prev_tail, new_tail, rollback_tokens = diff_tails(prev_tokens, tokens)
            change_class = classify_change(prev_tail, new_tail)
            if change_class == "genuine-correction" and source == "refined":
                # Distinguish a real lexical fix from a wholesale refined
                # re-decode bleeding through the "streaming hypothesis"
                # (heuristic: bleed if the rolled-back span is long relative
                # to what replaced it, or replaces >1 word with a very
                # different word run with little lexical overlap).
                prev_set = set(strip_punct(w) for w in prev_tail)
                new_set = set(strip_punct(w) for w in new_tail)
                overlap = prev_set & new_set
                if rollback_tokens >= 3 and len(overlap) == 0:
                    change_class = "refined-bleed"

            max_rollback = max(max_rollback, rollback_tokens)

            churns.append(
                {
                    "turn_id": turn_id,
                    "idx": idx,
                    "source": source,
                    "action": action,
                    "rollback_tokens": rollback_tokens,
                    "class": change_class,
                    "prev_text": prev_text,
                    "new_text": text,
                    "prev_tail": " ".join(prev_tail),
                    "new_tail": " ".join(new_tail),
                    "audio_samples": samples,
                }
            )

        prev_text, prev_tokens = text, tokens

    return {
        "turn_id": turn_id,
        "churns": churns,
        "streaming_emits": streaming_emits,
        "refined_emits": refined_emits,
        "rollback_actions": rollback_actions,
        "max_rollback": max_rollback,
        "num_events": len(events),
        "sample_rate": sample_rate,
    }


def main():
    files = sorted(glob.glob(os.path.join(DEBUG_DIR, "*.json")))
    dual_turns = []

    for f in files:
        try:
            d = json.load(open(f))
        except Exception as e:
            print(f"SKIP {f}: {e}")
            continue
        if d.get("pipeline") != "dual_stream":
            continue
        events = d.get("live_display_events", [])
        if not events:
            continue
        turn_id = d.get("turn_id")
        sample_rate = d.get("sample_rate", 16000)
        num_samples = d.get("num_samples", 0)
        dur_s = num_samples / sample_rate if sample_rate else 0.0
        full_text = d.get("full_text") or d.get("emitted_text") or ""
        result = analyze_turn(turn_id, events, sample_rate)
        result["file"] = os.path.basename(f)
        result["dur_s"] = dur_s
        result["full_text"] = full_text
        dual_turns.append(result)

    print(f"Total dual_stream turns with live_display_events: {len(dual_turns)}")
    total_dual_files = sum(
        1 for f in files if json.load(open(f)).get("pipeline") == "dual_stream"
    )
    print(f"(Total dual_stream sidecars incl. those with 0 events: {total_dual_files})")

    # -------- worst turns by max rollback --------
    ranked = sorted(dual_turns, key=lambda r: r["max_rollback"], reverse=True)
    print("\n=== WORST 8 TURNS BY MAX ROLLBACK ===")
    worst = []
    for r in ranked[:8]:
        worst_churn = max(r["churns"], key=lambda c: c["rollback_tokens"]) if r["churns"] else None
        print(f"\nturn_id={r['turn_id']} file={r['file']} dur={r['dur_s']:.1f}s "
              f"max_rollback={r['max_rollback']} n_churns={len(r['churns'])} "
              f"streaming_emits={r['streaming_emits']} refined_emits={r['refined_emits']} "
              f"rollback_actions={r['rollback_actions']}")
        if worst_churn:
            print(f"  worst example [{worst_churn['source']}/{worst_churn['action']}] "
                  f"class={worst_churn['class']}")
            print(f"    prev: ...{worst_churn['prev_text'][-90:]!r}")
            print(f"    new : ...{worst_churn['new_text'][-90:]!r}")
        worst.append(
            {
                "turn_id": r["turn_id"],
                "max_rollback_tokens": r["max_rollback"],
                "streaming_emits": r["streaming_emits"],
                "refined_emits": r["refined_emits"],
                "rollbacks": len(r["churns"]),
                "dur_s": round(r["dur_s"], 1),
                "example": (
                    f"prev: ...{worst_churn['prev_text'][-90:]!r} -> new: ...{worst_churn['new_text'][-90:]!r}"
                    if worst_churn
                    else ""
                ),
            }
        )

    # -------- taxonomy --------
    all_churns = [c for r in dual_turns for c in r["churns"]]
    class_counter = Counter(c["class"] for c in all_churns)
    class_examples = {}
    for c in all_churns:
        if c["class"] not in class_examples:
            class_examples[c["class"]] = c

    print("\n=== TAXONOMY (all dual turns) ===")
    for cls, cnt in class_counter.most_common():
        ex = class_examples[cls]
        print(f"{cls}: {cnt}")
        print(f"    e.g. [{ex['source']}] '{ex['prev_tail']}' -> '{ex['new_tail']}'")

    # -------- source breakdown of churning events specifically --------
    churn_source_counter = Counter(c["source"] for c in all_churns)
    print(f"\nChurn events by source: {dict(churn_source_counter)}")

    # -------- aggregate stats --------
    n_churn_gt2 = sum(1 for r in dual_turns if len(r["churns"]) > 2)
    n_churn_gt4 = sum(1 for r in dual_turns if len(r["churns"]) > 4)
    total_streaming_emits = sum(r["streaming_emits"] for r in dual_turns)
    total_refined_emits = sum(r["refined_emits"] for r in dual_turns)
    ratio = (total_refined_emits / total_streaming_emits) if total_streaming_emits else float("inf")

    print(f"\nTurns with churn-event count > 2: {n_churn_gt2} / {len(dual_turns)}")
    print(f"Turns with churn-event count > 4: {n_churn_gt4} / {len(dual_turns)}")
    print(f"Total streaming emits: {total_streaming_emits}, total refined emits: {total_refined_emits}, "
          f"refined:streaming ratio = {ratio:.1f}:1")

    # correlation: churn count vs turn duration
    durs = [r["dur_s"] for r in dual_turns]
    churn_counts = [len(r["churns"]) for r in dual_turns]

    def pearson(xs, ys):
        n = len(xs)
        if n < 2:
            return float("nan")
        mx, my = sum(xs) / n, sum(ys) / n
        cov = sum((x - mx) * (y - my) for x, y in zip(xs, ys))
        vx = sum((x - mx) ** 2 for x in xs)
        vy = sum((y - my) ** 2 for y in ys)
        if vx == 0 or vy == 0:
            return float("nan")
        return cov / (vx**0.5 * vy**0.5)

    corr_dur = pearson(durs, churn_counts)
    print(f"\nPearson corr(churn_count, turn_dur_s) = {corr_dur:.3f}")

    # correlation with repeated-phrase content: approximate by counting
    # tokens in full_text that repeat later in full_text (simple bigram
    # repeat count) as a proxy for "sub agents / subagents / forked agents"
    # style repeated-terminology turns.
    def repeat_score(text):
        toks = [strip_punct(w) for w in tokenize(text) if strip_punct(w)]
        bigram_counts = Counter(zip(toks, toks[1:]))
        repeats = sum(c - 1 for c in bigram_counts.values() if c > 1)
        return repeats

    repeat_scores = [repeat_score(r["full_text"]) for r in dual_turns]
    corr_repeat = pearson(repeat_scores, churn_counts)
    print(f"Pearson corr(churn_count, repeated_bigram_score) = {corr_repeat:.3f}")

    # also correlate rollback size class-wise with source
    print("\n=== Class x Source crosstab ===")
    crosstab = defaultdict(Counter)
    for c in all_churns:
        crosstab[c["class"]][c["source"]] += 1
    for cls, sc in crosstab.items():
        print(f"  {cls}: {dict(sc)}")

    return {
        "total_dual_turns": len(dual_turns),
        "worst": worst,
        "taxonomy": [
            {"churn_class": cls, "count": cnt, "example": (
                f"[{class_examples[cls]['source']}] '{class_examples[cls]['prev_tail']}' -> "
                f"'{class_examples[cls]['new_tail']}'"
            )}
            for cls, cnt in class_counter.most_common()
        ],
        "aggregate": {
            "turns_churn_gt2": n_churn_gt2,
            "turns_churn_gt4": n_churn_gt4,
            "total_streaming_emits": total_streaming_emits,
            "total_refined_emits": total_refined_emits,
            "refined_to_streaming_ratio": ratio,
            "churn_events_by_source": dict(churn_source_counter),
            "corr_churn_vs_duration": corr_dur,
            "corr_churn_vs_repeated_bigrams": corr_repeat,
            "class_source_crosstab": {k: dict(v) for k, v in crosstab.items()},
        },
    }


if __name__ == "__main__":
    result = main()
    with open("/tmp/churn_analysis_result.json", "w") as f:
        json.dump(result, f, indent=2)
    print("\nWrote /tmp/churn_analysis_result.json")
