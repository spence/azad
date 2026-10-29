# Normal-app insertion verification — October 28, 2026

## Result

A real Google field in Brave reproduces the controlled clipboard ownership race.
The pre-fix production code inserts the replacement text and reports `Pasted`.
The deployed correction detects ownership loss and sends no paste shortcut.
This is consumer-level proof for the shared insertion path, without a Brave-specific behavior change.

The owner removed the Fluid-specific reproduction prerequisite. Neither daily desktop was an input fixture.
The exact natural trigger of each historical report remains unconfirmed.
This result does not establish that the controlled competing write caused those reports.

## Before and after in Google

The [driver](2026-10-28-insertion/brave-publication-replay.mjs) uses an owned macOS VM and real Google search field.
Native input comes from the production insertion code, not the browser debugging protocol.
Each ownership case first verifies a successful native paste into that field.
The [interferer](2026-10-28-insertion/clipboard-interferer.swift) then waits for a unique new transcript and replaces it.
The driver waits for its `armed` record before insertion and its completion before another case.

The [final capture](2026-10-28-insertion/brave-ownership-results.jsonl) contains:

| Code | Producer result | Actual Google field | Captured recipient events |
| --- | --- | --- | --- |
| Pre-fix | `Pasted` | Exact replacement text | Command-V, old paste data, and input |
| Current | `ClipboardWriteFailed` | Empty | No keyboard, paste, or input events through the 3.5-second deadline |

The producer wrapper exits nonzero in both fault cases. Before correction, only its final clipboard comparison fails.
The internal pre-fix insertion result still reports success after the wrong text reaches Google.
After correction, the internal result rejects the attempt before native shortcut output.
The recorded outcome requires both the expected producer result and the actual recipient observations.

The pre-fix probe and its three source modules are exact Git extracts from
`9853442b4f3f076415c9eec591e176e3b50eedf0`.
The [build record](2026-10-28-insertion/before-brave-build.json) identifies the files and dependency libraries.
The current probe directly includes the corrected production module from `43d5f3f`.
The only new native probe mode repeats Command-V for Brave without another clipboard write.
It preserves the VM, foreground-target, permission, and payload guards.
Both the [pre-fix](2026-10-28-insertion/before-brave-host-refusal.txt) and
[current](2026-10-28-insertion/brave-retry-host-refusal.txt) probes refuse host input before clipboard access.

This correction prevents the demonstrated wrong paste. It does not overwrite a competing clipboard owner or retry automatically.
The earlier [delivery report](2026-10-28-insertion-followup.md#delivered-correction) describes failure notices, history retention, and duplicate-event handling.

## Publication-gap hypothesis

The [private-board probe](2026-10-28-insertion/pasteboard-generation-probe.swift) uses separate processes and no general pasteboard.
Its [result](2026-10-28-insertion/pasteboard-generation-result.json) shows:

1. `clearContents` advances the generation and exposes empty types and text.
2. `writeObjects` publishes the new text at the same generation.

This establishes a platform publication boundary. It does not establish a natural insertion failure.
The deliberate pause in the guest helper exposes that boundary; it is not a proposed paste delay.

The [Brave capture](2026-10-28-insertion/brave-publication-results.jsonl) has four exact new-text matches:
`clearContents`/`writeObjects` and `declareTypes`/`setString`, each with stable and restored field focus.
The driver verifies focus before admission. Its gap check establishes host observation order, not precise guest timing.
Initial focus-admission failures came from the locked VM and are excluded from these results.

The [serialized VNC capture](2026-10-28-insertion/publication-replay-results.jsonl) also contains two exact matches.
The [raw receiver log](2026-10-28-insertion/rfb-publication-gap-final.jsonl) corroborates their receipt timestamps and zero configured commit delay.
These use the earlier instrumented RFB receiver, not a native remote editor or Fluid session.

Neither capture demonstrates stale publication-gap insertion or validates `declareTypes` as an atomic publication or prevention.
No speculative production publication change was made.

## Verification and runtime

Independent review accepted the final native probe, scripts, and bounded conclusions.
Gateway conversation: `conversation-18e2d9141d04b9c8-568147`.
Final source/ownership review: `cmsg-18e2db1d8a9d7460-574150`.
Publication review: `cmsg-18e2daec4131b768-573124`.
The two driver findings were corrected before the final native ownership rerun.
Node syntax validation and Clippy for the native probe passed.

The behavior correction remains live from `43d5f3f`; this follow-up changes only the fixture and evidence.
The prior app suite passed 312 tests, with one existing ignored test; all seven isolated interaction scenarios passed.
The earlier delivery report records install/restart/status, signing, and permission verification.
A fresh `just status` confirms PID 4626 still runs the same installed executable.
Its mapped and installed inode is `462641198`; UUID is `76DF1B33-3E5E-3798-827E-C1C411F67C15`.
This read-only runtime check does not claim a paste into a daily-driver field.

No configured delay was increased. The [typing options](../typing-cadence-investigation.md#implementation-options) remain separate research, not an unverified default change.
Raw fixture artifacts and review history remain in the declared kept evidence root:
`/Users/spencer/projects/azad/insertion-20261028`.
