# Runtime Agent Design - AGENTS Guide

## Purpose

This workspace is a single-repository speech transcription system centered on the `Azad` macOS menu bar app.
The project is focused on low-latency transcription, reliable hotkey/VAD interaction state, and predictable paste behavior.

## Responsibilities

- Redeploy Azad after making changes that affect app behavior, UI, hotkeys, settings, logging, or app state by running `just install`, `just restart` (or `just start`), and verifying with `just status` that the updated app is live.

## Quick Ramp-Up (Start Here)

When taking over a task, do this first:

1. Confirm which repo you are changing (most app issues are in `crates/azad`).
2. Read `PROJECT.md` at workspace root for current goals.
3. For hotkey/session behavior, read:
   - `crates/azad/docs/keyboard-shortcut-state-machine.md`
   - `crates/azad/src/interaction_sm.rs`
   - `crates/azad/src/app.rs`
4. Check current local changes across repos before editing:
   - `git -C <repo> status --short`

## Repository Map

- `crates/azad`: macOS app (overlay UI, hotkeys, settings, paste modes, lifecycle).
- `crates/azad-asr`: ASR/transcription engine crate used by app runtime and CLI.
- Forked/upstream third-party crates are Cargo dependencies, not submodules.

## Deep Specs

- `docs/README.md`: documentation index.
- `crates/azad-asr/SPECIFICATION.md`: ASR runtime architecture, turn pipeline, fallback/quality design, and change playbooks.
- `crates/azad/SPECIFICATION.md`: app interaction architecture, state machine integration, overlay/paste/settings behavior, and change playbooks.

## Primary Runtime Ownership (Non-Negotiable)

- The agent owns application lifecycle after behavior changes.
- If a change can affect runtime behavior, UI, hotkeys, settings, logging, or app state, the agent must:
  - rebuild/install,
  - restart service,
  - verify updated process is running.
- Task is not complete until updated app instance is live.

### Azad lifecycle commands (`crates/azad`)

- `just install`
- `just restart` (or `just start`)
- `just status`
- `just logs`

Expected service/app identifiers:
- LaunchAgent label: `ai.azad`
- App bundle executable: `~/Applications/Azad.app/Contents/MacOS/azad`
- Logs: `~/Library/Logs/Azad/`

## Current Behavioral Priorities

- Listen toggle must remain available from menu.
- Hotkey state transitions must be deterministic and tested.
- Defer/interrupt semantics must match documented state machine.
- No arbitrary sleeps in critical paths (except unavoidable platform boundaries).
- Prefer event-driven behavior over polling/spin loops.

## Testing Expectations

- Add or update unit tests for state-machine and transition changes.
- Run repo-appropriate tests before finishing:
  - `cargo test -q` (in touched Rust repos, especially `crates/azad`).
- Validate live runtime for UI/hotkey changes after restart.

## Validation without touching the running app (REQUIRED path)

The user daily-drives the installed `Azad.app` and may be mid-transcription at any moment.
**Never** validate against the running app: do not `just stop`/`restart` it, do not change its
input device, and do not toggle `AzadAlwaysListeningEnabled` or any other `ai.azad`
NSUserDefaults key for a test. Doing so hijacks the user's live dictation and mutates their
settings (e.g. flipping always-listening on). Assume we are NOT competing for GPU/CPU — just do
not interfere with the app's state.

Run all engine/transcription/EOU validation **UI-less** through the standalone `asr` binary,
which drives the *exact same pipeline* with no overlay, no paste, no audio device, and no
`ai.azad` defaults:

```
cargo build -p azad-asr --bin asr --release
M="$HOME/Library/Application Support/Azad/models/nemotron-3.5-mlx-bf16-v1"
./target/release/asr transcribe-file \
  --vad-model "$M/vad/silero_vad.mlmodelc" \
  --mlx-model-dir "$M/mlx" \
  --mlx-helper "$PWD/target/swift/azad-mlx-asr/release/azad-mlx-asr" \
  --language en-US \
  --vad-thold 0.30 --eou-min-silence-ms 350 --eou-max-silence-ms 1000 \
  --vad-in-speech-thold 0.10 --recovery-window-ms 250 --recovery-vad-thold 0.30 \
  <input.wav>                       # add --events-jsonl for the full render-event stream
```

Every EOU/VAD knob is a CLI flag, so sweep timing values here — never in the live app. Match the
app's defaults (see `crates/azad/src/config.rs`), which differ from the `asr` CLI defaults.

Dual-stream is the only transcription pipeline (an instant live caption plus a persistent refined
stream, flushed at finalize — no windowed re-decode/stitcher/bailout). For on-device evidence
(per-turn caption churn, finalize latency, correction magnitude), use
`python3 crates/azad-asr/scripts/live_metrics.py report <sidecar-dir>`.

- Pinned-fixture regressions: `cargo test -p azad-asr --test replay -- --ignored --test-threads=1`
  (`AZAD_TEST_REQUIRE_MODELS=1` to hard-fail instead of skip; needs `AZAD_MLX_ASR_HELPER` set or
  the helper on the default search path).
- App-side overlay/paste routing is covered by `cargo test -p azad` unit tests (e.g. the
  finalizing-caption replay), which need no models and no running app.
- If a real-audio/device path is genuinely required, run a SEPARATE headless
  `asr listen --device "BlackHole 2ch"` and `afplay` into BlackHole — still without changing the
  running `Azad.app`'s device or settings.

## Commit and Scope Hygiene

- This workspace is one Git repository. Keep commits grouped by workstream.
- Group commits by change domain (state machine, UI, engine, etc.), not by random file order.
- Do not revert unrelated local changes unless explicitly requested.
