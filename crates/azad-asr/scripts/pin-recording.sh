#!/usr/bin/env bash
#
# pin-recording.sh — copy a debug recording from the Azad live capture buffer
# into the regression-test fixture set, and print stubs the user can paste into
# `tests/fixtures/manifest.json` and `tests/replay.rs`.
#
# Workflow:
#   1. User reports a bug. Azad's debug-recording capture has saved a WAV+JSON
#      pair under ~/Library/Application Support/Azad/debug-recordings/.
#   2. Pin the pair into the test suite:
#         ./crates/azad-asr/scripts/pin-recording.sh <recording-id> <fixture-id>
#      where <recording-id> is the file stem (e.g. 1777223399553-turn-000029)
#      and <fixture-id> is a short, descriptive snake-case slug
#      (e.g. mid-word-vad-cut-2026-04-26).
#   3. Paste the printed manifest stub into tests/fixtures/manifest.json and the
#      printed test stub into tests/replay.rs.
#   4. Run the new test. It MUST fail on `main` first — that proves we captured
#      the real bug. Then fix the code and watch it flip green.
#
# Run from anywhere; uses absolute paths derived from the script location.

set -euo pipefail

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" || $# -ne 2 ]]; then
  cat <<'USAGE'
Usage: pin-recording.sh <recording-id> <fixture-id>

  <recording-id>   File stem of a recording in the debug-recordings dir.
                   List candidates with: ls ~/Library/Application\ Support/Azad/debug-recordings/
  <fixture-id>     Short snake-case slug for the test (e.g. mid-word-vad-cut-2026-04-26).
                   Becomes the filename, manifest id, and test function name.

Examples:
  pin-recording.sh 1777223399553-turn-000029 mid-word-vad-cut
  pin-recording.sh "$(ls -t ~/Library/Application\ Support/Azad/debug-recordings/*.wav | head -1 | xargs basename | sed 's/\.wav$//')" smoke-latest
USAGE
  exit 1
fi

recording_id="$1"
fixture_id="$2"

# Validate fixture-id is a clean slug (lowercase letters, digits, dashes, underscores).
if [[ ! "$fixture_id" =~ ^[a-z0-9_-]+$ ]]; then
  echo "error: fixture-id must match [a-z0-9_-]+ (got: $fixture_id)" >&2
  exit 1
fi

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
crate_dir="$(cd "$script_dir/.." && pwd)"
fixtures_dir="$crate_dir/tests/fixtures/audio"
manifest_path="$crate_dir/tests/fixtures/manifest.json"
replay_path="$crate_dir/tests/replay.rs"
src_dir="$HOME/Library/Application Support/Azad/debug-recordings"

src_wav="$src_dir/$recording_id.wav"
src_json="$src_dir/$recording_id.json"
dst_wav="$fixtures_dir/$fixture_id.wav"
dst_json="$fixtures_dir/$fixture_id.json"

if [[ ! -f "$src_wav" ]]; then
  echo "error: source WAV not found: $src_wav" >&2
  echo "available recordings:" >&2
  if compgen -G "$src_dir/*.wav" >/dev/null; then
    for w in "$src_dir"/*.wav; do
      stem="$(basename "$w" .wav)"
      echo "  $stem" >&2
    done
  else
    echo "  (none in $src_dir)" >&2
  fi
  exit 1
fi
if [[ ! -f "$src_json" ]]; then
  echo "error: source JSON not found: $src_json" >&2
  exit 1
fi
if [[ -e "$dst_wav" || -e "$dst_json" ]]; then
  echo "error: fixture already exists: $dst_wav (refuse to overwrite — pick a different fixture-id)" >&2
  exit 1
fi

mkdir -p "$fixtures_dir"
cp "$src_wav" "$dst_wav"
cp "$src_json" "$dst_json"

# Surface the captured ground truth so the user can craft the assertion.
full_text="$(python3 -c "import json,sys; d=json.load(open(sys.argv[1])); print(d.get('full_text',''))" "$dst_json" 2>/dev/null || true)"
emitted_text="$(python3 -c "import json,sys; d=json.load(open(sys.argv[1])); print(d.get('emitted_text',''))" "$dst_json" 2>/dev/null || true)"

# Convert dashes to underscores for the Rust function name.
fn_name="replay_${fixture_id//-/_}"

cat <<INFO

pinned:
  $dst_wav
  $dst_json

ground-truth full_text   : $full_text
ground-truth emitted_text: $emitted_text

next steps:
  1) paste this entry into the "fixtures" array in $manifest_path:
INFO

cat <<MANIFEST
    {
      "id": "$fixture_id",
      "wav": "$fixture_id.wav",
      "json": "$fixture_id.json",
      "description": "TODO: describe the bug this fixture pins. What was the user doing? What went wrong?",
      "assertions": {
        "must_contain": [],
        "must_not_contain": [],
        "min_word_count": 0
      }
    }
MANIFEST

cat <<INFO

  2) paste this test into $replay_path (and tweak the assertions):
INFO

cat <<RUST
#[test]
#[ignore = "requires MLX Nemotron + Silero VAD models on disk"]
fn $fn_name() {
  let Some(r) = run_fixture("$fixture_id") else {
    return;
  };
  assert!(r.errors.is_empty(), "pipeline emitted errors: {:?}", r.errors);
  // TODO: encode the bug. The test MUST FAIL on \`main\` first — that proves we
  // captured the real failure mode, not a passing-by-coincidence input. Examples:
  //   assert!(r.final_text.contains("expected word"), "got: {}", r.final_text);
  //   assert!(!r.final_text.contains("buggy fragment"), "got: {}", r.final_text);
}
RUST

cat <<TAIL

  3) run the new test (it should FAIL on main):
       cargo test -p azad-asr --test replay $fn_name -- --ignored --nocapture
     fix the code, then re-run — it should pass.

  4) commit the fixture:
       git add $manifest_path $replay_path "$dst_wav" "$dst_json"
       git status
TAIL
