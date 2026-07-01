# TDT empty-partial root cause

**Date**: 2026-05-13
**Wav under test**: `~/Library/Application Support/Azad/debug-recordings/1778730938768-turn-000057-bailout.wav`
**Failing range**: samples `[3399680, 3527680)` (8.0 s @ 16 kHz)
**Tools used**:
- Historical local `parakeet-rs` fork diagnostic CLI (`examples/tdt_diag.rs`)
- Historical local `parakeet-rs` fork trace hook (`AZAD_TDT_TRACE=1`)

## Conclusion (TL;DR)

The TDT joint network's LSTM state starts at zeros. When the first encoder frame of an 8-second incremental window happens to produce features that make the joint predict the blank token with high confidence (combined with `duration_step ≥ 1` on the TDT duration head), the LSTM never updates (state is only updated on non-blank emissions), the joint stays in blank-confident mode, and the model "strides through" the entire window without emitting anything. We call this the **cold-LSTM trap**.

The trap is **start-sample-specific** — exquisitely so. A 100 ms shift in either direction (1 600 samples) breaks it because the encoder's first frame contains different audio content, producing different first-frame logits and a different decoder trajectory. The full-pass on the whole turn doesn't hit the trap because by sample 3 399 680 the LSTM has been warmed by thousands of prior emissions.

Recommended fix: when an incremental partial returns empty in an EOU-confirmed-speech range, retry with a shifted start sample (±100–500 ms) before bailing to full-pass.

## Phase A — Reproducibility

Standalone replay of `audio[3399680..3527680]` (extracted from the preserved wav) through `ParakeetTDT::transcribe_samples` yields `text=""`, 0 tokens. RMS −25.7 dBFS, peak −5.8 dBFS — normal speech loudness, not anomalously quiet. Same audio in the full-turn pass decodes 2 788 chars including this span.

**Failure is model-side, not pipeline-side.**

## Phase B — Per-frame trace identifies the mechanism

`AZAD_TDT_TRACE=1` enables per-frame TSV from `greedy_decode`. The 101-frame trace for the failing window:

```
t=0   enc_l2=0.287 argmax_tok=8192(blank) logit=-0.788  top2=7618 logit=-4.666  dur=1  h_l2=0.000 c_l2=0.000
t=1   enc_l2=0.280 argmax_tok=8192(blank) logit=-1.342  top2=1502 logit=-6.840  dur=1  h_l2=0.000 c_l2=0.000
…
t=10  enc_l2=0.532 argmax_tok=8192(blank)               dur=4
t=14  enc_l2=0.534 argmax_tok=8192(blank)               dur=4
…  (continues, dur=4 dominates; only 38 frames visited of 101 because dur=4 skips 4 frames at a time)
t=100 enc_l2=0.786 argmax_tok=8192(blank)               dur=1
```

Three diagnostic features:

1. **Every frame predicts blank** (token id 8192). The blank logit dominates by ~3-6 logit units consistently (~50-400× confident in blank).
2. **Encoder is healthy**: `enc_l2` ranges 0.28–0.79 across frames. Not collapsed, not NaN.
3. **LSTM state stays at zero**: `h_l2 = c_l2 = 0.000` for the entire run. This is the smoking gun. The joint's recurrent state is initialized to zeros (`model_tdt.rs` ~line 189) and only updated when a non-blank token is emitted (~line 251–271). Every-blank means the LSTM never warms up.

The duration head amplifies the problem: it mostly predicts `dur=4` (skip 4 frames), which races `t` from 0 to 100 in ~38 iterations without giving the model any chance to recover.

**Mechanism: all-blank from cold LSTM. NOT encoder collapse. NOT duration-stride-out alone (the LSTM cold-start is the root cause; the duration head just accelerates the failure).**

## Phase C — Context-length sweep

Centred sweep, length varying:

| Window | Range | Length | Chars | Tokens |
|-|-|-|-|-|
| baseline | `[3399680, 3527680)` | 8.0 s | **0** | **0** |
| +0.5 s each side | `[3391680, 3535680)` | 9.0 s | 109 | 26 |
| +1 s each side | `[3383680, 3543680)` | 10.0 s | 109 | 26 |
| +2 s each side | `[3367680, 3559680)` | 12.0 s | 131 | 30 |
| +5 s each side | `[3319680, 3607680)` | 18.0 s | 217 | 49 |
| +20 s before, +5 s after | `[3079680, 3607680)` | 33.0 s | 383 | 85 |
| +25 s after only | `[3399680, 3927680)` | 33.0 s | 411 | 86 |

Just 0.5 s of extra context on each side flips the model from empty to fully decoding the span. 9.0 s output: `"System, priority system, uh like w I don't I don't know. Like I would love for you to go do some research…"` — exact match to what EOU heard.

This rules out "needs full-turn-pass left context." Tiny extra context is enough.

## Phase D — Boundary-shift sweep (8 s windows)

Holding length fixed at 8 s, varying start sample:

| Shift | Start sample | Chars | Text head |
|-|-|-|-|
| −400 ms | 3393280 | 106 | `system, priority system, uh like w I don't I don't know.` |
| −200 ms | 3396480 | 98 | `priority system, uh like w I don't I don't know.` |
| −100 ms | 3398080 | 98 | `Priority system, uh like w I don't I don't know.` |
| **0 (baseline)** | **3399680** | **0** | `` |
| +100 ms | 3401280 | 90 | `system uh like w I don't I don't know` |
| +200 ms | 3402880 | 90 | `system uh like w I don't I don't know` |
| +400 ms | 3406080 | 81 | `Like w I don't I don't know.` |
| +800 ms | 3412480 | 84 | `uh like w I don't I don't know.` |

**A 100 ms shift in either direction recovers the decoding.** Only this exact 8 s alignment is broken. The cold-LSTM trap is start-sample-specific — it depends on what audio content lands in the first encoder frame.

## Phase E — Asymmetric variant tests

Same start sample, vary length only (extend right):

| Variant | Range | Chars |
|-|-|-|
| 8.0 s baseline | `[3399680, 3527680)` | 0 |
| 8.1 s extend +100 ms right | `[3399680, 3529280)` | 0 |
| 8.2 s extend +200 ms right | `[3399680, 3530880)` | 0 |
| 8.5 s extend +500 ms right | `[3399680, 3535680)` | 81 |
| 9.0 s extend +1 s right | `[3399680, 3543680)` | 0 |
| 10.0 s extend +2 s right | `[3399680, 3559680)` | 0 |

Right-extension is unreliable — even 9 s and 10 s windows are empty when the start sample is held at the trap.

Same end sample, vary length only (extend left):

| Variant | Range | Chars |
|-|-|-|
| 8.0 s baseline | `[3399680, 3527680)` | 0 |
| 8.1 s extend +100 ms left | `[3398080, 3527680)` | 98 |
| 8.2 s extend +200 ms left | `[3396480, 3527680)` | 98 |
| 8.5 s extend +500 ms left | `[3391680, 3527680)` | 106 |
| 9.0 s extend +1 s left | `[3383680, 3527680)` | 102 |

Left-extension reliably recovers. Even 100 ms of extra prefix works.

Shrink (same start, shorter length):

| Variant | Length | Chars |
|-|-|-|
| 7 s | 112000 | 0 |
| 6 s | 96000 | 0 |
| 5 s | 80000 | 35 |
| 4 s | 64000 | 31 |
| 2 s | 32000 | 5 (`"Yeah."`) |

Even shrinking helps — shows the issue is not length-monotonic. It's about whether the encoder's first frame at the trapped start sample dominates a cold-LSTM emission of "blank."

## Why this is consistent across both observed failures

Same architectural pathology, different audio:
- Turn 9 morning, segment 9 `[773120, 901120)` — same model, same window-length, same cold-LSTM init. Lost the wav (predates the bailout-preservation commit), so can't confirm Phase D-style boundary-shift, but the all-blank-from-cold-LSTM mechanism is the only one consistent with "8 s slice empty, longer-context full-pass succeeds."
- Turn 57 evening, segment 36 `[3399680, 3527680)` — confirmed in detail above.

The mechanism is not specific to the user's voice or content — it's specific to the audio frame that lands in encoder time-step 0 of an incremental window. Given a sliding-window scheduler that emits a window every 6 s with 3 s overlap, there will be specific samples whose alignment falls into this trap, and they'll trigger empty output reliably for that audio at that alignment.

## Recommended pipeline fix

**Smallest-change, highest-confidence fix**: retry-with-shifted-start when an incremental partial returns empty in an EOU-confirmed-speech range. Phase D shows ±100 ms shifts reliably recover.

```rust
// In handle_incremental_result, when result.text.trim().is_empty() and EOU shows
// speech in the range (current bailout-non-corroborated branch):
if eou_chars >= EOU_SPEECH_THRESHOLD_FOR_RETRY {
    enqueue_retry_slice(start_sample + RETRY_SHIFT_SAMPLES, end_sample);
}
```

Cost: one extra TDT call per empty-in-speech partial. These are rare (~3% of partials per current data, and only the corroborated-speech subset retry). Latency cost: ~500 ms extra for the retry inference. Avoids the much-larger full-pass bailout (~40 s on a 4-minute turn) and the multi-turn paste race that followed.

**Larger but cleaner fix**: shift the incremental window scheduler to use **end-anchored slicing** with 0.5-1 s of *required* left padding past the previous slice's end. The current code already tries to do this via `incremental_left_context_ms=10000`, but the `INCREMENTAL_MAX_SEGMENT_MS=8000` cap silently truncates left context to zero in the common case. Lift the cap to 9-10 s and re-test. (Risk: longer windows = more model inference time; need to measure.)

**Avoid**: switching to beam search on the joint. Solves duration-stride-out but the cold-LSTM problem persists at beam=1's first step regardless.

## Diagnostic artefacts

These diagnostics were created in the local `parakeet-rs` fork before Azad moved
off in-repo third-party checkouts. They are not part of the default public
workspace. Recreate them in a separate fork checkout if this investigation needs
to be repeated.

The production mitigation described above does not depend on that tooling.
