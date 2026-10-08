# Post-paste missed speech reproduction — 2026-10-08

Reproduced twice in an isolated, real-time `run_pipeline_with_options` replay. No runtime
implementation was changed. The installed app was not restarted or driven with test input.

## Observed episode

The input/metrics logs identify the reported preceding utterance as turn 145 and the next,
textless turn as 146:

| Event | Unix timestamp, ms |
| --- | ---: |
| Enter on turn 145 | 1791493597491 |
| Turn 145 final text | 1791493597564 |
| Turn 145 paste completed | 1791493597707 |
| Next VAD speech start | 1791493598431 |
| Turn 146 recording completed, no draft or final text | 1791493601496 |

The turn-146 WAV contains the beginning of the question: a fresh standalone `asr
transcribe-file --realtime` replay recovered `Is that a`. This recording ends at the textless
turn's retirement, so it cannot supply the remainder of the complete question.

## Integration reproduction

`recorded_post_paste_followup_has_live_and_final_text` concatenates the original turn-145 and
turn-146 samples, requests Enter on the final processing chunk of turn 145, and appends 2.08 s
of already-pinned microphone room tone. It uses the app's pipeline defaults and real-time
audio delivery. Its checks independently verify the preceding finalized text, actual Enter
handling, a live draft for the follow-up, and a final transcript containing its recorded opening.

The test runs the production VAD and both model streams. It does not open a microphone, register
hotkeys, render an overlay, paste, access app preferences, or write live debug recordings.

```sh
CARGO_BUILD_JOBS=2 \
AZAD_TEST_REQUIRE_MODELS=1 \
AZAD_REPLAY_TRACE=1 \
AZAD_MLX_ASR_HELPER=/Users/spencer/Applications/Azad.app/Contents/MacOS/azad-mlx-asr \
cargo test -p azad-asr --test replay recorded_post_paste_followup_has_live_and_final_text \
  -- --ignored --nocapture --test-threads=1
```

Both runs failed identically (38.53 s and 38.54 s):

```text
chunks=240 turn_starts=[9, 208]
starts=[9, 208] draft_turns={1} finished_turn=1 followup=""
```

Enter finalizes turn 1 at chunk 202. The next turn opens at chunk 208, 960 ms later, or
approximately 800 ms after the finishing chunk's end. No nonempty draft or final text follows.
At chunks 224–226, VAD reports renewed speech (raw probabilities 0.235, 1.000, 1.000). The
pipeline is back at its idle threshold at chunk 228.

The code's relevant failure path is `PipelineCore::should_timeout_empty_vad_turn`: a textless
VAD turn expires after `eou_max_silence_ms * 3` (3 s), even if new speech is arriving. The
check runs before appending/decoding the current chunk. The replay retires the turn at this
boundary while the VAD meter still reports speech. This demonstrates an engine-side missed
start; it does not depend on overlay/paste scheduling. No corrective behavior is implemented.

The companion `immediate_followup_after_manual_finalize_has_live_and_final_text` passed at
0, 160, 320, 640, 960, and 1600 ms gaps using older pinned speech. Ordinary short gaps alone
do not reproduce this recorded failure. An older resume test also uses `320` as a chunk offset
for Enter, placing it 51.2 s after speech rather than 320 ms; that existing test is unchanged.

`cargo test -q -p azad-asr -- --test-threads=1` passed all 131 ordinary tests; the 10 model
replays remain opt-in. The short-gap control passed again on the final test code (96.88 s).

## Provenance and limits

- Runtime source at reproduction: `2b387e58046a668379a23dc6a22656a5072856d1` plus test-only changes.
- The helper is launched as separate test-owned child processes from the installed artifact,
  not contacted through the running app. SHA-256:
  `f0298f9b4ed71a9265bc13a1fbcdd520b80104ff8c3e2cb00f452e44400e1c04`.
- `post-paste-previous-utterance.wav`: 519680 mono float32 samples at 16 kHz; SHA-256
  `8e2589913491c3f77d4ac0f0de10f126fc5e18ccc9999bb1084aaf527c28ad11`.
- `post-paste-missed-start.wav`: 61440 mono float32 samples at 16 kHz; SHA-256
  `85ba0ca71313179f5ab246d83f3b7f29f36b5bea32b47bcd26ddea124acfb147`.
- The preceding sidecar retains only relevant metadata, text and event counts; both WAVs are
  unchanged copies of the captured audio.
- Catalyst, 10 logical CPUs, one model replay at a time, Cargo build jobs 2; initial available
  disk 34 GiB and memory-pressure report 87% free. This is a correctness reproduction, not a
  latency benchmark or proof of uninterrupted hardware capture.
- The live Azad PID remained 2475, started 2026-10-06 14:34:50, with the same three ASR children.
- The model-dependent test remains opt-in and intentionally red until the requested fix is made.
