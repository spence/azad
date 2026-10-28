# Direct-typing cadence — October 28, 2026

## Findings

Evenly spaced keyboard events do not guarantee evenly spaced visible characters.
Azad, macOS, the browser, the web page, and the display each have a separate timing boundary.
A longer clipboard delay does not solve direct-typing cadence.

### Current Azad behavior

[The insertion code](../src/platform/paste.rs) has two direct-typing paths:

- **Unpaced (`0`):** send every Unicode scalar as a key-down/key-up pair in one synchronous loop.
  There is no deliberate interval between characters.
- **Finite rate:** send one pair, then schedule a one-shot `NSTimer` on Azad's main run loop.
  The timer also participates in menu tracking mode. Its minimum requested interval is 1 ms.

[The cursor](../src/typing.rs) schedules each next character from the actual emission time.
It does not send several characters to compensate for a late callback.
Thus, delayed callbacks make the whole operation slower; they do not cause an Azad catch-up burst.
The existing `early_and_late_callbacks_never_burst_or_skip_a_scalar` test covers this behavior.

The rate counts Unicode scalars, not complete visual characters.
A composed emoji or an accented character can require several scalars and several intervals.

| Selected rate | Requested interval | Minimum time for 1,000 scalars |
| --- | --- | --- |
| 20/s | 50 ms | 49.95 s |
| 40/s | 25 ms | 24.975 s |
| 80/s | 12.5 ms | 12.4875 s |
| 160/s | 6.25 ms | 6.24375 s |
| 320/s | 3.125 ms | 3.121875 s |
| Unpaced | No deliberate interval | Depends on event construction and delivery |

These are calculations from the code, not recipient measurements. They exclude callback delays,
event creation, browser work, and display latency. There is no interval after the last scalar.

### Existing native measurements

The [October 16 VM evidence](evidence/2026-10-16-typing-key-rate.md) records actual AppKit receipt
for a short Unicode sample. The production insertion code ran beside a non-key panel.

| Selected rate | Calculated minimum span | Received span |
| --- | --- | --- |
| 20/s | 900 ms | 1,026 ms |
| 80/s | 225 ms | 365 ms |
| 320/s | 56.25 ms | 219 ms |
| Unpaced | No added wait | 89 ms |

Thus, the selected rate is not a measured delivery rate. At 320/s, this receiver took almost
four times the calculated minimum. These totals do not identify callback jitter, individual
event latency, or browser frame timing. They are not measurements of the reported Google field.

### Why characters can appear in groups

Apple documents that `Timer` is not a real-time mechanism. A busy run loop can delay a callback.
Azad's timer shares its main thread with speech-event handling, overlay updates, and other app work.
This makes emission jitter possible. Existing tests prove cursor ordering, not native callback timing.
[Apple Timer documentation](https://developer.apple.com/documentation/foundation/timer)

Chromium separates input routing, page execution, rendering, and compositing.
Its rendering scheduler follows the display cadence, not each individual keyboard event.
Consequently, one rendered frame can contain several text changes.
[Chromium RenderingNG architecture](https://developer.chrome.com/docs/chromium/renderingng-architecture)

For example, a 60 Hz display has approximately 16.7 ms between frames.
At 320 scalars/s, about 5.3 scalars arrive per frame, even with perfect emission timing.
At 120 Hz, the corresponding average is about 2.7 scalars per frame.
These calculations explain a possible grouping mechanism; they do not measure this Google field.
Page scripts, autocomplete, and layout work can add further delay.
The unpaced loop provides even less opportunity for a visible character-by-character progression.

### Would a blocking wait improve it?

Not on Azad's main thread. A blocking wait would also delay app events and cancellation.
A dedicated timer queue could isolate emission from Azad's UI work, but it cannot control Brave's
event processing or its next frame. Apple's strict dispatch timer still provides best-effort timing,
not a hard deadline. A spin loop would consume CPU without an end-to-end timing guarantee.
[Apple dispatch timer scheduling](https://developer.apple.com/documentation/dispatch/dispatchsourcetimer/schedule(deadline:repeating:leeway:)-hvhp)

## Smallest useful next experiment

Use an isolated browser session, not the daily-driver desktop. Record four timestamps:

1. The intended character deadline.
2. The actual timer callback and event-post time.
3. The recipient's text-input event.
4. The next rendered frame that contains the text.

Compare the current main-run-loop timer with a dedicated serial dispatch timer at the same rates.
Keep the same Unicode pairs, ordering, and no-catch-up rule.
If callback jitter dominates, isolate the emitter from Azad's UI work.
If recipient or rendering delay dominates, a more precise emitter will not remove the visible groups.
For character-by-character appearance, a rate below the observed display and editor capacity is
more promising than the highest rate. Clipboard paste remains the shortest operation for long text.

No timer, rate default, or typing architecture was changed for this research.
