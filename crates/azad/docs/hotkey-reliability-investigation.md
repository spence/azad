# Hotkey reliability investigation

Investigation: 2026-09-28. Source baseline: `da639bc`.
Status: recommendation, not an implemented or fully verified application fix.

## Finding

Keep the existing keyboard shortcuts and interaction semantics unchanged.
The strongest practical recommendation within Azad's current app architecture is
one shared shortcut handler supplied by an active HID event tap and registered
system hotkeys (`RegisterEventHotKey`). They cover different failure conditions;
the registration path is not redundant with the tap.

A tap that exists and reports enabled can still receive no keyboard events.
Secure Input is one demonstrated cause. Recreating the tap does not remove that
system-wide gate. Another application can also install an earlier consuming tap.
Neither API provides unconditional priority over every other application.

This investigation changes no application code, bindings, host preferences or
permissions, or installed runtime. No input was injected into the user's desktop. Runtime
experiments used a separate disposable macOS VM, stopped and deleted afterward.

## Read-only observations on the affected Mac

- Host: catalyst, macOS 26.5; Azad PID 70787, started at 17:59 local time.
- During the reported failure, `IsSecureEventInputEnabled()` returned true.
  The console-session `kCGSSessionSecureInputPID` was 687, the running Jump Desktop
  process. Brave was foreground during an observation: the owner need not be
  the frontmost application.
- Azad's HID tap was present and enabled. A process sample showed its tap thread
  waiting in the CFRunLoop and the main AppKit run loop alive, rather than an
  observed application mutex deadlock.
- The source has only the tap-based capture path. Commit `da639bc` removed Carbon
  registrations while adding tap maintenance. That removal eliminated a mechanism
  that can receive shortcuts during Secure Input.
- A later read-only check returned Secure Input false; Jump and Azad still had
  the same PIDs and start times. The investigation did not release Secure Input,
  change Jump settings, or restart either app. No controlled user-input test was
  performed to establish recovery of every shortcut.
- The input log also showed repeated `stale_latency` tap recreations, reaching
  generation 8 during this run. Those observations do not establish why latency
  was high or prove that the recreations restored delivery. They should not be
  conflated with the independently observed Secure Input gate.

Apple documents that Secure Input can block keyboard interception system-wide,
including when the process enabling it is in the background:
[TN2150](https://developer.apple.com/library/archive/technotes/tn2150/_index.html).
The owner PID is diagnostic evidence, not a supported control for forcibly
disabling another application's security state.

## Isolated macOS API evidence

The disposable VM ran macOS 26.6.2, with no host audio or clipboard sharing. A
small Objective-C probe refused execution unless `hw.model` began with
`VirtualMac`. Guest-only test processes supplied a foreground key sink, an active
HID tap, Carbon registrations, a Secure Input owner, and a competing consuming
tap. All were finite-lived. Guest permissions were granted only to test tools.

These were mechanism tests, not a modified Azad build or an end-to-end test of
the real app. The guest image had SIP disabled; production permission behavior
was not established by these tests.

| Check | Observed result |
|---|---|
| HID-posted sequence, Secure Input off, tap plus registrations | Tap received 18 down/up events; Carbon received zero. Claimed keys did not reach the foreground sink. Shift+Return passed through. |
| Same HID-posted sequence, Secure Input on | Tap received zero. Carbon received 10 press/release events for the tested Space, Return, and Escape gestures, including Space release after Option release. |
| System Events sequence, Secure Input on | Carbon received all 24 expected press/release events across the registered test combinations. Only the intentional Shift+Return bypass reached the sink. |
| Consuming HID tap installed after the receiver, Secure Input off | Competitor received all 18 HID-posted events. The receiver's tap and Carbon handler both received zero. |

The System Events sequence exercised Option+Space twice, held Space with
Option+Up, Return, Escape, four arrows, Numpad Enter, and Option-modified Return
and Numpad Enter. This tested OS delivery of combinations, not Azad's contextual
registration policy or double-tap timing. In Azad, history entry must remain
Option+Space+Up; Option+Up must not become a new standalone global command.

Two test limitations were caught by controls:

- Low-level `CGEventCreateKeyboardEvent`/`CGEventPost` arrow events reached the
  tap but did not exercise Carbon navigation registrations faithfully. System
  Events delivered those registrations. Adding Fn-modifier registrations was
  not a fix: both forms fired in the test. No Fn variants are proposed.
- System Events reached Carbon without traversing the HID tap even with Secure
  Input off. Therefore its zero tap count is not evidence of Secure Input
  suppression. The identical HID-posted on/off comparison supplies that evidence.

The existing `just interaction-test` also passed all seven scenarios. It proves
the headless classifier/reducer sequences, not macOS delivery or suppression.
Registration success, enabled status, and reducer tests must not be reported as
proof of real keyboard capture.

## Minimal implementation recommendation

The affected surface is the platform capture adapter and its tests, not the
transcription engine or interaction model.

1. Restore system hotkey registrations for the **existing** shortcut policy and
   feed the same semantic handler as the HID tap. Keep the required registrations
   available during Secure Input instead of waiting for a polling-based switch.
   A missing tap callback cannot activate a fallback by itself.
2. Preserve exact contextual ownership: hold/double-tap stay global; history
   entry stays conditional on the claimed Space hold; Enter/Escape and history
   navigation stay conditional on the existing overlay gates. Preserve configured
   modifier superset matching, Option raw behavior, Numpad Enter, and Shift+Enter
   pass-through. Carbon's exact-combination matching must not narrow or broaden
   those existing rules.
3. Share pressed-key state across sources so one physical gesture emits one
   action. Explicitly test Secure Input changing during a hold, modifier release
   before Space release, autorepeat, and a key-up arriving through a different
   source. The steady-state API tests above do not prove these transitions.
4. Preserve Azad's synthetic-paste bypass. Carbon notifications do not carry the
   tap's existing synthetic-event marker in the same form. Auto-submit must not
   be recaptured; an app-level integration test is required before deployment.
5. Distinguish denied permission, failed registration, disabled/invalid tap,
   Secure Input, and upstream consumption in diagnostics. Log transitions and
   source/action counts without logging unrelated keystrokes or text. Do not
   treat an enabled but silent tap as evidence that hotkeys are healthy, or
   restart continuously just because no keys arrive.

The related history text-entry path needs explicit verification before claiming
the whole history workflow works during Secure Input. Its tap currently captures
printable text; shortcut registration is not a replacement for arbitrary text
input. The existing nonactivating NSPanel, NSTextField, and text-change delegate
are the first path to verify for normal focused text delivery. This investigation
does not claim that path is already verified under Secure Input.

If implemented, update the existing tap-only comments/documentation and isolated
adapter tests in the same focused change. Do not restore a second independent
action-dispatch tree, add app-specific branches, change shortcuts, or change the
interaction reducer without a demonstrated need.

## Limits and alternatives

- **Competing taps:** head insertion precedes taps already installed, not taps
  another app may install later. The isolated competing-tap result disproves
  an unconditional priority guarantee for the proposed pair of APIs.
- **Competing registrations:** the SDK's `CarbonEvents.h` documents that an
  exclusive registration can suppress nonexclusive registrations even when
  their registration calls succeeded. Exclusive registration itself can fail
  because another process already owns the combination. Exclusivity is not an
  unconditional ownership override.
- **Permissions:** Azad cannot grant itself revoked macOS permissions. Capturing
  a registered shortcut also does not prove microphone, paste, or accessibility
  operations are permitted. Permission failure must be distinguished from
  hook failure, not hidden behind repeated restart attempts.
- **Lower-level capture:** Karabiner demonstrates a stronger hardware path using
  privileged device seizure plus a virtual-HID driver. This carries root services,
  system-extension approval, device ownership, and substantially broader keyboard
  responsibilities. It was researched, not installed or tested here. It is not
  a small Azad reliability fix, and does not justify promising victory over any
  competing privileged device owner. See its
  [architecture](https://karabiner-elements.pqrs.org/docs/help/advanced-topics/security/)
  and [capture-method discussion](https://github.com/pqrs-org/Karabiner-Elements/blob/main/DEVELOPMENT.md).

Before shipping a capture-adapter fix, verify its actual production adapter and
existing interaction harness in isolation, including foreground pass-through,
contextual registration changes, cross-source releases, history text entry, and
synthetic paste. An OS-level VM test must exercise both capture and foreground
non-delivery; a callback counter alone is insufficient. Physical-keyboard and
Jump-specific behavior remain outside the evidence collected here. Deployment
verification remains install/restart/status and read-only inspection, never
synthetic input into the user's desktop.
