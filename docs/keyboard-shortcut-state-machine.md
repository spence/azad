# Keyboard Shortcut State Machine

This document defines the keyboard shortcut behavior in Azad, including how
global VAD mode and manual hold-to-talk interact.

It is the source of truth for expected behavior during future changes.

## Scope

- Hold-to-talk hotkey: `Option+Space` press/release
- VAD toggle gesture: `Option+Space` double tap
- Manual finalize key: Enter / Numpad Enter
- Overlay lifecycle and paste behavior

Raw mode badges/formatting are out of scope here.

## Design Boundary

`asr` does not know about keyboard shortcuts.

`azad` translates keyboard gestures into `SpeechSession` controls:

- `start_or_resume_manual_hold()`
- `release_manual_hold()`
- `set_auto_vad_enabled(bool)`
- `set_capture_enabled(bool)`
- `finalize_current_turn()`
- `cancel_current_turn()`

## State Model

Primary state flags in `src/app.rs`:

- `always_listening_enabled`: global VAD mode (`on`/`off`)
- `manual_hold_active`: whether hold key is currently down
- `release_should_finalize_turn`: whether current key release should force finalize
- `manual_finalize_pending`: previous manual finalize requested, waiting on turn completion
- `engine_state` + `finalizing_turn_id` + `latest_draft`: active-turn context
- `last_hold_hotkey_pressed_at`: double-tap detector
- `last_pasted_turn_id`: dedupe paste per turn id (not per app session)

Derived term used by routing logic:

- `has_turn_context`:
  - speech is active, or
  - finalizing is active, or
  - there is visible draft text, or
  - manual finalize is pending

## Rules

1. Single press starts/continues manual hold.
2. Releasing hold finalizes if `release_should_finalize_turn` is true.
3. Double tap toggles VAD only when:
  - VAD is currently on, or
  - VAD is off and there is no active turn context.
4. VAD-off, rapid release/re-press must allow multiple consecutive pastes:
  - prior turn finalizes/pastes,
  - next turn remains visible and active,
  - next release also finalizes/pastes.
5. Paste dedupe is per turn id, not per process session.
6. If a finalize arrives while another hold is active, do not hide overlay for the active hold.

## Transition Table

### A. VAD OFF (manual mode)

| Start | Event | Action | Result |
|---|---|---|---|
| idle | press hold | start manual hold | overlay visible, draft starts |
| holding speech | release hold | finalize current turn | paste turn N when final arrives |
| finalize pending from turn N | press hold quickly again | keep pending finalize context, start new hold | turn N can still paste; turn N+1 visible/live |
| holding turn N+1 | release hold | finalize turn N+1 | paste turn N+1 |

Expected: each released segment pastes independently.

### B. VAD ON (always listening)

| Start | Event | Action | Result |
|---|---|---|---|
| active VAD turn | press hold | manual assist on same turn | continue capture through pauses |
| assisted VAD turn | release hold | release assist only | VAD logic continues |
| active VAD turn | double tap | disable VAD immediately, continue as manual while key held | release of second tap finalizes/pastes |

### C. Pure VAD Mode Toggle

| Start | Event | Action | Result |
|---|---|---|---|
| VAD OFF, no turn context | double tap | enable VAD | no manual turn started |
| VAD ON, no turn context | double tap | disable VAD | no manual turn started |

## Overlay + Paste Contract

- Overlay is shown while actively capturing or finalizing current visible turn.
- When a finalized turn pastes during an active new hold, overlay stays visible.
- Overlay hide is suppressed during active hold to avoid dropping live feedback.
- `Esc` cancels current turn and closes overlay.
- Enter finalizes only when overlay/session context exists.

## Implementation Pointers

Main routing:

- `handle_hotkey_pressed`
- `handle_hotkey_released`
- `handle_finalize_hotkey_pressed`
- `apply_always_listening_toggle`

Finalize/paste handling:

- `SpeechEvent::Finalizing`
- `SpeechEvent::FinalText`
- `SpeechEvent::SessionEnded`

Reset paths:

- `reset_turn_state`
- `handle_overlay_cancel`

## Regression Checklist

Before merging keyboard logic changes, verify:

1. VAD off: release/paste, quick repress, release/paste again.
2. VAD off: double tap toggles on without starting/sticking overlay.
3. VAD on: active speech + double tap transitions to manual hold and finalizes on release.
4. Active second hold does not get hidden by first turn's final paste.
5. No turn pastes more than once.
