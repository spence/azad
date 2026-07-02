# Build and Run (macOS)

## Prerequisites

- macOS (Aqua session for launchd UI agent)
- Rust toolchain installed
- `just` installed (`brew install just`)

## Public Install

Install the signed and notarized DMG from GitHub Releases for normal use.

## Development Quickstart

```bash
just install
just start
just status
```

The first source install can take several minutes while Rust crates, Swift MLX
packages, and MLX Metal kernels build. Subsequent installs are much faster
because the local build caches are warm.

## Common Operations

```bash
just restart
just stop
just logs
just uninstall
```

## Optional Overrides

`just install` works without local signing config. Optional machine-specific
install settings go in `.codesign.env` at the workspace root. The file is
ignored by Git.

```bash
cp .codesign.env.example .codesign.env
```

Supported settings:

- `AZAD_BUILD_PROFILE=release`
- `AZAD_APP_DIR="$HOME/Applications/Azad.app"`
- `AZAD_CODESIGN_IDENTITY="<40-character certificate hash>"`

When `AZAD_CODESIGN_IDENTITY` is unset, `just install` installs an unsigned
development build and does not run `codesign`. On Apple silicon, the executable
may still be linker-signed ad-hoc; that is not a stable app-bundle signature for
macOS permission preservation.

Explicit environment variables override values from `.codesign.env`.

## Release Builds

Maintainer release builds use the separate distribution path:

```bash
cp .codesign.env.example .codesign.env
just dist
```

`just dist` requires a Developer ID Application certificate and a notarytool
profile. It signs with hardened runtime, notarizes, staples, and creates
`dist/Azad-<version>.dmg`.

Explicit environment variables override values from `.codesign.env`.

## Permissions

- Microphone permission is required for transcription.
- Accessibility permission is required for synthetic paste into the focused app.

Reset and relaunch:

```bash
just reset-permissions
just restart
```
