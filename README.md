# Azad

Azad is a macOS menu bar dictation app with local VAD, streaming ASR, final-pass
refinement, overlay feedback, and paste automation.

## Install

For normal use, install the signed and notarized DMG from GitHub Releases. That
is the supported path for people who are not developing Azad, because the app
has a stable Developer ID signature and macOS permissions survive app updates.

## Source Development

Source installs are for development:

```bash
git clone https://github.com/spence/azad.git
cd azad
just doctor
just install
just start
just status
```

On first launch, Azad opens onboarding and downloads its MLX Nemotron model pack
into `~/Library/Application Support/Azad/models`. The default pack is about
1.3 GB and is not stored in Git.

## Requirements

- macOS 14 or newer
- Rust stable, with Rust 2024 edition support
- Xcode Command Line Tools
- Full Xcode for the MLX Metal toolchain used by source installs
- `just`
- `cmake`
- network access to crates.io, GitHub, and Hugging Face for dependencies/models

Homebrew setup:

```bash
xcode-select --install
brew install just cmake
```

If `just install` cannot find Apple’s `metal` compiler, it will try to use
`/Applications/Xcode.app` and download Xcode’s Metal Toolchain component.

## Repository Layout

```text
crates/
  azad/       macOS menu bar app, onboarding, settings, overlay, hotkeys
  azad-asr/   owned ASR engine crate used by the app and CLI
```

The workspace no longer uses Git submodules. Owned code lives in this repo. Forked
or upstream third-party crates are Cargo dependencies pinned in `Cargo.lock`.

Current notable dependency choices:

- `azad-mlx-asr` is a bundled Swift helper that runs
  `mlx-community/nemotron-3.5-asr-streaming-0.6b` with MLXAudio Swift. Source
  installs build this helper during `just install`.
- `whisper-cpp-plus` is used for Silero VAD only. Azad does not use Whisper for
  speech-to-text. Its sys crate downloads the pinned `whisper.cpp` source during
  build when needed.

## Common Commands

```bash
just check      # cargo check --workspace
just test       # cargo test --workspace
just install    # build and install ~/Applications/Azad.app
just start      # start launchd service
just restart    # restart launchd service
just status     # print launchd status
just logs       # tail app logs
just dist       # maintainer-only signed/notarized DMG build
```

## Local Signing

`just install` works without local signing config. By default it installs an
unsigned development build and does not run `codesign`.

To preserve macOS Microphone/Accessibility permissions across rebuilds on a
development machine, copy `.codesign.env.example` to `.codesign.env` and set
`AZAD_CODESIGN_IDENTITY` to a local codesigning certificate hash.
Explicit environment variables override values from `.codesign.env`.

On Apple silicon, the linker may still leave a per-binary ad-hoc signature on
the executable. That is not a stable app-bundle signature and should not be used
for TCC permission preservation.

## Public Releases

Public builds are produced from tags by `.github/workflows/release.yml`. The
workflow imports a Developer ID certificate from GitHub Actions secrets, signs
the app with hardened runtime, notarizes/staples the app and DMG, verifies the
result, and uploads the DMG to the GitHub Release.

Required release secrets:

- `APPLE_DEVELOPER_ID_CERTIFICATE_BASE64`
- `APPLE_DEVELOPER_ID_CERTIFICATE_PASSWORD`
- `APPLE_KEYCHAIN_PASSWORD`
- `APPLE_ID`
- `APPLE_TEAM_ID`
- `APPLE_APP_SPECIFIC_PASSWORD`

For local release builds, `.codesign.env` provides defaults and explicit environment
variables override values from that file.

## Permissions

Azad needs:

- Microphone permission for transcription
- Accessibility permission for paste automation

Use `just reset-permissions` to reset prompts during development.

## Usage

- [Keyboard workflow](crates/azad/docs/keyboard-workflow.md)

## License

MIT. See [LICENSE](LICENSE).
