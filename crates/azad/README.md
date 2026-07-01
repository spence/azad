# Azad (macOS background app)

Development workflow now targets a launchd-managed app instance.

For normal non-development installs, use the signed and notarized DMG from
GitHub Releases. The commands below are for source development.

## Commands

```bash
just install   # build + install ~/Applications/Azad.app and LaunchAgent plist
just start     # start/restart via launchctl
just stop      # stop via launchctl
just restart   # stop + start
just status    # launchctl print
just logs      # tail stdout/stderr logs
just uninstall # remove LaunchAgent plist (keeps app bundle)
```

## Defaults

- App bundle: `~/Applications/Azad.app`
- LaunchAgent: `~/Library/LaunchAgents/ai.azad.plist`
- Logs: `~/Library/Logs/Azad/{stdout,stderr}.log`

## Prerequisites

- Rust toolchain
- `just` (`brew install just`)

## Microphone Permission

Azad requires macOS microphone permission.

- Reset permission prompt:
  - `tccutil reset Microphone ai.azad`
- Then restart Azad:
  - `just restart`

Azad also checks Accessibility permission on startup (required for auto-paste) and opens the Accessibility settings pane if it is missing.

## Behavior Docs

- `docs/keyboard-shortcut-state-machine.md` - Hotkey/VAD interaction rules and transitions.
- `SPECIFICATION.md` - architecture, design decisions, subsystem boundaries, and change playbooks.
