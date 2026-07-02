# Quality Plan

This plan keeps cleanup safe by separating behavior-preserving guardrails from larger refactors.

## Phase 1: Source Install and Documentation Guardrails

- Keep public source installs free of private forks and submodules.
- Keep local signing optional through `.codesign.env`.
- Verify Rust workspace tests, formatting, Clippy, and Swift helper build from root commands.
- Keep root and crate READMEs linked to the active docs.
- Remove obsolete historical model/fork investigations from the active tree.

## Phase 2: Runtime Module Boundaries

- Split `crates/azad/src/platform.rs` by platform concern: hotkeys, paste, overlay, settings, history, onboarding.
- Split `crates/azad/src/app.rs` by behavior concern: interaction adapter, overlay state, paste flow, history flow, gateway flow, recovery.
- Split `crates/azad-asr/src/pipeline.rs` by engine concern: turn lifecycle, VAD gate, incremental finalization, stitcher, worker orchestration, debug audit.

Do these extractions in small commits with tests passing after each move. The goal is clearer ownership with no behavior change.

## Phase 3: Helper Protocol Hardening

- Define typed Rust request/response structs for the helper protocol.
- Add Swift helper smoke tests for argument parsing and protocol errors.
- Keep the current JSON-line protocol until measurement proves it is the bottleneck.

## Phase 4: Performance Regression Coverage

- Add explicit checks for idle CPU behavior, VAD cold-start timing, first-token latency, and finalization fallback rate.
- Keep model-dependent replay tests opt-in locally, but provide a maintainer command that fails if models are absent.
