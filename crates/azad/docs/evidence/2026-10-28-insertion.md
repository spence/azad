# Missing or stale insertion — October 28, 2026

## Result and limit

The two reported turns are present in the logs. The rare failure is **not reproduced or fixed**.
The user identified a Google text field in Brave on this Mac, without remote desktop.
The user also reported that manual Command-V worked afterward.
This narrows the investigation toward the shortcut/recipient boundary. It does not establish
the exact clipboard state at the time of the failed shortcut.

No paste-delay increase, retry, typing-rate change, or speculative behavior fix is included.
The change adds a VM-only native insertion probe and debug metadata for the next occurrence.
The unresolved defect remains open in `MS-INSERTION-RELIABILITY.T1`.

## Historical evidence

The October 28 records in `~/Library/Logs/Azad/stderr.log` show:

| Turn | Local time (PT) | Recognized text starts with | Insertion result |
| --- | --- | --- | --- |
| 103 | 1:52:06–1:52:07pm | Does it require a special… | `clipboard_paste`, 136 characters, `pasted`, 152 ms |
| 108 | 1:55:07–1:55:08pm | These are typically difficult things… | `clipboard_paste`, 123 characters, `pasted`, 151 ms |

Both records have auto-submit off. Finalizing timestamps are `1793220726880` and
`1793220907851` ms since the Unix epoch. Turn numbers repeat after app restarts;
the timestamp and text identify the reported records.

```text
AZAD_FINALIZING_RECV ts_ms=1793220726880 turn_id=103 draft_chars=137 overlay_visible=true prior_finalizing_turn_id=None
AZAD_INSERT_PERF turn_id=103 transcript_mode=normal method=clipboard_paste chars=136 result=pasted insert_ms=152 auto_submit_mode=off auto_submit_sent=false auto_submit_ms=0 auto_submit_ok=true total_ms=152
AZAD_FINALIZING_RECV ts_ms=1793220907851 turn_id=108 draft_chars=121 overlay_visible=true prior_finalizing_turn_id=None
AZAD_INSERT_PERF turn_id=108 transcript_mode=normal method=clipboard_paste chars=123 result=pasted insert_ms=151 auto_submit_mode=off auto_submit_sent=false auto_submit_ms=0 auto_submit_ok=true total_ms=151
```

There are no logged Enigo or clipboard-write errors for these two insertions.
These records establish producer completion, not Brave receipt. Direct typing is not the path
used by these records.

## Bounded reproductions

### Private pasteboard

The [Swift probe](2026-10-28-insertion/pasteboard-probe.swift) uses only a unique named
pasteboard. It does not use the general pasteboard or post input events.
It performs 1,000 immediate write/read comparisons and 12 child-process reads, with no added delay.

```text
private_pasteboard_writes=1000 immediate_mismatches=0 cross_process_reads=12 cross_process_mismatches=0 added_delay_ms=0 elapsed_ms=1126.3525
```

This does not exercise native shortcut delivery or Chromium's clipboard handling.

### Native Brave recipient

The [native probe](../../src/bin/azad_insertion_probe.rs) directly compiles production `paste.rs`.
It requires an explicit input flag, `VirtualMac` hardware, Brave in the foreground, and
event-posting permission before insertion. It does not use Azad app state, audio, or preferences.
The hardware refusal also passed on the host, before clipboard or event access.

The owned macOS 26.6.2 VM ran with audio and clipboard sharing disabled, 2 CPUs, and 4 GB RAM.
Brave 1.98.60 (`156.1.98.60`) used a clean guest profile and a real Google search field.
The [driver](2026-10-28-insertion/brave-replay.mjs) uses the guest's isolated CDP endpoint for
page focus and observations. Native Command-V comes from the production insertion recipe,
not CDP. The driver verifies field focus, final field contents, paste data, and keyboard/input events.
It does not equate the producer's JSON result with receipt.

The driver is an evidence snapshot, not a general desktop-control tool. Its guest IP, tunnel,
known-hosts path, and standard fixture credentials belong to this owned VM.
Do not point it at the host browser. The native binary independently refuses host input.

The [final JSONL results](2026-10-28-insertion/brave-results.jsonl) contain six scenarios:

| Scenario | Observed recipient result |
| --- | --- |
| First reported text | Exact new text received |
| Second reported text | Exact new text received |
| 50 ms browser keydown stall | Exact new text received |
| 250 ms browser keydown stall | Exact new text received |
| 1,000 ms browser keydown stall | Exact new text received after the stall |
| Page rejects one Command-V keydown | Empty field; producer still reports `Pasted`; detector reports the mismatch |

The last case is a controlled negative case, **not the trigger of the historical failure**.
It proves that the receiver detector can distinguish a missing insertion from producer success.
It does not prove that Google rejected either reported shortcut.
No admitted scenario inserted the prior transcription.

The 1,000 ms stall outlasted the producer's approximately 213 ms completion.
The new text still arrived. Thus, this stall did not reproduce text loss after source cleanup.
Earlier admitted native trials also succeeded without the new handoff metadata.

Initial trials lacked foreground admission because the guest Setup Assistant had focus.
Those are fixture failures, not Azad failures. They are excluded from the recipient results.
An orphan Return-key-up experiment produced no corresponding DOM events and was removed.
It does not establish anything about the user's physical Enter sequence.
A CDP-only recovery shortcut also failed to paste; that control was removed rather than
treated as a native Command-V result. The final driver has no recovery claim.

## Diagnostic change

With the existing debug flag enabled:

- `AZAD_PASTE_HANDOFF` records clipboard-write, pre-shortcut, and post-shortcut timestamps,
  change counts, foreground bundle IDs, permission state, and expected character counts.
- `AZAD_PASTE_HID` records Azad-tagged Command/V keyboard events at Azad's own HID tap.
  The events still pass through unchanged. This is not a receiving-app acknowledgment.

The production diagnostics do not read or log clipboard contents.
Configured waits and shortcut order are unchanged. Debug queries and synchronous log output
add execution overhead; these are not zero-overhead observations.
The native probe demonstrated the handoff records. Host HID observations were not driven
with synthetic input.

Source inspection also found that `send_command_v` logs native recipe errors without returning
them to `insert_text`. This reporting defect is recorded separately as
`ISS-CLIPBOARD-INSERTION-REPORTS-SUCCESS-AFTE`. No such errors appear in the reported turns,
so it is not their established cause and is not silently treated as their fix.

## Verification

- App suite: 305 passed, 1 existing ignored test.
- `just interaction-test`: all 7 scenarios passed, including the headless safety boundary.
- Clippy: app, native insertion probe, and existing typing probe passed with `-D warnings`.
- Independent source review: Codex `gpt-6.1-sol`, high effort, accepted the diagnostic code.
  Review conversation: `conversation-18e2ce375718c920-545288`.
- The same reviewer accepted the final driver, raw observations, and evidence distinctions.
  Requests: `cmsg-18e2cfe70e624150-548777` and `cmsg-18e2d02040285678-549747`.

### Live diagnostic deployment

The canonical commands completed:

```sh
AZAD_SKIP_MLX_HELPER=1 CARGO_BUILD_JOBS=2 \
DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer just install
just restart
just status
```

The unchanged MLX helper was retained. The process scope was inspected before lifecycle actions:
Azad hosted only its ASR helpers and zombie children, not an agent host. The legacy service was absent.
At 2:54pm PT, status reported `ai.azad` running as PID 53175. Startup logs reported Accessibility,
Microphone, and Input Monitoring granted, and the hotkey tap installed.
The built and installed executable UUIDs matched:

```text
UUID: 8126DDC4-8F62-3D1D-B7CE-DB36FF9A66D8 (arm64)
PID 53175 mapped inode: 462502239
Installed executable inode: 462502239
```

`codesign --verify --deep --strict` passed. The designated requirement retains identifier
`ai.azad` and Micro team `35A87BDK48`; it is not an ad-hoc identity.
This proves deployment and process identity, not successful insertion into the host's Brave field.
The owned VM and its SSH tunnel were stopped after evidence collection. Its retained disk and
the Gateway conversations were not deleted.

The [typing-cadence investigation](../typing-cadence-investigation.md) separates source timing,
native measurements, display cadence, and inference. No typing architecture changed.

Detailed local logs and the stopped-VM source artifacts are retained under
`/Users/spencer/projects/azad/insertion-20261028`, with a keep declaration.
The host's installed app, microphone, devices, and preferences were not test controls.
