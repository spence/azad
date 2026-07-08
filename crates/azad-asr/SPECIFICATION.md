# Azad ASR Specification

## 1. Purpose

`azad-asr` is the speech runtime engine used by both:

- the terminal CLI (`asr listen`, `asr transcribe-file`), and
- the Azad macOS app (via embedded session API).

It owns real-time audio ingestion, VAD gating, turn lifecycle, MLX streaming draft generation, dual-stream refinement (a second, higher-quality streaming session), and debug observability events.

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
- `src/logging.rs`: native helper logging setup hook.
- `src/ui.rs`, `src/render.rs`: TUI-only rendering path.

## 3. Core Runtime Contracts

### 3.1 High-Level Pipeline

1. Audio input is normalized/prepared to 16kHz mono chunks.
2. CoreML Silero VAD determines speech transitions.
3. MLX Nemotron generates the live streaming draft (80ms chunks) at low latency.
4. Stability tracker splits draft into committed/live segments.
5. Every chunk is also fed to a second, persistent refined streaming session (560ms chunks) running off the live thread; its deltas fold into the caption's bounded mutable tail.
6. On finalize, the refined session is flushed (cheap — no whole-turn re-decode) and its text replaces the draft.
7. If the refined session produced nothing, the live draft stands as the final text.
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
- streaming/stability/live-display state reset, and the refined session is reset for the new turn,
- renderer emits `Status(Speech)` and initial `Active`.

## 4.2 During Speech

- The live streaming chunk feed updates cumulative draft text and stability state.
- stability tracker emits committed/live split.
- each chunk is mirrored to the refined session (non-blocking); its deltas accumulate and fold into the caption's mutable tail.

## 4.3 End Conditions

A turn ends via one of:

- forced finish request,
- VAD silence + streaming/silence thresholds,
- empty-turn timeout for false-positive VAD starts,
- explicit cancel.

Finalize path:

- emit `FinalLine` with the best draft (draft-history safety net),
- emit `Finalizing`,
- flush the refined session and emit `ReplaceLine` with the refined final (or the draft if the refined session was empty).

## 5. Dual-Stream Refinement Design

`src/pipeline.rs` runs two streaming sessions concurrently so the caption stays instant while a
stronger decode sharpens it:

- **Live stream (80ms chunks):** the low-latency draft shown as the user speaks. Anti-churn is
  enforced by a bounded mutable tail — settled text never flip-flops.
- **Refined stream (560ms chunks):** a second, persistent MLX session on the finalization worker
  thread, fed the same audio continuously (`RefineChunk` per chunk, `RefineReset` per turn). Its
  deltas (`RefinedDelta`) accumulate; a token stabilizer folds them into only the caption's
  volatile tail (`LIVE_DISPLAY_MUTABLE_TAIL_TOKENS`), so in-place corrections land without
  rewriting already-read text.

Finalize is O(one chunk), not O(turn):

1. emit `FinalLine` with the live draft (draft-history safety net),
2. send `RefineFlush`; the worker flushes its own streaming tail (`RefinedFinal`) — no whole-turn
   re-decode,
3. emit `ReplaceLine` with the refined final, or fall back to the draft if the refined session
   produced nothing.

There is no text stitching across windowed re-decodes, no coverage-gap ladder, and no full-pass
bailout — the refined text is a single coherent transcript by construction, so the classes of bugs
those guards existed to catch (repeated-phrase false anchors, dropped middle clauses) cannot occur.

The live-display composition helpers (`stitch_incremental_text` and friends in
`src/pipeline/stitch.rs`) survive as the tokenizer/merge used to append the live stream's tail to
the refined text — not as a windowed-finalization stitcher.

## 6. Debug and Quality Observability

When debug stats are enabled:

- a slim recorder persists each turn's wav + sidecar (draft, refined final, live-display and EOU
  events) off the hot path, with no model re-decode,
- the draft->refined-final token divergence is emitted as the quality signal.

`DebugStatsEvent`:

- `PartialAuditResult` (tokens/edit distance/wer-like/lcp) — the draft->final divergence.

Design intent:

- Keep recording non-blocking and background-priority.
- Never block foreground capture/finalization for debug-only observability.

(`metrics_log` retains read-side parsing of the legacy `PartialFinalizeOutcome`/`PartialAuditError`
records so historical logs stay summarizable; those events are no longer emitted.)

## 7. Threading and Performance Decisions

Explicit QoS separation:

- live pipeline thread: user-interactive.
- refined-stream / finalization worker: user-initiated (user-visible latency path).
- debug recorder: background.

Other important choices:

- avoid blocking live capture for refined inference,
- keep the refined feed non-blocking (drop on a full channel rather than stall the live thread),
- keep capture responsive even under session control changes.

## 8. Configuration Surface

`PipelineConfig` governs VAD/streaming/finalization behavior.
Key classes of knobs:

- VAD thresholds and start confirmation,
- silence/end-of-turn thresholds,
- live/final MLX chunk sizes (the refined session runs at the final chunk size),
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

### 9.2 Change refinement / live-display composition behavior

Primary files:

- `src/pipeline.rs` (`feed_eou` refined feed, `finish_turn_dual_stream`, `apply_refined_delta`, `emit_replacement_live_display`, the stabilizer + `LIVE_DISPLAY_MUTABLE_TAIL_TOKENS`).
- `src/pipeline/stitch.rs` (`stitch_incremental_text` — live-display tail composition only).

Validate:

- refined final quality (no dropped/duplicated clauses),
- caption anti-churn (rollback ≤ mutable tail; no large swaps of settled text),
- finalize latency (flush stays O(chunk)).

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
- Debug recording must remain optional and non-blocking.
- The live caption must never rewrite settled text beyond the bounded mutable tail.

## 11. Testing Guidance

- Unit tests in `src/pipeline.rs` cover live-display composition and the stabilizer's mutable-tail behavior.
- Pinned-fixture regressions live in `tests/replay.rs` (run with `--ignored`, models on disk).
- Integration behavior should be validated through Azad and CLI paths.
- Regressions should add tests near the logic that enforces the invariant (not only at outer wrappers).

## 12. Relationship to Azad

Azad spec: `../azad/azad/SPECIFICATION.md`

Boundary summary:

- `azad-asr`: audio+ASR runtime and event stream.
- `azad`: interaction state machine, overlay UX, settings, paste workflows, launch lifecycle.
