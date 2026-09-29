# Release-candidate verification

Verification date: 2026-09-29 UTC. Product source: commit `6ded6d1` on
`feat/device-capture-integration`; matrix evaluator at `7187d4c` plus ssh retries. Everything
below ran against one build in disposable, SIP-enabled macOS 26.6.2 VMs. Nothing ran on the host
desktop. Raw outputs are in
[`evidence/2026-09-29-release-candidate/`](evidence/2026-09-29-release-candidate/).

## Build under test

`Azad.app` built by the repository's `just install` script (staged with `AZAD_APP_DIR`) and
signed with the Developer ID of team `35A87BDK48`; the helper is embedded as
`Contents/Library/Helpers/Azad Capture.app` and registered by the app through `SMAppService`.

| Artifact | Identity |
|---|---|
| `Azad.app` | `ai.azad`, CDHash `bb32743888e8e867adfa3ce8ed9b4d2bed8217d0`, executable SHA-256 `9b9ae340…2425853` |
| `Azad Capture.app` | `ai.azad.capture`, hardened runtime, CDHash `ebb686bdfd9a87c6c927162fc63cf06b3e09bb53`, executable SHA-256 `b4d7a364…828f109e` |
| Virtual keyboard driver | `org.pqrs.Karabiner-DriverKit-VirtualHIDDevice` 1.8.0 (team `G43BCU2T37`); package 8.6.0, SHA-256 `ff8c7fdc…47424387` (main VM); Karabiner-Elements 16.3.0 with package 8.5.0 (coexistence VM) |

Full values: [`identities.txt`](evidence/2026-09-29-release-candidate/identities.txt).

## Fresh installation and onboarding

The main VM started from a clean slate: Azad removed, its background item removed from Login
Items & Extensions, Input Monitoring grants reset, guest rebooted. Then:

1. Installing and launching `Azad.app` registered the helper; `SMAppService` reported
   `RequiresApproval`, Azad opened Login Items & Extensions and showed "Keyboard shortcuts are
   off - Allow Azad in Login Items & Extensions"
   ([screenshot](evidence/2026-09-29-release-candidate/onboarding/01-login-items-approval.png)).
2. After allowing "Azad", launchd ran the helper from inside the bundle
   (`program identifier = Contents/Library/Helpers/Azad Capture.app/...`). It reported
   `permission_denied` with no devices, and Azad raised macOS's "Azad Capture would like to
   receive keystrokes" prompt
   ([screenshot](evidence/2026-09-29-release-candidate/onboarding/04-input-monitoring-prompt.png)).
3. In that state the no-permission negative control ran (below): nothing seized, no actions,
   claimed keys reached the foreground.
4. Open System Settings listed "Azad Capture" switched off; one switch granted it. The helper
   re-executed into the grant by itself (`permission_granted_restart`) and reported `capturing`
   ([record](evidence/2026-09-29-release-candidate/onboarding/permission-wait-granted.json)).

## Upgrade

Reinstalling `Azad.app` over itself replaced the helper binary. The running helper exited on
its own, launchd started the new one (PID 3468 -> 3718), the Input Monitoring grant stayed
(`ai.azad.capture|2`) with no prompt, capture resumed, and `app_hold_release` passed
([record](evidence/2026-09-29-release-candidate/upgrade/)). Earlier in development one grant
also stayed effective across rebuilt helpers with different code.

## Results

Every scenario requires its source keyboard to post its whole script, so no check can pass
because nothing happened. The scenario definitions and all checks are in
`crates/azad-capture/tests/vm/run_matrix.py`; the helper-only scenarios ran against the
installed helper with Azad quit so the fixture client was the helper's client.

### Interference and access (fixture client)

| Scenario | Condition | Result |
|---|---|---|
| `baseline` | Secure Input off, no competing tap. | pass (9 checks) |
| `secure_throughout` | Secure Input enabled for the entire sequence. | pass (10 checks) |
| `consuming_tap` | An earlier consuming HID event tap swallows every key event it sees. | pass (10 checks) |
| `secure_transition_consuming_tap` | Secure Input turns on during the first Space hold and off before Return, with a consuming HID tap installed. | pass (11 checks) |
| `cooperative_remapper` | A preexisting exclusive owner (Karabiner-style remapper) seizes the keyboard and re-emits it through its own driver keyboard; Azad captures that output. | pass (11 checks) |
| `after_helper_restart` | The helper is restarted by launchd before the sequence; the existing Input Monitoring grant must still yield per-device report flow. | pass (9 checks) |
| `crash_during_hold` | The helper is killed (SIGKILL) while Space is held. The kernel must release the keyboard: later ordinary typing reaches the foreground with no stuck modifier, and launchd restarts the helper. | pass (5 checks) |
| `app_hang_releases_context` | The app publishes a history-search context (all typing claimed) and then stops heartbeating while staying connected. After the lease lapses the helper keeps only the listen chord: typing and Return reach the foreground again. | pass (5 checks) |
| `karabiner_elements` | Karabiner-Elements 16.3.0 is installed, owns the VM keyboard and runs its own driver daemon. Azad attaches to that daemon, yields the physical keyboard, captures Karabiner's output keyboard, and Karabiner never grabs Azad's output. | pass (15 checks) |
| `negative_uncooperative_owner` | A preexisting exclusive owner discards input. Azad must report the device as owned by another process and receive nothing. | pass (5 checks) |
| `negative_unauthorized_client` | A client not signed as Azad connects. It must be rejected and capture must not start. | pass (5 checks) |

### Integrated app

| Scenario | Condition | Result |
|---|---|---|
| `app_hold_release` | Option+Space hold with listening off; Option released before Space. | pass (5 checks) |
| `app_double_tap_on` | Two quick Option+Space taps turn always-listening on. | pass (5 checks) |
| `app_double_tap_off` | Two quick Option+Space taps turn always-listening back off. | pass (5 checks) |
| `app_history_search_us` | Hold+Up opens history; the key at HID 0x04 types 'a' on the US layout, filters to 'awerty fixture two', and Enter pastes it into the foreground app. | pass (10 checks) |
| `app_history_search_french` | The same keys on the French (AZERTY) layout type 'q', so history search is resolved with the active layout and pastes 'qwerty fixture one'. | pass (10 checks) |
| `app_overlay_keys` | During a hold: Shift+Return reaches the foreground, Down navigates, keypad Enter finalizes, Escape cancels; none of the claimed keys reach the foreground. | pass (5 checks) |
| `app_caps_lock_and_repeat` | Caps Lock toggles through the virtual keyboard (the next 'a' is capitalized, then not), and holding 'a' auto-repeats with a single release. | pass (8 checks) |
| `app_ordinary_typing` | With the overlay hidden, a, Return, Escape, Up and Shift+A all reach the foreground exactly once and Azad handles none of them. | pass (5 checks) |

### Failure and recovery

| Scenario | Condition | Result |
|---|---|---|
| `app_quit_mid_hold` | The Azad app is killed mid-hold (helper loses its client). | pass (5 checks) |
| `helper_hang_mid_hold` | The helper's main loop hangs mid-hold; its watchdog exits so the kernel releases the keyboards, launchd restarts it, and the app ends the hold. | pass (7 checks) |
| `driver_daemon_killed_mid_hold` | The virtual keyboard daemon dies mid-hold (taking every driver keyboard, including the test source, with it); the helper releases the keyboards and the hold, restarts the daemon, and a new keyboard's typing is forwarded. | pass (5 checks) |
| `forwarding_fails_mid_hold` | Posting to the virtual keyboard fails mid-hold while the keyboard stays connected; the helper stops capturing, so the rest reaches the OS directly. | pass (5 checks) |
| `keyboard_removed_mid_hold` | The keyboard disappears while Option+Space is held; the helper releases its keys and the app ends the hold. A second keyboard then types. | pass (5 checks) |
| `two_keyboards` | Option held on one keyboard, Space pressed on another, then typing on the second: the chord is claimed and typing arrives once. | pass (5 checks) |
| `client_not_console_user` | A correctly signed client running as a user who does not own the console (fast user switching): the helper must not capture for it. | pass (8 checks) |

### Negative control before the Input Monitoring grant

| Scenario | Condition | Result |
|---|---|---|
| `negative_no_permission` | Input Monitoring is revoked. The helper must report permission_denied, seize nothing, and claimed keys reach the foreground (the loss the grant prevents). | pass (5 checks) |

Process-local suites at the same commit: `cargo test -p azad-capture` (50 tests: policy,
engine incl. fn translation, pointer, randomized no-stranding sequences, driver encoding checked
byte for byte against the pinned headers), `cargo test -p azad` (281 tests) and
`just interaction-test` (14 scenarios through the production key policy, including the
delayed-delivery negative control).

## Findings recorded during verification

- A VM that once had a standalone `ai.azad.capture` LaunchDaemon kept a legacy background-item
  record; `SMAppService` then reported `Enabled` for the embedded helper while launchd never
  loaded it. Resetting background items (`sfltool resetbtm`) fixed it. Only machines used for
  that earlier test setup can have the record.
- A guest reboot clears `/tmp`, where the VM fixtures live; the matrix now fails scenarios whose
  source did not play instead of passing them vacuously.

## Not yet proven

- Physical keyboards: the built-in keyboard and Keychron Q6 HE, including F-row and fn
  translation, Globe, Caps Lock LED, mouse keys and key repeat on real hardware.
- Sleep and wake.
- Host deployment of this build.

These need the owner at the Mac; `deployment.md` and `tests/physical/session.py` describe that
session.
