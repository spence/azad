# Typing key rate design

Add a small **Key rate** slider to the right of **Insert method** in General settings,
visible for **Type** and **Type and copy**. Both methods share one saved value. The default
is **Max**, preserving the current unpaced behavior.

This approved design does not change clipboard paste timing or claim to fix the separately
reported intermittent paste failure. Implementation verification is recorded in
[the key-rate evidence](evidence/2026-10-16-typing-key-rate.md).

Decision: the owner approved implementation of this design on 2026-10-16 ("impl it. set a goal").

## Settings layout

```text
Insert method   [Type and copy ▾]   Key rate [───●───]  80/s
```

For **Paste**, omit the Key rate label, slider, and readout entirely. Keep the saved value so
switching back to either typing method restores it. Changing insertion methods does not change
the rate, and changing the rate does not change the insertion method.

Use a native small `NSSlider` with six discrete stops, increasing left to right:
**20/s, 40/s, 80/s, 160/s, 320/s, Max**. These are approved control choices, not measured
compatibility limits. Max adds no intentional inter-character wait; the finite values limit
character-pair emission, not the operating system's physical keyboard repeat rate.

Keep the current 656 × 391 pt window, sidebar, row order, and row heights. Reclaim space only
in General: reduce its leading content inset from 32 to 16 pt and pass `labelWidth: 104`
to its existing `FormRow`s instead of changing the shared 150 pt default. Other tabs keep
their existing insets and label widths.

The resulting control budget is `656 - 158 - 1 - 16 - 24 - 104 - 14 = 339 pt`.
Budget the insert control as follows:

| Item | Width in points |
| --- | ---: |
| Existing large insertion-method popup | 145 |
| Gap | 8 |
| Key rate label | 52 |
| Gap | 6 |
| Small slider | 72 |
| Gap | 6 |
| Fixed-width monospaced rate readout | 44 |
| Total | 333 |

Native AppKit sizing on the current host reports fitting widths of 145 pt for the existing
popup, 102 pt for the General label “Overlay position”, 51 pt for “Key rate”, and 37 pt for the
monospaced “320/s” readout. These fit their allocations above; the current popup's 210 pt
constraint is wider than necessary. Complete control-frame and clipping checks remain part
of implementation validation, not an assumed consequence of this arithmetic.

Keep the slider keyboard accessible. Its accessibility label is “Key rate”; its value is
“80 characters per second”, for example, or “Maximum, no added delay”. No tooltip or new
explanatory row is needed. Commit the setting on release or keyboard adjustment, then refresh
the readout through the existing settings update path; do not rebuild the window continuously
while the thumb is being dragged.

## One preference and one meaning

Add one persisted `AzadTypingKeyRate` integer, using characters per second; `0` means Max.
Accept only `0, 20, 40, 80, 160, 320`. Missing or unsupported values use Max. Ignore malformed
UI change events rather than interpreting a missing value as the slowest rate.

Keep one `typing_key_rate` value in Rust controller state and `typingKeyRate` in the Rust–Swift
settings model. Use the existing UI-event integer payload for `setTypingKeyRate`; no new event
transport, migration, setting group, or separate Type and copy preference is needed. Preserve
the field in both existing Swift model-copy constructors and in the preview fixture.

Here a character means one Rust `char`, a Unicode scalar, matching the existing `text.chars()`
typing loop. Each scalar retains its current `set_string` payload and complete key-down/key-up
pair. Do not split UTF-16 surrogate pairs or introduce a different grapheme segmentation rule.
A combining mark or a component of a joined emoji therefore consumes its own pacing interval.
There are two keyboard events per pacing unit; “80/s” does not mean 80 individual down/up events.

Snapshot the rendered text, selected method, key rate, auto-submit mode, and originating
turn/session or history-request identity when an insertion is requested. Setting changes apply
to subsequent requests, never halfway through an insertion already admitted for delivery.
Retain the original text, timing/metrics context, and caller cleanup policy for completion;
do not reconstruct them from whichever turn or settings happen to be current later.

## Delivery without blocking the app

The existing direct-typing loop runs synchronously on the AppKit event-drain path. Adding
`sleep` inside it would freeze settings, overlays, and controller event handling; 1,000 scalars
at 20/s would add almost 50 seconds. A detached typing thread that immediately reports success
would instead copy, submit, and dismiss too early. Neither is an acceptable implementation.

Use one dedicated, main-run-loop, one-shot **typing timer**, following the existing timer
pattern in `platform.rs`. Its callback advances only the active insertion. Do not increase the
general UI tick frequency: `AppController::on_tick` also checks permissions, renders overlays,
and advances animations. No worker thread, polling loop, or new service is needed.

The paced operation owns the text, a UTF-8 byte cursor, its event source, selected interval,
next due time, and original completion context. One callback emits at most one complete scalar
pair. Prepare both down and up events before posting either. Emit the first pair immediately;
schedule each subsequent pair no earlier than one interval after the previous pair completed.
There is no wait before the first pair, after the last pair, or between down and up. A late
timer resumes at the selected spacing rather than sending a catch-up burst. The rate is an
upper bound; AppKit scheduling can make delivery slower. A callback validates its original
insertion identity and due time; an invalidated or stale callback emits nothing.

Stamp typing down/up and paste-chord events with the existing `AZAD_SYNTHETIC_MARKER`. The tap already
passes marked events through before hotkey/history handling. Without this marker, opening
history during paced output could consume Azad's own Unicode payloads as search input. Reuse
that existing bypass; do not change tap registration, key bindings, or capture policy.

Implementation review exposed a required overlap adjustment: synchronous Max/Paste requests
can execute from the queue while history is open, so they need the same marker too. History
search must stay tap-driven, with its existing custom caret, without making the overlay key
or its search field first responder. Otherwise marked output can still land in Azad through
AppKit focus. History navigation/search that takes over during a paced selection restores tap
capture; the original insertion completion must leave that newer browsing state intact. This
removes redundant native focus ownership, not adds target activation or focus restoration.

Keep the existing synchronous path for Max when no earlier insertion is outstanding. This
preserves the default path's ordering and lack of added pacing delay. For finite rates, extract
only the existing per-scalar event recipe needed by the timer and its recording test emitter;
do not introduce a general keyboard-injection abstraction.

### Overlap and completion

Use one local FIFO of insertion snapshots behind the active operation. This is necessary to
preserve repeated dictation and history requests without interleaving their characters or
silently replacing a transcript. It is not a general job queue: no retry policy, priority,
capacity setting, new daemon, or persistence. Requests arriving during paced output, including
a later Paste or Max request, wait their turn instead of overtaking it. Determine the existing
terminal clipboard fallback when each request starts executing; the queue does not add new
application-specific rules.

Mark a speech source identity handled at admission, independently of whether insertion succeeds.
Deduplicate it while queued or active and after success, failure, or cancellation, until late
final/session-end events for that source can no longer retry it. Otherwise the session-end
fallback could retype an already emitted prefix. This is in-memory source-turn bookkeeping,
not a persistent status system; history requests remain distinct intentional insertions.

Pending or handled is not pasted. Keep successful-insertion bookkeeping, history recording,
auto-submit, and overlay cleanup attached to completion after the final key-up and the existing
Type and copy clipboard attempt. A failed or canceled paced emission must not claim full
insertion or auto-submit. Preserve the existing nonfatal clipboard-copy failure semantics and
the untouched synchronous history path; do not bundle their repair into this feature. Do not
start the next request until the prior request's copy/submit sequence has finished. Completion
means the event recipe has been posted, not an acknowledgment from the receiving application.

The completion context identifies its original turn and overlay lane. An older insertion may
finish while the next speech is already visible: it must not hide or reset that newer overlay,
clear a newer turn's capture state, or use mutable “latest turn” fields as its identity. Preserve
the existing raw-finalize, normal-finalize, split-overlay, and history cleanup decisions;
defer only the insertion-dependent parts. Normal speech processing continues while keys are
paced. Do not hold capture disabled until a long typing operation completes.

Existing Escape/cancel handling, when received, cancels outstanding paced output and discards
its queued tail; shutdown invalidates the timer and drops pending requests. Do not post more
characters, copy a partial string as a completed Type and copy result, or auto-submit afterward.
Already emitted text is not rolled back or retried. Canceling a pending request must not restart
the speech session or undo unrelated connector work.

Type retains its existing foreground routing: the user keeps the intended field focused until
typing finishes. Do not steal focus, pin a process, or add foreground-change cancellation. Such
a guard would introduce truncation on ordinary app switching and still would not protect field
changes inside the same app or remote desktop. This control is not a guarantee of receipt by
every application.

## Behaviors that stay unchanged

- **Paste:** clipboard write, `paste_delay_ms`, Cmd+V event recipe, and any existing screen-sharing
  sync nudge. It never gains per-character delays. The only new wait is behind already active
  paced output, when ordering requires it.
- **Type:** emits the current transformed text without copying it to the clipboard.
- **Type and copy:** types the whole string, then copies that whole string after successful
  emission, as it does today. It does not move the copy ahead of typing.
- **Terminal fallback:** both typing methods retain their existing clipboard fallback and ignore
  key rate when that fallback executes.
- **Text and submission:** existing text cleanup, emoji/number handling, raw text selection,
  insertion-method choices, and submit chords remain intact. Submit follows insertion, not the
  start of its timer. Do not repair unrelated existing clipboard error reporting or history
  submit behavior in this feature.
- **Speech and shortcuts:** no ASR/model, microphone, VAD/EOU, global-hook, shortcut-binding,
  Secure Input, or connector changes.

## Implementation boundary

The necessary edits are the existing Swift settings view/model and preview fixture; Rust
settings, preferences, UI model/bridge and settings handler; and the insertion path in
`app.rs`, `app/history.rs`, `platform/paste.rs`, plus the small dedicated timer callback in
`platform.rs`. Both direct `insert_text` callers must receive the same setting and completion
handling. Search every insertion and submit call site rather than updating only normal finalize.

One small paced-operation record and its local FIFO are justified by responsiveness and ordered
completion. Do not restructure `AppController`, replace the hotkey state machine, change the
shared form component, add a generic output framework, or bundle clipboard reliability work.
Update the existing app specification's settings/output paragraphs with the approved behavior
when implementing; this proposal does not replace the current specification.

## Validation before shipping

1. **Settings:** test valid/missing/unsupported rates, malformed UI events, Rust–Swift JSON,
   model-copy preservation, and the shared value across Paste → Type → Type and copy → Paste.
   Use isolated preferences, never the installed app's defaults as test controls.
2. **Layout:** instantiate the actual General controls without activating a window; check all
   three methods at the existing window size, longest popup title, and both rate endpoints.
   Assert no added layout ambiguity relative to the unchanged shared FormRow, no clipped labels,
   and no out-of-bounds frames. Offscreen AppKit reports the same row ambiguity for the untouched
   Text tab; this feature does not change that shared component. Keep other tabs'
   constraints unchanged. Any visual preview that activates an application runs only in an
   isolated macOS desktop/VM; the existing preview executable activates itself.
3. **Production pacing logic:** a recording event emitter and supplied monotonic time exercise
   the same advancement function used by the real timer, not the existing `cfg(test)` early
   success return in `insert_text`. Cover empty/one/many scalars, Max, no first/last pacing
   waits, no catch-up burst, exact scalar pair order, accents, non-BMP and joined emoji.
   Zero/one scalar adds no waits; N > 1 adds exactly N − 1 spacing intervals.
   Feed a marked paced event through the real tap callback with history capture enabled, without
   posting it: it must pass through and must not enqueue a history-search or hotkey action.
4. **Completion:** replay normal/raw/history requests, overlapping turns, duplicate final/session
   events, mid-insertion preference changes, and insertion failure/cancel/shutdown. Assert serial
   output, copy after the last pair, no early submit, correct original-turn bookkeeping, and no
   stale overlay dismissal or held speech capture. Verify Paste and terminal fallback still use
   their original recipe and are unaffected by the selected finite rate. Explicitly replay
   failure → late SessionEnded and cancel → late FinalText: neither may enqueue the source again.
5. **Regression and receiver:** run app unit tests and `just interaction-test`. In an isolated
   macOS desktop, drive the production emitter into an owned text receiver to verify complete
   Unicode text, receive timing, copy ordering, and submit ordering. Fake event tests alone do
   not establish macOS delivery, and an owned receiver is not an all-app compatibility claim.

No application rebuild, deployment, microphone exercise, or desktop input is needed for reviewing
this design. Implementation is not ready to ship merely because the slider fits or routing tests
pass; pacing and completion checks above must also pass.

## Red team review

| Challenge | Design resolution |
| --- | --- |
| A slider plus a sleep freezes Azad during long output. | Use a dedicated one-shot typing timer, not sleeps or a faster general UI tick. |
| Fire-and-forget typing pastes or submits before output finishes. | Preserve actual completion ownership, copy ordering, and submit ordering. |
| The next turn or history request interleaves or disappears. | One local FIFO and handled-source deduplication; no replacement or overtaking. |
| History search swallows the paced Unicode events. | Stamp down/up with the existing synthetic marker; test the real tap bypass without posting input. |
| A failed/canceled turn gets typed again by late completion. | Retain handled source identity independently of success; test both late-event sequences. |
| An older completion clears the next utterance's overlay. | Complete against the original request, turn/session, and overlay lane. |
| A shared layout change damages other settings tabs. | General-only insets and explicit row label widths; native-size control checks. |
| ASCII-only tests miss broken emoji or misleading rate units. | Retain scalar payloads and test surrogate-pair and joined-emoji strings. |
| Focus protection quietly becomes a new feature. | Retain foreground routing; no focus management or app-switch cancellation. |
| This grows into a paste reliability project. | Clipboard timing, delivery reporting, retries, and per-app policies are excluded. |

The initial architecture review rejected a sleep loop and fire-and-forget worker. Independent
draft review then found the history self-capture and late-event retry regressions above. Both
now have explicit design requirements and acceptance cases. A final independent review of the
revised design found no remaining blockers. The completed implementation and receiver checks
are recorded in the linked evidence; native results are not an all-app compatibility claim.

## Source anchors

- [Settings window and General pane](../../azad-ui/Sources/AzadUI/SettingsWindowController.swift)
  and [existing form rows](../../azad-ui/Sources/AzadUI/Controls.swift).
- [Swift settings model](../../azad-ui/Sources/AzadUI/Models.swift),
  [Rust UI model](../src/ui_model.rs), [UI events](../src/ui_bridge.rs),
  [preferences](../src/preferred_store.rs), and [settings handler](../src/app/settings_ui.rs).
- [Direct typing, copy ordering, and terminal fallback](../src/platform/paste.rs),
  [app event drain and insertion bookkeeping](../src/app.rs), and
  [history insertion](../src/app/history.rs).
- [Existing main-loop timer pattern](../src/platform.rs) and
  [isolated interaction test boundary](isolated-interaction-harness.md).
