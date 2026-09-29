# Physical keyboard rerun, built-in keyboard

The owner started `session.py --rerun` at 12:07 PDT (tool at `6bc2b59`, installed build app
`7d206309…` / helper `ebb686bd…`). They finished the built-in keyboard's steps, then stopped
before the Keychron's; the session was interrupted, so no `report.json` was written. The owner
then chose to accept the evidence gathered so far (`ESC-PHYSICAL-EVIDENCE-GAP`, option B).

Two raw records remain:

- [`sink.jsonl`](sink.jsonl): every key event the foreground test window received, as keycodes
  and modifier flags, with no characters.
- [`app-events.json`](app-events.json): Azad's input-log event names and times for the same
  minutes.

Decoded in order, the window received:

| Step (prompt) | Foreground window received | Azad received | Result |
|---|---|---|---|
| Before the window was ready | Command+= three times (not a test key) | — | — |
| Overlay keys: hold Option+Space, Shift+Return, Down, Escape | Option down/up/down, then Shift+Return once (with Shift and Option held). **No Space, Down or Escape.** | `hotkey_pressed`, `arrow_navigate(+1)`, `overlay_cancel`, `hotkey_released` (after a first quick tap: `hotkey_pressed`, `hotkey_released`) | pass: claimed keys withheld, Shift+Return delivered once |
| Ordinary typing: `azad` Return | a, z, a, d, Return, each down and up exactly once, no modifiers | nothing (the next event is speech from always-listening) | pass |
| Secure Input: hold Option+Space, type `xy` | x, y once each | `hotkey_pressed`, `hotkey_released` at 12:08:43 | typing and shortcut pass; the `secure` helper's confirmation output was lost when the session was interrupted |
| fn+Left | one plain Left, then fn with Left arriving as Home | — | pass: the fn translation map applies |
| Caps Lock | two plain `a`s, no Caps Lock event | — | not applicable: the owner does not use Caps Lock on this Mac. No software remap exists (`HIDKeyboardModifierMappingPairs` is empty on every keyboard), so capture bypasses nothing. |
| Key repeat: hold `k` | one `k` down, 61 auto-repeats, one up | — | pass |

Not covered by any host run:

- the Keychron's foreground side (its app-side actions passed in the first session);
- a consuming tap with a physical keyboard (the owner's terminal may not install an event
  tap);
- confirmation that Secure Input was on during this rerun.

These rest on the VM matrix and the owner's acceptance.
