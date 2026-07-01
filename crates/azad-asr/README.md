# azad-asr

Terminal live speech-to-text (English) using:
- `cpal` (microphone capture)
- Parakeet (EOU streaming partials + TDT refinement) via `parakeet-rs`
- Silero VAD (ggml) for utterance start/stop via `whisper-cpp-plus`
- `hound` for WAV input and `symphonia` for common compressed formats (e.g., `.m4a`) in `transcribe-file`

## Specification

- `SPECIFICATION.md` - architecture, design decisions, runtime contracts, and change playbooks.

## Prereqs

- macOS tested (should work on other platforms supported by `cpal`)

## Models

### Parakeet + VAD

Download the dev model assets into `models/{parakeet,vad}`:

```bash
crates/azad-asr/scripts/download-parakeet-models.sh
```

The expected layout is:

- `models/parakeet/eou/`:
  - `encoder.onnx`
  - `decoder_joint.onnx`
  - `tokenizer.json`
- `models/parakeet/tdt/`:
  - `encoder-model.onnx`
  - `encoder-model.onnx.data`
  - `decoder_joint-model.onnx`
  - `vocab.txt`

- `models/vad/ggml-silero-v6.2.0.bin`

## Build

```bash
cargo build -p azad-asr
```

The `whisper-cpp-plus-sys` build script downloads its pinned `whisper.cpp`
source when needed; no `whisper.cpp` submodule checkout is required.

## Run

List devices:

```bash
cargo run -p azad-asr -- devices
```

Listen + transcribe:

```bash
cargo run -p azad-asr -- listen --select-device
```

Transcribe a recording through the exact same pipeline:

```bash
cargo run -p azad-asr -- transcribe-file ./path/to/recording.m4a
```

Quit with `q` (or `Ctrl-C`).
