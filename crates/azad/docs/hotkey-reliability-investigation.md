# Hotkey reliability investigation

Investigation: 2026-09-28. Source baseline: `da639bc`.
Status: goal remains open. A privileged device-capture prototype survives the
Secure Input and consuming-event-tap cases that defeated the app-level mechanisms.
It is not an integrated Azad fix or an unconditional ownership guarantee. Shipping
it would add a root helper and an approved virtual-keyboard driver; that product
scope needs an owner decision. No shortcut changes or host driver installation
have been made.

## Finding

Keep the existing keyboard shortcuts and interaction semantics unchanged. The
tap-plus-Carbon proposal is insufficient for the requested reliability: a later
consuming HID tap blocked both in the isolated test. Completing that investigation
did not complete the user's goal.

The stronger candidate is a privileged input broker: exclusively read the keyboard
device, send claimed actions directly to Azad, and forward unclaimed input through
a signed virtual-HID driver. This operates below application event taps. The
prototype received and suppressed the fixture shortcuts during Secure Input and
in the presence of a consuming tap, while forwarding ordinary typing. It also
released the device when deliberately crashed.

A tap that exists and reports enabled can still receive no keyboard events.
Secure Input is one demonstrated cause. Recreating the tap does not remove that
system-wide gate. Another application can also install an earlier consuming tap.
Neither API provides unconditional priority over every other application.

The remaining hard boundary is exclusive device ownership. A second privileged
reader was rejected with `kIOReturnExclusiveAccess` and received no input while
the first reader owned the device. An existing device-level keyboard tool needs
cooperation or an explicitly verified forwarding arrangement, not another
competing grab. Host inspection found no running Karabiner, Kanata, KMonad,
SteerMouse, or BetterTouchTool process and no installed HID system extension;
that is an observation about this Mac, not proof of universal compatibility.

This investigation changes no application code, bindings, host preferences or
permissions, or installed runtime. No input was injected into the user's desktop.
Runtime experiments used disposable macOS VMs with guest-only synthetic input.
Both VMs were stopped and deleted after their experiments; no test keyboard
service or driver was installed on the host.

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

## Rejected as a complete solution: tap plus registrations

System registrations demonstrated useful coverage during Secure Input. Restoring
them could address that narrower failure, but the consuming-tap counterexample
rules out presenting them as the requested solution. No Carbon restoration has
been implemented. Polling, tap recreation, or additional Fn bindings do not
resolve that counterexample.

## Device-level capture evidence

A second disposable VM used the published, signed and notarized
Karabiner-DriverKit-VirtualHIDDevice package 8.6.0, whose active driver reports
1.8.0. The package was installed and approved only in the VM. The source and relay
clients used pinned revision `ba98de7fae2d529b9debe82890765dc66246f4ff`.

The test topology was:

```text
signed virtual source keyboard (fixture only)
  -> root IOHIDManager exclusive reader
       -> claimed press/release counters
       -> unclaimed reports -> separate signed virtual output keyboard
            -> competing event tap -> foreground AppKit key sink
```

The virtual source produces actual device reports, unlike `CGEventPost` or VNC
input. Source and output have different fixture product IDs; the relay cannot
recapture its own output. Every native input-producing/capturing fixture refuses to run
unless the machine identifies as `VirtualMac`. The host only compiled the probes
and controlled the explicitly owned VM over SSH. VNC was used for guest driver
approval, not as the device-input oracle: control tests showed that VNC keystrokes
reached the tap but bypassed the raw device reader.

The sequence contains 25 reports: two Option+Space presses, Up while Space is
held, Option release before Space release, Return, Escape, all four arrows,
Numpad Enter, intentional Shift+Return pass-through, and ordinary `a` typing.
The fixture claims ten key-down actions with their matching releases. Its
contextual policy is deliberately simplified; it is not Azad's interaction
reducer and does not prove the double-tap action or overlay behavior.

| Check | Observed result |
|---|---|
| Root nonexclusive device reader, Secure Input on | Received all 28 relevant HID element changes; the HID event tap received zero key events. |
| Exclusive reader with consuming tap, Secure Input off | Reader received all 28 changes; tap and foreground sink received zero key events. |
| Relay baseline | Received 25 reports and claimed ten actions; the sink received only Shift+Return and `a` down/up, four events total. |
| Relay with Secure Input on throughout | Same 25 reports and ten actions, with matched releases; only the four pass-through events reached the sink. |
| Secure Input enabled during the first Space hold, then disabled; consuming tap active | Same 25 reports and ten actions. The consuming tap saw only the four forwarded pass-through events, never the claimed keys. |
| Relay intentionally exits with status 86 during the second Space hold | Subsequent input reached the foreground sink without restarting the source or OS. Ordinary `a` arrived with no stuck Option/Shift modifier. Claimed-key protection is absent while the helper is down. |
| Second root exclusive reader while the first owns the source device | Both its manager/device opens reported `0xe00002c5` (`kIOReturnExclusiveAccess`); it received zero values while the owner received all 28. |

In a follow-up ownership-release probe, a reader whose opens had failed remained
silent after the owner exited, despite the source emitting its full sequence.
Merely leaving that failed reader alive was not recovery; a production broker
needs explicit failed-open/reacquisition handling.

Apple's open-source
[IOHIDDevice ownership logic](https://github.com/apple-oss-distributions/IOHIDFamily/blob/777ccd9698845aadf711e32d843c8c9b777431d9/IOHIDFamily/IOHIDDevice.cpp)
rejects a different client while an exclusive owner exists and releases that
ownership when the owner closes. Its
[IOHIDLibUserClient implementation](https://github.com/apple-oss-distributions/IOHIDFamily/blob/777ccd9698845aadf711e32d843c8c9b777431d9/IOHIDFamily/IOHIDLibUserClient.cpp)
also distinguishes privileged clients from the Secure Input gate. These source
paths explain the observations; the VM tests establish behavior on the tested OS,
not source equivalence with Apple's shipped kernel.

This is an established architecture, not a new event-tap priority trick:
[Karabiner's architecture](https://karabiner-elements.pqrs.org/docs/help/advanced-topics/security/)
and [capture discussion](https://github.com/pqrs-org/Karabiner-Elements/blob/d130433e5854eeff125f6ed45f6ac1bdad0aeeab/DEVELOPMENT.md)
describe device-level capture. The
[virtual driver integration interface](https://github.com/pqrs-org/Karabiner-DriverKit-VirtualHIDDevice/blob/ba98de7fae2d529b9debe82890765dc66246f4ff/README.md)
supports third-party clients. Its published signed package avoids making a new
Azad-owned driver identity a prerequisite for the prototype. Kanata's
[macOS setup](https://github.com/jtroo/kanata/blob/main/docs/setup-macos.md)
documents the same root/driver dependency and exclusive-owner conflict; an
established remapper is not exempt from that conflict either.

### What remains unproved

- The guest has SIP disabled. Permission grants and signed-helper deployment on
  a normal SIP-enabled installation have not been validated. There is no proposal
  to disable SIP on the user's Mac.
- The source is a virtual fixture, not a built-in, USB, or Bluetooth keyboard.
  The relay decodes that fixture's report format, not arbitrary keyboard report
  descriptors. A generic adapter must normalize batched HID elements before
  shortcut classification: the initial probe delivered Space before Option
  within one report, so per-element immediate classification would miss a chord.
- No production Azad adapter, overlay, history text input, paste bypass, key
  repeat, multi-keyboard modifiers, sleep/wake, session switching, reconnect, or
  output-driver failure recovery has been verified with this mechanism.
- The crash test establishes restored ordinary input in that run, not a
  crash-proof service or absence of a temporary capture gap. A hung helper also
  needs a bounded fail-open path; process-exit cleanup alone is insufficient.
- Events injected above the device layer need separate analysis. The device
  proof covers keyboard-originated input, not every software-generated key event.
- Seizing an existing remapper's virtual output is only a possible integration
  if that remapper forwards the required keys. It cannot recover keys the owner
  discards, and has not been validated here.

### Implementation boundary

The focused product change would be a platform input broker plus its lifecycle
and tests, not a rewrite of ASR, UI, or the interaction state machine. Before
shipping, the existing classifier must receive normalized input with one source
of pressed-key ownership. Preserve hold/double-tap, history only during the
claimed Space hold, contextual Enter/Escape/arrows, modifier supersets, Option
raw mode, Numpad Enter, Shift+Enter bypass, and synthetic-paste bypass.

The helper would read all keyboard reports to forward unclaimed input. That
creates a privileged trust boundary: keep arbitrary text out of logs and app IPC,
authenticate the app connection, and release devices if the forwarding path or
consumer cannot operate safely. Treat permission denial and device-ownership
conflicts as explicit states, not as an enabled-but-silent success. Verify both
claimed-key suppression and ordinary-input delivery under injected failures.

Adding a root keyboard service and a user-approved driver changes installation,
security, and coexistence obligations materially. The research supports further
isolated implementation of this candidate, not silently installing those
components into the user's working environment. The original goal stays open.

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
- **Lower-level capture:** The VM prototype above addresses the reproduced
  event-tap and Secure Input failures. It does not override a different exclusive
  device owner or grant its own revoked permissions. Cooperation or an explicit
  compatibility constraint is required at that boundary.

Before shipping a capture-adapter fix, verify its actual production adapter and
existing interaction harness in isolation, including foreground pass-through,
contextual registration changes, cross-source releases, history text entry, and
synthetic paste. An OS-level VM test must exercise both capture and foreground
non-delivery; a callback counter alone is insufficient. Physical-keyboard and
Jump-specific behavior remain outside the evidence collected here. Deployment
verification remains install/restart/status and read-only inspection, never
synthetic input into the user's desktop.
