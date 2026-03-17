# Charter
Runtime Agent Design delivers low-latency transcription with deterministic hotkey/listen-mode behavior and predictable overlay/paste UX in the Azad macOS app.

Active milestones:
- Idle VAD Silence Compute Reduction
- Root Rust Workspace Cutover

Completed milestones:
- Debug Recent Quality Session Association
- Tail Finalization Timing Telemetry
- Paste Trailing Space Toggle
- CoreAudio Input Range Guard
- Debug Audit Quality Reliability
- Listen Toggle Notice Visual Alignment
- Listen Toggle Overlay Feedback

# Milestones
## Debug Recent Quality Session Association
- [x] Prevent recent transcription quality rows from reusing stale audit scores across app sessions.
- [x] Use per-row nearest-in-time matching for audit/error/finalize records so `turn_id` reuse cannot mislabel quality.
- [x] Keep summary aggregates accurate when `turn_id` values repeat after restarts.

## Idle VAD Silence Compute Reduction
- [o] Skip VAD model inference for hard-silence idle chunks while preserving start-gating behavior.
- [o] Validate live always-listening idle CPU after redeploy to confirm the baseline drops.

## Root Rust Workspace Cutover
- [o] Build Azad from repository root via a top-level Cargo workspace and root `just` workflow.
- [o] Fix crate pathing and model path defaults so the new `crates/*` layout works without old directory assumptions.
- [o] Update setup docs/scripts to root-first commands and `crates/*` paths.

## Tail Finalization Timing Telemetry
- [x] Emit debug-only timing logs for tail incremental enqueue, completion, timeout, and late dropped results.
- [x] Include wait budget and measured latency in tail telemetry so timeout analysis is measurable from logs alone.

## CoreAudio Input Range Guard
- [x] Prevent invalid CoreAudio buffer metadata from crashing the input stream setup path.
- [x] Fall back to backend-default buffer sizing when device-reported ranges are invalid.
- [x] Add unit coverage for buffer range selection edge cases.

## Debug Audit Quality Reliability
- [x] Prevent queued partial-audit jobs from being silently dropped when debug toggle state changes.
- [x] Surface partial-audit enqueue failures in metrics so recent rows do not show unexplained missing quality.
- [x] Show explicit `queued` / `error` quality states for recent rows when audit data is pending or failed.

## Paste Trailing Space Toggle
- [x] Ship a user-visible settings toggle to control whether pasted transcripts append a trailing space.
- [x] Persist trailing-space preference across launches and reflect it in the settings UI state.
- [x] Gate paste text construction on the new toggle so disabled mode preserves exact transcript ending.

## Listen Toggle Notice Visual Alignment
- [x] Ship unified single-line listen toggle notices with uppercase state labels (`Listen ENABLED` / `Listen DISABLED`).
- [x] Keep listen toggle notice title centered in the overlay for both states.
- [x] Remove listen-toggle accessory hints (keycaps/chips) so the notice shows only the centered title.
- [x] Validate live runtime behavior for notice transitions after install/restart/status verification.

## Listen Toggle Overlay Feedback (Completed)
- [x] Add colorful Pulse Banner visualization for global listen enable/disable notices.
- [x] Use green/cyan theme for listen ON and amber/orange theme for listen OFF.
- [x] Animate medium-intensity pulse + subtle wave glow over a 600ms notice window.
- [x] Keep all non-listen notices on the standard neutral notice style.
- [x] Render listen-off hotkey hint using key symbol combo with outlined keycaps (`[⌥] + [Space]`) instead of plain `Option+Space` text.
