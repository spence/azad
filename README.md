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

The first source install can take several minutes because it builds the Rust app,
resolves the Swift MLX dependency graph, and compiles the bundled MLX Metal
kernels. Later installs reuse local build caches.

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

## Build Commands

```bash
just verify                 # doctor + fmt + check + test + Swift helper build + Clippy
just test-replay            # run ignored ASR replay tests when local models are available
just test-replay-required   # same, but fail if replay models are missing
just check                  # cargo check --workspace
just fmt-check              # cargo fmt --all --check
just test                   # cargo test --workspace
just clippy                 # cargo clippy --workspace --all-targets -- -D warnings
just swift-build            # build the bundled Swift MLX/CoreML helper
just install                # build and install ~/Applications/Azad.app
just start                  # start launchd service
just restart                # restart launchd service
just status                 # print launchd status
just logs                   # tail app logs
just dist                   # maintainer-only signed/notarized DMG build
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

For local release builds, `.codesign.env` provides defaults and explicit environment
variables override values from that file.

## Permissions

Azad needs:

- Microphone permission for transcription
- Accessibility permission for paste automation

Use `just reset-permissions` to reset prompts during development.

## Usage

- [Documentation index](docs/README.md)
- [Keyboard workflow](crates/azad/docs/keyboard-workflow.md)

## License

MIT. See [LICENSE](LICENSE).

## Changelog

### 0.2.0

- Replaced Parakeet ASR with [Nemotron 3.5 ASR Streaming 0.6B](https://huggingface.co/mlx-community/nemotron-3.5-asr-streaming-0.6b) on MLX for streaming and finalization.
- Replaced ggml VAD with [Silero VAD v6.2.1 CoreML](https://huggingface.co/aufklarer/Silero-VAD-v6.2.1-CoreML).
- Removed idle CPU spinning by blocking audio capture instead of polling, based on Andrew Schreiber's closed PR [#1](https://github.com/spence/azad/pull/1).
- Cleaned up the project for public source installs with owned crates, no submodules, and focused app/ASR/platform modules.

### 0.1.1

- Added "Hey Claude" support through the external Local Agent Gateway project.
- Added streamed Claude replies, follow-up turns, and gateway error handling in the overlay.
- Added transcript history lookup with keyboard navigation, search, expansion, timestamps, and paste-on-Enter.
- Added configurable listen modifiers, removed-words settings, and trailing-space paste behavior.

### 0.1.0

- Initial macOS menu bar dictation release with local capture, overlay feedback, paste automation, and model downloads.
- Used [Parakeet realtime EOU 120M ONNX](https://huggingface.co/altunenes/parakeet-rs) for streaming/end-of-utterance text.
- Used [Parakeet TDT 0.6B v3 ONNX](https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx) for finalization.
- Used [Silero VAD v6.2.0 ggml](https://huggingface.co/ggml-org/whisper-vad) through the old whisper.cpp-era runtime path.
