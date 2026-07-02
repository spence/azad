# Azad Surface Design Specification

This document is a handoff brief for a design agent. Its job is to describe the
current Azad product surfaces, the user workflows they support, and the states a
redesign must account for.

Azad is a native macOS menu bar dictation app. The core experience is fast
speech-to-text insertion into the currently focused app, with optional always-on
listening, transcript history recall, and a Claude connector that routes spoken
queries to a local agent gateway.

## Design Goal

Design Azad as a quiet, high-trust macOS utility. It should feel like a tool the
user can leave running all day: compact, predictable, reactive, and legible at a
glance.

The redesign should cover every user-facing surface as one coherent system:

1. Welcome / first-run onboarding.
2. Menu bar icon and dropdown.
3. Settings window and all tabs.
4. Dictation overlay.
5. History browser / search overlay.
6. Ask Claude conversation overlay.

Do not design a marketing landing page. The first-run experience is onboarding
for a local utility, not a product website.

## Product Concepts

### Listen Mode

Azad has two primary dictation modes:

- Manual hold-to-talk: hold the listen shortcut to capture, release to finalize.
- Listen mode: Azad listens for voice activity and starts a turn automatically.

The default listen shortcut is `Option+Space`, but users can change the modifier
set in onboarding. The shortcut is always `Space` plus at least one modifier.

### Finalization

While the user speaks, Azad streams draft transcription to the overlay. When the
turn ends, Azad finalizes the utterance and inserts it into the focused app. A
raw finalize path bypasses the final cleanup path and uses the current
draft/finalizing text immediately.

### Text Insertion

Azad can insert text by:

- Clipboard paste.
- Direct typing.
- Direct typing while also copying the text to the clipboard.

It can optionally append a trailing space after insertion and optionally send an
auto-submit chord after insertion.

### Models And Permissions

Azad needs a local model pack and two macOS permissions:

- Microphone permission for audio capture.
- Accessibility permission for paste / typing automation.

The current default model pack is downloaded to
`~/Library/Application Support/Azad/models` and is not stored in Git.

### History

If history is enabled, Azad stores completed dictations in a searchable local
history index. The current surface is a history browser/search overlay. Some
implementation fields still use "autocomplete" names, but the active user
experience is history browsing, not live autocomplete suggestions.

### Connectors

Azad has one built-in connector today:

- Display name: Claude.
- Trigger phrase: `hey claude`.
- Overlay tag: Claude.

When an enabled utterance starts with `hey claude`, Azad strips the trigger,
routes the cleaned query to the local agent gateway, and shows a conversation
overlay instead of pasting text into the focused app.

## Surface 1: Welcome / First-Run Onboarding

### User Jobs

- Understand what Azad needs before first use.
- Download the required local model pack.
- Grant Microphone and Accessibility permissions.
- Pick the initial listening behavior and shortcut.
- Pick the initial insertion behavior.
- Pick the microphone device.
- Finish setup and start using the app.

### Current Layout And Content

The current onboarding surface is a centered, chromeless AppKit window:

- Size: approximately `640 x 640`.
- Title: `Welcome to Azad`.
- Subtitle: `Let's get you set up - finish below to start dictating.`
- No visible close/minimize/zoom controls.
- Movable by dragging the background.
- Setup completes through `Get started`.

Current rows and controls:

- Model
  - Two-line status: pack display name + size, then install/progress state.
  - `Download` button.
- Start listening
  - Popup: `Automatically`, `Manually (hold shortcut)`.
- Listen shortcut
  - Modifier checkboxes: Shift, Control, Option, Command glyphs.
  - Fixed `+ Space` suffix.
  - At least one modifier is required.
- History
  - Checkbox: `Keep a searchable history of dictations`.
- Insert text by
  - Popup: `Paste`, `Direct`, `Direct + copy to clipboard`.
- Trailing space
  - Checkbox: `Append a space after each insert`.
- Overlay position
  - Popup: `Follow cursor`, `Primary display`, `Active window`.
- Startup
  - Checkbox: `Open Azad automatically at login`.
- Permissions
  - `Accessibility` status row with `Open Settings`.
  - `Microphone` status row with `Open Settings`.
  - Helper copy: `Microphone and Accessibility are required to use Azad.`
- Microphone device
  - Popup of detected input devices.
- Primary CTA
  - `Get started`, Return key equivalent.

### States To Design

- Fresh install, model not downloaded, permissions not granted.
- Model downloading, including visible progress.
- Model installed.
- Model download failed or incomplete.
- Accessibility missing / granted.
- Microphone missing / granted.
- Both permissions granted.
- No microphone devices detected.
- One or more microphone devices detected, with selected device.
- Listen mode set to automatic.
- Listen mode set to manual hold.
- History enabled / disabled.
- Startup enabled / disabled.
- `Get started` disabled because setup requirements are unmet.
- `Get started` enabled.

### Behavior Notes

- Preferences selected during onboarding persist immediately.
- `Get started` is enabled only when a model is ready or downloading and both
  permissions are granted.
- Startup opt-in creates or updates the LaunchAgent preference.
- Clearing the last listen modifier is rejected; the UI syncs back to the last
  valid modifier set.

### Design Output Needed

- First-run onboarding layout.
- Loading/downloading treatment for the model row.
- Permission missing and permission granted treatments.
- Disabled/enabled CTA treatments.
- Compact error treatment for failed or incomplete model setup.
- A clear first-use path that does not require users to understand internal
  model names.

## Surface 2: Menu Bar Icon And Dropdown

### User Jobs

- See that Azad is running.
- Toggle Listen mode quickly.
- See or change the active microphone.
- Open Settings.
- Quit Azad.

### Current Menu Bar Item

- Status item uses a template Azad icon when available.
- If icon loading fails, it falls back to text: `Azad`.

### Current Dropdown Content

- `Listen` custom row with a full-row toggle switch.
- Separator.
- Microphone device header row:
  - Shows the current input device label.
  - Shows a chevron-style expanded/collapsed state.
- Expanded device list:
  - One row per detected device.
  - Checkmark on selected device.
  - Disabled `No input devices` row if none exist.
- Separator.
- `Settings...` menu item.
- `Quit` menu item.

### States To Design

- Listen enabled.
- Listen disabled.
- Listen toggle requested while an active turn is in progress.
- Device list collapsed.
- Device list expanded.
- Current device available.
- Current device missing or no device detected.
- Settings item available.
- Quit item available.

### Behavior Notes

- The device list refreshes when the menu opens.
- The device list collapses when the menu closes.
- Toggling Listen during an active turn may defer the actual mode change until
  the turn boundary.
- Selecting a device persists the preferred input device and updates the capture
  controller.

### Design Output Needed

- Menu bar icon treatment for light/dark menu bars.
- Dropdown layout with compact native-feeling rows.
- Listen toggle treatment, including pending/deferred state.
- Collapsed and expanded microphone picker states.
- Empty-device state.

## Surface 3: Settings Window

### User Jobs

- Adjust core dictation behavior after onboarding.
- Manage model download state.
- Fix permissions.
- Inspect debug metrics.
- Enable or disable connectors.
- Copy build information when reporting issues.

### Current Window Structure

- Native titled window: `Azad Settings`.
- Size: approximately `820 x 460`.
- Left sidebar with tabs:
  - General.
  - Models.
  - Permissions.
  - Debug.
  - Connectors.
- Bottom-right build info footer:
  - Git SHA and build time.
  - Dim, selectable text.

### General Tab

Current controls:

- Checkbox: `Run Azad on startup`.
- `Insert method` popup:
  - `Paste`.
  - `Direct`.
  - `Direct + copy to clipboard`.
- `Auto submit` popup:
  - `Off`.
  - `Enter`.
  - `Ctrl+Enter`.
  - `Shift+Enter`.
- `Overlay position` popup:
  - `Follow cursor`.
  - `Primary display`.
  - `Active window`.
- Checkbox: `Append trailing space after paste`.
- `Removed words`
  - Existing removed words rendered as removable chips.
  - Chip label is the word plus an `x` remove affordance.
  - Text input placeholder: `Enter word`.
  - `Add` button.

States to design:

- Startup enabled / disabled.
- Each insert method selected.
- Each auto-submit mode selected.
- Each overlay position selected.
- Trailing space enabled / disabled.
- Removed words empty.
- Removed words populated.
- Duplicate or empty removed-word entry rejected.

Current gap to address in design:

- Onboarding exposes listen shortcut modifier selection, but Settings does not
  currently expose a way to change the listen shortcut after setup.

### Models Tab

Current controls:

- Model pack display name.
- Model pack description.
- Model status text.
- Progress bar while downloading.
- `Download` button.
- `Cancel` button while downloading.

Current model states:

- Installed.
- Not downloaded.
- Incomplete.
- Downloading with done/total bytes and percent.
- Download error.

### Permissions Tab

Current controls:

- Accessibility status row with `Open Settings`.
- Microphone status row with `Open Settings`.
- Helper copy: `Required to capture audio and insert text. Click Open Settings to grant.`

States to design:

- Accessibility not determined / denied / granted.
- Microphone not determined / denied / granted.
- Both permissions granted.
- One or both permissions missing.

### Debug Tab

Current controls:

- Checkbox: `Enable debug statistics`.
- `Refresh` button.
- Scrollable monospace metrics text area.

States to design:

- Debug statistics enabled / disabled.
- Metrics loaded.
- Metrics failed to load.
- Empty metrics.
- Long metrics requiring scrolling.

### Connectors Tab

Current controls:

- Helper copy: `Open an utterance with a connector's phrase (e.g. "hey claude") to tag it.`
- Checkbox rows for connectors.
- Current built-in row: `Claude`.

States to design:

- Claude enabled.
- Claude disabled.
- Future additional connectors without redesigning the tab.
- No connectors available, if the registry is ever empty.

### Design Output Needed

- Full settings window layout.
- Sidebar and selected-tab treatments.
- Per-tab empty/loading/error/success states.
- Compact forms with native macOS hierarchy.
- Clear copy for model and permission requirements.
- Build-info placement that remains visible but does not compete with controls.

## Surface 4: Dictation Overlay

### User Jobs

- Know when Azad is listening.
- See live transcription as speech streams in.
- See when finalization is running.
- Know whether raw or hold behavior is active.
- Understand errors without losing the current task.
- Keep speaking while a previous turn finalizes.

### Current Overlay Structure

The current overlay is a non-activating, borderless AppKit panel:

- Transparent window with a dark rounded card.
- Above the normal menu/window level.
- Can join all spaces and fullscreen auxiliary spaces.
- Width responds to screen size, roughly `300-620` points.
- Height grows with content, roughly `60-540` points.
- Default position is horizontally centered near the bottom of the target
  screen.
- Target screen comes from the `Overlay position` preference:
  - Follow cursor.
  - Primary display.
  - Active window.
- User can move the overlay by dragging the background.

Visual elements:

- Centered multiline transcription text.
- Audio activity wave behind the text.
- Busy border / rotating border treatment while finalizing or thinking.
- Optional connector chip at top-left.
- Optional `raw` badge.
- Optional `hold` badge.

### States To Design

- Hidden / idle.
- Manual hold active, no text yet.
- Manual hold active with live streaming text.
- Listen mode voice-activity turn active with live streaming text.
- Live speech with no connector.
- Live speech with Claude connector latched.
- Finalizing current turn.
- Finalizing with visible draft text.
- Raw finalize available / raw mode active.
- Hold badge visible.
- Split overlay:
  - Previous turn finalizing in the upper/read-only lane.
  - New turn streaming in the lower/live lane.
- Listen toggle notice:
  - `Listen ENABLED`.
  - `Listen DISABLED`.
- Permission or paste error notice.
- Missing model notice.
- Stream/runtime error notice.
- Empty/no-speech cleanup, where the overlay closes quietly.

### Behavior Notes

- Holding the listen shortcut starts capture immediately whether Listen mode is
  on or off.
- Releasing the listen shortcut finalizes and inserts if speech was captured.
- If no speech was captured, the overlay closes without inserting.
- When Listen mode is on and voice activity already started the turn, holding
  the shortcut acts as a manual silence override. Releasing the shortcut does
  not force-finalize that VAD-started turn.
- If new speech begins while a previous turn is finalizing, Azad can show split
  lanes so the previous turn can finish while the new turn remains live.
- The overlay must never hide the fact that finalization or gateway thinking is
  in progress.

### Design Output Needed

- Base dictation overlay design.
- Streaming text state.
- Finalizing state with a visible busy indicator.
- Split-lane state.
- Raw/hold badge treatments.
- Connector chip treatment.
- Notice/error states.
- Motion guidance for the audio activity wave and busy indicator.

## Surface 5: History Browser / Search Overlay

### User Jobs

- Recall a previous dictation without leaving the keyboard.
- Search local dictation history.
- Preview, expand, and paste a selected transcript.
- Exit without changing the focused app.

### Current Entry Point

Open history by holding the listen shortcut and pressing `Up` while the overlay
is visible. With the default shortcut, this is:

`Hold Option+Space`, then press `Up`.

History owns the overlay while browsing. It cancels the in-flight capture,
pauses capture, and enables keyboard input capture for search.

### Current Layout And Content

The history browser uses the same dark overlay card family as dictation:

- A list of saved transcripts.
- Selected row highlighted in deep blue.
- Transcript body text, currently capped to two lines in list mode.
- Search match highlights in translucent yellow.
- Right-side metadata per row:
  - Character count.
  - Compact time-ago timestamp.
  - Expand marker when truncated.
- Search bar pinned to the bottom.
- Custom blinking caret in the search bar.
- Empty-state message.

Current empty states:

- `No transcripts` when history has no entries.
- `No matches` when a search query filters to zero entries.

### States To Design

- History enabled with entries.
- History enabled but no entries.
- Search query empty.
- Search query active with matching rows.
- Search query active with no matches.
- Selected row in normal list mode.
- Selected row truncated with expand affordance.
- Selected row expanded.
- Long expanded transcript constrained by overlay max height.
- Keyboard focus in search.
- Click outside overlay dismisses history.
- Paste selected transcript and close.
- Exit without paste.

### Behavior Notes

- `Up` selects an older transcript.
- `Down` selects a newer transcript.
- `Right` expands the selected item only if it is truncated.
- `Left` collapses expanded view.
- Typing filters history.
- `Backspace` deletes a search character.
- `Option+Backspace` deletes a search word.
- `Command+Backspace` clears search.
- `Enter` pastes the selected transcript and exits.
- `Esc` exits without pasting.
- Pressing the listen shortcut while in history exits history and starts a fresh
  dictation turn.
- Pasted history text still uses removed-words filtering, trailing-space
  behavior, insertion method, and auto-submit settings.

### Design Output Needed

- History browser layout.
- Search field treatment.
- Row anatomy: body, highlight, timestamp, character count, expand marker.
- Empty history and no-match states.
- Expanded transcript state.
- Keyboard-focused state.
- Dismissal and paste transition guidance.

## Surface 6: Ask Claude Conversation Overlay

### User Jobs

- Route a spoken query to Claude without typing or pasting.
- See that the query has been routed to Claude.
- See thinking, streaming, done, and error states.
- Continue the conversation with voice follow-ups.
- Dismiss the conversation.

### Current Entry Point

If the Claude connector is enabled and the utterance starts with `hey claude`,
Azad strips `hey claude` from the query and routes the cleaned query to the local
agent gateway instead of pasting text.

Example:

- User says: `hey claude summarize this file`.
- Query shown/routed: `summarize this file`.

### Current Layout And Content

The conversation view uses the same overlay card family as dictation:

- Connector chip pinned near the top.
- Chip text includes Claude plus gateway model/effort metadata.
- User query shown near the top.
- Divider line between query and lower content when needed.
- Lower content area:
  - `Thinking...` status.
  - Streaming reply.
  - Done reply.
  - Error message.
- Reply text is scrollable when it exceeds the card budget.
- Voice activity strip pinned to the bottom for follow-up dictation.
- Busy border during thinking/streaming.

### States To Design

- Claude connector detected during live dictation.
- Query finalized and submitted.
- Thinking, no reply yet.
- Streaming reply.
- Done reply.
- Error reply.
- Long reply with scroll.
- User starts a follow-up while the conversation is still visible.
- Follow-up query shown before reply.
- Conversation dismissed with `Esc`.

### Current Error Cases To Accommodate

- Gateway did not acknowledge the request.
- Gateway unavailable or local daemon not running.
- Browser adapter unavailable.
- Connection lost.
- Approval required.
- Protocol mismatch.
- Claude stopped responding.

### Behavior Notes

- While a gateway conversation is live, follow-up dictation continues that
  conversation.
- `Esc` closes the gateway conversation.
- `Option+Enter` during a gateway turn or live conversation submits the current
  query to the gateway instead of raw-pasting it.
- Conversation mode is mutually exclusive with the normal speech overlay and
  history browser.

### Design Output Needed

- Conversation overlay layout.
- Connector chip and metadata treatment.
- Query/reply hierarchy.
- Thinking and streaming treatments.
- Error states.
- Long reply scroll behavior.
- Follow-up dictation state.

## Keyboard Workflow

The listen shortcut is always `Space` plus one or more modifiers. The default is
`Option+Space`. Tables below use the default shortcut.

### Core Dictation

| Action | Keys | Result |
|---|---|---|
| Hold to talk | Hold `Option+Space` | Starts capture immediately, whether Listen mode is on or off. |
| Finish held dictation | Release `Option+Space` | If speech was captured, finalizes and inserts text. If no speech was captured, closes quietly. |
| Toggle Listen mode | Quickly double-tap `Option+Space` | Toggles always-listening without pasting. |
| Finalize now | `Enter` or `Numpad Enter` while overlay is actionable | Finalizes the current turn immediately. |
| Raw finalize | `Option+Enter` or `Option+Numpad Enter` | Uses current draft/finalizing text immediately, bypassing normal final cleanup. |
| Soft newline passthrough | `Shift+Enter` | Lets the focused app receive the chord; Azad does not claim it. |
| Cancel current turn | `Esc` | Discards the current overlay/turn without pasting. |

### History Browser

| Action | Keys | Result |
|---|---|---|
| Open history | Hold `Option+Space`, then press `Up` while overlay is visible | Cancels capture and opens transcript history. |
| Move older | `Up` | Selects an older transcript. |
| Move newer | `Down` | Selects a newer transcript. |
| Expand selected | `Right` | Expands selected item if truncated. |
| Collapse selected | `Left` | Collapses expanded item. |
| Search | Type normally | Filters history. |
| Delete search character | `Backspace` | Deletes one character. |
| Delete search word | `Option+Backspace` | Deletes one word. |
| Clear search | `Command+Backspace` | Clears the search query. |
| Paste selected | `Enter` or `Numpad Enter` | Pastes selected transcript and exits history. |
| Exit history | `Esc` | Closes history without pasting. |
| Start fresh dictation | `Option+Space` while in history | Exits history and starts a new dictation turn. |

### Ask Claude

| Action | Keys / Speech | Result |
|---|---|---|
| Start Claude query | Say `hey claude ...` | Routes cleaned query to local gateway instead of pasting. |
| Submit raw/current query | `Option+Enter` | Sends current query to gateway. |
| Continue conversation | Speak while conversation overlay is live | Sends follow-up into the same conversation. |
| Close conversation | `Esc` | Dismisses the gateway conversation. |

### Key Capture Boundaries

- Azad should claim overlay keys only when the relevant overlay context is
  active.
- `Up` only opens history while the listen shortcut is actively held and the
  overlay is visible.
- Voice-activity-only overlays should not hijack `Up` or `Down` from the
  focused app.
- When the overlay is open from manual hold, releasing `Option` before `Space`
  must not leak a literal space into the focused app.
- When Listen mode is on and no manual overlay capture is active, normal typing
  in the focused app, including spaces, should remain unaffected.

## Cross-Surface Requirements

- Preserve native macOS utility feel.
- Prefer compact, stable layouts over decorative marketing composition.
- Make critical state visible: recording, finalizing, downloading, permission
  missing, gateway thinking, and errors.
- Avoid relying on hidden tooltip-only instruction. The user should not need to
  hover to understand the main state.
- Avoid adding new instructional copy inside the overlay during normal use; the
  overlay should stay task-focused.
- Surfaces should work in both light and dark macOS appearances, even where the
  overlay itself remains dark.
- Text must fit in controls at normal macOS window sizes.
- Long model names, device names, transcript rows, and gateway replies must
  truncate, wrap, or scroll predictably.
- Keyboard workflows must remain deterministic and discoverable enough that a
  user can operate dictation without using the mouse.
- The design should be coherent across all overlay modes: dictation, history,
  and Ask Claude should feel related but clearly distinguishable.

## Design Deliverables Requested From The Design Agent

For each of the six surfaces, produce:

- Default layout.
- Empty state.
- Loading/progress state, where applicable.
- Error or blocked state.
- Success/ready state.
- Keyboard-focused state, where applicable.
- Notes on animation/motion for busy indicators and audio activity.
- Component inventory and reusable tokens.
- Copy recommendations.
- Accessibility considerations.

The design should show these surfaces as a whole application system, not as
isolated mockups.

## Open Product Questions

These are not blockers for design exploration, but the design agent should make
them visible:

- Should onboarding allow `Get started` while a model is still downloading, or
  should setup require a fully installed model?
- Should the history surface be named "History", "Transcript History", or
  something closer to "Recent Dictations"?
- Should connector settings show trigger phrases inline, or only display the
  connector name?
- Should Settings include the listen shortcut control that currently exists
  only in onboarding?
- Should Ask Claude remain visually inside the dictation overlay family, or
  should it gain a more distinct conversation identity?
- Should model names be exposed to first-time users, or should onboarding speak
  in terms of "speech model" and reserve model details for Settings?

## Source Anchors

Current behavior is implemented or documented in:

- `README.md`
- `docs/README.md`
- `crates/azad/docs/keyboard-workflow.md`
- `crates/azad/docs/keyboard-shortcut-state-machine.md`
- `crates/azad/SPECIFICATION.md`
- `crates/azad/src/platform.rs`
- `crates/azad/src/app.rs`
- `crates/azad/src/app/history.rs`
- `crates/azad/src/app/settings_ui.rs`
- `crates/azad/src/connectors.rs`
- `crates/azad/src/settings.rs`
