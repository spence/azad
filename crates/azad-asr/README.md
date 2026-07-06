# azad-asr

Terminal live speech-to-text (English) using:
- `cpal` (microphone capture)
- MLX Nemotron streaming/final ASR via the bundled `azad-mlx-asr` helper
- CoreML Silero VAD v6.2.1 for utterance start/stop via the bundled `azad-mlx-asr` helper
- `hound` for WAV input and `symphonia` for common compressed formats (e.g., `.m4a`) in `transcribe-file`

## Specification

- `SPECIFICATION.md` - architecture, design decisions, runtime contracts, and change playbooks.
- `../../docs/README.md` - repository-wide documentation index.

## Prereqs

- macOS 14+ for the bundled MLX/CoreML helper.
- `just swift-build` must succeed before using MLX ASR or CoreML VAD from the CLI.
- The runtime model files must exist either under local `models/` paths or explicit CLI paths.

## Models

### MLX Nemotron + VAD

The expected layout is:

- `models/nemotron-mlx/`:
  - `config.json`
  - `model.safetensors`
  - `tokenizer.model`
  - `vocab.txt`
- `models/vad/silero_vad.mlmodelc/`:
  - `analytics/coremldata.bin`
  - `coremldata.bin`
  - `metadata.json`
  - `model.mil`
  - `weights/weight.bin`

The macOS app downloads the same files into
`~/Library/Application Support/Azad/models/nemotron-3.5-mlx-bf16-v1/`.
For CLI work, either mirror those files into `models/nemotron-mlx` or pass
`--mlx-model-dir` and `--vad-model`.

Replay tests use the same local model paths and are ignored by default. Run:

```bash
just test-replay
```

For maintainer verification that must fail when models are absent:

```bash
just test-replay-required
```

## Build

```bash
cargo build -p azad-asr
```

The Swift helper is built during app install and is used for both MLX ASR and
CoreML VAD. No `whisper.cpp` checkout or build dependency is required.

Build the helper directly from the repository root with:

```bash
just swift-build
```

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
  --vad-model "$HOME/Library/Application Support/Azad/models/nemotron-3.5-mlx-bf16-v1/vad/silero_vad.mlmodelc"
```

Transcribe a recording through the exact same pipeline:

```bash
cargo run -p azad-asr -- transcribe-file ./path/to/recording.m4a
```

Replay a saved debug WAV and print the renderer event stream:

```bash
cargo run -p azad-asr -- transcribe-file \
  "$HOME/Library/Application Support/Azad/debug-recordings/<recording-id>.wav" \
  --events-jsonl
```

Quit with `q` (or `Ctrl-C`).
