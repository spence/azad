# Post-paste missed speech reproduction — 2026-10-08

The baseline below reproduced the failure twice without runtime changes or live-app test input.
The validated correction and deployment evidence follow the baseline record.

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

The baseline failure path is `PipelineCore::should_timeout_empty_vad_turn`: a textless
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
- The model-dependent test remains opt-in; its correct-behavior assertion fails on the baseline.

+## Validated correction — 2026-10-08

The owner approved the engine-local inactivity-timeout recommendation and required reevaluation
if it did not fix this recording. The first corrected replay passed, so no alternate strategy,
model change, input-device change, shortcut change, or overlay redesign was needed.

The correction processes the current chunk before empty-turn cleanup. A private sample counter
measures audio without raw VAD speech at the existing in-speech threshold, rather than wall time
since turn creation. Fresh speech resets the counter; smoothed hangover does not. Text from either
decoder still protects the turn, and explicit finish/cancel retain priority.

The threshold is unchanged. This correction does not change VAD's classification of noise or
introduce a maximum duration for continuously detected speech.

Verification:

- First corrected, unchanged recorded-case assertion: passed in 38.67 s, with live and final
  follow-up text (`Is that a`).
- All 10 opt-in model replays: passed, 224.57 s. The existing resume test's Enter offset is
  corrected to 2 chunks (320 ms), so it actually exercises manual finalize.
- Workspace tests: 287 app tests passed (1 unrelated opt-in test ignored), 134 engine tests
  passed, and 57 text tests passed. App tests used a private `HOME` under `target/`, with the
  existing Cargo/Rustup homes explicitly retained; live metrics/preferences were not test controls.
- `just interaction-test`: all 7 headless scenarios passed.
- `cargo clippy -q -p azad-asr --all-targets -- -D warnings` and `cargo fmt --all --check`: passed.
- The recorded-case assertion also passed in the optimized release profile against the signed,
  staged helper, 38.54 s. The staged main app was never launched for testing.

Release reproduction:

```sh
CARGO_BUILD_JOBS=2 AZAD_TEST_REQUIRE_MODELS=1 \
AZAD_MLX_ASR_HELPER=/Users/spencer/Applications/Azad.app/Contents/MacOS/azad-mlx-asr \
cargo test --release -p azad-asr --test replay \
  recorded_post_paste_followup_has_live_and_final_text \
  -- --ignored --nocapture --test-threads=1
```

Deployment:

- The signed release was staged independently. `AZAD_SKIP_MLX_HELPER=1` reuses the unchanged
  model helper; no model weights or helper implementation were changed.
- Canonical live deployment completed: `just install`, one `just restart`, and `just status`.
- Before lifecycle actions, the matching app processes were the previous main PID 2475 and
  its three pure ASR helpers, not agent hosts. The legacy service was absent. App shutdown
  cancels its audio session only; no foreign agent, editor, VM, or gateway daemon was stopped.
- LaunchAgent `ai.azad` is running as PID 66977, started 2026-10-08 16:27:42 PDT.
  `lsof` confirms its mapped executable is the installed bundle's main binary.
- Stable identity remains `ai.azad`, team `35A87BDK48`; strict deep signature verification passes.
  Main CDHash `38ec14544ec5afea9d4ab68af4e9c8facb6e6f03` and helper CDHash `32a8fbc96c3a02532dd470c36528c8f4894027f4`
  match the independently staged release. File SHA differences after signing reflect signature
  metadata; code-directory identities match.
- The tap logs `installed`, reason `startup`, generation 1 at Unix ms 1791502063027.
- No synthetic desktop input, live microphone test, input-device change, preference test control,
  permission reset, or second runtime restart was used.

Installed/source SHA-256 at verification:

- Main app: `1bbb541aa4105e2c1570682db045cc3acf02336b190ec0633616cffef5296c0a`.
- Model helper: `2edacd61420685495fade94d30006cc286fd4f7c2efa795db35d947e8d6bb406`.
- Engine source `src/pipeline.rs`: `8994a468017164c3855b28d18296d9e090c1b108e8c87f5a9d253cf1f48723ea`.

Burndown owns current completion state under `MS-POST-PASTE-SPEECH.T1` and
`MS-POST-PASTE-SPEECH.G1`, linked from `ISS-FOLLOW-UP-SPEECH-CAN-BE-DETECTED-BUT-LOS`.
