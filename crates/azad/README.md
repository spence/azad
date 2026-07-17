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

## Behavior Docs

- `docs/keyboard-workflow.md` - User-facing keyboard workflow for dictation, history, and connectors.
- `docs/keyboard-shortcut-state-machine.md` - Hotkey/VAD interaction rules and transitions.
- `docs/isolated-interaction-harness.md` - Process-local interaction validation and safety boundary.
- `SPECIFICATION.md` - architecture, design decisions, subsystem boundaries, and change playbooks.
- `../../docs/README.md` - repository-wide documentation index.
