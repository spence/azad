# Investigation: "live caption freezes on long turns" — refuted

**Outcome: the diagnosed freeze bug is not supported by evidence.** The `TOON_LIVE_STREAM_GAP`
markers that the diagnosis was built on count **speaker pauses**, not model stalls. Reproduced
headlessly, the decode keeps pace with real time and the refined stream is not involved. No fix was
shipped because there is no reproducible defect to fix. This doc records the evidence so the phantom
isn't chased again.

## What was claimed
The live caption freezes 2–5 s during active speech on long turns, caused by the refined 560 ms
stream monopolizing the shared Metal GPU and starving the 80 ms streaming decode (memory
`azad-live-caption-longturn-lag.md`, since corrected).

## How it was tested (Phase 0, headless, never touched the running app)
Added `--debug-stats` to `asr transcribe-file` (emits `TOON_LIVE_STREAM_GAP` etc.). Snapshotted a
long recorded turn out of the rotating `debug-recordings/` dir. Ran `asr transcribe-file --realtime
--debug-stats` (real-time paced) with the app's defaults (streaming 80 / refined 560, tail 4).
Temporary `AZAD_DISABLE_REFINE_FEED` env gate + per-emit wall-clock/refined-cadence logging (all
reverted after).

## What the evidence showed
1. **Refined stream is not the cause (A/B).** Baseline (refined ON) vs refined-feed-DISABLED were
   byte-identical: same 3 gaps, same values (3200 / 2080 / 2080 ms), same total. Disabling refined
   changed nothing → GPU-contention hypothesis refuted.
2. **No wall-clock lag.** Replay ran 64.3 s for 61 s of audio (~1.05× real time); streaming emits
   ran slightly *ahead* of the audio position throughout (lag ≈ −0.74 s, the preroll offset). The
   decode keeps up; there is no growing backlog.
3. **The "gaps" are pauses, not stalls.** Fine-grained RMS of the largest gap (4.0–7.2 s) showed it
   is mostly silence (clear pauses at 4.75–5.5 s and 6.5–7.0 s) with two brief word-bursts; the
   draft grew +5 chars across the whole span. A whole-span RMS average masked the embedded silence.
4. **Confirmed on real daily-use data.** Classifying every ≥2 s gap in the last ~24 real sidecars as
   STALL (≥4 words shown after the gap → speech delayed) vs PAUSE (≤3 words → speaker silent):
   **0 stalls, 40 pauses.** Even the 285 s turn: 12 gaps, all 1–2 words. Not one case of transcribed
   speech being shown late.

## Finalize latency (checked as the alternate "delay at larger lengths" candidate)
From `input.log` (`engine_speech_finalizing` → `engine_final_text`), real turns: p50 ≈ 600 ms, p95
≈ 1.2 s, and **flat with length** (292 ms < 50 chars → ~600 ms and plateaus by 50 chars). The
sidecar flush loop itself is ~33 ms; the remainder is the tentative-recovery window + overhead. This
is a consistent perceptible post-speech wait, but it does **not** scale with turn length, so it does
not explain "worse at larger lengths" either.

## What remains untested
The one variable not reproduced: the user's **machine under heavy load**. The replay ran on a
relatively unloaded machine; the CPU-side pipeline (audio IPC, VAD, helper stdin/stdout round-trips)
could fall behind under real load in a way this replay cannot show. Confirming that needs in-situ
measurement on the user's machine under load (not safe to simulate here — must not compete for the
running app's CPU/GPU), or more specific user detail on when the delay is felt (during speech, on a
pause-resume, or the post-speech paste).

## Kept from this investigation
- `asr transcribe-file --debug-stats` — headless `TOON_*` instrumentation (incl.
  `TOON_LIVE_STREAM_GAP`), the reusable way to reproduce this analysis without the app.
- Reminder for future readers: `TOON_LIVE_STREAM_GAP` is an audio-position jump between caption
  updates; it fires on speaker pauses and must **not** be read as a user-visible freeze without
  checking whether text actually grew across the gap.
