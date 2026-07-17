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
- App bundle executable: `/Users/spence/Applications/Azad.app/Contents/MacOS/azad`
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
- Run `just interaction-test` for shortcut, overlay-state, and session-control changes.
- After deployment, use `just status` to verify process health. Do not drive the installed app to
  validate interactions.

## Desktop Interaction Safety (Non-Negotiable)

The installed `Azad.app` is the user's daily driver, and the active macOS desktop is not a test
fixture.

- Never post synthetic keyboard or mouse events into the user's login session. This includes
  `CGEvent`, AppleScript/System Events, HID injection, and similar desktop automation.
- Never use the installed app, its microphone, its input-device selection, or `ai.azad`
  `NSUserDefaults` values as test controls.
- Exercise shortcut-to-overlay behavior with `just interaction-test`. Its JSONL events are consumed
  only by a separate headless process and cannot reach the window server.
- Exercise transcription, VAD, and EOU behavior with the standalone `asr` binary described below.
- A runtime deployment check is limited to install/restart/status and read-only logs or process
  inspection. It is not permission to synthesize user input.

## Runtime and Engine Validation

App behavior changes must be installed, restarted, and verified with the lifecycle commands above.
For repeatable engine/transcription/EOU sweeps, the standalone `asr` binary drives the exact same
pipeline without overlay or paste behavior:

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

Every EOU/VAD knob is a CLI flag, so use these flags for timing sweeps and match the app's defaults
(see `crates/azad/src/config.rs`), which differ from the `asr` CLI defaults.

Dual-stream is the only transcription pipeline (an instant live caption plus a persistent refined
stream, flushed at finalize — no windowed re-decode/stitcher/bailout). For on-device evidence
(per-turn caption churn, finalize latency, correction magnitude), use
`python3 crates/azad-asr/scripts/live_metrics.py report <sidecar-dir>`.

- Pinned-fixture regressions: `cargo test -p azad-asr --test replay -- --ignored --test-threads=1`
  (`AZAD_TEST_REQUIRE_MODELS=1` to hard-fail instead of skip; needs `AZAD_MLX_ASR_HELPER` set or
  the helper on the default search path).
- App-side overlay/paste routing is covered by `cargo test -p azad` unit tests (e.g. the
  finalizing-caption replay), which need no models and no running app.
- Shortcut-to-overlay interaction sequences are covered by `just interaction-test`; see
  `crates/azad/docs/isolated-interaction-harness.md`.
- If a real-audio/device path is required, a headless `asr listen --device "BlackHole 2ch"` plus
  `afplay` into BlackHole provides a repeatable device-path test.

## Commit and Scope Hygiene

- This workspace is one Git repository. Keep commits grouped by workstream.
- Group commits by change domain (state machine, UI, engine, text, docs, etc.), not by random file order.
- **When a workstream is complete, commit it.** Do not leave finished work uncommitted waiting for an explicit "please commit." A workstream is complete when its code/docs are in place, relevant tests pass, and (if it affects app behavior) the app has been redeployed per lifecycle rules above. Prefer one focused commit per workstream; if several independent workstreams finished in the same session, commit each separately.
- Do not revert unrelated local changes unless explicitly requested.
- Conventional Commits; never add AI co-author trailers or override git authorship.
