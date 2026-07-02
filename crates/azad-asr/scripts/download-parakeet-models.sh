#!/bin/sh

set -eu

# Download legacy Parakeet ASR assets for azad-asr CLI/replay debugging.
# The Azad app downloads the MLX Nemotron pack from onboarding/settings.
#
# Default destination: <workspace>/models/{parakeet,vad}.
# Pass an alternate model root as the first argument.

get_script_dir() {
  if command -v realpath >/dev/null 2>&1; then
    dirname "$(realpath "$0")"
  else
    cd -- "$(dirname "$0")" >/dev/null 2>&1 || exit 1
    pwd -P
  fi
}

SCRIPT_DIR="$(get_script_dir)"
WORKSPACE_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd -P)"
DEST_ROOT="${1:-"$WORKSPACE_ROOT/models"}"

PARAKEET_DIR="$DEST_ROOT/parakeet"
TDT_DIR="$PARAKEET_DIR/tdt"
EOU_DIR="$PARAKEET_DIR/eou"
VAD_DIR="$DEST_ROOT/vad"

mkdir -p "$TDT_DIR" "$EOU_DIR" "$VAD_DIR"

src_tdt="https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/8f23f0c03c8761650bdb5b40aaf3e40d2c15f1ce"
src_eou="https://huggingface.co/altunenes/parakeet-rs/resolve/a61d2818df4659c956b9661a9447f46e98c15126/realtime_eou_120m-v1-onnx"
src_vad="https://huggingface.co/ggml-org/whisper-vad/resolve/9ffd54a1e1ee413ddf265af9913beaf518d1639b"

download() {
  url="$1"
  out="$2"

  if [ -f "$out" ]; then
    printf "OK  %s\n" "$out"
    return 0
  fi

  tmp="${out}.part"

  printf "GET %s\n" "$out"

  if command -v curl >/dev/null 2>&1; then
    curl -L --fail --retry 3 --retry-delay 1 --connect-timeout 10 -C - -o "$tmp" "$url"
  elif command -v wget2 >/dev/null 2>&1; then
    wget2 --no-config --progress bar -O "$tmp" "$url"
  elif command -v wget >/dev/null 2>&1; then
    wget --no-config --quiet --show-progress -O "$tmp" "$url"
  else
    echo "error: need curl or wget to download models" >&2
    exit 1
  fi

  mv "$tmp" "$out"
}

echo "Downloading Parakeet EOU (streaming) assets to: $EOU_DIR"
download "$src_eou/encoder.onnx" "$EOU_DIR/encoder.onnx"
download "$src_eou/decoder_joint.onnx" "$EOU_DIR/decoder_joint.onnx"
download "$src_eou/tokenizer.json" "$EOU_DIR/tokenizer.json"

echo ""
echo "Downloading Parakeet TDT (refinement) assets to: $TDT_DIR"
download "$src_tdt/encoder-model.onnx" "$TDT_DIR/encoder-model.onnx"
download "$src_tdt/encoder-model.onnx.data" "$TDT_DIR/encoder-model.onnx.data"
download "$src_tdt/decoder_joint-model.onnx" "$TDT_DIR/decoder_joint-model.onnx"
download "$src_tdt/vocab.txt" "$TDT_DIR/vocab.txt"

echo ""
echo "Downloading Silero VAD asset to: $VAD_DIR"
download "$src_vad/ggml-silero-v6.2.0.bin" "$VAD_DIR/ggml-silero-v6.2.0.bin"

echo ""
echo "Done."
echo ""
echo "You can now run:"
echo "  cargo run -p azad-asr -- listen --select-device"
