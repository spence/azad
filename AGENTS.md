# Runtime Agent Design - AGENTS Guide

## Purpose

This workspace is a multi-repo speech transcription system centered on the `Azad` macOS menu bar app.
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
- `crates/asr-rs`: ASR/transcription engine crate used by app runtime.
- `crates/whisper-cpp-plus-rs`: whisper integration layer, examples, benches, tests.
- `crates/parakeet-rs`: additional model/audio components.
- `crates/whisper.cpp`: upstream dependency checkout.

## Deep Specs

- `crates/asr-rs/SPECIFICATION.md`: ASR runtime architecture, turn pipeline, fallback/quality design, and change playbooks.
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
- Validate live runtime for UI/hotkey changes after restart.

## Commit and Scope Hygiene

- This workspace has multiple git repos. Commit in the correct repo(s).
- Group commits by change domain (state machine, UI, engine, etc.), not by random file order.
- Do not revert unrelated local changes unless explicitly requested.
