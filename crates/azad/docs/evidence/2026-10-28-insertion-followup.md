# Stale insertion follow-up — October 28, 2026

## Result and limit

The new Jump report is present in the local logs. Controlled replays reproduce two causes
of stale or uncertain insertion: a pending receiver clipboard update and a competing local
clipboard write. The second cause has a verified local prevention. Separate failed-insertion
replays also exposed unsafe auto-submit and automatic retry paths; those paths are corrected.

The pending-update replay uses Jump's VNC path and an instrumented receiver with a deliberate
commit delay. It is **not a reproduction of Fluid's natural internal trigger**. The reported
Fluid defect remains open in `MS-INSERTION-RELIABILITY.T1`. The earlier
[Brave report](2026-10-28-insertion.md) remains historical evidence, not proof of this correction.

## Reported Fluid turn

The user identified catalyst-mini as the remote Mac, Fluid as the protocol, and a subsequent
manual Command-V as successful. At approximately 3:23:38pm PT, local turn 58 recognized:

> By old designs, I meant any old code.

The reported remote insertion was:

> 0050's null wrapper: the inline oneof fixes this, with less clutter.

Local `AZAD_PASTE_HANDOFF` records show `com.p5sys.jump.mac.viewer.web` in the foreground,
event-posting permission present, and change count 26910 at write, before shortcut, and after
shortcut. The write timestamp is `1793226218301`; the V events are at `1793226218328`.
Producer completion is `pasted`, 38 characters, 164 ms, with auto-submit off.

The preceding local turn was a different, 178-character protobuf transcript. The searched local
ASR records did not contain the reported null-wrapper text. These facts support a stale remote
clipboard candidate, not an established reuse of the prior ASR slot. The old records did not
compare the clipboard body with the expected text.

Read-only inspection of catalyst-mini found this earlier record in the Fluid session that
spanned the reported time:

```text
2026-10-28 13:19:31:253 ERROR [rtc.clip 40lxssh2] Can not give content to peer while waiting for content from peer
```

The session ended at 3:40:56pm. The error demonstrates a send/receive state conflict in that
session. Default server logs do not identify the clipboard receipt for the 3:23pm paste.
Thus, the error does not prove that this state persisted until the failure. Local viewer version
was 10.15.31.0; remote Connect version was 10.15.46.0. No remote settings or processes were changed.

## Reproduction before correction

All native input below occurred inside the owned macOS VM, with host clipboard sharing and
audio disabled. The production probe requires `VirtualMac`, an explicit input mode, the expected
foreground application, and event-posting permission. It refuses the host before clipboard access.

### Pending receiver clipboard commit

The [loopback receiver](2026-10-28-insertion/rfb-receiver.py) implements a bounded RFB fixture.
Guest Jump connects to guest loopback, not the host desktop or catalyst-mini. The receiver records
clipboard and keyboard messages and emulates a text field; it is not a native remote editor.

In the [delayed replay](2026-10-28-insertion/rfb-delayed-before.jsonl), the receiver starts with
the reported old text and delays the new clipboard commit by 500 ms. Jump sends the new clipboard
before the V key. The V arrives about 200 ms later, before the imposed commit, and inserts the
old text. After the commit, the same production shortcut, without another clipboard write,
inserts the new sentence. The local clipboard matches the new sentence throughout.

This demonstrates the reported old-then-new pattern under a controlled pending-update fault.
It also demonstrates why local verification and a successful event-post call cannot prove
remote readiness. The RFB protocol has separate clipboard and key messages; the fixture supplies
the deferred-commit fault, not evidence that Jump's Fluid server uses this implementation.
[RFB 3.8 specification](https://www.rfc-editor.org/rfc/rfc6143.html)

### Competing local clipboard owner

The [VM-only interferer](2026-10-28-insertion/clipboard-interferer.swift) waits for a unique
fixture transcript, then replaces it with the reported old text. It posts no input.

Before correction, the [producer log](2026-10-28-insertion/ownership-before.log) reports count 3
after write and count 4 before shortcut. It still sends Command-V and returns `Pasted`, although
the clipboard no longer matches the requested transcript. The receiver inserts the replacement.
The [receiver log](2026-10-28-insertion/rfb-ownership-before-after.jsonl) records that old-text paste;
its later interval is not used as proof of an active connection or absence of output.

After correction, the [pre-shortcut replay](2026-10-28-insertion/ownership-after.log) stops on
the changed generation and returns `ClipboardWriteFailed`. A second,
[live-receiver replay](2026-10-28-insertion/ownership-final.log) catches replacement during the
write/readback itself. Its [receiver capture](2026-10-28-insertion/rfb-guard-final.jsonl) first
contains a successful positive paste, then no additional key or paste before its snapshot.
The connection has no end record in that capture. This is bounded proof of prevention, not a
claim that the local ownership race caused the historical Fluid or Brave failures.

### Failed insertion, auto-submit, and late retry

The [history-insertion replay](2026-10-28-insertion/history-failure-before.log) fails before
correction because a failed synchronous history insertion sends Enter.

The [late-event replay](2026-10-28-insertion/sync-retry-before.log) injects a synchronous native
failure. A duplicate final-text event then triggers a second attempt and Enter. The correction
marks every transcript source handled at admission, including synchronous failures. A failure
is not permission for an automatic retry whose first delivery remains uncertain.

### Repeat the native replays

Use only the owned VM. Put the reviewed receiver, interferer, and signed production probe inside
that VM. The guest probe must already have Accessibility permission. Start the receiver in a
guest terminal, then open guest Jump to `vnc://127.0.0.1:15900` and leave that connection in front.
For the pending-update fault, start the receiver with:

```sh
python3 -u rfb-receiver.py 500
```

In a separate guest terminal or Guest Agent command, run:

```sh
./azad-insertion-probe --allow-vm-jump-input 'By old designs, I meant any old code.'
```

After `clipboard_committed` appears in the receiver log, repeat only the shortcut:

```sh
./azad-insertion-probe --allow-vm-jump-retry 'By old designs, I meant any old code.'
```

The first receiver paste must be the old fixture text; the second must be the new sentence.
The current local generation guard deliberately cannot prevent this receiver-side fault.

For the local ownership fault, use a fresh receiver with delay `0`. Start the interferer first
with a unique new token and the old replacement; wait for its `armed` line. Within its 10-second
deadline, run `--allow-vm-jump-input` with that token. The corrected probe must return
`ClipboardWriteFailed`; a live receiver must contain no new paste shortcut for that attempt.
Use a successful positive paste on the same connection before that negative check.

## Delivered correction

- Capture the generation claimed by `clearContents`; verify that generation and exact text after
  write and before the shortcut. A failed check emits no paste shortcut.
- Return native Command-V recipe errors as `InputEventFailed`. Attempt V and Command cleanup
  after a failed V click. Do not treat event posting as recipient acknowledgment.
- Do not auto-submit failed insertions, including history insertions. Do not automatically retry
  a handled source after late transcription events.
- Keep failed normal transcripts in history only when history is enabled. Do not advance
  last-pasted state on failure. Show the existing overlay notice without replacing newer speech.
- Record expected and observed SHA-256 values, generation checks, foreground application, and
  permission state. Discard observed text and its hash if the generation changes during readback.
  Log no clipboard body. Preserve embedded null bytes in the exact comparison.
- Distinguish Azad-tagged V events from external Command-V events in HID metadata, including source
  process, source state, and keyboard type. This remains a local observation, not remote receipt.

Clipboard readback is not atomic. A change after the pre-shortcut check remains possible.
No configured paste delay was increased. No automatic retry or typing default was added.

## Clipboard verification cost — October 29, 2026

The owner limited this follow-up to the performance impact of clipboard verification.
An unsaved read-only observation of `~/Library/Logs/Azad/stderr.log` at 4:51pm PT found 575 attempts
with matching text and generation at all three local handoff stages. These include turns 259–263
from the owner's October 29 message; the logs do not contain destination receipt.
The [fixture](2026-10-28-insertion/clipboard-validation-latency.swift) uses a private
`NSPasteboard`, not the general clipboard. It posts no input events.
It measures a generation read, a text read, another generation read, and exact UTF-8 comparison.
The API sequence matches the production guard, but its Swift conversion cost is not the Rust cost.

The optimized run used catalyst, MacBookPro18,4, macOS 26.5, and Apple Swift 6.3.2.
The source baseline was `bf044d7512dfc6e1dee4af0a9587102497b6f22f`, with this fixture added.
The run completed at approximately 5pm PT on October 29. No CPU was reserved or pinned.
Each payload has 50 warm-up iterations and 1,000 retained samples.
The [saved result](2026-10-28-insertion/clipboard-validation-latency.json) contains:

| Payload bytes | One verification, median | One verification, p95 |
| ------------- | ------------------------ | --------------------- |
| 49            | 0.030 ms                 | 0.044 ms              |
| 347           | 0.031 ms                 | 0.046 ms              |
| 4,105         | 0.055 ms                 | 0.067 ms              |
| 65,545        | 0.422 ms                 | 0.449 ms              |

The fixture also records three consecutive verifications of the same generation.
That total is not pre-paste latency: production has two validation stages before Command-V
and one after it.
Production separates its stages with other work; the fixture runs its three reads consecutively.
Clipboard writes, logging, foreground queries, waits, event posting, and destination receipt
are outside the timed region. These results estimate AppKit readback cost on an uncontended
private board, not general-board or end-to-end latency.
They do not show that validation prevents a stale paste or change the installed app.

Positive readback and negative stale-text, stale-generation, and unequal-encoding controls passed.
The payloads include Unicode but no embedded NUL. No mid-read ownership change is forced by this fixture.

Build from the repository root and run in the retained internal-storage evidence target:

```sh
swiftc -O \
  -module-cache-path ~/projects/azad/insertion-20261028/swift-module-cache \
  crates/azad/docs/evidence/2026-10-28-insertion/clipboard-validation-latency.swift \
  -o ~/projects/azad/insertion-20261028/clipboard-validation-latency
~/projects/azad/insertion-20261028/clipboard-validation-latency
```

Initial source review: `conversation-18e3256bdefd8f48-627359`.
Final acceptance: `conversation-18e325ba3d1b08a0-627925`.
The reviews require these timing limits; neither accepts a stale-paste fix.

## Long keyboard output

A separate admitted run used the production unpaced direct-typing path through guest Jump VNC.
The [receiver capture](2026-10-28-insertion/rfb-ascii-long.jsonl) contains exactly 10,000 ASCII
characters, with the same SHA-256 as the
[input fixture](2026-10-28-insertion/typing-long-ascii-input.txt). The
[comparison](2026-10-28-insertion/typing-long-ascii-summary.json) reports an exact match and no
connection end. The [producer](2026-10-28-insertion/typing-long-ascii-producer.log) took 2,314 ms.
This is one transport/emulated-field run, not proof of Fluid, a native editor, visual smoothness,
or universal long-text reliability.

A separate Unicode VNC run reached a receiver snapshot error on surrogate code units. It is
excluded from receipt claims; the fixture cannot establish that run's Unicode field contents.
The optional long-input edit to the AppKit probe was removed rather than extending its fixed
short-sample deadline and adopting a second measurement branch.

See [typing options](../typing-cadence-investigation.md#implementation-options) for the tradeoffs.

## Verification and live deployment

- [App suite](2026-10-28-insertion/unit-guard-reviewed.log): 312 passed, 1 existing ignored test.
- [Interaction harness](2026-10-28-insertion/interaction-reviewed.log): all 7 scenarios passed.
- Clippy: app, insertion probe, and unchanged typing probe passed with `-D warnings`.
- Independent review accepted corrected code and the bounded fixture. Conversation
  `conversation-18e2d4b39bb877d8-556066`, request `cmsg-18e2d76137d27e20-563039`.
  The final evidence review also accepted request `cmsg-18e2d816d0e7ad78-566605`.
- Native ownership prevention and exact long ASCII receipt are recorded above. Host refusal passed.

The canonical `just install`, `just restart`, and `just status` commands completed. The install
retained the unchanged MLX helper with `AZAD_SKIP_MLX_HELPER=1`. Before lifecycle actions, process
inspection found only Azad and its ASR helpers in the target scope; the legacy service was absent.
The updated app runs as PID 4626. Startup records show Accessibility, Microphone, and Input Monitoring
granted, with the hotkey tap installed. Natural external Command-V events appear in the new metadata.

Built and installed UUID: `76DF1B33-3E5E-3798-827E-C1C411F67C15` (arm64).
Installed executable and mapped process inode: `462641198`.
`codesign --verify --deep --strict` passed. The designated requirement remains `ai.azad`,
Micro team `35A87BDK48`. This proves live deployment, not insertion into a daily-driver field.

Detailed logs and the Gateway conversation remain under the declared kept artifact root
`/Users/spencer/projects/azad/insertion-20261028`. No account tokens or license files are included
in this evidence. Neither daily desktop nor the installed app was an input fixture.
The owned receiver, keep-awake process, and VM were stopped after collection. Retained artifacts
and foreign processes were not removed.
