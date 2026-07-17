# Keyboard Workflow

This document is the user-facing keyboard contract for Azad. It describes what
keys do in normal use. The engineering state-machine details live in
`keyboard-shortcut-state-machine.md`.

The listen shortcut is always `Space` plus one or more modifiers. The default is
`Option+Space`. Users can change the modifier set in onboarding or settings; the
tables below use the default shortcut name.

## Core Dictation

| Action | Keys | Result |
|---|---|---|
| Hold to talk | Hold `Option+Space` | Starts capture immediately, whether Listen mode is on or off. |
| Finish held dictation | Release `Option+Space` | If speech was captured, finalizes and pastes/types into the focused app. If no speech was captured, closes quietly. |
| Toggle Listen mode | Quickly double-tap `Option+Space` | Toggles always-listening on or off without pasting. |
| Finalize now | `Enter` or `Numpad Enter` while the overlay is actionable | Finalizes the current turn immediately. |
| Raw finalize | `Option+Enter` or `Option+Numpad Enter` | Uses the current draft/finalizing text immediately, bypassing the normal final cleanup path. |
| Soft newline passthrough | `Shift+Enter` | Lets the focused app receive the chord; Azad does not claim it. |
| Cancel current turn | `Esc` | Discards the current overlay/turn without pasting. |

## Listen Mode

When Listen mode is off, `Option+Space` is hold-to-talk.

When Listen mode is on, Azad can auto-start capture from voice activity. If a
voice-activity-started turn is already active, holding `Option+Space` acts as a
manual silence override: capture stays live through silence, and releasing the
shortcut does not force-finalize that turn.

The double-tap Listen toggle only applies before transcription has started for
the current turn. If speech has already started, the second press stays in the
active capture flow instead of toggling Listen mode.

## Overlay And Paste

When final text is ready, Azad inserts it into the currently focused app using
the configured paste method:

- clipboard paste,
- direct typing,
- or direct typing while also copying the text to the clipboard.

If auto-submit is enabled, Azad sends the configured submit chord after inserting
text: `Enter`, `Control+Enter`, or `Shift+Enter`.

If a previous turn is finalizing while new speech begins, the overlay can show
split lanes: the older lane finalizes and pastes while the newer lane remains
live.

## History Browser

History must be enabled for these shortcuts to have useful entries.

| Action | Keys | Result |
|---|---|---|
| Open history | Hold `Option+Space`, then press `Up` | Cancels the in-flight capture, stops capture while browsing, and opens transcript history even before speech text appears. You can release `Option+Space` after history opens. |
| Move older | `Up` | Selects an older transcript. |
| Move newer | `Down` | Selects a newer transcript. |
| Expand selected item | `Right` | Expands the selected item if it is truncated. |
| Collapse expanded item | `Left` | Collapses an expanded item back to list view. In list view, it is a no-op. |
| Search history | Type normally | Filters the history list. |
| Delete search character | `Backspace` | Deletes one search character. |
| Delete search word | `Option+Backspace` | Deletes one search word. |
| Clear search | `Command+Backspace` | Clears the search query. |
| Paste selected item | `Enter` or `Numpad Enter` | Pastes the selected transcript and exits history. |
| Exit history | `Esc` | Closes history without pasting. |
| Start fresh dictation | `Option+Space` while in history | Exits history without pasting and starts a new dictation turn. |

In expanded history view, `Up` and `Down` first collapse back to list view and
then move the selection.

## Connector Workflow

Azad has a built-in Claude connector. If an enabled turn starts with
`hey claude`, the connector latches for that turn, the trigger phrase is stripped
from the query, and finalizing routes the query to the local gateway instead of
pasting into the focused app.

While a gateway conversation is live, follow-up dictation continues that
conversation. `Esc` closes the gateway conversation.

`Option+Enter` during a gateway turn or live gateway conversation submits the
current query to the gateway instead of raw-pasting it.

## Key Capture Boundaries

Azad claims overlay keys only while the relevant overlay context is active. For
example, `Up` only opens history while the listen shortcut is actively held and
the overlay is visible; voice-activity-only overlays do not hijack `Up` or
`Down` from the focused app.
