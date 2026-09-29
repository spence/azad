# Isolated Interaction Harness

`azad-interaction-harness` validates Azad's shortcut-to-overlay interaction path without touching
the installed app or the active macOS desktop. It is a separate headless process that accepts
process-local JSONL events and emits JSONL actions and state snapshots.

Run the regression scenarios from the repository root:

```bash
just interaction-test
```

The command verifies the harness safety boundary before running scenarios. It fails if the binary
links desktop, input, accessibility, or audio frameworks, or imports known global-input and UI
symbols. The harness itself reports that it does not register hotkeys, post CoreGraphics events,
open AppKit windows, access the microphone, read or write user defaults, or paste text.

## Production Logic Boundary

The harness compiles these production sources directly:

- `src/platform/hotkeys.rs` for raw Space and hold-plus-Up classification.
- `src/interaction_sm.rs` for gesture timing and interaction transitions.

A headless recording backend applies the resulting effects to in-memory capture, overlay, history,
finalize, cancel, and paste-request state. This gives the shortcut pipeline a deterministic test
surface without duplicating the key classifier or reducer. App-controller unit tests cover the
production adapter around that core. In test builds, preference access resolves to built-in
defaults without opening `NSUserDefaults`, transcript history is in-memory, and paste/auto-submit
helpers cannot post input. Standalone `asr` tests cover the transcription engine.

## Commands

```bash
target/debug/azad-interaction-harness describe
target/debug/azad-interaction-harness self-test
target/debug/azad-interaction-harness run [events.jsonl]
```

`run` reads standard input when no path is supplied. Event timestamps must be monotonic. For
example:

```jsonl
{"type":"initialize","at_ms":0,"always_listening_enabled":false,"history_entries":3}
{"type":"key_down","at_ms":1000,"key":"space","modifiers":["option"]}
{"type":"speech_draft","at_ms":1200,"text":"hello"}
{"type":"key_up","at_ms":1500,"key":"space","modifiers":["option"]}
{"type":"speech_finalized","at_ms":1700,"text":"hello"}
```

Every input produces one output object containing the interpreted actions and complete recorded
state. Built-in scenarios cover immediate manual-hold overlay, spoken-hold finalization,
double-tap listen toggle, history entry/navigation, cancellation, Enter-finalize cleanup, and
always-listening VAD assist.

## Safety and Scope

The harness intentionally does not test macOS hotkey registration, pixel rendering, the physical
microphone, or paste delivery into another application. Those capabilities would cross the process
boundary and interfere with the user's session. Validate their pure routing and rendering logic in
unit or snapshot tests, validate ASR through the standalone `asr` binary, and limit installed-app
verification to deployment and read-only process health. Never validate by posting synthetic input
into the active desktop.
