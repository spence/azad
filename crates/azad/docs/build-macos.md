# Build and Run (macOS)

## Prerequisites

- macOS (Aqua session for launchd UI agent)
- Rust toolchain installed
- `just` installed (`brew install just`)

## Quickstart

```bash
just install
just start
just status
```

## Common Operations

```bash
just restart
just stop
just logs
just uninstall
```

## Optional Overrides

```bash
AZAD_BUILD_PROFILE=release just install
AZAD_APP_DIR="$HOME/Applications/Azad.app" just install
AZAD_CODESIGN_IDENTITY="Azad Dev Code Signing Root" just install
```

## Permissions

- Microphone permission is required for transcription.
- Accessibility permission is required for synthetic paste into the focused app.

Reset and relaunch:

```bash
just reset-permissions
just restart
```
