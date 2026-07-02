# azad-mlx-asr

`azad-mlx-asr` is the Swift helper bundled next to the Azad app binary.

It has two modes:

- ASR mode: runs `mlx-community/nemotron-3.5-asr-streaming-0.6b` through MLXAudio Swift.
- VAD mode: runs Silero VAD v6.2.1 through CoreML.

The Rust runtime starts this helper as a child process and communicates over newline-delimited JSON on stdin/stdout. Stderr is inherited for diagnostics.

## Build

From the repository root:

```bash
just swift-build
```

Equivalent direct command:

```bash
swift build -c release \
  --package-path crates/azad-mlx-asr \
  --scratch-path target/swift/azad-mlx-asr
```

`just install` builds this helper and bundles `azad-mlx-asr` plus `mlx.metallib` into `Azad.app/Contents/MacOS/`.

## Protocol

Startup returns:

```json
{"type":"ready","ok":true}
```

ASR commands:

- `{"type":"chunk","samples":[...]}`
- `{"type":"reset"}`
- `{"type":"finish"}`
- `{"type":"final_samples","samples":[...]}`
- `{"type":"shutdown"}`

VAD commands:

- `{"type":"vad","samples":[...]}`
- `{"type":"reset"}`
- `{"type":"shutdown"}`

Every command returns an object with `ok: true` on success, or `ok: false` and `error` on failure.
