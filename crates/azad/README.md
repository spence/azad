# Azad (macOS app)

Development workflow installs a local app bundle and starts it directly unless
the user has explicitly enabled startup/login behavior.

For normal non-development installs, use the signed and notarized DMG from
GitHub Releases. The commands below are for source development.

## Commands

```bash
just interaction-test # isolated shortcut/overlay interaction scenarios
just install          # build + install ~/Applications/Azad.app
just start            # start Azad
just stop             # stop Azad
just restart          # stop + start
just status           # print runtime status
just logs             # tail stdout/stderr logs
just uninstall        # stop Azad and remove LaunchAgent plist if present
```

## Defaults

- App bundle: `~/Applications/Azad.app`
- Startup LaunchAgent, only after user opt-in: `~/Library/LaunchAgents/ai.azad.plist`
- Logs: `~/Library/Logs/Azad/{stdout,stderr}.log`

## Prerequisites

- macOS 14 or newer
- Rust toolchain
- Xcode Command Line Tools with Swift
- Full Xcode for the MLX Metal toolchain used by source installs
- `just` (`brew install just`)

## Microphone Permission

Azad requires macOS microphone permission.

- Reset permission prompt:
  - `tccutil reset Microphone ai.azad`
- Then restart Azad:
  - `just restart`

Azad also checks Accessibility permission on startup (required for auto-paste) and opens the Accessibility settings pane if it is missing.

## Keyboard Shortcuts

Shortcuts are captured by Azad's keyboard-capture helper, which works while other apps use
Secure Input or their own keyboard hooks. It needs, once:

- the virtual keyboard driver: `just install-capture-driver` (skip if Karabiner-Elements is
  installed), then allow it in System Settings -> General -> Login Items & Extensions;
- Azad's background item allowed in Login Items & Extensions (Azad opens the pane);
- Input Monitoring for "Azad Capture" (Azad shows the macOS prompt).

`just status` reports the helper's state.

## Behavior Docs

- `docs/keyboard-workflow.md` - User-facing keyboard workflow for dictation, history, and connectors.
- `docs/keyboard-shortcut-state-machine.md` - Hotkey/VAD interaction rules and transitions.
- `docs/isolated-interaction-harness.md` - Process-local interaction validation and safety boundary.
- `SPECIFICATION.md` - architecture, design decisions, subsystem boundaries, and change playbooks.
- `../../docs/README.md` - repository-wide documentation index.
