//! Regression test surface for the speech pipeline.
//!
//! Each fixture is a `(WAV, JSON)` pair under `tests/fixtures/audio/`. The WAV is replayed
//! through `run_pipeline` exactly as the live mic pipeline would consume it; the JSON is the
//! sidecar that was captured alongside the original recording (ground-truth `full_text` from
//! the full TDT pass, plus partials and metadata). New fixtures get added per the bug-report
//! workflow:
//!
//!   1. User reports a bug. The debug-recording capture (in `pipeline.rs::save_debug_recording`)
//!      has already saved the WAV+JSON pair to `~/Library/Application Support/Azad/debug-recordings/`.
//!   2. Run `crates/azad-asr/scripts/pin-recording.sh <recording-id> <fixture-id>` to pin the
//!      pair into `tests/fixtures/audio/` and add a manifest entry.
//!   3. Add a `#[test]` function below that asserts the **correct** behaviour. The test MUST
//!      fail on `main` first — that proves we captured the real bug, not a passing-by-coincidence
//!      input.
//!   4. Fix the code. The test flips green.
//!
//! Because the pipeline needs Parakeet EOU, Parakeet TDT, and Silero VAD, every test in this
//! file is `#[ignore]` by default — `cargo test -p azad-asr` skips them, so contributors without
//! models on disk don't see spurious failures. To run them:
//!
//!     cargo test -p azad-asr --test replay -- --ignored --test-threads=1
//!
//! Set `AZAD_TEST_REQUIRE_MODELS=1` to fail (instead of skip) when models are missing — useful
//! when you want to verify the harness actually exercises the pipeline.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use serde_json::Value;

use asr::audio::wav_input::WavInput;
use asr::pipeline::{PipelineConfig, run_pipeline};
use asr::render::{RenderEvent, Renderer};

const FIXTURE_AUDIO_DIR: &str = "tests/fixtures/audio";
const MANIFEST_REL: &str = "tests/fixtures/manifest.json";

#[test]
#[ignore = "requires Parakeet EOU/TDT + Silero VAD models on disk"]
fn replay_empty_audio_emits_nothing() {
  let Some(r) = run_fixture("empty-audio") else {
    return;
  };
  assert!(r.errors.is_empty(), "pipeline emitted errors: {:?}", r.errors);
  assert!(
    r.final_text.trim().is_empty(),
    "expected empty transcription for silence; got `{}`",
    r.final_text
  );
}

/// Real recording from 2026-04-26. The user says "I want you to undo the fix..." which contains
/// duplicated phrases ("I want you", "have a conversation", "why don't you do an investigation")
/// that give the incremental stitcher a tempting false anchor mid-utterance. With the chain of
/// stitcher fixes from `10309a4` through `fb31c66`, the prefix is preserved correctly. Without
/// any one of them, the stitcher locks onto the duplicate, treats left's prefix as a pseudo-
/// suffix, and emits only the trailing fragment ("reproduce it, then let's have a conversation
/// ...") — dropping ~50 words from the start.
#[test]
#[ignore = "requires Parakeet EOU/TDT + Silero VAD models on disk"]
fn replay_stitcher_preserves_prefix_pseudo_suffix() {
  let Some(r) = run_fixture("stitcher-preserves-prefix-pseudo-suffix") else {
    return;
  };
  assert!(r.errors.is_empty(), "pipeline emitted errors: {:?}", r.errors);
  for must in ["undo the fix", "commit it", "actually exists"] {
    assert!(
      r.final_text.contains(must),
      "stitcher dropped the prefix again — final text missing `{must}`.\n  got: {}",
      r.final_text
    );
  }
}

/// Synthesized fixture proving the tentative-finalize recovery window fires
/// on a gap that fits inside the window. Built by concatenating two real
/// recordings (turn-000008 + 200 ms silence + turn-000009). The 200 ms gap
/// is well inside `recovery_window_ms` (250), so recovery un-latches and the
/// turn continues as a single emission.
///
/// The strongest assertion is `lines.len() == 1`. If recovery breaks or the
/// window shrinks below 200 ms, this fixture's transcript arrives as two
/// lines and the test fails.
#[test]
#[ignore = "requires Parakeet EOU/TDT + Silero VAD models on disk"]
fn replay_recovery_bridges_200ms_gap() {
  let Some(r) = run_fixture("recovery-bridges-200ms-gap") else {
    return;
  };
  assert!(r.errors.is_empty(), "pipeline emitted errors: {:?}", r.errors);
  assert_eq!(
    r.lines.len(),
    1,
    "recovery should bridge the 200ms gap into a single turn; got {} lines: {:?}",
    r.lines.len(),
    r.lines
  );
  for must in ["for reference", "kerning"] {
    assert!(
      r.final_text.contains(must),
      "merged transcript missing `{must}`; got: {}",
      r.final_text
    );
  }
}

/// Companion to `replay_recovery_bridges_200ms_gap`. Same source clips with
/// a 400 ms silence gap. **Was a split-into-two-turns test before commit
/// "fix(pipeline): lower in-speech VAD floor".** Under the lower
/// `vad_in_speech_thold = 0.10`, soft trailing speech on the end of clip A
/// no longer accumulates as silence, so `silence_ms` doesn't reach
/// `eou_min_silence_ms = 350` ms before clip B starts — the engine now
/// merges the 400 ms gap into a SINGLE turn. This matches the user's
/// reported intent ("if I'm still talking softly, keep listening").
///
/// Gap-split coverage is preserved by `replay_recovery_splits_long_gap`
/// (TODO: pin a longer-gap fixture, e.g. 1500 ms, when the tooling is
/// reachable). For now, this fixture pins the merge behaviour at 400 ms
/// so a future tightening of `vad_in_speech_thold` will visibly flip it.
#[test]
#[ignore = "requires Parakeet EOU/TDT + Silero VAD models on disk"]
fn replay_recovery_merges_400ms_gap_under_low_in_speech_thold() {
  let Some(r) = run_fixture("recovery-splits-400ms-gap") else {
    return;
  };
  assert!(r.errors.is_empty(), "pipeline emitted errors: {:?}", r.errors);
  assert_eq!(
    r.lines.len(),
    1,
    "with vad_in_speech_thold=0.10, 400ms gap merges into a single turn — \
     soft trailing speech extends silence_ms accumulation; got {}: {:?}",
    r.lines.len(),
    r.lines
  );
  for must in ["for reference", "kerning"] {
    assert!(r.final_text.contains(must), "transcript missing `{must}`; got: {}", r.final_text);
  }
}

#[derive(Debug)]
struct ReplayResult {
  /// One entry per turn, in turn-id order. For a fixture with multiple turns this surfaces
  /// the structure; `final_text` is the joined-and-lowercased convenience form.
  #[allow(dead_code)]
  lines: Vec<String>,
  /// All emitted lines joined by a single space and lower-cased. Use this for `must_contain`-
  /// style assertions; it's lossy but matches how a human would skim the transcript.
  final_text: String,
  /// Pipeline-emitted error messages. Should be empty for any healthy run.
  errors: Vec<String>,
  /// The fixture's sidecar JSON, parsed loosely. Tests that want to compare against the
  /// originally-captured `full_text` can read it via `result.sidecar["full_text"]`.
  #[allow(dead_code)]
  sidecar: Value,
}

/// Replay fixture `id` end-to-end. Returns `None` (and prints a skip notice) when models
/// aren't available, unless `AZAD_TEST_REQUIRE_MODELS=1` is set — in that case it panics.
fn run_fixture(id: &str) -> Option<ReplayResult> {
  asr::logging::init_quiet();

  let manifest = load_manifest();
  let entry = find_fixture_entry(&manifest, id)
    .unwrap_or_else(|| panic!("fixture `{id}` not found in {MANIFEST_REL}"));

  let wav_name = require_str(&entry, "wav");
  let json_name = require_str(&entry, "json");

  let crate_dir = crate_dir();
  let wav_path = crate_dir.join(FIXTURE_AUDIO_DIR).join(wav_name);
  let json_path = crate_dir.join(FIXTURE_AUDIO_DIR).join(json_name);

  if !wav_path.is_file() {
    panic!(
      "fixture `{id}` references missing WAV {}\n(if this is an LFS pointer, run `git lfs pull`)",
      wav_path.display()
    );
  }
  let sidecar: Value = serde_json::from_str(
    &std::fs::read_to_string(&json_path)
      .unwrap_or_else(|e| panic!("failed to read sidecar {}: {e}", json_path.display())),
  )
  .unwrap_or_else(|e| panic!("failed to parse sidecar {}: {e}", json_path.display()));

  let cfg = match resolve_pipeline_config() {
    Some(cfg) => cfg,
    None => {
      if env_truthy("AZAD_TEST_REQUIRE_MODELS") {
        panic!(
          "AZAD_TEST_REQUIRE_MODELS is set but Parakeet/VAD models were not found at the \
           workspace dev paths (models/parakeet/{{eou,tdt}} and models/vad/ggml-silero-v6.2.0.bin)"
        );
      }
      eprintln!(
        "[replay] skipping fixture `{id}`: Parakeet/VAD models not found at workspace dev paths.\n\
         set AZAD_TEST_REQUIRE_MODELS=1 to make this a hard failure."
      );
      return None;
    }
  };

  let mut input = WavInput::open(&wav_path, 20)
    .unwrap_or_else(|e| panic!("failed to open fixture wav {}: {e}", wav_path.display()));
  let renderer = Arc::new(CollectingRenderer::default());
  let shutdown = Arc::new(AtomicBool::new(false));

  if let Err(e) = run_pipeline(&mut input, renderer.clone(), cfg, shutdown) {
    panic!("run_pipeline failed for fixture `{id}`: {e:#}");
  }

  let lines = renderer.snapshot_lines();
  let errors = renderer.snapshot_errors();
  let final_text = lines.join(" ").to_lowercase();

  Some(ReplayResult { lines, final_text, errors, sidecar })
}

#[derive(Default)]
struct CollectingRenderer {
  lines: Mutex<BTreeMap<u64, String>>,
  errors: Mutex<Vec<String>>,
}

impl CollectingRenderer {
  fn snapshot_lines(&self) -> Vec<String> {
    self
      .lines
      .lock()
      .unwrap()
      .iter()
      .filter_map(|(id, text)| if *id == 0 { None } else { Some(text.clone()) })
      .collect()
  }

  fn snapshot_errors(&self) -> Vec<String> {
    self.errors.lock().unwrap().clone()
  }
}

impl Renderer for CollectingRenderer {
  fn emit(&self, ev: RenderEvent) {
    match ev {
      RenderEvent::FinalLine { id, text } | RenderEvent::ReplaceLine { id, text } => {
        self.lines.lock().unwrap().insert(id, text);
      }
      RenderEvent::Error { message } => {
        self.errors.lock().unwrap().push(message);
      }
      _ => {}
    }
  }
}

fn load_manifest() -> Value {
  let path = crate_dir().join(MANIFEST_REL);
  let raw = std::fs::read_to_string(&path)
    .unwrap_or_else(|e| panic!("failed to read manifest {}: {e}", path.display()));
  serde_json::from_str(&raw)
    .unwrap_or_else(|e| panic!("failed to parse manifest {}: {e}", path.display()))
}

fn find_fixture_entry(manifest: &Value, id: &str) -> Option<Value> {
  let arr = manifest.get("fixtures").and_then(|v| v.as_array())?;
  arr.iter().find(|e| e.get("id").and_then(|v| v.as_str()) == Some(id)).cloned()
}

fn require_str(entry: &Value, key: &str) -> String {
  entry
    .get(key)
    .and_then(|v| v.as_str())
    .map(ToOwned::to_owned)
    .unwrap_or_else(|| panic!("manifest fixture entry missing `{key}`: {entry}"))
}

fn crate_dir() -> PathBuf {
  PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn repo_root() -> PathBuf {
  // <repo>/crates/azad-asr/ -> <repo>
  crate_dir()
    .parent()
    .and_then(|p| p.parent())
    .map(PathBuf::from)
    .unwrap_or_else(|| crate_dir())
}

/// Mirror of `crates/azad/src/config.rs::resolve_pipeline_paths` (debug branch): prefer
/// in-repo dev paths so tests don't depend on the per-user model pack location. Returns
/// `None` when any required model file is missing.
fn resolve_pipeline_config() -> Option<PipelineConfig> {
  let root = repo_root();
  let parakeet = root.join("models").join("parakeet");
  let eou = parakeet.join("eou");
  let tdt = parakeet.join("tdt");

  let vad = root.join("models").join("vad").join("ggml-silero-v6.2.0.bin");
  if !vad.is_file() {
    return None;
  }

  for required in [
    eou.join("encoder.onnx"),
    eou.join("decoder_joint.onnx"),
    eou.join("tokenizer.json"),
    tdt.join("encoder-model.onnx"),
    tdt.join("encoder-model.onnx.data"),
    tdt.join("decoder_joint-model.onnx"),
    tdt.join("vocab.txt"),
  ] {
    if !required.is_file() {
      return None;
    }
  }

  Some(PipelineConfig {
    vad_model_path: vad,
    vad_thold: 0.45,
    vad_start_chunks: 1,
    pre_roll_ms: 800,
    eou_min_silence_ms: 350,
    eou_max_silence_ms: 1_000,
    vad_in_speech_thold: 0.10,
    recovery_window_ms: 250,
    recovery_vad_thold: 0.30,
    stable_k: 3,
    stable_h: 5,
    enable_tdt_final_pass: true,
    incremental_finalization_enabled: true,
    incremental_slice_ms: 6_000,
    incremental_overlap_ms: 3_000,
    incremental_left_context_ms: 10_000,
    incremental_min_new_audio_ms: 1_200,
    incremental_wait_tail_result_ms: 220,
    parakeet_tdt_dir: tdt,
    parakeet_eou_dir: eou,
  })
}

fn env_truthy(key: &str) -> bool {
  std::env::var(key)
    .ok()
    .map(|raw| raw.trim().to_ascii_lowercase())
    .is_some_and(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"))
}
