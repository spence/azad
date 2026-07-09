use anyhow::{Context, Result, anyhow};
use clap::{Args, Parser, Subcommand};
use cpal::traits::{DeviceTrait, HostTrait};
use crossbeam_channel as chan;
use std::collections::BTreeMap;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use asr::audio::cpal_input::{CpalInput, CpalInputConfig};
use asr::audio::decoded_input::DecodedInput;
use asr::audio::wav_input::WavInput;
use asr::audio::{AudioChunk, AudioHealth, AudioInput, AudioSpec};
use asr::logging;
use asr::pipeline::{
  PipelineConfig, PipelineControls, PipelineRunOptions, StreamingModelConfig, run_pipeline,
  run_pipeline_with_options,
};
use asr::render::{RenderEvent, Renderer};
use asr::ui;

#[derive(Parser)]
#[command(name = "asr")]
#[command(about = "Terminal live speech-to-text (English) via MLX Nemotron (cpal capture)", long_about = None)]
#[command(version)]
struct Cli {
  #[command(subcommand)]
  command: Commands,
}

#[derive(Subcommand)]
enum Commands {
  /// List microphone capture devices.
  Devices,

  /// Listen on the microphone, detect utterances, and stream text into the terminal.
  Listen(ListenArgs),

  /// Transcribe a WAV file through the exact same pipeline as `listen`.
  TranscribeFile(TranscribeFileArgs),
}

#[derive(Args, Debug, Clone)]
struct CommonArgs {
  /// Path to a CoreML Silero VAD .mlmodelc directory.
  #[arg(long = "vad-model")]
  vad_model: Option<PathBuf>,

  /// VAD speech threshold (Silero avg prob).
  #[arg(long, default_value_t = 0.45)]
  vad_thold: f32,

  /// Require N consecutive VAD "speech" detections to enter speech state.
  #[arg(long, default_value_t = 1)]
  vad_start_chunks: usize,

  /// Audio prepended when speech starts (ms).
  #[arg(long, default_value_t = 800)]
  pre_roll_ms: u32,

  /// Minimum silence (ms) before we trust EOU to end the turn.
  #[arg(long, default_value_t = 240)]
  eou_min_silence_ms: u32,

  /// Fallback: force end if silence exceeds this (ms), even if EOU didn't fire.
  #[arg(long, default_value_t = 1000)]
  eou_max_silence_ms: u32,

  /// VAD probability floor while a turn is in progress. Sub-floor chunks
  /// accumulate against `eou_max_silence_ms`. Lower than `vad_thold` so soft
  /// continuation keeps the turn alive (see PipelineConfig::vad_in_speech_thold).
  #[arg(long, default_value_t = 0.10)]
  vad_in_speech_thold: f32,

  /// Tentative-finalize recovery window (ms). 0 disables (commit immediately).
  #[arg(long, default_value_t = 500)]
  recovery_window_ms: u32,

  /// VAD probability above which "still talking" recovery counts during the window.
  #[arg(long, default_value_t = 0.30)]
  recovery_vad_thold: f32,

  /// Stable prefix requires agreement across the last K hypotheses.
  #[arg(long, default_value_t = 3)]
  stable_k: usize,

  /// Max hypotheses stored (H).
  #[arg(long, default_value_t = 5)]
  stable_h: usize,

  /// Trailing live-caption tokens the lagging refined stream may still rewrite (older tokens are
  /// frozen). Smaller confines mid-speech churn to fewer trailing words. Sweep this here.
  #[arg(long, default_value_t = asr::pipeline::DEFAULT_LIVE_DISPLAY_MUTABLE_TAIL_TOKENS)]
  live_display_mutable_tail: usize,

  /// MLX Nemotron model directory (contains config.json, model.safetensors, tokenizer.model, vocab.txt).
  #[arg(long = "mlx-model-dir")]
  mlx_model_dir: Option<PathBuf>,

  /// Path to azad-mlx-asr helper. Defaults to the bundled helper or target/swift build output.
  #[arg(long = "mlx-helper")]
  mlx_helper: Option<PathBuf>,

  /// ASR language tag passed to the MLX helper.
  #[arg(long, default_value = "en-US")]
  language: String,

  /// MLX streaming chunk size for live partial text (ms).
  #[arg(long, default_value_t = 80)]
  streaming_chunk_ms: u32,

  /// MLX chunk size for finalization passes (ms).
  #[arg(long, default_value_t = 560)]
  final_chunk_ms: u32,
}

#[derive(Args, Debug, Clone)]
struct ListenArgs {
  #[command(flatten)]
  common: CommonArgs,

  /// Input device index (see `asr devices`).
  #[arg(long)]
  device: Option<usize>,

  /// Prompt to select an audio device before starting.
  #[arg(long)]
  select_device: bool,

  /// Capture chunk size (ms).
  #[arg(long, default_value_t = 20)]
  chunk_ms: u32,

  /// Capture ring buffer size (ms).
  #[arg(long, default_value_t = 120_000)]
  buffer_ms: u32,
}

#[derive(Args, Debug, Clone)]
struct TranscribeFileArgs {
  #[command(flatten)]
  common: CommonArgs,

  /// Input WAV path.
  #[arg(value_name = "WAV_PATH")]
  path: PathBuf,

  /// File read chunk size (ms).
  #[arg(long, default_value_t = 20)]
  chunk_ms: u32,

  /// Print the renderer event stream as JSON lines instead of final text only.
  #[arg(long = "events-jsonl")]
  events_jsonl: bool,

  /// Pace the file feed to wall-clock (real time) so the concurrent 560ms refined worker runs at
  /// the same cadence as live capture. Without this the fire-hose feed starves the refined stream,
  /// so the live-caption composition never exercises and file-replay cannot reproduce caption churn.
  #[arg(long)]
  realtime: bool,

  /// Turn on the engine's debug-stats instrumentation so diagnostic `TOON_*` lines are emitted to
  /// stderr — notably `TOON_LIVE_STREAM_GAP`, which marks each multi-second live-caption freeze.
  /// Pair with `--realtime` to measure live-caption pacing exactly as the app experiences it.
  #[arg(long)]
  debug_stats: bool,
}

#[derive(Clone)]
struct ChanRenderer {
  tx: chan::Sender<RenderEvent>,
}

impl Renderer for ChanRenderer {
  fn emit(&self, ev: RenderEvent) {
    let _ = self.tx.send(ev);
  }
}

struct CollectingRenderer {
  lines: Mutex<BTreeMap<u64, String>>,
  errors: Mutex<Vec<String>>,
  events: Mutex<Vec<serde_json::Value>>,
  event_seq: AtomicU64,
  record_events: bool,
}

impl CollectingRenderer {
  fn new(record_events: bool) -> Self {
    Self {
      lines: Mutex::new(BTreeMap::new()),
      errors: Mutex::new(Vec::new()),
      events: Mutex::new(Vec::new()),
      event_seq: AtomicU64::new(1),
      record_events,
    }
  }

  fn snapshot_lines(&self) -> Vec<String> {
    let lines = self.lines.lock().unwrap();
    lines
      .iter()
      .filter_map(|(id, text)| if *id == 0 { None } else { Some(text.clone()) })
      .collect()
  }

  fn snapshot_events(&self) -> Vec<serde_json::Value> {
    self.events.lock().unwrap().clone()
  }

  fn record_event(&self, ev: &RenderEvent) {
    if !self.record_events {
      return;
    }
    let seq = self.event_seq.fetch_add(1, Ordering::Relaxed);
    let Some(event) = replay_event_json(seq, ev) else {
      return;
    };
    self.events.lock().unwrap().push(event);
  }
}

impl Renderer for CollectingRenderer {
  fn emit(&self, ev: RenderEvent) {
    self.record_event(&ev);
    match ev {
      RenderEvent::Active { .. }
      | RenderEvent::Finalizing { .. }
      | RenderEvent::FinalizingCancelled { .. } => {}
      RenderEvent::FinalLine { id, text } => {
        let mut lines = self.lines.lock().unwrap();
        lines.insert(id, text);
      }
      RenderEvent::ReplaceLine { id, text } => {
        let mut lines = self.lines.lock().unwrap();
        lines.insert(id, text);
      }
      RenderEvent::Error { message } => {
        self.errors.lock().unwrap().push(message);
      }
      RenderEvent::Status(_)
      | RenderEvent::SpeechStartedByVad
      | RenderEvent::TurnStarted { .. }
      | RenderEvent::CaptureHealth(_)
      | RenderEvent::Meter(_)
      | RenderEvent::DebugStats(_) => {}
    }
  }
}

fn replay_event_json(seq: u64, ev: &RenderEvent) -> Option<serde_json::Value> {
  match ev {
    RenderEvent::Status(v) => Some(serde_json::json!({
      "seq": seq,
      "event": "status",
      "state": format!("{:?}", v.state),
      "detail": v.detail,
    })),
    RenderEvent::SpeechStartedByVad => Some(serde_json::json!({
      "seq": seq,
      "event": "speech_started_by_vad",
    })),
    RenderEvent::TurnStarted { reason } => Some(serde_json::json!({
      "seq": seq,
      "event": "turn_started",
      "reason": format!("{:?}", reason),
    })),
    RenderEvent::Active { id, committed, live } => {
      let merged = format!("{committed}{live}").trim().to_string();
      Some(serde_json::json!({
        "seq": seq,
        "event": "active",
        "turn_id": id,
        "committed": committed,
        "live": live,
        "merged": merged,
        "merged_chars": merged.chars().count(),
      }))
    }
    RenderEvent::Finalizing { id, text } => Some(serde_json::json!({
      "seq": seq,
      "event": "finalizing",
      "turn_id": id,
      "text": text,
      "text_chars": text.chars().count(),
    })),
    RenderEvent::FinalizingCancelled { id } => Some(serde_json::json!({
      "seq": seq,
      "event": "finalizing_cancelled",
      "turn_id": id,
    })),
    RenderEvent::FinalLine { id, text } => Some(serde_json::json!({
      "seq": seq,
      "event": "final_line",
      "turn_id": id,
      "text": text,
      "text_chars": text.chars().count(),
    })),
    RenderEvent::ReplaceLine { id, text } => Some(serde_json::json!({
      "seq": seq,
      "event": "replace_line",
      "turn_id": id,
      "text": text,
      "text_chars": text.chars().count(),
    })),
    RenderEvent::Error { message } => Some(serde_json::json!({
      "seq": seq,
      "event": "error",
      "message": message,
    })),
    RenderEvent::CaptureHealth(_) | RenderEvent::Meter(_) | RenderEvent::DebugStats(_) => None,
  }
}

fn main() -> Result<()> {
  logging::init_quiet();

  let cli = Cli::parse();
  match cli.command {
    Commands::Devices => cmd_devices(),
    Commands::Listen(args) => cmd_listen(args),
    Commands::TranscribeFile(args) => cmd_transcribe_file(args),
  }
}

fn cmd_devices() -> Result<()> {
  let host = cpal::default_host();
  let default = host.default_input_device().and_then(|d| device_id_string(&d));

  let devs = CpalInput::list_input_devices()?;
  if devs.is_empty() {
    return Err(anyhow!("no input devices found"));
  }

  for (i, dev) in devs.into_iter().enumerate() {
    let name = device_name(&dev).unwrap_or_else(|| "<unknown>".to_string());
    let dev_id = device_id_string(&dev).unwrap_or_default();
    let mark = default.as_ref().map(|d| if d == &dev_id { "*" } else { " " }).unwrap_or(" ");
    println!("{mark} {i}: {name}");
  }
  Ok(())
}

fn cmd_listen(args: ListenArgs) -> Result<()> {
  let (device_index, device_label) = choose_input_device(args.select_device, args.device)?;

  let cfg = pipeline_config_from_common(&args.common)?;
  let model_label = cfg.model_label();

  let shutdown = Arc::new(AtomicBool::new(false));
  let (tx, rx) = chan::unbounded::<RenderEvent>();
  let renderer: Arc<dyn Renderer> = Arc::new(ChanRenderer { tx });

  let shutdown_engine = Arc::clone(&shutdown);
  let renderer_engine = Arc::clone(&renderer);
  let cfg_engine = cfg.clone();
  let chunk_ms = args.chunk_ms;
  let buffer_ms = args.buffer_ms;
  let engine_handle = std::thread::spawn(move || {
    let res: Result<()> = (|| {
      let device = open_device_by_index(device_index)?;
      let input_shutdown = Arc::clone(&shutdown_engine);
      let mut input = CpalInput::open_with_device(
        device,
        CpalInputConfig {
          chunk_ms,
          buffer_ms,
          capture_enabled: None,
          shutdown: Some(input_shutdown),
        },
      )
      .context("failed to open microphone capture")?;
      run_pipeline(&mut input, renderer_engine, cfg_engine, shutdown_engine)
    })();

    if let Err(e) = res {
      // Best-effort: surface pipeline errors in the UI.
      renderer.emit(RenderEvent::Error { message: e.to_string() });
    };
  });

  let ui_res = ui::run_ui(rx, shutdown.clone(), model_label, device_label);

  shutdown.store(true, Ordering::Relaxed);
  let _ = engine_handle.join();

  ui_res
}

fn cmd_transcribe_file(args: TranscribeFileArgs) -> Result<()> {
  let cfg = pipeline_config_from_common(&args.common)?;

  let mut input: Box<dyn AudioInput> = if is_wav(&args.path) {
    Box::new(WavInput::open(&args.path, args.chunk_ms)?)
  } else {
    Box::new(DecodedInput::decode(&args.path, args.chunk_ms)?)
  };
  if args.realtime {
    input = Box::new(PacedInput::new(input));
  }

  let renderer = Arc::new(CollectingRenderer::new(args.events_jsonl));
  let shutdown = Arc::new(AtomicBool::new(false));
  if args.debug_stats {
    let controls = Arc::new(PipelineControls::default());
    controls.set_debug_stats_enabled(true);
    let options = PipelineRunOptions { controls: Some(controls), ..Default::default() };
    run_pipeline_with_options(&mut *input, renderer.clone(), cfg, shutdown, options)?;
  } else {
    run_pipeline(&mut *input, renderer.clone(), cfg, shutdown)?;
  }

  if args.events_jsonl {
    for event in renderer.snapshot_events() {
      println!("{}", serde_json::to_string(&event)?);
    }
  } else {
    for line in renderer.snapshot_lines() {
      println!("{}", line.trim());
    }
  }

  Ok(())
}

/// Wraps an `AudioInput` and sleeps in `read_chunk` so audio is delivered at wall-clock speed,
/// matching live capture cadence. The concurrent 560ms refined worker then runs at the same rate
/// it does live, so `transcribe-file --realtime --events-jsonl` reproduces live caption churn (the
/// fire-hose default feed instead starves the refined stream, hiding it).
struct PacedInput {
  inner: Box<dyn AudioInput>,
  start: Option<std::time::Instant>,
  sample_rate: u32,
  channels: u32,
}

impl PacedInput {
  fn new(inner: Box<dyn AudioInput>) -> Self {
    let spec = inner.spec();
    Self {
      inner,
      start: None,
      sample_rate: spec.sample_rate.max(1),
      channels: u32::from(spec.channels).max(1),
    }
  }
}

impl AudioInput for PacedInput {
  fn spec(&self) -> AudioSpec {
    self.inner.spec()
  }

  fn health(&self) -> AudioHealth {
    self.inner.health()
  }

  fn read_chunk(&mut self) -> Result<Option<AudioChunk>> {
    let chunk = self.inner.read_chunk()?;
    if let Some(c) = &chunk {
      let start = *self.start.get_or_insert_with(std::time::Instant::now);
      let end_frame = c.start_frame + (c.frames.len() as u64 / u64::from(self.channels));
      let target =
        std::time::Duration::from_secs_f64(end_frame as f64 / f64::from(self.sample_rate));
      if let Some(remaining) = target.checked_sub(start.elapsed()) {
        std::thread::sleep(remaining);
      }
    }
    Ok(chunk)
  }
}

fn choose_input_device(select: bool, device_index: Option<usize>) -> Result<(usize, String)> {
  let devs = CpalInput::list_input_devices()?;
  if devs.is_empty() {
    return Err(anyhow!("no input devices found"));
  }

  if select {
    eprintln!("Select input device:");
    for (i, dev) in devs.iter().enumerate() {
      let name = device_name(dev).unwrap_or_else(|| "<unknown>".to_string());
      eprintln!("  {i}: {name}");
    }
    eprint!("> ");
    io::stderr().flush().ok();
    let mut s = String::new();
    io::stdin().read_line(&mut s).context("failed to read selection")?;
    let idx: usize = s.trim().parse().context("invalid device index")?;
    let label = devs.get(idx).and_then(device_name).unwrap_or_else(|| format!("#{idx}"));
    return Ok((idx, label));
  }

  if let Some(idx) = device_index {
    let label = devs.get(idx).and_then(device_name).unwrap_or_else(|| format!("#{idx}"));
    return Ok((idx, label));
  }

  let host = cpal::default_host();
  if let Some(def) = host.default_input_device() {
    let def_id = device_id_string(&def);
    if let Some(def_id) = def_id {
      let def_name = device_name(&def).unwrap_or_else(|| "default input".to_string());
      if let Some((i, _)) = devs
        .iter()
        .enumerate()
        .find(|(_i, d)| device_id_string(d).as_deref() == Some(def_id.as_str()))
      {
        return Ok((i, def_name));
      }
    }
  }

  // Fallback: first enumerated device.
  let label = devs.first().and_then(device_name).unwrap_or_else(|| "#0".to_string());
  Ok((0, label))
}

fn open_device_by_index(idx: usize) -> Result<cpal::Device> {
  let devs = CpalInput::list_input_devices()?;
  devs.into_iter().nth(idx).ok_or_else(|| anyhow!("device index out of range"))
}

fn device_name(device: &cpal::Device) -> Option<String> {
  device.description().ok().map(|d| d.name().to_string())
}

fn device_id_string(device: &cpal::Device) -> Option<String> {
  device.id().ok().map(|id| id.to_string())
}

fn workspace_root() -> PathBuf {
  // <repo>/crates/azad-asr -> <repo>
  Path::new(env!("CARGO_MANIFEST_DIR"))
    .parent()
    .and_then(Path::parent)
    .unwrap_or_else(|| Path::new("."))
    .to_path_buf()
}

fn default_mlx_model_dir() -> PathBuf {
  workspace_root().join("models").join("nemotron-mlx")
}

fn default_vad_model_path() -> PathBuf {
  workspace_root().join("models").join("vad").join("silero_vad.mlmodelc")
}

fn is_wav(path: &Path) -> bool {
  path
    .extension()
    .and_then(|e| e.to_str())
    .map(|e| e.eq_ignore_ascii_case("wav"))
    .unwrap_or(false)
}

fn pipeline_config_from_common(args: &CommonArgs) -> Result<PipelineConfig> {
  let vad_model_path = args.vad_model.clone().unwrap_or_else(default_vad_model_path);
  let model_dir = args.mlx_model_dir.clone().unwrap_or_else(default_mlx_model_dir);

  // Friendlier errors: validate that required model files exist up-front.
  ensure_coreml_vad_model(&vad_model_path)?;
  ensure_file(&model_dir.join("config.json"), "MLX Nemotron config")?;
  ensure_file(&model_dir.join("model.safetensors"), "MLX Nemotron weights")?;
  ensure_file(&model_dir.join("tokenizer.model"), "MLX Nemotron tokenizer")?;
  ensure_file(&model_dir.join("vocab.txt"), "MLX Nemotron vocab")?;

  Ok(PipelineConfig {
    vad_model_path,
    vad_helper_path: args.mlx_helper.clone(),
    streaming_model: StreamingModelConfig::MlxNemotron {
      model_dir,
      language: args.language.clone(),
      streaming_chunk_ms: args.streaming_chunk_ms,
      final_chunk_ms: args.final_chunk_ms,
      helper_path: args.mlx_helper.clone(),
    },
    vad_thold: args.vad_thold,
    vad_start_chunks: args.vad_start_chunks,
    pre_roll_ms: args.pre_roll_ms,
    eou_min_silence_ms: args.eou_min_silence_ms,
    eou_max_silence_ms: args.eou_max_silence_ms,
    vad_in_speech_thold: args.vad_in_speech_thold,
    recovery_window_ms: args.recovery_window_ms,
    recovery_vad_thold: args.recovery_vad_thold,
    stable_k: args.stable_k,
    stable_h: args.stable_h,
    live_display_mutable_tail: args.live_display_mutable_tail,
    finalizing_pulse_enabled: true,
  })
}

fn ensure_file(path: &Path, label: &str) -> Result<()> {
  if path.is_file() {
    return Ok(());
  }
  Err(anyhow!("{label} missing: {}", path.display()))
}

fn ensure_coreml_vad_model(path: &Path) -> Result<()> {
  for file in [
    "analytics/coremldata.bin",
    "coremldata.bin",
    "metadata.json",
    "model.mil",
    "weights/weight.bin",
  ] {
    ensure_file(&path.join(file), "CoreML VAD model file")?;
  }
  Ok(())
}
