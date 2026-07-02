# Azad ASR Specification

## 1. Purpose

`azad-asr` is the speech runtime engine used by both:

- the terminal CLI (`asr listen`, `asr transcribe-file`), and
- the Azad macOS app (via embedded session API).

It owns real-time audio ingestion, VAD gating, turn lifecycle, MLX streaming draft generation, incremental/final MLX refinement, and debug observability events.

It intentionally does **not** own:

- global hotkeys,
- overlay UI,
- menu/settings semantics,
- paste/typing behavior.

Those concerns live in Azad.

## 2. Repository Layout

- `src/main.rs`: CLI entrypoint (`devices`, `listen`, `transcribe-file`).
- `src/lib.rs`: exported crate modules.
- `src/pipeline.rs`: core streaming engine and refinement pipeline.
- `src/render.rs`: renderer event contract.
- `src/embed/mod.rs`: embedding API used by Azad (`spawn_session`, `SessionControl`, `SessionEvent`).
- `src/audio/`: input abstractions and implementations.
  - `cpal_input.rs`: live mic capture with pause/resume and health counters.
  - `wav_input.rs`, `decoded_input.rs`: file-based sources.
- `src/devices/mod.rs`: input-device discovery/controller infrastructure.
- `src/stability.rs`: stable-prefix tracking for streaming draft text.
- `src/thread_qos.rs`: QoS policy helpers for foreground/background worker separation.
- `src/logging.rs`: native whisper/ggml log suppression or enablement.
- `src/ui.rs`, `src/render.rs`: TUI-only rendering path.

## 3. Core Runtime Contracts

### 3.1 High-Level Pipeline

1. Audio input is normalized/prepared to 16kHz mono chunks.
2. VAD (`WhisperVadProcessor`) determines speech transitions.
3. MLX Nemotron generates streaming draft text at low latency.
4. Stability tracker splits draft into committed/live segments.
5. During speech, incremental MLX finalization slices refine text in background.
6. On finalize, assembled incremental text is preferred.
7. If incremental output is unavailable or unsafe, whole-turn MLX finalization is used.
8. Renderer emits state/text/debug events to embedding app or TUI.

### 3.2 Event Surface (`src/render.rs`)

`RenderEvent` is the canonical output contract:

- status/engine state (`Status`, `SpeechStartedByVad`)
- live meter/health (`Meter`, `CaptureHealth`)
- live/final text (`Active`, `Finalizing`, `FinalLine`, `ReplaceLine`)
- debug observability (`DebugStats`)
- errors (`Error`)

Azad relies on these semantics, especially:

- `Active` as streaming draft updates,
- `Finalizing` as boundary between capture and finalize UX,
- `ReplaceLine` as final text for the turn.

### 3.3 Embedded Session API (`src/embed/mod.rs`)

Azad interacts through:

- `SessionControl`: hold start/release, auto-VAD toggle, capture toggle, force finish, cancel turn/session, debug toggle.
- `SessionEvent`: mapped from `RenderEvent` with session IDs.

Design intent:

- Keep embed API explicit and imperative.
- Keep hotkey semantics out of engine.
- Allow Azad to control capture lifecycle without tearing down process-level runtime.

## 4. Turn Lifecycle

## 4.1 Start Conditions

Turn starts when either condition is met:

- manual force start (`PipelineControls::request_force_start`) from host app, or
- auto-VAD start gate is satisfied (`vad_start_chunks`, confidence overrides).

On start:

- pre-roll is prepended,
- turn ID increments,
- streaming/stability/incremental state reset,
- renderer emits `Status(Speech)` and initial `Active`.

## 4.2 During Speech

- `feed_eou()` updates cumulative draft.
- stability tracker emits committed/live split.
- incremental slices are periodically scheduled when enough new audio exists.
- speculative finalize can be scheduled at silence transitions (non-incremental mode path).

## 4.3 End Conditions

A turn ends via one of:

- forced finish request,
- VAD silence + streaming/silence thresholds,
- empty-turn timeout for false-positive VAD starts,
- explicit cancel.

Finalize path:

- emit `FinalLine` with best draft,
- emit `Finalizing`,
- enqueue final output path (incremental assembled text preferred, full pass fallback).

## 5. Incremental Refinement and Fallback Design

`src/pipeline.rs` intentionally optimizes for low-latency live behavior:

- Heavy finalization inference runs off the live capture thread.
- Incremental segments are stitched to avoid re-running full pass by default.
- A tail-plan guard (`finalize_tail_plan`) decides if explicit tail segment is required.
- Finalization output priority:
  1. assembled incremental output,
  2. draft output,
  3. full pass bailout (with reason).

This is tracked in `PartialFinalizeOutcome`:

- `Assembled`
- `DraftEmit`
- `FullPassBailout(reason)`

Reasons are expected and diagnostic, but frequent bailouts in normal usage indicate regression or load pressure.

## 6. Debug and Quality Observability

When debug stats are enabled:

- partial outcome events are emitted,
- partial-vs-full audit worker computes quality metrics,
- text traces for partials/emitted/full can be logged.

`DebugStatsEvent` includes:

- `PartialFinalizeOutcome`
- `PartialAuditResult` (tokens/edit distance/wer-like/lcp)
- `PartialAuditError`

Design intent:

- Keep audits non-blocking and background-priority.
- Never block foreground capture/finalization for debug-only validation.

## 7. Threading and Performance Decisions

Explicit QoS separation:

- live pipeline thread: user-interactive.
- finalization worker: user-initiated (user-visible latency path).
- partial-audit worker: background.

Other important choices:

- avoid blocking live capture for final inference,
- keep only latest speculative jobs (drop stale),
- keep capture responsive even under session control changes.

## 8. Configuration Surface

`PipelineConfig` governs VAD/streaming/incremental/finalization behavior.
Key classes of knobs:

- VAD thresholds and start confirmation,
- silence/end-of-turn thresholds,
- incremental cadence/overlap/context/wait timings,
- model paths (VAD + MLX Nemotron).

Host applications (Azad/CLI) are expected to provide coherent defaults. Azad default values live in `azad/src/config.rs`.

## 9. Change Playbooks

### 9.1 Change live speech start/end behavior

Primary files:

- `src/pipeline.rs` (`on_chunk`, `start_turn`, silence gating, timeout logic).

Validate:

- no capture stalls,
- no early cutoffs,
- no stuck speech state.

### 9.2 Change partial stitching/fallback behavior

Primary files:

- `src/pipeline.rs` (`maybe_schedule_incremental_slice`, `submit_incremental_final_pass`, `stitch_incremental_text`, `finalize_tail_plan`).

Validate:

- assembled text quality,
- bailout rate,
- tail coverage correctness.

### 9.3 Change embed integration contract

Primary files:

- `src/embed/mod.rs`, `src/render.rs`.

Rules:

- Keep event semantics stable unless Azad contract is updated in lockstep.
- Avoid hidden behavior changes in `SessionControl` methods.

### 9.4 Change capture behavior

Primary files:

- `src/audio/cpal_input.rs`,
- `src/devices/mod.rs`.

Rules:

- preserve pause/resume capture semantics (`capture_enabled`),
- maintain stream error propagation and health metrics.

## 10. Invariants

- Hotkey semantics must not leak into engine logic.
- Engine must remain usable from non-Azad hosts.
- Finalization must always produce deterministic turn completion events.
- Debug auditing must remain optional and non-blocking.
- Full-pass fallback is a safety net, not the default happy path.

## 11. Testing Guidance

- Unit tests in `src/pipeline.rs` cover stitching/tail-plan behavior.
- Integration behavior should be validated through Azad and CLI paths.
- Regressions should add tests near the logic that enforces the invariant (not only at outer wrappers).

## 12. Relationship to Azad

Azad spec: `../azad/azad/SPECIFICATION.md`

Boundary summary:

- `azad-asr`: audio+ASR runtime and event stream.
- `azad`: interaction state machine, overlay UX, settings, paste workflows, launch lifecycle.
