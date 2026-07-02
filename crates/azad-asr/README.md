# azad-asr

Terminal live speech-to-text (English) using:
- `cpal` (microphone capture)
- MLX Nemotron streaming/final ASR via the bundled `azad-mlx-asr` helper
- Silero VAD (ggml) for utterance start/stop via `whisper-cpp-plus`
- `hound` for WAV input and `symphonia` for common compressed formats (e.g., `.m4a`) in `transcribe-file`

## Specification

- `SPECIFICATION.md` - architecture, design decisions, runtime contracts, and change playbooks.

## Prereqs

- macOS tested (should work on other platforms supported by `cpal`)

## Models

### MLX Nemotron + VAD

The expected layout is:

- `models/nemotron-mlx/`:
  - `config.json`
  - `model.safetensors`
  - `tokenizer.model`
  - `vocab.txt`
- `models/vad/ggml-silero-v6.2.0.bin`

The macOS app downloads the same files into
`~/Library/Application Support/Azad/models/nemotron-3.5-mlx-bf16-v1/`.
For CLI work, either mirror those files into `models/nemotron-mlx` or pass
`--mlx-model-dir` and `--vad-model`.

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

With explicit model paths:

```bash
cargo run -p azad-asr -- listen --select-device \
  --mlx-model-dir "$HOME/Library/Application Support/Azad/models/nemotron-3.5-mlx-bf16-v1/mlx" \
  --vad-model "$HOME/Library/Application Support/Azad/models/nemotron-3.5-mlx-bf16-v1/vad/ggml-silero-v6.2.0.bin"
```

Transcribe a recording through the exact same pipeline:

```bash
cargo run -p azad-asr -- transcribe-file ./path/to/recording.m4a
```

Quit with `q` (or `Ctrl-C`).
