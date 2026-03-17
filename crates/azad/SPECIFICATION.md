# Azad Specification

## 1. Purpose

Azad is a macOS menu bar app that orchestrates real-time speech transcription with:

- global hotkeys,
- listen-mode control (auto-VAD on/off),
- overlay visualization (single-lane and split-lane),
- finalization and paste/typing output,
- settings and debug metrics.

It is the interaction and UI layer over `asr-rs` runtime sessions.

## 2. Architectural Boundary

Azad owns:

- keyboard/menu interaction semantics,
- state machine and overlay behavior,
- paste delivery modes and post-paste submit behavior,
- settings persistence and launchd lifecycle preferences,
- metrics parsing/rendering in settings.

Azad does not own:

- raw ASR inference internals,
- VAD model execution internals,
- EOU/TDT stitching internals.

Those are in `asr-rs`.

## 3. Repository Layout

- `src/main.rs`: startup + single-instance enforcement.
- `src/app.rs`: central runtime controller and behavior orchestrator.
- `src/interaction_sm.rs`: pure interaction reducer (hotkeys/menu -> effects).
- `src/hotkey_sm.rs`: compatibility re-exports for interaction reducer types.
- `src/platform.rs`: macOS/AppKit integration (menus, windows, overlay, global hotkeys, paste).
- `src/speech.rs`: bridge from `asr-rs` session events/controls to Azad events.
- `src/device.rs`: bridge for `asr-rs` device controller.
- `src/config.rs`: Azad defaults + `asr-rs` pipeline defaults.
- `src/preferred_store.rs`: NSUserDefaults persistence.
- `src/settings.rs`: user-facing setting enums (`PasteMethod`, `AutoSubmitMode`).
- `src/metrics_log.rs`: append/read/summarize debug metrics from logs.
- `src/single_instance.rs`: lock-file based single-instance guard.
- `docs/keyboard-shortcut-state-machine.md`: interaction source-of-truth.

## 4. Runtime Model

## 4.1 Central Event Loop

`src/app.rs` is the hub:

- receives `AppEvent` from platform callbacks and speech/device bridges,
- applies deterministic state transitions,
- pushes side effects to session/platform,
- ticks periodically through `on_tick()` via AppKit timer callback.

The app should be interpreted as:

- pure reducer (`interaction_sm`) + imperative adapter (`app.rs`) + platform I/O (`platform.rs`).

## 4.2 Session Ownership

Azad maintains at most one active `SpeechSession` at a time.
Session restart conditions include:

- startup,
- device change,
- stream fault recovery behavior.

`SpeechEvent` is filtered by `session_id` so stale session events are ignored.

## 5. Interaction State Machine

Primary source: `docs/keyboard-shortcut-state-machine.md`
Implementation source: `src/interaction_sm.rs`

Core rules:

- `Option+Space` starts/manual-assists capture regardless of global listen mode.
- `Option+Space+Space` within double-tap window toggles global listen mode.
- `Esc` cancels current actionable overlay state.
- `Enter`/numpad enter finalizes actionable overlay state.
- menu listen toggle can be deferred while turn is active and applied when turn boundary is reached.

Reducer contract:

- input: `InteractionInput`
- output: `InteractionEffect`
- no platform calls inside reducer

Adapter contract (`app.rs`):

- applies effects to session controls and overlay state.
- must preserve reducer intent without adding hidden alternate paths.

## 6. Overlay System

Azad supports two lanes:

- bottom/live lane (current draft),
- top/finalizing lane (previous turn while next speech starts).

Key behavior:

- split overlay appears only when there is meaningful live lane text or explicit divergence/hint.
- top lane can remain visible while bottom continues streaming.
- finalizing top completion must not delete active bottom lane.

Key entry points:

- `render_listening_overlay()`
- `render_finalizing_overlay_state()`
- split helpers in `app.rs` (`split_overlay_*`, `split_top_completion_for_state`).

Overlay styling and drawing primitives are in `platform.rs`.

## 7. Listen Mode and Accessibility

`always_listening_enabled` controls auto-VAD capture behavior.

Important:

- listen mode can be toggled by menu and double-tap hotkey.
- menu toggle should remain available; while active turn is in progress it may defer and apply after boundary.
- missing Accessibility permission disables listening and surfaces an overlay notice.

This ensures the app does not keep capturing speech when auto-paste cannot execute.

## 8. Finalization and Raw Mode

Standard path:

1. `Finalizing` event shows busy overlay state.
2. `FinalText` event triggers paste in normal mode.

Raw path:

- if raw is requested (`Option` modifier / raw finalize hotkey path), Azad can finalize and paste draft text immediately without waiting for final pass output.
- raw behavior uses `try_finalize_with_raw_text()`.

This path is designed to unblock user flow while preserving optional higher-fidelity finalization path for normal mode.

## 9. Paste and Output Delivery

Paste methods (`settings.rs`):

- `ClipboardPaste`
- `DirectTyping`
- `DirectTypingAndCopyClipboard`

Auto-submit modes:

- off,
- enter,
- ctrl+enter,
- shift+enter.

Implementation:

- `platform::insert_text()` executes chosen method.
- `platform::send_auto_submit()` optionally sends submit chord after successful insert.
- `app.rs::try_paste()` handles metrics logging and failure behavior.

Design constraints:

- keep behavior deterministic and observable,
- Accessibility failures should be surfaced immediately and disable listening.

## 10. Settings and Preferences

Preferences persisted in NSUserDefaults (`preferred_store.rs`):

- preferred input device,
- always listening enabled,
- run on startup,
- debug stats enabled,
- paste method,
- auto-submit mode.

Settings window:

- tabs: General, Debug
- debug tab renders metrics summary text generated from log parsing.

## 11. Debug Metrics and Quality Reporting

Azad persists metrics as append-only JSONL (`metrics_log.rs`) and reconstructs summaries on open.

Captured events include:

- turn complete durations,
- paste durations and results,
- partial finalize outcomes and reasons,
- partial-vs-full audit quality stats,
- recent transcription snapshots for debug display.

Design choice:

- avoid long-lived in-memory metrics store,
- derive debug view from logs each refresh.

## 12. Device and Recovery Behavior

Device flow:

- `device.rs` bridges `asr-rs` device controller to Azad events.
- device changes debounce before session restart.

Recovery flow:

- stream-fault signatures are classified in `app.rs`.
- repeated faults escalate from `Healthy` -> `Recovering` -> `Degraded`.
- immediate restarts are bounded; degraded mode waits for stabilization events.

## 13. Single Instance and App Lifecycle

Single instance:

- `src/single_instance.rs` lock file prevents multiple Azad instances.
- secondary launch attempts to focus existing instance.

Operational lifecycle:

- launchd label: `ai.azad`
- app bundle: `~/Applications/Azad.app`
- ownership requirement: after runtime-affecting changes, rebuild/install/restart and verify live process.

## 14. Change Playbooks

### 14.1 Change hotkey semantics

Edit:

- `docs/keyboard-shortcut-state-machine.md`
- `src/interaction_sm.rs`
- `src/app.rs` effect-application paths

Then:

- add reducer tests and adapter tests,
- validate real runtime behavior.

### 14.2 Change overlay visuals or layout

Edit:

- `src/platform.rs` rendering/layout constants and drawing code
- keep `src/app.rs` state semantics intact

Then:

- verify live behavior on multi-monitor and drag/move flows.

### 14.3 Change finalization/paste behavior

Edit:

- `src/app.rs` (`Finalizing`/`FinalText` handling, raw finalize path, `try_paste`)
- `src/platform.rs` (insert method internals)

Then:

- validate both normal and raw paths,
- validate Accessibility-failure handling.

### 14.4 Change debug metrics UX

Edit:

- `src/metrics_log.rs` (schema/summarization/rendering)
- `src/app.rs` debug event ingestion
- settings window binding in `src/platform.rs`

Then:

- verify settings debug view layout and log parsing compatibility.

## 15. Invariants

- Interaction reducer remains pure and testable.
- App adapter must not bypass reducer semantics with hidden branch logic.
- Overlay should never remain stuck visible after terminal turn completion/cancel.
- Listen toggle must not silently deadlock UI controls.
- Debug observability must not block speech/paste critical path.

## 16. Testing Strategy

- `src/interaction_sm.rs`: reducer behavior and gesture sequencing.
- `src/app.rs`: adapter + overlay/state invariants and regression tests.
- behavior docs and tests should evolve together for interaction changes.

When regressions occur:

- add test reproducing the exact event sequence first,
- fix implementation second,
- verify both unit and live runtime behavior.

## 17. Relationship to ASR-RS

ASR spec: `../asr-rs/SPECIFICATION.md`

Integration contract summary:

- Azad issues session controls and receives session events.
- Azad should treat ASR as an engine, not as a place for UI/hotkey policy.
