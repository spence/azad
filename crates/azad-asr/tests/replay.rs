//! Regression test surface for the speech pipeline.
//!
//! Each fixture is a `(WAV, JSON)` pair under `tests/fixtures/audio/`. The WAV is replayed
//! through `run_pipeline` exactly as the live mic pipeline would consume it; the JSON is the
//! sidecar that was captured alongside the original recording (ground-truth `full_text` from
//! the finalization pass, plus partials and metadata). New fixtures get added per the bug-report
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
//! Because the pipeline needs MLX Nemotron and Silero VAD, every test in this
//! file is `#[ignore]` by default — `cargo test -p azad-asr` skips them, so contributors without
//! models on disk don't see spurious failures. To run them:
//!
//!     cargo test -p azad-asr --test replay -- --ignored --test-threads=1
//!
//! Set `AZAD_TEST_REQUIRE_MODELS=1` to fail (instead of skip) when models are missing — useful
//! when you want to verify the harness actually exercises the pipeline.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::Value;

use asr::audio::wav_input::WavInput;
use asr::pipeline::{
  PipelineConfig, PipelineControls, PipelineRunOptions, StreamingModelConfig, run_pipeline,
  run_pipeline_with_options,
};
use asr::render::{RenderEvent, Renderer};

const FIXTURE_AUDIO_DIR: &str = "tests/fixtures/audio";
const MANIFEST_REL: &str = "tests/fixtures/manifest.json";

#[test]
#[ignore = "requires MLX Nemotron + Silero VAD models on disk"]
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
/// that historically gave the finalizer a tempting false anchor mid-utterance, dropping ~50 words
/// from the start and emitting only the trailing fragment ("reproduce it, then let's have a
/// conversation ..."). Under the dual-stream pipeline the finalized transcript must still keep the
/// prefix; this fixture pins that against any future prefix-dropping regression.
#[test]
#[ignore = "requires MLX Nemotron + Silero VAD models on disk"]
fn replay_repeated_phrase_preserves_prefix() {
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

/// Pinned 2026-07-08. Real turn where the live draft was cut off mid-word ("...the overall
/// weekly tra") and the 560 ms refined finalize flush completed the final word ("cker"). The
/// flush tail's leading word-boundary marker used to be trimmed off by the helper, so a downstream
/// char-collision heuristic re-inserted a space and split "tracker" into "tra cker.". The helper
/// now preserves that marker and the join appends the tail verbatim like any chunk delta, so the
/// final word stays whole. Fails on the pre-fix code (which emits "tra cker.").
#[test]
#[ignore = "requires MLX Nemotron + Silero VAD models on disk"]
fn replay_flush_tail_completes_final_word() {
  let Some(r) = run_fixture("flush-tail-completes-final-word") else {
    return;
  };
  assert!(r.errors.is_empty(), "pipeline emitted errors: {:?}", r.errors);
  assert!(
    r.final_text.contains("weekly tracker"),
    "finalize flush split the final word — expected a whole `weekly tracker`.\n  got: {}",
    r.final_text
  );
  assert!(
    !r.final_text.contains("tra cker"),
    "finalize flush split `tracker` into `tra cker`.\n  got: {}",
    r.final_text
  );
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
#[ignore = "requires MLX Nemotron + Silero VAD models on disk"]
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
  // Anchor tokens: "for reference" is the pre-gap opener; "pure science" is a tail phrase
  // from the post-gap clip. Both present in one line proves the merge kept the whole span.
  // (Avoids "kerning", which the current model pack transcribes as "kernel"/"kernels" — a
  // benign ASR word-choice drift, not a dropped utterance.)
  for must in ["for reference", "pure science"] {
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
/// This fixture pins the merge behaviour at 400 ms so a future tightening of
/// `vad_in_speech_thold` will visibly flip it. A true long-gap split fixture
/// should be added separately when we have a representative recording.
#[test]
#[ignore = "requires MLX Nemotron + Silero VAD models on disk"]
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
  // See `replay_recovery_bridges_200ms_gap`: "pure science" is a stable post-gap tail
  // anchor; "kerning" drifted to "kernel" under the current model pack.
  for must in ["for reference", "pure science"] {
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
          "AZAD_TEST_REQUIRE_MODELS is set but MLX/VAD models were not found at the \
           workspace dev paths (models/nemotron-mlx and models/vad/silero_vad.mlmodelc)"
        );
      }
      eprintln!(
        "[replay] skipping fixture `{id}`: MLX/VAD models not found at workspace dev paths.\n\
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
    .unwrap_or_else(crate_dir)
}

/// Mirror of `crates/azad/src/config.rs::resolve_pipeline_paths` (debug branch): prefer
/// in-repo dev paths so tests don't depend on the per-user model pack location. Returns
/// `None` when any required model file is missing.
fn resolve_pipeline_config() -> Option<PipelineConfig> {
  let root = repo_root();
  let model_dir = root.join("models").join("nemotron-mlx");

  let vad = root.join("models").join("vad").join("silero_vad.mlmodelc");
  for required in [
    vad.join("analytics").join("coremldata.bin"),
    vad.join("coremldata.bin"),
    vad.join("metadata.json"),
    vad.join("model.mil"),
    vad.join("weights").join("weight.bin"),
  ] {
    if !required.is_file() {
      return None;
    }
  }

  if !vad.is_dir() {
    return None;
  }

  for required in [
    model_dir.join("config.json"),
    model_dir.join("model.safetensors"),
    model_dir.join("tokenizer.model"),
    model_dir.join("vocab.txt"),
  ] {
    if !required.is_file() {
      return None;
    }
  }

  Some(PipelineConfig {
    vad_model_path: vad,
    vad_helper_path: None,
    streaming_model: StreamingModelConfig::MlxNemotron {
      model_dir,
      language: "en-US".to_string(),
      streaming_chunk_ms: 80,
      final_chunk_ms: 560,
      helper_path: None,
    },
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
    live_display_mutable_tail: asr::pipeline::DEFAULT_LIVE_DISPLAY_MUTABLE_TAIL_TOKENS,
    finalizing_pulse_enabled: true,
  })
}

fn env_truthy(key: &str) -> bool {
  std::env::var(key)
    .ok()
    .map(|raw| raw.trim().to_ascii_lowercase())
    .is_some_and(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"))
}

const SR: usize = 16_000;
/// `pipeline::CHUNK_SAMPLES` — one `on_chunk`/`Meter` tick (160 ms).
const CHUNK_SAMPLES: usize = 2_560;

/// Whole chunks covering `ms`. All fixture geometry is chunk-exact so the chunk indices the
/// assertions talk about are the same ones the engine sees.
fn chunks_for_ms(ms: usize) -> usize {
  (SR * ms / 1000).div_ceil(CHUNK_SAMPLES)
}

fn samples_for_ms(ms: usize) -> usize {
  chunks_for_ms(ms) * CHUNK_SAMPLES
}

/// The pipeline knobs Azad actually ships (`crates/azad/src/config.rs`). The fixtures above run a
/// stricter `vad_thold` (0.45) with an 800 ms pre-roll; the post-finalize behaviour under test is
/// governed by the app's own 0.30 / `vad_start_chunks = 1` start gate, so these tests must use the
/// shipped values or they measure a pipeline nobody runs.
fn azad_app_pipeline_config() -> Option<PipelineConfig> {
  let mut cfg = resolve_pipeline_config()?;
  cfg.vad_thold = 0.30;
  cfg.vad_start_chunks = 1;
  cfg.pre_roll_ms = 1_500;
  cfg.eou_min_silence_ms = 350;
  cfg.eou_max_silence_ms = 1_000;
  cfg.vad_in_speech_thold = 0.10;
  cfg.recovery_window_ms = 250;
  cfg.recovery_vad_thold = 0.30;
  Some(cfg)
}

#[derive(Debug, Default)]
struct TurnTrace {
  /// `Meter` fires exactly once per 160 ms `on_chunk`, so this doubles as the audio clock.
  chunks_seen: usize,
  /// Chunk index of every `TurnStarted`.
  turn_starts: Vec<usize>,
  /// Per-chunk `(rms-ish peak_db, raw vad_prob, effective thold, is_speech)`.
  meters: Vec<(f32, f32, f32, bool)>,
  lines: BTreeMap<u64, String>,
  errors: Vec<String>,
}

impl TurnTrace {
  /// Compact per-chunk VAD dump. `vad_speech = true` while `vad_prob` sits near zero and the
  /// threshold reads 0.30 is the fingerprint of the smoothed EMA — not the audio — holding the
  /// start gate open.
  fn dump(&self, tag: &str) {
    eprintln!("[trace {tag}] chunks={} turn_starts={:?}", self.chunks_seen, self.turn_starts);
    for (i, (peak, prob, thold, speech)) in self.meters.iter().enumerate() {
      eprintln!(
        "  chunk {i:3} t={:6.2}s peak_db={peak:7.1} vad_prob={prob:.3} thold={thold:.2} \
         speech={speech}{}",
        i as f32 * 0.16,
        if self.turn_starts.contains(&i) { "  <-- TURN START" } else { "" },
      );
    }
    for (id, text) in &self.lines {
      eprintln!("  line turn {id}: {text:?}");
    }
  }
}

impl TurnTrace {
  fn transcript(&self) -> String {
    self
      .lines
      .values()
      .map(|s| s.trim())
      .filter(|s| !s.is_empty())
      .collect::<Vec<_>>()
      .join(" ")
      .to_lowercase()
  }

  /// Chunks where the mic delivered speech, the engine's own VAD agreed, and yet no turn was open
  /// to record it. A chunk that opens a turn still reports the idle threshold (it is computed
  /// before `in_speech` flips), so turn starts are excluded — with `vad_start_chunks = 1` a healthy
  /// engine turns every other speech-positive idle chunk into a turn on the spot. Anything left is
  /// audio the user spoke and the engine threw away.
  fn deaf_chunks(&self, idle_thold: f32) -> Vec<usize> {
    self
      .meters
      .iter()
      .enumerate()
      .filter(|(i, (_, _, thold, speech))| {
        *speech && (thold - idle_thold).abs() < 1e-6 && !self.turn_starts.contains(i)
      })
      .map(|(i, _)| i)
      .collect()
  }

  /// Longest unbroken run in `deaf_chunks`, as `(first_chunk, len)`.
  fn longest_deaf_run(&self, idle_thold: f32) -> Option<(usize, usize)> {
    let deaf = self.deaf_chunks(idle_thold);
    let mut best: Option<(usize, usize)> = None;
    let mut run_start = None;
    let mut prev = None;
    for c in deaf.iter().copied().chain(std::iter::once(usize::MAX)) {
      match prev {
        Some(p) if c == p + 1 => {}
        _ => {
          if let (Some(s), Some(p)) = (run_start, prev) {
            let len = p - s + 1;
            if best.is_none_or(|(_, b)| len > b) {
              best = Some((s, len));
            }
          }
          run_start = Some(c);
        }
      }
      prev = Some(c);
    }
    best
  }
}

/// Records the turn structure on the engine's own clock and, at `finish_at_chunk`, plays the user
/// pressing Enter (`request_force_finish`). Driving the finalize off `Meter` instead of wall time
/// keeps the whole replay sample-deterministic.
struct TurnTracingRenderer {
  trace: Mutex<TurnTrace>,
  controls: Arc<PipelineControls>,
  finish_at_chunk: usize,
  finish_fired: AtomicBool,
}

impl TurnTracingRenderer {
  fn new(controls: Arc<PipelineControls>, finish_at_chunk: usize) -> Self {
    Self {
      trace: Mutex::new(TurnTrace::default()),
      controls,
      finish_at_chunk,
      finish_fired: AtomicBool::new(false),
    }
  }

  fn snapshot(&self) -> TurnTrace {
    let t = self.trace.lock().unwrap();
    TurnTrace {
      chunks_seen: t.chunks_seen,
      turn_starts: t.turn_starts.clone(),
      meters: t.meters.clone(),
      lines: t.lines.clone(),
      errors: t.errors.clone(),
    }
  }
}

impl Renderer for TurnTracingRenderer {
  fn emit(&self, ev: RenderEvent) {
    let mut fire_finish = false;
    {
      let mut t = self.trace.lock().unwrap();
      match ev {
        RenderEvent::Meter(m) => {
          let idx = t.chunks_seen;
          t.chunks_seen += 1;
          t.meters.push((m.peak_db, m.vad_prob, m.vad_thold, m.vad_speech));
          if idx == self.finish_at_chunk && !self.finish_fired.swap(true, Ordering::Relaxed) {
            fire_finish = true;
          }
        }
        // Emitted from inside `start_turn`, i.e. still within the chunk whose `Meter` we just
        // counted.
        RenderEvent::TurnStarted { .. } => {
          let at = t.chunks_seen.saturating_sub(1);
          t.turn_starts.push(at);
        }
        RenderEvent::FinalLine { id, text } | RenderEvent::ReplaceLine { id, text } => {
          t.lines.insert(id, text);
        }
        RenderEvent::Error { message } => t.errors.push(message),
        _ => {}
      }
    }
    if fire_finish {
      // Consumed later in the same `on_chunk`, exactly like a hotkey that lands mid-chunk.
      self.controls.request_force_finish();
    }
  }
}

fn fixture_samples(wav_name: &str) -> Vec<f32> {
  let path = crate_dir().join(FIXTURE_AUDIO_DIR).join(wav_name);
  let mut reader = hound::WavReader::open(&path)
    .unwrap_or_else(|e| panic!("failed to open fixture wav {}: {e}", path.display()));
  let spec = reader.spec();
  assert_eq!(spec.sample_rate as usize, SR, "fixture must be 16 kHz");
  assert_eq!(spec.channels, 1, "fixture must be mono");
  reader
    .samples::<f32>()
    .map(|s| s.unwrap_or_else(|e| panic!("bad sample in {}: {e}", path.display())))
    .collect()
}

fn write_wav(path: &Path, samples: &[f32]) {
  let spec = hound::WavSpec {
    channels: 1,
    sample_rate: SR as u32,
    bits_per_sample: 32,
    sample_format: hound::SampleFormat::Float,
  };
  let mut writer = hound::WavWriter::create(path, spec)
    .unwrap_or_else(|e| panic!("failed to create {}: {e}", path.display()));
  for &s in samples {
    writer.write_sample(s).expect("wav write");
  }
  writer.finalize().expect("wav finalize");
}

fn tmp_wav_path(tag: &str) -> PathBuf {
  static SEQ: AtomicUsize = AtomicUsize::new(0);
  let n = SEQ.fetch_add(1, Ordering::Relaxed);
  let dir = std::env::temp_dir().join(format!("azad-replay-{}-{n}", std::process::id()));
  std::fs::create_dir_all(&dir).expect("tmp dir");
  dir.join(format!("{tag}.wav"))
}

/// Real mic room tone and real speech carved out of an already-pinned recording, so these tests
/// add no new binary fixtures. Digital silence would not do: `on_chunk` hard-gates chunks below
/// -60 dBFS as non-speech *and* slams `vad_avg_ema` to zero, which is precisely the state the
/// engine fails to reach on a real desk mic.
struct SourceClips {
  room_tone: Vec<f32>,
  speech_a: Vec<f32>,
  speech_b: Vec<f32>,
}

impl SourceClips {
  fn load() -> Self {
    let src = fixture_samples("recovery-bridges-200ms-gap.wav");
    // 0.00-0.64 s is the pre-speech room tone the mic recorded before the user spoke
    // (-60..-45 dBFS). 0.64-5.12 s and 6.40 s-end are two separate spoken passages.
    Self {
      room_tone: src[..samples_for_ms(640)].to_vec(),
      speech_a: src[samples_for_ms(640)..samples_for_ms(5_120)].to_vec(),
      speech_b: src[samples_for_ms(6_400)..].to_vec(),
    }
  }

  fn tone(&self, ms: usize) -> Vec<f32> {
    self.room_tone.iter().copied().cycle().take(samples_for_ms(ms)).collect()
  }
}

/// A dictation the way Azad sees one: mic room tone, an utterance, then the room tone that keeps
/// arriving after the user stops. `finish_at_chunk` is where the user hits Enter.
struct Dictation {
  audio: Vec<f32>,
  finish_at_chunk: usize,
}

impl Dictation {
  /// `enter_after_ms` is the human gap between the last word and the Enter press.
  fn new(clips: &SourceClips, enter_after_ms: usize) -> Self {
    let mut audio = clips.tone(640);
    audio.extend_from_slice(&clips.speech_a);
    let last_speech_chunk = audio.len() / CHUNK_SAMPLES;
    Self { audio, finish_at_chunk: last_speech_chunk + chunks_for_ms(enter_after_ms) }
  }

  fn then_tone(mut self, clips: &SourceClips, ms: usize) -> Self {
    self.audio.extend(clips.tone(ms));
    self
  }

  fn then_tone_chunks(mut self, clips: &SourceClips, n: usize) -> Self {
    self
      .audio
      .extend(clips.room_tone.iter().copied().cycle().take(n * CHUNK_SAMPLES));
    self
  }

  /// Append the follow-up utterance, returning the chunk index where it starts. The clip's first
  /// chunk is dropped so the utterance opens on a fully voiced chunk (Silero ~1.0) instead of a
  /// half-silent attack chunk (~0.2) — a word begun in earnest, not eased into.
  fn then_speech_b(&mut self, clips: &SourceClips) -> usize {
    let onset = self.audio.len() / CHUNK_SAMPLES;
    self.audio.extend_from_slice(&clips.speech_b[CHUNK_SAMPLES..]);
    onset
  }
}

/// Delivers a WAV at 1x wall-clock speed. Required whenever the behaviour under test is gated on
/// real time rather than sample count — `should_timeout_empty_vad_turn` compares
/// `turn_started_at.elapsed()` against `eou_max_silence_ms * 3`, which a free-running replay
/// (the live decode costs ~0.3x realtime) never reaches until far more audio has gone by than on
/// a real mic.
struct PacedInput {
  inner: WavInput,
  started: std::time::Instant,
  frames_read: u64,
}

impl PacedInput {
  fn new(inner: WavInput) -> Self {
    Self { inner, started: std::time::Instant::now(), frames_read: 0 }
  }
}

impl asr::audio::AudioInput for PacedInput {
  fn spec(&self) -> asr::audio::AudioSpec {
    self.inner.spec()
  }

  fn read_chunk(&mut self) -> anyhow::Result<Option<asr::audio::AudioChunk>> {
    let spec = self.inner.spec();
    let chunk = self.inner.read_chunk()?;
    if let Some(c) = &chunk {
      self.frames_read += (c.frames.len() / spec.channels.max(1) as usize) as u64;
      let due =
        std::time::Duration::from_secs_f64(self.frames_read as f64 / f64::from(spec.sample_rate));
      if let Some(wait) = due.checked_sub(self.started.elapsed()) {
        std::thread::sleep(wait);
      }
    }
    Ok(chunk)
  }

  fn health(&self) -> asr::audio::AudioHealth {
    self.inner.health()
  }
}

struct PostFinalizeRun {
  trace: TurnTrace,
  finish_chunk: usize,
}

/// Replay `samples` through the shipped pipeline, pressing Enter at `finish_at_chunk`.
fn run_post_finalize(
  tag: &str,
  samples: &[f32],
  finish_at_chunk: usize,
) -> Option<PostFinalizeRun> {
  run_post_finalize_paced(tag, samples, finish_at_chunk, false)
}

fn run_post_finalize_paced(
  tag: &str,
  samples: &[f32],
  finish_at_chunk: usize,
  realtime: bool,
) -> Option<PostFinalizeRun> {
  asr::logging::init_quiet();
  let cfg = match azad_app_pipeline_config() {
    Some(cfg) => cfg,
    None => {
      if env_truthy("AZAD_TEST_REQUIRE_MODELS") {
        panic!("AZAD_TEST_REQUIRE_MODELS is set but MLX/VAD models were not found");
      }
      eprintln!("[replay] skipping `{tag}`: MLX/VAD models not found at workspace dev paths.");
      return None;
    }
  };

  let path = tmp_wav_path(tag);
  write_wav(&path, samples);

  let controls = Arc::new(PipelineControls::default());
  let renderer = Arc::new(TurnTracingRenderer::new(Arc::clone(&controls), finish_at_chunk));
  let wav = WavInput::open(&path, 20).expect("open generated wav");
  let shutdown = Arc::new(AtomicBool::new(false));

  let options =
    PipelineRunOptions { controls: Some(Arc::clone(&controls)), stop_after_turn: false };
  let result = if realtime {
    let mut input = PacedInput::new(wav);
    run_pipeline_with_options(&mut input, renderer.clone(), cfg, shutdown, options)
  } else {
    let mut input = wav;
    run_pipeline_with_options(&mut input, renderer.clone(), cfg, shutdown, options)
  };
  if let Err(e) = result {
    panic!("run_pipeline failed for `{tag}`: {e:#}");
  }
  let _ = std::fs::remove_file(&path);

  let trace = renderer.snapshot();
  assert!(trace.errors.is_empty(), "pipeline emitted errors: {:?}", trace.errors);
  if env_truthy("AZAD_REPLAY_TRACE") {
    trace.dump(tag);
  }
  Some(PostFinalizeRun { trace, finish_chunk: finish_at_chunk })
}

/// **Root cause of the "dead period after a paste" report.**
///
/// `finish_turn` tears down turn state but leaves `vad_avg_ema` at the value the just-finished
/// utterance drove it to. When the user finalizes by hand (Enter / `request_force_finish`) — which
/// production logs show is how ~83 % of turns end — that EMA is still ~0.9. The idle start gate
/// scores `max(vad_avg, vad_avg_ema)` against `vad_thold = 0.30` with `vad_start_chunks = 1`, so
/// the very next chunk of room tone re-opens a turn. Nothing was said; the turn is pure VAD
/// hysteresis.
///
/// The engine must not open a turn on room tone just because it was mid-utterance a moment ago.
///
/// KNOWN RED on `main` — this is the pinned reproduction, landed ahead of the fix. It reports one
/// phantom turn opening 160 ms after the finalize.
#[test]
#[ignore = "requires MLX Nemotron + Silero VAD models on disk"]
fn manual_finalize_does_not_restart_a_turn_on_room_tone() {
  let clips = SourceClips::load();
  // Enter 320 ms after the last word, then 5 s of the same room tone the mic was already
  // recording — long enough for a phantom turn to be born, stall, and finalize.
  let d = Dictation::new(&clips, 320).then_tone(&clips, 5_000);
  let Some(run) = run_post_finalize("manual-finalize-room-tone", &d.audio, d.finish_at_chunk)
  else {
    return;
  };

  let after: Vec<usize> = run
    .trace
    .turn_starts
    .iter()
    .copied()
    .filter(|c| *c > run.finish_chunk)
    .collect();
  assert!(
    after.is_empty(),
    "engine opened {} phantom turn(s) on room tone after the manual finalize \
     (finalize at chunk {}, restarts at chunks {:?}, all turn starts {:?}); \
     stale vad_avg_ema is re-triggering the start gate",
    after.len(),
    run.finish_chunk,
    after,
    run.trace.turn_starts,
  );
}

/// **User-visible consequence — "I start talking again and nothing happens."**
///
/// The phantom turn from the test above is still open when the user resumes, so their words are
/// swallowed into a turn that VAD opened on room tone seconds earlier. That turn is still
/// text-less when it hits `should_timeout_empty_vad_turn` (`eou_max_silence_ms * 3` = 3 s of wall
/// clock), which fires whether or not VAD currently says speech. The turn is discarded with an
/// empty draft — nothing pasted, nothing shown — and because it counts as an *empty VAD turn* it
/// arms `vad_rearm_required` and resets the Silero VAD mid-word. The rearm gate then refuses every
/// automatic start until a chunk scores below `vad_thold`, which does not happen while the user is
/// still talking. Production logs show 6.4 % of all turns dying on this timeout, over half of them
/// with VAD reporting speech more than 2 s in.
///
/// Runs at 1x wall-clock speed: the timeout is wall-clock gated, so a free-running replay cannot
/// see it. The resume offset is swept because where the user lands inside the phantom decides
/// whether the timeout catches them.
///
/// KNOWN RED on `main` — this is the pinned reproduction, landed ahead of the fix. Resuming
/// 2560 ms after the last word costs 3040 ms of speech, dropped with no overlay and no transcript.
#[test]
#[ignore = "requires MLX Nemotron + Silero VAD models on disk"]
fn speech_resumed_into_a_phantom_turn_is_not_discarded() {
  let clips = SourceClips::load();
  let mut failures = Vec::new();

  for gap_chunks in [12usize, 14, 16] {
    let mut d = Dictation::new(&clips, 320).then_tone_chunks(&clips, gap_chunks);
    let b_onset = d.then_speech_b(&clips);
    let d = d.then_tone(&clips, 2_000);

    let tag = format!("resume-into-phantom-gap-{gap_chunks}c");
    let Some(run) = run_post_finalize_paced(&tag, &d.audio, d.finish_at_chunk, true) else {
      return;
    };

    if let Some((from, len)) = run.trace.longest_deaf_run(0.30) {
      failures.push(format!(
        "resume {} ms after the last word (follow-up opens at chunk {b_onset}): engine went deaf \
         for {len} chunks ({} ms) from chunk {from} — its own VAD reported speech on every one of \
         them and no turn was open. Turn starts {:?} (Enter at chunk {}). Transcript: {:?}",
        gap_chunks * 160,
        len * 160,
        run.trace.turn_starts,
        run.finish_chunk,
        run.trace.transcript(),
      ));
    }
  }

  assert!(
    failures.is_empty(),
    "speech resumed into the phantom turn was discarded:\n  {}",
    failures.join("\n  ")
  );
}
