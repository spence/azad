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

The harness uses these production sources directly:

- `azad_capture::policy`, the key policy the capture helper applies to every key edge.
- `src/key_context.rs`, which derives the helper's key context from Azad's overlay, history and
  search surfaces.
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
state. `passed_through` marks a key the policy does not claim, which reaches the focused app.
Built-in scenarios cover immediate manual-hold overlay, spoken-hold finalization, double-tap
listen toggle, history entry/navigation, cancellation, Enter-finalize cleanup, always-listening
VAD assist, Shift+Enter pass-through, keypad Enter, claimed releases after the overlay closes,
auto-repeat, history search typing, ordinary typing, and delayed delivery.

`key_down.at_ms` is the capture time the helper stamps; `delivered_at_ms` is when the app
handles it. With `"clock":"delivery"` in `initialize`, the reducer sees delivery time instead,
which is the negative control for delayed delivery: two holds a second apart that are delivered
two milliseconds apart turn into a double tap only on the delivery clock.

## Safety and Scope

The harness intentionally does not test device capture, pixel rendering, the physical
microphone, or paste delivery into another application; device capture and delivery are
verified in a disposable VM by `crates/azad-capture/tests/vm/run_matrix.py`. Those capabilities would cross the process
boundary and interfere with the user's session. Validate their pure routing and rendering logic in
unit or snapshot tests, validate ASR through the standalone `asr` binary, and limit installed-app
verification to deployment and read-only process health. Never validate by posting synthetic input
into the active desktop.
