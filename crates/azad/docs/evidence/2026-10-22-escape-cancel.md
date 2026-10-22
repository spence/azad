# Escape cancellation — 2026-10-22

Proof for MS-ESC-CANCEL.T1 and MS-ESC-CANCEL.G1.

## Recorded failure and reproduction

The last dictation was turn 24. These records came from
`~/Library/Logs/Azad/input.log`. Times use PDT. State snapshots precede event handling.

| Time | Event | Overlay visible | Finalizing turn |
| --- | --- | --- | --- |
| 14:31:38.459 | `overlay_cancel` | true | none |
| 14:31:38.468 | `engine_speech_finalizing`, turn 24 | false | none |
| 14:31:39.536 | `overlay_cancel` | true | 24 |

The engine log also records a queued draft for turn 24 after the first Escape.
Draft suppression was active, but `Finalizing` restored the overlay 9 ms after cancellation.
Both dismissal events were Escape cancellations, not Enter finalization.

Baseline: `fd3a40b`. The first app-controller replay stopped at a native AppKit window call.
Test-only guards then disabled window creation and live modifier reads.
Before the production correction, this command reproduced the recorded failure:

```text
CARGO_BUILD_JOBS=2 cargo test -q -p azad --bin azad escape_keeps_overlay_closed_when_cancelled_turn_finalizes_late -- --test-threads=2

late Finalizing restored the cancelled overlay
test result: FAILED. 0 passed; 1 failed; 0 ignored
```

## Correction and regression proof

Escape advances the turn acceptance floor past the discarded turn.
A retained finalizing lane remains eligible for completion.
The regression tests exercise the production app controller with an in-memory speech session.
They cover queued drafts, tentative finalization, final text, suppression expiry, new dictation,
and cancellation of a live lane beside an older finalizing lane.
Discarded output creates no insertion request or transcript history entry.

All commands ran on `catalyst.local`. Cargo used two build jobs; unit tests used two threads.

| Command | Result |
| --- | --- |
| `cargo fmt --all --check` | Passed |
| `git diff --check` | Passed |
| `CARGO_BUILD_JOBS=2 cargo test -q -p azad -- --test-threads=2` | 305 passed; one existing ignored test; 7.46 seconds |
| `CARGO_BUILD_JOBS=2 just interaction-test` | Seven scenarios passed, including the harness safety checks |

No synthetic keyboard or mouse events were posted.
The replay did not use the installed app, microphone, input-device selection, or saved preferences.

## Installed runtime

`AZAD_SKIP_MLX_HELPER=1 CARGO_BUILD_JOBS=2 just install` completed successfully.
The unchanged installed MLX helper was retained. The release build took 1 minute 36 seconds.
`just restart` and `just status` then completed successfully.
Before restart, process inspection found only Azad, ASR helpers, and defunct children in its process tree.

```text
state = running
program = /Users/spencer/Applications/Azad.app/Contents/MacOS/azad
previous PID = 61180
updated PID = 12931
process start = 2026-10-22 14:44:03 PDT
```

`lsof` confirmed that PID 12931 loaded the installed executable.
`dwarfdump --uuid` returned the same ARM64 UUID for the release and installed executables:
`FACE1331-05A7-3B85-8F6B-F1B176AAC8CD`.
Installed executable SHA-256:
`d9f362a05618e372b8710723bca5bd46a0267d0cd3e1183c67aa514ab659a121`.

Runtime verification used lifecycle commands and read-only process inspection, not desktop interaction.
