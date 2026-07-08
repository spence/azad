use crate::audio::{AudioHealth, AudioInput, AudioSpec};
use crate::coreml_vad::{CoreMlVadConfig, CoreMlVadProcessor};
use crate::mlx::{MlxNemotronAsr, MlxNemotronConfig};
use crate::render::{RenderEvent, Renderer, TurnStartedReason};
use crate::stability::StabilityTracker;
use crate::thread_qos;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

mod audio_prep;
mod stitch;

use audio_prep::{AudioPrep, SampleQueue, health_to_view, levels_dbfs, round_up_to_chunk};

use stitch::{
  INCREMENTAL_STITCH_MIN_OVERLAP_TOKENS, INCREMENTAL_STITCH_TAIL_WINDOW_TOKENS,
  normalize_chunk_case, normalize_stitch_token, stitch_incremental_text, tokenize_for_stitch,
};

const TARGET_SR: u32 = 16_000;
const CHUNK_SAMPLES: usize = 2_560; // 160ms @ 16kHz
const LIVE_STREAM_GAP_LOG_THRESHOLD_SAMPLES: usize = TARGET_SR as usize * 2;
const LIVE_REFINE_MAX_STITCH_EXTRA_TOKENS: usize = 8;
const LIVE_REFINE_STREAM_LEAD_TOKENS: usize = 8;
const LIVE_DISPLAY_STABLE_OVERLAP_TOKENS: usize = 4;
/// The refined 560ms stream lags the live 80ms stream by ~1-2 words and can re-segment, so it must
/// only ever correct the volatile live tail — never rewrite settled text. This tight mutable tail
/// freezes the committed prefix so subtle in-place corrections (and filler decisions) stick instead
/// of flip-flopping, while a large divergence can rewrite at most this many trailing tokens.
const LIVE_DISPLAY_MUTABLE_TAIL_TOKENS: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FinalizingPulsePlan {
  Emit,
  SkipDisabled,
  SkipAudioEmpty,
  SkipDraftEmpty,
}

fn finalizing_pulse_plan(
  finalizing_pulse_enabled: bool,
  audio_has_samples: bool,
  draft_has_text: bool,
) -> FinalizingPulsePlan {
  if !finalizing_pulse_enabled {
    FinalizingPulsePlan::SkipDisabled
  } else if !audio_has_samples {
    FinalizingPulsePlan::SkipAudioEmpty
  } else if !draft_has_text {
    FinalizingPulsePlan::SkipDraftEmpty
  } else {
    FinalizingPulsePlan::Emit
  }
}

fn compose_live_display_text(refined_text: &str, streaming_text: &str) -> String {
  let refined = refined_text.trim();
  let streaming = streaming_text.trim();
  if refined.is_empty() {
    return streaming.to_string();
  }
  if streaming.is_empty() {
    return refined.to_string();
  }

  let refined_tokens = live_token_count(refined);
  let streaming_tokens = live_token_count(streaming);
  if refined_tokens == 0 {
    return streaming.to_string();
  }
  if streaming_tokens == 0 {
    return refined.to_string();
  }

  if let Some(with_streaming_tail) = append_streaming_tail_to_refinement(refined, streaming) {
    return with_streaming_tail;
  }

  if streaming_tokens > refined_tokens.saturating_add(LIVE_REFINE_STREAM_LEAD_TOKENS) {
    streaming.to_string()
  } else {
    refined.to_string()
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum LiveDraftRenderPlan {
  StreamingHypothesis(String),
  ReplacementDisplay(String),
}

fn plan_live_draft_render(refined_text: &str, streaming_text: &str) -> Option<LiveDraftRenderPlan> {
  let display = normalize_chunk_case("", compose_live_display_text(refined_text, streaming_text));
  let display = display.trim().to_string();
  if display.is_empty() {
    return None;
  }

  if refined_text.trim().is_empty() {
    Some(LiveDraftRenderPlan::StreamingHypothesis(display))
  } else {
    Some(LiveDraftRenderPlan::ReplacementDisplay(display))
  }
}

fn plan_live_draft_render_after_previous(
  previous_display: &str,
  refined_text: &str,
  streaming_text: &str,
) -> Option<LiveDraftRenderPlan> {
  let plan = plan_live_draft_render(refined_text, streaming_text)?;
  let LiveDraftRenderPlan::ReplacementDisplay(display) = &plan else {
    return Some(plan);
  };

  if live_streaming_should_supersede_replacement(previous_display, display, streaming_text) {
    let streaming = normalize_chunk_case("", streaming_text.trim().to_string()).trim().to_string();
    if !streaming.is_empty() {
      return Some(LiveDraftRenderPlan::ReplacementDisplay(streaming));
    }
  }

  Some(plan)
}

fn live_streaming_should_supersede_replacement(
  previous_display: &str,
  replacement_display: &str,
  streaming_text: &str,
) -> bool {
  let previous = previous_display.trim();
  let streaming = streaming_text.trim();
  if previous.is_empty() || streaming.is_empty() {
    return false;
  }

  let previous_tokens = live_display_token_count(previous);
  let streaming_tokens = live_display_token_count(streaming);
  if streaming_tokens <= previous_tokens || !live_display_can_replace(previous, streaming) {
    return false;
  }

  let replacement_tokens = live_display_token_count(replacement_display);
  !live_display_can_replace(previous, replacement_display) || replacement_tokens <= previous_tokens
}

fn live_token_count(text: &str) -> usize {
  tokenize_for_stitch(text).len()
}

fn live_stream_output_gap(
  previous_audio_samples: usize,
  current_audio_samples: usize,
) -> Option<(usize, u32)> {
  let gap = current_audio_samples.saturating_sub(previous_audio_samples);
  (gap >= LIVE_STREAM_GAP_LOG_THRESHOLD_SAMPLES).then(|| (gap, samples_to_ms_at_target_sr(gap)))
}

const LIVE_DISPLAY_TOKEN_ROLLBACK_TOLERANCE: usize = 1;

fn live_display_token_count(text: &str) -> usize {
  live_token_count(text)
}

fn live_display_can_replace(previous: &str, candidate: &str) -> bool {
  let candidate = candidate.trim();
  if candidate.is_empty() {
    return false;
  }
  let previous = previous.trim();
  if previous.is_empty() {
    return true;
  }

  let previous_tokens = live_display_token_count(previous);
  let candidate_tokens = live_display_token_count(candidate);
  candidate_tokens.saturating_add(LIVE_DISPLAY_TOKEN_ROLLBACK_TOLERANCE) >= previous_tokens
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LiveDisplayTokenSpan {
  end: usize,
  match_key: String,
}

fn stabilize_live_display_replacement(
  previous: &str,
  candidate: &str,
  mutable_tail: usize,
) -> String {
  let previous = previous.trim();
  let candidate = candidate.trim();
  if previous.is_empty() || candidate.is_empty() {
    return candidate.to_string();
  }

  let previous_tokens = live_display_token_spans(previous);
  if previous_tokens.len() <= mutable_tail {
    return candidate.to_string();
  }

  let candidate_tokens = live_display_token_spans(candidate);
  let stable_len = previous_tokens.len().saturating_sub(mutable_tail);
  if stable_len < LIVE_DISPLAY_STABLE_OVERLAP_TOKENS
    || candidate_tokens.len() < LIVE_DISPLAY_STABLE_OVERLAP_TOKENS
  {
    return candidate.to_string();
  }

  let Some(candidate_boundary) = find_live_display_stable_boundary(
    &previous_tokens[..stable_len],
    &candidate_tokens,
    mutable_tail,
  ) else {
    return previous.to_string();
  };

  let prefix_end = previous_tokens[stable_len - 1].end;
  let tail_start = candidate_tokens[candidate_boundary].end;
  join_live_display_prefix_and_tail(&previous[..prefix_end], &candidate[tail_start..])
}

fn live_display_token_spans(text: &str) -> Vec<LiveDisplayTokenSpan> {
  let mut spans = Vec::new();
  let mut token_start = None;

  for (idx, ch) in text.char_indices() {
    if ch.is_whitespace() {
      if let Some(start) = token_start.take() {
        push_live_display_token_span(text, start, idx, &mut spans);
      }
    } else if token_start.is_none() {
      token_start = Some(idx);
    }
  }

  if let Some(start) = token_start {
    push_live_display_token_span(text, start, text.len(), &mut spans);
  }

  spans
}

fn push_live_display_token_span(
  text: &str,
  start: usize,
  end: usize,
  spans: &mut Vec<LiveDisplayTokenSpan>,
) {
  let match_key = normalize_stitch_token(&text[start..end]);
  if !match_key.is_empty() {
    spans.push(LiveDisplayTokenSpan { end, match_key });
  }
}

fn find_live_display_stable_boundary(
  previous_stable_tokens: &[LiveDisplayTokenSpan],
  candidate_tokens: &[LiveDisplayTokenSpan],
  mutable_tail: usize,
) -> Option<usize> {
  let max_overlap = LIVE_DISPLAY_STABLE_OVERLAP_TOKENS
    .min(previous_stable_tokens.len())
    .min(candidate_tokens.len());
  if max_overlap < INCREMENTAL_STITCH_MIN_OVERLAP_TOKENS {
    return None;
  }

  for overlap in (INCREMENTAL_STITCH_MIN_OVERLAP_TOKENS..=max_overlap).rev() {
    let previous_start = previous_stable_tokens.len() - overlap;
    let expected_candidate_start = previous_start;
    let min_candidate_start = expected_candidate_start.saturating_sub(mutable_tail);
    let max_candidate_start =
      (expected_candidate_start + mutable_tail).min(candidate_tokens.len().saturating_sub(overlap));

    let mut best: Option<(usize, usize)> = None;
    for candidate_start in min_candidate_start..=max_candidate_start {
      let candidate_slice = &candidate_tokens[candidate_start..candidate_start + overlap];
      if previous_stable_tokens[previous_start..]
        .iter()
        .zip(candidate_slice.iter())
        .all(|(left, right)| left.match_key == right.match_key)
      {
        let distance = candidate_start.abs_diff(expected_candidate_start);
        let replace = best
          .map(|(best_distance, best_start)| {
            distance < best_distance || (distance == best_distance && candidate_start > best_start)
          })
          .unwrap_or(true);
        if replace {
          best = Some((distance, candidate_start));
        }
      }
    }

    if let Some((_, candidate_start)) = best {
      return Some(candidate_start + overlap - 1);
    }
  }

  None
}

fn join_live_display_prefix_and_tail(prefix: &str, tail: &str) -> String {
  let prefix = prefix.trim();
  let tail = tail.trim();
  if prefix.is_empty() {
    tail.to_string()
  } else if tail.is_empty() {
    prefix.to_string()
  } else {
    format!("{prefix} {tail}")
  }
}

fn append_streaming_tail_to_refinement(refined: &str, streaming: &str) -> Option<String> {
  let refined_tokens = tokenize_for_stitch(refined);
  let streaming_tokens = tokenize_for_stitch(streaming);
  if refined_tokens.len() < INCREMENTAL_STITCH_MIN_OVERLAP_TOKENS
    || streaming_tokens.len() < INCREMENTAL_STITCH_MIN_OVERLAP_TOKENS
  {
    return None;
  }

  let max_overlap = refined_tokens.len().min(streaming_tokens.len());
  for overlap in (INCREMENTAL_STITCH_MIN_OVERLAP_TOKENS..=max_overlap).rev() {
    let refined_start = refined_tokens.len() - overlap;
    let refined_tail = &refined_tokens[refined_start..];
    for stream_start in (0..=streaming_tokens.len() - overlap).rev() {
      let stream_end = stream_start + overlap;
      if stream_end == streaming_tokens.len() {
        continue;
      }
      let stream_slice = &streaming_tokens[stream_start..stream_end];
      if refined_tail
        .iter()
        .zip(stream_slice.iter())
        .all(|(left, right)| left.match_key == right.match_key)
      {
        let tail = streaming_tokens[stream_end..]
          .iter()
          .map(|token| token.original.as_str())
          .collect::<Vec<_>>()
          .join(" ");
        if tail.is_empty() || live_token_count(&tail) > LIVE_REFINE_MAX_STITCH_EXTRA_TOKENS {
          return None;
        }
        return Some(stitch_incremental_text(
          refined,
          &tail,
          INCREMENTAL_STITCH_TAIL_WINDOW_TOKENS,
          INCREMENTAL_STITCH_MIN_OVERLAP_TOKENS,
          None,
          0,
        ));
      }
    }
  }

  None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineState {
  Idle,
  Speech,
}

#[derive(Debug, Clone)]
pub enum DebugStatsEvent {
  PartialFinalizeOutcome {
    turn_id: u64,
    outcome: String,
    reason: String,
  },
  PartialAuditResult {
    turn_id: u64,
    emitted_kind: String,
    exact: bool,
    partial_count: usize,
    emitted_tokens: usize,
    full_tokens: usize,
    edit_distance: usize,
    wer_like: f64,
    lcp_tokens: usize,
    lcp_pct: f64,
  },
  PartialAuditError {
    turn_id: u64,
    emitted_kind: String,
    partial_count: usize,
    message: String,
  },
}

#[derive(Debug, Clone)]
pub struct StatusView {
  pub state: EngineState,
  pub detail: String,
}

#[derive(Debug, Clone, Copy)]
pub struct MeterView {
  pub peak_db: f32,
  pub vad_speech: bool,
  pub vad_prob: f32,
  pub vad_thold: f32,
}

#[derive(Debug, Clone, Copy)]
pub struct AudioHealthView {
  pub gap_ms: u64,
  pub worst_gap_ms: u64,
  pub dropped_ms: u64,
  pub backlog_ms: u64,
  pub worst_backlog_ms: u64,
}

#[derive(Debug, Clone)]
pub enum StreamingModelConfig {
  MlxNemotron {
    model_dir: PathBuf,
    language: String,
    streaming_chunk_ms: u32,
    final_chunk_ms: u32,
    helper_path: Option<PathBuf>,
  },
}

#[derive(Debug, Clone)]
pub struct PipelineConfig {
  pub vad_model_path: PathBuf,
  pub vad_helper_path: Option<PathBuf>,
  pub streaming_model: StreamingModelConfig,
  pub vad_thold: f32,
  pub vad_start_chunks: usize,
  pub pre_roll_ms: u32,

  pub eou_min_silence_ms: u32,
  pub eou_max_silence_ms: u32,

  /// VAD probability floor while a turn is in progress. Audio chunks with
  /// `vad_prob` below this value count as silence and accumulate against
  /// `eou_max_silence_ms`. Distinct from `vad_thold` (the turn-START threshold) —
  /// the in-speech floor is set much lower so soft continuation, trailing-off
  /// speech, and quieter passages keep the turn alive instead of being misread
  /// as silence. Starting a turn requires confidence; continuing one should
  /// require very little.
  ///
  /// Was implicitly `(vad_thold - 0.15).max(0.15) = 0.30` for years. Production
  /// turn 252 (2026-05-01) showed sustained `vad_prob` of 0.01-0.24 during
  /// continuous user speech that was sub-0.30; the engine accumulated 1.12 s of
  /// "silence" and force-ended mid-clause. 0.10 keeps any non-trivial voice
  /// activity above the floor while staying above typical mic / room noise
  /// floor (< 0.05 in tests). The `eou_max_silence_ms = 1000 ms` ceiling is
  /// still the ultimate backstop — turns can't run forever even if the floor
  /// is breached by a noisy environment.
  pub vad_in_speech_thold: f32,

  /// Tentative-finalize recovery window. After EOU latches and `eou_min_silence_ms`
  /// has been satisfied, the pipeline waits this long before actually committing
  /// the turn. During the window, audio keeps appending and EOU keeps being fed;
  /// if VAD picks up speech AND EOU produces meaningful text, the latch is undone
  /// and the turn continues. Set to 0 to disable (commit immediately, today's
  /// behaviour).
  pub recovery_window_ms: u32,
  /// VAD probability threshold for "user is still talking" during the recovery
  /// window. Should be lower than `vad_thold` (the turn-start threshold) — false-
  /// positive recovery only costs latency, false-negative cuts the user off.
  pub recovery_vad_thold: f32,

  pub stable_k: usize,
  pub stable_h: usize,

  /// Controls the UI "finalizing" pulse shown between the live draft and the refined replace.
  pub finalizing_pulse_enabled: bool,
}

impl PipelineConfig {
  pub fn model_label(&self) -> String {
    match &self.streaming_model {
      StreamingModelConfig::MlxNemotron {
        model_dir,
        language,
        streaming_chunk_ms,
        final_chunk_ms,
        ..
      } => {
        let model = model_dir.file_name().and_then(|s| s.to_str()).unwrap_or("nemotron-mlx");
        format!(
          "nemotron-3.5-mlx ({model}, {language}, live={streaming_chunk_ms}ms, final={final_chunk_ms}ms)"
        )
      }
    }
  }
}

enum StreamingAsr {
  MlxNemotron(MlxNemotronAsr),
}

impl StreamingAsr {
  fn load(cfg: &PipelineConfig) -> Result<Self> {
    match &cfg.streaming_model {
      StreamingModelConfig::MlxNemotron {
        model_dir,
        language,
        streaming_chunk_ms,
        final_chunk_ms,
        helper_path,
      } => {
        let mlx = MlxNemotronAsr::load(&MlxNemotronConfig {
          model_dir: model_dir.clone(),
          language: language.clone(),
          streaming_chunk_ms: *streaming_chunk_ms,
          final_chunk_ms: *final_chunk_ms,
          helper_path: helper_path.clone(),
        })
        .with_context(|| format!("failed to load MLX Nemotron model at {}", model_dir.display()))?;
        Ok(Self::MlxNemotron(mlx))
      }
    }
  }

  fn transcribe_chunk(&mut self, piece: &[f32]) -> Result<(String, bool)> {
    match self {
      Self::MlxNemotron(mlx) => {
        let out = mlx.transcribe_chunk(piece)?;
        Ok((out, false))
      }
    }
  }

  fn reset_turn(&mut self) -> Result<()> {
    match self {
      Self::MlxNemotron(mlx) => mlx.reset_turn(),
    }
  }

  fn reset_after_tentative_finalize(&mut self) -> Result<()> {
    match self {
      Self::MlxNemotron(_) => Ok(()),
    }
  }
}

/// Config for the persistent refined-stream worker: it runs its streaming session at the
/// higher-quality final chunk size (fed the turn's audio continuously), so its live `chunk` step
/// already decodes at final quality and finalize is a cheap flush.
fn finalizer_config(model: &StreamingModelConfig) -> MlxNemotronConfig {
  match model {
    StreamingModelConfig::MlxNemotron {
      model_dir, language, final_chunk_ms, helper_path, ..
    } => MlxNemotronConfig {
      model_dir: model_dir.clone(),
      language: language.clone(),
      streaming_chunk_ms: *final_chunk_ms,
      final_chunk_ms: *final_chunk_ms,
      helper_path: helper_path.clone(),
    },
  }
}

#[derive(Debug)]
pub struct PipelineControls {
  manual_hold_active: AtomicBool,
  auto_vad_enabled: AtomicBool,
  capture_enabled: AtomicBool,
  debug_stats_enabled: AtomicBool,
  start_min_rms_db_bits: AtomicU32,
  force_start_requested: AtomicBool,
  force_finish_requested: AtomicBool,
  cancel_turn_requested: AtomicBool,
  /// Wall-clock instant of the latest false→true `capture_enabled`
  /// transition. Used as the t=0 reference for the cold-start observability
  /// logs (`AZAD_AUDIO_FIRST_NONZERO`, `AZAD_VAD_COLDSTART`,
  /// `AZAD_VAD_START_LATENCY`). `None` until the first enable.
  last_capture_enable_at: std::sync::Mutex<Option<Instant>>,
  /// Wake channel for the audio consumer thread. The CPAL callback signals
  /// this on every buffer push, and `set_capture_enabled` signals it on a
  /// capture-state flip. The mutex guards no shared state; it exists solely to
  /// satisfy the `Condvar` API. The predicate is ring-buffer occupancy owned by
  /// the single consumer, and a bounded backstop timeout covers lost wakeups so
  /// the notifier never has to lock from the real-time audio callback.
  wake_lock: std::sync::Mutex<()>,
  wake: std::sync::Condvar,
}

impl PipelineControls {
  pub fn set_manual_hold_active(&self, active: bool) {
    self.manual_hold_active.store(active, Ordering::Relaxed);
  }

  pub fn manual_hold_active(&self) -> bool {
    self.manual_hold_active.load(Ordering::Relaxed)
  }

  pub fn set_auto_vad_enabled(&self, enabled: bool) {
    self.auto_vad_enabled.store(enabled, Ordering::Relaxed);
  }

  pub fn auto_vad_enabled(&self) -> bool {
    self.auto_vad_enabled.load(Ordering::Relaxed)
  }

  #[track_caller]
  pub fn set_capture_enabled(&self, enabled: bool) {
    let prev = self.capture_enabled.swap(enabled, Ordering::Relaxed);
    if prev != enabled {
      // Record the wake instant on a fresh enable; clear it on disable so
      // post-resume diagnostics never see a stale t=0.
      let now = if enabled { Some(Instant::now()) } else { None };
      if let Ok(mut slot) = self.last_capture_enable_at.lock() {
        *slot = now;
      }
      // Wake the audio consumer so it resumes/pauses the CPAL stream promptly
      // rather than after the next backstop timeout. The paused-capture waiter
      // checks `capture_enabled` while holding this same lock; taking it here
      // prevents a capture-enable notify from landing between that check and
      // the wait call.
      self.notify_control_wake();
      if self.debug_stats_enabled() {
        let loc = std::panic::Location::caller();
        eprintln!(
          "AZAD_CAPTURE ts_ms={} capture_enabled {} -> {} at {}:{}",
          now_ms(),
          prev,
          enabled,
          loc.file(),
          loc.line()
        );
      }
    }
  }

  /// Signal the audio consumer that fresh samples were pushed to the ring
  /// buffer. Called from the real-time CPAL callback, so it must stay
  /// lock-free: `Condvar::notify_one` wakes a waiter without acquiring
  /// `wake_lock`. A missed wakeup (notify between the consumer's occupancy
  /// check and its wait) is bounded by the consumer's backstop timeout.
  pub fn notify_audio(&self) {
    self.wake.notify_one();
  }

  pub fn notify_control_wake(&self) {
    if let Ok(_guard) = self.wake_lock.lock() {
      self.wake.notify_all();
    } else {
      self.wake.notify_all();
    }
  }

  /// Block until the next `notify_audio`/`set_capture_enabled` signal, or until
  /// `backstop` elapses (whichever comes first). The consumer re-checks its own
  /// predicate after returning, so spurious and timed-out wakeups are benign.
  pub fn wait_for_wake(&self, backstop: Duration) {
    if let Ok(guard) = self.wake_lock.lock() {
      let _ = self.wake.wait_timeout(guard, backstop);
    }
  }

  pub fn wait_for_capture_enable_or_wake(&self, backstop: Duration) {
    if let Ok(guard) = self.wake_lock.lock() {
      if self.capture_enabled() {
        return;
      }
      let _ = self.wake.wait_timeout(guard, backstop);
    }
  }

  pub fn capture_enabled(&self) -> bool {
    self.capture_enabled.load(Ordering::Relaxed)
  }

  /// Returns the [`Instant`] of the most recent false→true `capture_enabled`
  /// transition, or [`None`] if capture has never been enabled (or is
  /// currently disabled). Used as the t=0 reference for the cold-start
  /// observability logs.
  pub fn capture_enabled_since(&self) -> Option<Instant> {
    self.last_capture_enable_at.lock().ok().and_then(|slot| *slot)
  }

  pub fn set_debug_stats_enabled(&self, enabled: bool) {
    self.debug_stats_enabled.store(enabled, Ordering::Relaxed);
  }

  pub fn debug_stats_enabled(&self) -> bool {
    self.debug_stats_enabled.load(Ordering::Relaxed)
  }

  pub fn set_start_min_rms_db(&self, rms_db: f32) {
    self
      .start_min_rms_db_bits
      .store(rms_db.clamp(-120.0, 0.0).to_bits(), Ordering::Relaxed);
  }

  pub fn start_min_rms_db(&self) -> f32 {
    f32::from_bits(self.start_min_rms_db_bits.load(Ordering::Relaxed)).clamp(-120.0, 0.0)
  }

  pub fn request_force_start(&self) {
    self.force_start_requested.store(true, Ordering::Relaxed);
  }

  pub fn take_force_start(&self) -> bool {
    self
      .force_start_requested
      .compare_exchange(true, false, Ordering::Relaxed, Ordering::Relaxed)
      .is_ok()
  }

  pub fn request_force_finish(&self) {
    self.force_finish_requested.store(true, Ordering::Relaxed);
  }

  pub fn force_finish_requested(&self) -> bool {
    self.force_finish_requested.load(Ordering::Relaxed)
  }

  pub fn take_force_finish(&self) -> bool {
    self
      .force_finish_requested
      .compare_exchange(true, false, Ordering::Relaxed, Ordering::Relaxed)
      .is_ok()
  }

  pub fn request_cancel_turn(&self) {
    self.cancel_turn_requested.store(true, Ordering::Relaxed);
  }

  pub fn take_cancel_turn(&self) -> bool {
    self
      .cancel_turn_requested
      .compare_exchange(true, false, Ordering::Relaxed, Ordering::Relaxed)
      .is_ok()
  }
}

impl Default for PipelineControls {
  fn default() -> Self {
    Self {
      manual_hold_active: AtomicBool::new(false),
      auto_vad_enabled: AtomicBool::new(true),
      capture_enabled: AtomicBool::new(true),
      debug_stats_enabled: AtomicBool::new(false),
      start_min_rms_db_bits: AtomicU32::new((-60.0f32).to_bits()),
      force_start_requested: AtomicBool::new(false),
      force_finish_requested: AtomicBool::new(false),
      cancel_turn_requested: AtomicBool::new(false),
      wake_lock: std::sync::Mutex::new(()),
      wake: std::sync::Condvar::new(),
      last_capture_enable_at: std::sync::Mutex::new(None),
    }
  }
}

#[derive(Debug, Clone, Default)]
pub struct PipelineRunOptions {
  pub controls: Option<Arc<PipelineControls>>,
  pub stop_after_turn: bool,
}

pub fn run_pipeline(
  input: &mut dyn AudioInput,
  renderer: Arc<dyn Renderer>,
  cfg: PipelineConfig,
  shutdown: Arc<AtomicBool>,
) -> Result<()> {
  run_pipeline_with_options(input, renderer, cfg, shutdown, PipelineRunOptions::default())
}

pub fn run_pipeline_with_options(
  input: &mut dyn AudioInput,
  renderer: Arc<dyn Renderer>,
  cfg: PipelineConfig,
  shutdown: Arc<AtomicBool>,
  options: PipelineRunOptions,
) -> Result<()> {
  // Capture + VAD + EOU are "live" workloads. Keep this thread responsive.
  thread_qos::user_interactive();

  renderer.emit(RenderEvent::Status(StatusView {
    state: EngineState::Idle,
    detail: "starting".to_string(),
  }));
  renderer.emit(RenderEvent::Status(StatusView {
    state: EngineState::Idle,
    detail: "loading models".to_string(),
  }));

  let input_spec = input.spec();

  let vad = CoreMlVadProcessor::load(&CoreMlVadConfig {
    model_path: cfg.vad_model_path.clone(),
    helper_path: cfg.vad_helper_path.clone(),
  })
  .with_context(|| {
    format!("failed to load CoreML Silero VAD model at {}", cfg.vad_model_path.display())
  })?;

  let streaming_asr = StreamingAsr::load(&cfg)?;

  // Finalization runs in a separate helper process so live streaming stays responsive. That
  // worker hosts the persistent refined 560ms session (fed continuously, off the live thread) and
  // flushes it at turn end.
  let worker_cfg = finalizer_config(&cfg.streaming_model);
  let (final_tx, async_rx, final_handle) = spawn_final_worker(worker_cfg, Arc::clone(&renderer));
  let (debug_recording_tx, debug_recording_handle) = spawn_debug_recording_worker();

  renderer
    .emit(RenderEvent::Status(StatusView { state: EngineState::Idle, detail: "idle".to_string() }));

  let runtime = PipelineRuntimeParts {
    vad,
    streaming_asr,
    final_tx,
    debug_recording_tx,
    async_rx,
    controls: options.controls,
    stop_after_turn: options.stop_after_turn,
  };
  let mut runner = Runner::new(input_spec, renderer, cfg, runtime);

  let mut aborted = false;
  loop {
    if shutdown.load(Ordering::Relaxed) {
      aborted = true;
      break;
    }

    let Some(chunk) = input.read_chunk()? else {
      if shutdown.load(Ordering::Relaxed) {
        aborted = true;
      }
      break;
    };

    runner.push_interleaved(&chunk.frames);
    runner.drain_ready(input.health(), &shutdown)?;
    if runner.is_complete() {
      break;
    }
  }

  if aborted {
    // Fast shutdown: drop capture + pipeline state and let the process exit without waiting
    // for any remaining buffered audio or the finalization worker.
    drop(runner);
    return Ok(());
  }

  runner.flush_end(input.health())?;

  // Close worker channels (drops senders) then wait for pending work.
  drop(runner);
  let _ = final_handle.join();
  let _ = debug_recording_handle.join();
  Ok(())
}

struct Runner {
  prep: AudioPrep,
  q: SampleQueue,
  core: PipelineCore,
}

struct PipelineRuntimeParts {
  vad: CoreMlVadProcessor,
  streaming_asr: StreamingAsr,
  final_tx: crossbeam_channel::Sender<FinalJob>,
  debug_recording_tx: crossbeam_channel::Sender<DebugRecordingJob>,
  async_rx: crossbeam_channel::Receiver<FinalResult>,
  controls: Option<Arc<PipelineControls>>,
  stop_after_turn: bool,
}

impl Runner {
  fn new(
    input_spec: AudioSpec,
    renderer: Arc<dyn Renderer>,
    cfg: PipelineConfig,
    runtime: PipelineRuntimeParts,
  ) -> Self {
    let prep = AudioPrep::new(input_spec, TARGET_SR);
    let q = SampleQueue::default();
    let core = PipelineCore::new(input_spec, renderer, cfg, runtime);
    Self { prep, q, core }
  }

  fn is_complete(&self) -> bool {
    self.core.session_complete
  }

  fn push_interleaved(&mut self, interleaved: &[f32]) {
    self.prep.process_interleaved_into(interleaved, &mut self.q);
  }

  fn drain_ready(&mut self, health: AudioHealth, shutdown: &AtomicBool) -> Result<()> {
    while self.q.available() >= CHUNK_SAMPLES {
      if shutdown.load(Ordering::Relaxed) {
        break;
      }
      let chunk16 = self.q.peek(CHUNK_SAMPLES);
      self.core.on_chunk(chunk16, health)?;
      self.q.pop(CHUNK_SAMPLES);
    }
    Ok(())
  }

  fn flush_end(&mut self, health: AudioHealth) -> Result<()> {
    // Pad any remaining audio so we can process the final partial chunk via the same path.
    let rem = self.q.available();
    if rem > 0 {
      let need = CHUNK_SAMPLES.saturating_sub(rem.min(CHUNK_SAMPLES));
      if need > 0 {
        self.q.push_zeros(need);
      }
      while self.q.available() >= CHUNK_SAMPLES {
        let chunk16 = self.q.peek(CHUNK_SAMPLES);
        self.core.on_chunk(chunk16, health)?;
        self.q.pop(CHUNK_SAMPLES);
      }
    }

    self.core.on_end(health)
  }
}

struct PipelineCore {
  cfg: PipelineConfig,
  renderer: Arc<dyn Renderer>,
  input_spec: AudioSpec,

  vad: CoreMlVadProcessor,
  streaming_asr: StreamingAsr,
  final_tx: crossbeam_channel::Sender<FinalJob>,
  debug_recording_tx: crossbeam_channel::Sender<DebugRecordingJob>,
  async_rx: crossbeam_channel::Receiver<FinalResult>,
  controls: Option<Arc<PipelineControls>>,
  stop_after_turn: bool,
  session_complete: bool,

  pre_roll_samples: usize,
  pre_roll: Vec<f32>,

  in_speech: bool,
  silence_samples: usize,
  vad_avg_ema: f32,
  start_run: usize,
  idle_vad_cadence: usize,
  start_confirm_chunks: usize,
  /// Cold-start observability: cached `capture_enabled_since` value so we
  /// can detect a fresh wake (false→true) inside `on_chunk` and (a) emit
  /// `AZAD_VAD_RESUME` once, and (b) start the per-chunk `AZAD_VAD_COLDSTART`
  /// log window. Reset to `None` whenever capture goes false.
  prev_capture_enable_at: Option<Instant>,
  /// Until this instant, every chunk emits a per-chunk
  /// `AZAD_VAD_COLDSTART` line for diagnosing slow-start. Updated to
  /// `now + 10s` on every fresh wake. `None` outside that window.
  cold_start_log_until: Option<Instant>,
  /// Counter of chunks observed since the latest wake — used as
  /// `chunk_idx` in the cold-start logs.
  cold_start_chunk_idx: u32,
  /// Number of chunks remaining in the post-wake VAD seed-grace window.
  /// While > 0, the EMA update path skips its self-seed branch so a
  /// near-silent first chunk after wake can't lock in a low EMA floor.
  /// Set to ~10 (≈ 200 ms at 20 ms/chunk) on every fresh wake.
  vad_seed_grace_chunks: u32,

  tracker: StabilityTracker,
  turn_id: u64,
  turn_audio: Vec<f32>,
  turn_started_at: Instant,
  turn_started_by_vad: bool,

  eou_draft: String,
  prev_silence_ms: u32,
  seen_eou_since_speech: bool,

  // Tentative-finalize state. When the end-condition fires, instead of calling
  // `finish_turn` immediately we enter `tentative_active`. Audio keeps appending,
  // EOU keeps being fed, VAD keeps being scored. If recovery evidence accrues
  // (VAD above `recovery_vad_thold` AND EOU produced meaningful text), we exit
  // tentative and continue the turn. If `recovery_window_ms` elapses without
  // recovery, we commit (the deferred `finish_turn` finally runs).
  tentative_active: bool,
  tentative_latched_at_audio_samples: usize,
  tentative_latch_reason: &'static str,
  tentative_recovery_eou_text_seen: bool,
  tentative_recovery_vad_above_thr: bool,
  // Diagnostic counters — surface as TOON_TENTATIVE log fields on commit so
  // we can spot the EOU-stall fingerprint (chunks > 0 with text_chunks == 0
  // means EOU produced no text during the entire window, which is the
  // failure mode the streaming decoder reset in `enter_tentative_finalize`
  // is meant to prevent).
  tentative_active_chunks: u32,
  tentative_active_with_text: u32,

  live_display: LiveDisplayState,

  // Don't clear "Active" immediately at turn end; short turns may only produce
  // a draft right before finalization, and clearing in the same cycle makes it
  // effectively invisible in the TUI.
  pending_active_clear_chunks: usize,

  last_health_emit: Instant,
  health_interval: Duration,
  last_activation_gate_block_log_at: Option<Instant>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TurnStartReason {
  Vad,
  ManualOverride,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CaptureEnableTransition {
  Unchanged,
  Enabled(Instant),
  Disabled,
}

fn capture_enable_transition(
  previous: Option<Instant>,
  current: Option<Instant>,
) -> CaptureEnableTransition {
  if current == previous {
    return CaptureEnableTransition::Unchanged;
  }
  match current {
    Some(at) => CaptureEnableTransition::Enabled(at),
    None => CaptureEnableTransition::Disabled,
  }
}

fn activation_level_blocks_start(
  in_speech: bool,
  is_speech: bool,
  rms_db: f32,
  start_min_rms_db: f32,
) -> bool {
  !in_speech && is_speech && rms_db < start_min_rms_db
}

impl PipelineCore {
  fn new(
    input_spec: AudioSpec,
    renderer: Arc<dyn Renderer>,
    cfg: PipelineConfig,
    runtime: PipelineRuntimeParts,
  ) -> Self {
    let PipelineRuntimeParts {
      vad,
      streaming_asr,
      final_tx,
      debug_recording_tx,
      async_rx,
      controls,
      stop_after_turn,
    } = runtime;
    let pre_roll_samples = round_up_to_chunk(
      ((TARGET_SR as u64) * (cfg.pre_roll_ms as u64) / 1000) as usize,
      CHUNK_SAMPLES,
    );

    Self {
      start_confirm_chunks: cfg.vad_start_chunks.max(1),
      tracker: StabilityTracker::new(cfg.stable_k, cfg.stable_h),
      pre_roll: Vec::with_capacity(pre_roll_samples),
      pre_roll_samples,
      cfg,
      renderer,
      input_spec,
      vad,
      streaming_asr,
      final_tx,
      debug_recording_tx,
      async_rx,
      controls,
      stop_after_turn,
      session_complete: false,
      in_speech: false,
      silence_samples: 0,
      vad_avg_ema: 0.0,
      start_run: 0,
      idle_vad_cadence: 0,
      prev_capture_enable_at: None,
      cold_start_log_until: None,
      cold_start_chunk_idx: 0,
      vad_seed_grace_chunks: 0,
      turn_id: 0,
      turn_audio: Vec::new(),
      turn_started_at: Instant::now(),
      turn_started_by_vad: false,
      eou_draft: String::new(),
      prev_silence_ms: 0,
      seen_eou_since_speech: false,
      tentative_active: false,
      tentative_latched_at_audio_samples: 0,
      tentative_latch_reason: "",
      tentative_recovery_eou_text_seen: false,
      tentative_recovery_vad_above_thr: false,
      tentative_active_chunks: 0,
      tentative_active_with_text: 0,
      live_display: LiveDisplayState::new(Instant::now()),
      pending_active_clear_chunks: 0,
      last_health_emit: Instant::now(),
      health_interval: Duration::from_millis(200),
      last_activation_gate_block_log_at: None,
    }
  }

  fn debug_stats_enabled(&self) -> bool {
    self
      .controls
      .as_ref()
      .map(|ctrl| ctrl.debug_stats_enabled())
      .unwrap_or_else(partials_debug_env_enabled)
  }

  fn on_chunk(&mut self, chunk16: &[f32], health: AudioHealth) -> Result<()> {
    debug_assert_eq!(chunk16.len(), CHUNK_SAMPLES);
    self.drain_async_results();

    let (rms_db, peak_db) = levels_dbfs(chunk16);

    // Cold-start wake detection. The pipeline thread doesn't otherwise see
    // capture-enable transitions, so we compare against the controls'
    // `capture_enabled_since`. On a fresh wake: emit `AZAD_VAD_RESUME` once
    // and arm the 10 s `AZAD_VAD_COLDSTART` window.
    let cur_enable_at = self.controls.as_ref().and_then(|c| c.capture_enabled_since());
    match capture_enable_transition(self.prev_capture_enable_at, cur_enable_at) {
      CaptureEnableTransition::Enabled(at) => {
        // A capture wake is the user's intent boundary. Pre-roll must start
        // after this point so audio heard before Listen was enabled cannot
        // seed the next turn.
        self.pre_roll.clear();
        self.cold_start_log_until = Some(at + Duration::from_secs(10));
        self.cold_start_chunk_idx = 0;
        // Reset VAD smoothing state and arm the seed-grace window so a
        // near-silent first chunk after wake can't lock in a low EMA
        // floor. Without this, the EMA self-seed branch below would set
        // `vad_avg_ema = vad_avg_of_first_chunk`; if that chunk is
        // near-silent (mic still warming up), the EMA crawls upward over
        // many windows even once real speech arrives.
        if self.debug_stats_enabled() {
          eprintln!(
            "AZAD_VAD_RESUME ts_ms={} ms_since_enable=0 prev_ema={:.3}",
            now_ms(),
            self.vad_avg_ema
          );
        }
        self.vad_avg_ema = 0.0;
        self.start_run = 0;
        self.vad_seed_grace_chunks = 10;
        self.prev_capture_enable_at = cur_enable_at;
      }
      CaptureEnableTransition::Disabled => {
        self.pre_roll.clear();
        self.cold_start_log_until = None;
        self.prev_capture_enable_at = None;
      }
      CaptureEnableTransition::Unchanged => {}
    }

    // Pre-roll ring buffer: only while idle.
    if !self.in_speech && self.pre_roll_samples > 0 {
      self.pre_roll.extend_from_slice(chunk16);
      if self.pre_roll.len() > self.pre_roll_samples {
        let excess = self.pre_roll.len() - self.pre_roll_samples;
        self.pre_roll.drain(..excess);
      }
    }

    // VAD threshold with hysteresis. Asymmetric on purpose: starting a turn
    // requires `vad_on` confidence (avoid false starts on background noise);
    // continuing one only needs `vad_in_speech_thold` (a much lower floor) so
    // soft / trailing speech doesn't accumulate as silence and force-end the
    // turn mid-utterance.
    let vad_on = self.cfg.vad_thold;
    let effective_thold = if self.in_speech { self.cfg.vad_in_speech_thold } else { vad_on };

    // While idle, we already hard-gate very low-energy chunks as non-speech.
    // Skip VAD inference for those chunks to avoid paying model compute cost in silence.
    const HARD_SILENCE_RMS_DB: f32 = -60.0;
    const IDLE_VAD_INTERVAL_CHUNKS: usize = 2;
    let deep_silence_idle = !self.in_speech && rms_db < HARD_SILENCE_RMS_DB;
    let cadence_skip_idle = if !self.in_speech && self.start_run == 0 {
      self.idle_vad_cadence = (self.idle_vad_cadence + 1) % IDLE_VAD_INTERVAL_CHUNKS;
      self.idle_vad_cadence != 0
    } else {
      self.idle_vad_cadence = 0;
      false
    };

    // Silero can occasionally fail to produce output for a chunk; treat it as decay.
    let alpha = 0.20;
    let mut vad_avg = 0.0f32;
    if deep_silence_idle {
      self.vad_avg_ema = 0.0;
    } else if cadence_skip_idle {
      self.vad_avg_ema *= 1.0 - alpha;
    } else {
      let probs = self.vad.probabilities(chunk16)?;
      vad_avg = if probs.is_empty() { 0.0 } else { probs.iter().sum::<f32>() / probs.len() as f32 };

      if self.vad_seed_grace_chunks > 0 {
        // Post-wake: skip the self-seed branch. Always mix; a near-silent
        // first chunk just nudges the EMA from 0 toward `vad_avg` at
        // `alpha`, instead of locking it in.
        self.vad_seed_grace_chunks -= 1;
        self.vad_avg_ema = alpha * vad_avg + (1.0 - alpha) * self.vad_avg_ema;
      } else if self.vad_avg_ema == 0.0 {
        self.vad_avg_ema = vad_avg;
      } else {
        self.vad_avg_ema = alpha * vad_avg + (1.0 - alpha) * self.vad_avg_ema;
      }
    }

    let score = vad_avg.max(self.vad_avg_ema);
    let mut is_speech = score >= effective_thold;
    let start_min_rms_db = self
      .controls
      .as_ref()
      .map(|c| c.start_min_rms_db())
      .unwrap_or(HARD_SILENCE_RMS_DB);
    let activation_level_blocks_start =
      activation_level_blocks_start(self.in_speech, is_speech, rms_db, start_min_rms_db);
    if activation_level_blocks_start {
      let now = Instant::now();
      let should_log = self
        .last_activation_gate_block_log_at
        .map(|last| now.duration_since(last) >= Duration::from_millis(250))
        .unwrap_or(true);
      if self.debug_stats_enabled() && should_log {
        self.last_activation_gate_block_log_at = Some(now);
        eprintln!(
          "AZAD_VAD_START_BLOCKED reason=activation_level rms_db={:.1} peak_db={:.1} \
           min_rms_db={:.1} vad_prob={:.3} vad_ema={:.3} thold={:.3}",
          rms_db, peak_db, start_min_rms_db, vad_avg, self.vad_avg_ema, effective_thold,
        );
      }
      is_speech = false;
    }

    // Cold-start observability: per-chunk diagnostics for the first 10 s
    // after a wake. Reveals whether the slow-start signature comes from
    // (a) literal silent audio (low rms_db), (b) the EMA seed lock-in
    // (vad_avg high but vad_ema crawling), or (c) something else.
    if let Some(until) = self.cold_start_log_until {
      let now = Instant::now();
      if now >= until {
        self.cold_start_log_until = None;
      } else if self.debug_stats_enabled() {
        let ms = self.prev_capture_enable_at.map(|t| t.elapsed().as_millis()).unwrap_or(0);
        eprintln!(
          "AZAD_VAD_COLDSTART ms_since_enable={} rms_db={:.1} peak_db={:.1} \
           vad_prob={:.3} vad_ema={:.3} thold={:.3} chunk_idx={}",
          ms,
          rms_db,
          peak_db,
          vad_avg,
          self.vad_avg_ema,
          effective_thold,
          self.cold_start_chunk_idx,
        );
        self.cold_start_chunk_idx = self.cold_start_chunk_idx.saturating_add(1);
      }
    }

    // Hard override for start-gating only: treat very low energy as non-speech while idle.
    // Do not apply this during an active turn, or quiet speech can be cut off early.
    if deep_silence_idle || cadence_skip_idle {
      is_speech = false;
    }

    if !self.in_speech && self.vad_avg_ema < 0.05 {
      self.vad_avg_ema = 0.0;
    }

    self.renderer.emit(RenderEvent::Meter(MeterView {
      peak_db,
      vad_speech: is_speech,
      vad_prob: vad_avg,
      vad_thold: effective_thold,
    }));

    // Capture health (mic only): surfaced even while idle so loss is obvious.
    let now = Instant::now();
    if now.duration_since(self.last_health_emit) >= self.health_interval {
      self.last_health_emit = now;
      self
        .renderer
        .emit(RenderEvent::CaptureHealth(health_to_view(health, self.input_spec)));
    }

    if !self.in_speech {
      if let Some(ctrl) = &self.controls {
        // Clear stale manual-finish signals while idle.
        let _ = ctrl.take_force_finish();
        let _ = ctrl.take_cancel_turn();
      }

      // If we just ended a turn, keep the final "Active" draft visible briefly
      // (one processing chunk) before clearing.
      if self.pending_active_clear_chunks > 0 {
        self.pending_active_clear_chunks = self.pending_active_clear_chunks.saturating_sub(1);
        if self.pending_active_clear_chunks == 0 {
          self.renderer.emit(RenderEvent::Active {
            id: self.turn_id,
            committed: String::new(),
            live: String::new(),
          });
        }
      }

      if self.controls.as_ref().map(|c| c.take_force_start()).unwrap_or(false) {
        self.start_turn(chunk16, TurnStartReason::ManualOverride)?;
        return Ok(());
      }

      let auto_vad_enabled = self.controls.as_ref().map(|c| c.auto_vad_enabled()).unwrap_or(true);
      if !auto_vad_enabled {
        self.start_run = 0;
        return Ok(());
      }

      // Require consecutive VAD speech hits to start.
      if is_speech {
        self.start_run = self.start_run.saturating_add(1);
      } else {
        self.start_run = 0;
      }

      // If very confident, start immediately.
      if vad_avg >= (vad_on + 0.15) {
        self.start_run = self.start_confirm_chunks;
      }

      if self.start_run < self.start_confirm_chunks {
        return Ok(());
      }

      self.start_turn(chunk16, TurnStartReason::Vad)?;
      return Ok(());
    }

    // in_speech == true
    self.start_run = 0;

    let cancel_turn_requested =
      self.controls.as_ref().map(|c| c.take_cancel_turn()).unwrap_or(false);
    if cancel_turn_requested {
      self.abort_turn();
      return Ok(());
    }

    // Guard against false-positive VAD starts that produce no draft text and never
    // naturally transition back to idle (e.g. noisy environments hovering near threshold).
    if self.should_timeout_empty_vad_turn() {
      if self.tentative_active {
        self.commit_finalize_from_tentative()?;
      } else {
        self.finish_turn(false)?;
        self.complete_turn_after_finish();
      }
      return Ok(());
    }

    let silence_ms: u32;
    if is_speech {
      self.silence_samples = 0;
      silence_ms = 0;
    } else {
      self.silence_samples = self.silence_samples.saturating_add(chunk16.len());
      silence_ms = ((self.silence_samples as u64) * 1000 / (TARGET_SR as u64)) as u32;
    }
    let speech_resumed = silence_ms == 0 && self.prev_silence_ms > 0;
    let silence_started = silence_ms > 0 && self.prev_silence_ms == 0;
    let debug_enabled = self.debug_stats_enabled();

    // Track silence-run boundaries and keep EOU latching independent of the exact
    // frame where silence first appears.
    if speech_resumed {
      if debug_enabled && self.seen_eou_since_speech {
        eprintln!(
          "TOON_EOU_LATCH turn_id={} action=clear reason=speech_resumed audio_samples={} \
           prev_silence_ms={}",
          self.turn_id,
          self.turn_audio.len(),
          self.prev_silence_ms
        );
      }
      self.seen_eou_since_speech = false;
      // Strong tentative recovery: VAD cleared the speech threshold during the
      // window. Un-latch and continue the turn.
      if self.tentative_active {
        self.exit_tentative_finalize_recover("speech_resumed");
      }
    }
    if debug_enabled && silence_started {
      eprintln!(
        "TOON_VAD_SILENCE ts_ms={} turn_id={} action=start vad_prob={:.3} thold={:.3} \
         audio_samples={} eou_latched={}",
        now_ms(),
        self.turn_id,
        vad_avg,
        vad_on,
        self.turn_audio.len(),
        self.seen_eou_since_speech
      );
    }
    self.prev_silence_ms = silence_ms;

    self.turn_audio.extend_from_slice(chunk16);

    let eou_draft_len_before = self.eou_draft.len();
    let saw_eou = self.feed_eou(chunk16)?;
    let eou_added_alpha_text = self.eou_draft.len() > eou_draft_len_before
      && self.eou_draft[eou_draft_len_before..].chars().any(|c| c.is_alphabetic());
    if saw_eou && !self.seen_eou_since_speech && debug_enabled {
      eprintln!(
        "TOON_EOU_LATCH turn_id={} action=fire audio_samples={} silence_ms={} is_speech={}",
        self.turn_id,
        self.turn_audio.len(),
        silence_ms,
        is_speech
      );
    }
    if saw_eou {
      self.seen_eou_since_speech = true;
    }

    // Weak tentative recovery: VAD didn't cross the turn-start threshold (so the
    // strong-recovery branch above didn't fire), but the soft attack of a new word
    // is bumping VAD probability AND EOU is decoding it as text. Together these
    // are strong evidence the user is still speaking — un-latch.
    if self.tentative_active {
      self.tentative_active_chunks = self.tentative_active_chunks.saturating_add(1);
      if eou_added_alpha_text {
        self.tentative_active_with_text = self.tentative_active_with_text.saturating_add(1);
      }
      if score >= self.cfg.recovery_vad_thold {
        self.tentative_recovery_vad_above_thr = true;
      }
      if eou_added_alpha_text {
        self.tentative_recovery_eou_text_seen = true;
      }
      if self.tentative_recovery_vad_above_thr && self.tentative_recovery_eou_text_seen {
        self.exit_tentative_finalize_recover("vad_plus_eou");
        // Also clear the EOU latch so subsequent silence doesn't immediately
        // re-trigger an end-condition.
        self.seen_eou_since_speech = false;
        self.silence_samples = 0;
      }
    }

    let force_finish_requested =
      self.controls.as_ref().map(|c| c.force_finish_requested()).unwrap_or(false);
    if force_finish_requested {
      if let Some(ctrl) = &self.controls {
        let _ = ctrl.take_force_finish();
      }
      if self.tentative_active {
        self.commit_finalize_from_tentative()?;
      } else {
        self.finish_turn(false)?;
        self.complete_turn_after_finish();
      }
      return Ok(());
    }

    // Never end while VAD says speech.
    if is_speech {
      return Ok(());
    }

    let suppress_auto_end = self.controls.as_ref().map(|c| c.manual_hold_active()).unwrap_or(false);
    if suppress_auto_end {
      return Ok(());
    }

    // If we're already in tentative-finalize, manage the recovery window: commit
    // when it elapses (or on max-silence), otherwise stay tentative for another
    // chunk. Recovery itself was already evaluated above (strong + weak paths).
    if self.tentative_active {
      let elapsed_ms = self.tentative_window_elapsed_ms();
      let must_commit =
        silence_ms >= self.cfg.eou_max_silence_ms || elapsed_ms >= self.cfg.recovery_window_ms;
      if must_commit {
        self.commit_finalize_from_tentative()?;
      }
      return Ok(());
    }

    // In silence: decide whether to end the utterance.
    let (end_now, reason) = if silence_ms < self.cfg.eou_min_silence_ms {
      (false, "silence_below_min")
    } else if self.seen_eou_since_speech {
      (true, "eou_latched_and_min_silence_met")
    } else if silence_ms >= self.cfg.eou_max_silence_ms {
      (true, "max_silence_reached")
    } else {
      (false, "waiting_for_eou_or_max_silence")
    };

    if !end_now {
      return Ok(());
    }

    if debug_enabled {
      eprintln!(
        "TOON_END_TURN ts_ms={} turn_id={} reason={} silence_ms={} eou_min_ms={} eou_max_ms={} \
         eou_latched={} audio_samples={} vad_prob={:.3} thold={:.3}",
        now_ms(),
        self.turn_id,
        reason,
        silence_ms,
        self.cfg.eou_min_silence_ms,
        self.cfg.eou_max_silence_ms,
        self.seen_eou_since_speech,
        self.turn_audio.len(),
        vad_avg,
        vad_on
      );
    }

    // If recovery is disabled (recovery_window_ms == 0), commit immediately —
    // matches the pre-tentative-finalize behaviour exactly.
    if self.cfg.recovery_window_ms == 0 {
      self.finish_turn(false)?;
      self.complete_turn_after_finish();
      return Ok(());
    }

    // Otherwise enter the tentative window. Audio keeps appending, EOU keeps
    // being fed, VAD keeps being scored. Strong/weak recovery branches above
    // can un-latch us; otherwise this very block re-fires next chunk and
    // commits when the window elapses.
    self.enter_tentative_finalize(reason, vad_avg)?;
    Ok(())
  }

  fn on_end(&mut self, _health: AudioHealth) -> Result<()> {
    self.drain_async_results();
    if self.in_speech {
      if self.tentative_active {
        // Stream ended mid-tentative; commit through the same path so flags
        // clear and `complete_turn_after_finish` runs.
        self.commit_finalize_from_tentative()?;
      } else {
        self.finish_turn(false)?;
      }
    }
    self.drain_async_results();
    Ok(())
  }

  fn start_turn(&mut self, current_chunk: &[f32], reason: TurnStartReason) -> Result<()> {
    self.start_run = 0;
    self.in_speech = true;
    self.silence_samples = 0;
    self.tracker.reset();
    self.pending_active_clear_chunks = 0;
    self.turn_started_at = Instant::now();
    self.turn_started_by_vad = reason == TurnStartReason::Vad;

    self.streaming_asr.reset_turn()?;
    // Reset the background refined session for the new turn.
    let _ = self.final_tx.send(FinalJob {
      turn_id: self.turn_id,
      audio: Vec::new(),
      kind: FinalJobKind::RefineReset,
    });
    self.eou_draft.clear();
    self.prev_silence_ms = 0;
    self.seen_eou_since_speech = false;
    self.tentative_active = false;
    self.tentative_recovery_eou_text_seen = false;
    self.tentative_recovery_vad_above_thr = false;
    self.live_display.reset(Instant::now());

    self.turn_audio.clear();
    self.turn_id = self.turn_id.wrapping_add(1);

    // Single source of truth for "turn started, by what mechanism." VAD-driven
    // starts already log `AZAD_VAD_START_LATENCY` below; non-VAD starts
    // (`ManualOverride` from `force_start`, e.g. push-to-talk) had no
    // observable log line — they only became visible via downstream
    // `TOON_VAD_SILENCE turn_id=N action=start` events from the new turn id.
    // That made it impossible to tell whether a turn was created via VAD or
    // force-start when post-mortem-debugging an "overlay never showed up" turn.
    if self.debug_stats_enabled() {
      eprintln!(
        "TOON_TURN_START ts_ms={} turn_id={} reason={:?} audio_samples={}",
        now_ms(),
        self.turn_id,
        reason,
        self.turn_audio.len(),
      );
    }

    self.renderer.emit(RenderEvent::Status(StatusView {
      state: EngineState::Speech,
      detail: "speech".to_string(),
    }));
    // Unified turn-start signal. Fires for every start_turn regardless of
    // reason so the renderer can arm overlay state for `ManualOverride`
    // turns that have no other engine-side cue. `SpeechStartedByVad` below
    // remains the VAD-specific event (load-bearing for status text and
    // SpeechEvent consumers); both fire on Vad starts (idempotent).
    self.renderer.emit(RenderEvent::TurnStarted {
      reason: match reason {
        TurnStartReason::Vad => TurnStartedReason::Vad,
        TurnStartReason::ManualOverride => TurnStartedReason::Manual,
      },
    });
    if reason == TurnStartReason::Vad {
      self.renderer.emit(RenderEvent::SpeechStartedByVad);
      // Cold-start observability: how long after the latest wake did the
      // VAD finally confirm speech-start? This is the single number that
      // proves "fixed". Only meaningful for the first VAD start after a
      // wake; subsequent starts will have large values, easy to filter.
      if self.debug_stats_enabled() {
        if let Some(at) = self.controls.as_ref().and_then(|c| c.capture_enabled_since()) {
          eprintln!(
            "AZAD_VAD_START_LATENCY ts_ms={} ms_since_enable={} reason=vad",
            now_ms(),
            at.elapsed().as_millis()
          );
        }
      }
    }
    self.renderer.emit(RenderEvent::Active {
      id: self.turn_id,
      committed: String::new(),
      live: String::new(),
    });

    // Feed pre-roll if available; it already includes the current chunk.
    if self.pre_roll_samples > 0 && !self.pre_roll.is_empty() {
      let pre = std::mem::take(&mut self.pre_roll);
      self.turn_audio.extend_from_slice(&pre);
      for piece in pre.chunks_exact(CHUNK_SAMPLES) {
        let _ = self.feed_eou(piece)?;
      }
    } else {
      self.turn_audio.extend_from_slice(current_chunk);
      let _ = self.feed_eou(current_chunk)?;
    }
    self.pre_roll.clear();

    Ok(())
  }

  fn feed_eou(&mut self, piece: &[f32]) -> Result<bool> {
    // Mirror the live audio into the background refined session. Non-blocking send keeps the live
    // thread responsive (goal: zero-lag caption); the refined stream runs slightly behind and its
    // deltas land via `drain_async_results`.
    if !piece.is_empty() {
      let _ = self.final_tx.send(FinalJob {
        turn_id: self.turn_id,
        audio: piece.to_vec(),
        kind: FinalJobKind::RefineChunk,
      });
    }
    let (out, saw_eou) = self.streaming_asr.transcribe_chunk(piece)?;

    if !out.is_empty() {
      let was_empty = self.eou_draft.is_empty();
      let cleaned = out.replace('▁', " ");
      let cleaned = normalize_chunk_case(&self.eou_draft, cleaned);
      let delta_chars = cleaned.chars().count();
      self.eou_draft.push_str(&cleaned);

      // Record per-emission timing and the surface form so the debug-recording sidecar can record
      // what EOU heard for each span. The audio_samples value matches the TOON_EOU_TEXT log line
      // below so log fixtures and runtime data line up exactly.
      self.live_display.eou_emissions.push(EouEmission {
        audio_samples: self.turn_audio.len(),
        delta_chars,
        text: cleaned.clone(),
      });

      self.log_live_stream_output_gap();
      self.emit_active_draft();

      // Diagnostic: capture per-emission timing so we can correlate the renderer's
      // overlay-show gate (`overlay_pending_vad_text` cleared by the first non-empty
      // DraftUpdated) against EOU's actual text-emission cadence. `first=true` marks
      // the very first non-empty emission of this turn — i.e. the moment the renderer
      // is finally allowed to show the listening overlay. Pairs with TOON_EOU_LATCH
      // (which fires on `<EOU>` token detection regardless of whether `out` was empty)
      // to disambiguate "model fired EOU but produced no text" from "model emitted
      // text". Per-chunk overhead is one eprintln, fires only with debug stats on.
      if self.debug_stats_enabled() {
        eprintln!(
          "TOON_EOU_TEXT turn_id={} audio_samples={} first={} delta_chars={} \
           draft_chars={}",
          self.turn_id,
          self.turn_audio.len(),
          was_empty,
          delta_chars,
          self.eou_draft.chars().count(),
        );
      }
    }

    Ok(saw_eou)
  }

  fn log_live_stream_output_gap(&mut self) {
    let current = self.turn_audio.len();
    let previous = self.live_display.last_live_output_audio_samples.replace(current);
    let Some(previous) = previous else {
      return;
    };
    let Some((gap, gap_ms)) = live_stream_output_gap(previous, current) else {
      return;
    };
    if self.debug_stats_enabled() {
      eprintln!(
        "TOON_LIVE_STREAM_GAP turn_id={} from_samples={} to_samples={} gap_samples={} \
         gap_ms={} draft_chars={} refined_chars={}",
        self.turn_id,
        previous,
        current,
        gap,
        gap_ms,
        self.eou_draft.chars().count(),
        self.live_display.live_refined_text.chars().count(),
      );
    }
  }

  fn emit_active_draft(&mut self) {
    // Both modes fold the refined stream into the live caption so the stronger model sharpens it
    // in place (goal #3). Anti-churn (goal #1) is enforced downstream in
    // `emit_replacement_live_display`, which freezes the committed prefix and only mutates a
    // bounded tail — tightly bounded in dual-stream so the lagging 560ms stream can never rewrite
    // a large already-shown span.
    match plan_live_draft_render_after_previous(
      &self.live_display.last_live_display_text,
      &self.live_display.live_refined_text,
      &self.eou_draft,
    ) {
      Some(LiveDraftRenderPlan::StreamingHypothesis(display)) => {
        let (committed, live) = self.tracker.update(&display);
        let visible = format!("{committed}{live}").trim().to_string();
        if !visible.is_empty() {
          self.live_display.last_live_display_text = visible.clone();
          self.record_live_display_event("streaming", "emit", visible, None);
        }
        self.renderer.emit(RenderEvent::Active { id: self.turn_id, committed, live });
      }
      Some(LiveDraftRenderPlan::ReplacementDisplay(display)) => {
        self.emit_replacement_live_display(display);
      }
      None => {}
    }
  }

  fn emit_replacement_live_display(&mut self, display: String) {
    let display = stabilize_live_display_replacement(
      &self.live_display.last_live_display_text,
      &display,
      LIVE_DISPLAY_MUTABLE_TAIL_TOKENS,
    );
    if !live_display_can_replace(&self.live_display.last_live_display_text, &display) {
      let previous = self.live_display.last_live_display_text.clone();
      self.record_live_display_event("refined", "hold_rollback", previous, Some(display.clone()));
      if self.debug_stats_enabled() {
        eprintln!(
          "TOON_LIVE_DISPLAY turn_id={} action=hold_rollback previous_chars={} \
           previous_tokens={} candidate_chars={} candidate_tokens={}",
          self.turn_id,
          self.live_display.last_live_display_text.chars().count(),
          live_display_token_count(&self.live_display.last_live_display_text),
          display.chars().count(),
          live_display_token_count(&display),
        );
      }
      return;
    }

    self.live_display.last_live_display_text = display.clone();
    self.record_live_display_event("refined", "emit", display.clone(), None);
    self.renderer.emit(RenderEvent::Active {
      id: self.turn_id,
      committed: display,
      live: String::new(),
    });
  }

  fn record_live_display_event(
    &mut self,
    source: &'static str,
    action: &'static str,
    text: String,
    candidate_text: Option<String>,
  ) {
    self.live_display.live_display_events.push(LiveDisplayEvent {
      audio_samples: self.turn_audio.len(),
      source,
      action,
      text,
      candidate_text,
    });
  }

  fn enter_tentative_finalize(&mut self, reason: &'static str, vad_prob: f32) -> Result<()> {
    self.tentative_active = true;
    self.tentative_latched_at_audio_samples = self.turn_audio.len();
    self.tentative_latch_reason = reason;
    self.tentative_recovery_eou_text_seen = false;
    self.tentative_recovery_vad_above_thr = false;
    self.tentative_active_chunks = 0;
    self.tentative_active_with_text = 0;
    // Fire the Finalizing pulse immediately so the overlay's pulsing border
    // is visible during the entire recovery window plus the finalization pass.
    // this, the deferred emission collapsed the visible window to ~100 ms when
    self.emit_finalizing_pulse();
    // Reset the EOU decoder so it doesn't stay biased toward re-firing the
    // `<EOU>` token on every subsequent chunk. Without this, the decoder's
    // post-utterance state suppresses text output for the entire tentative
    // window, which (a) makes the overlay appear frozen and (b) prevents
    // the weak-recovery branch (which requires `feed_eou` to add alphabetic
    // text) from ever firing. Cost: we lose inter-chunk RNN context for the
    // recovery audio, which is acceptable since the user has just finished
    // a clause.
    self.streaming_asr.reset_after_tentative_finalize()?;
    if self.debug_stats_enabled() {
      eprintln!(
        "TOON_TENTATIVE turn_id={} action=enter reason={} audio_samples={} vad_prob={:.3}",
        self.turn_id,
        reason,
        self.turn_audio.len(),
        vad_prob
      );
    }
    Ok(())
  }

  fn exit_tentative_finalize_recover(&mut self, signal: &'static str) {
    if self.debug_stats_enabled() {
      eprintln!(
        "TOON_TENTATIVE turn_id={} action=recover signal={} latch_reason={} \
         audio_samples={} elapsed_samples={} chunks={} text_chunks={} \
         vad_evidence={} eou_evidence={}",
        self.turn_id,
        signal,
        self.tentative_latch_reason,
        self.turn_audio.len(),
        self.turn_audio.len().saturating_sub(self.tentative_latched_at_audio_samples),
        self.tentative_active_chunks,
        self.tentative_active_with_text,
        self.tentative_recovery_vad_above_thr,
        self.tentative_recovery_eou_text_seen,
      );
    }
    self.tentative_active = false;
    self.tentative_recovery_eou_text_seen = false;
    self.tentative_recovery_vad_above_thr = false;
    // Tell the renderer the finalize state we entered at tentative-entry is
    // off — clears the pulsing border, returns the overlay to live state.
    self.renderer.emit(RenderEvent::FinalizingCancelled { id: self.turn_id });
    // The existing `speech_resumed` path (or weak-recovery branch) already cleared
    // `seen_eou_since_speech`; no need to repeat here.
  }

  fn commit_finalize_from_tentative(&mut self) -> Result<()> {
    if self.debug_stats_enabled() {
      eprintln!(
        "TOON_TENTATIVE turn_id={} action=commit latch_reason={} audio_samples={} \
         elapsed_samples={} chunks={} text_chunks={} vad_evidence={} eou_evidence={}",
        self.turn_id,
        self.tentative_latch_reason,
        self.turn_audio.len(),
        self.turn_audio.len().saturating_sub(self.tentative_latched_at_audio_samples),
        self.tentative_active_chunks,
        self.tentative_active_with_text,
        self.tentative_recovery_vad_above_thr,
        self.tentative_recovery_eou_text_seen,
      );
    }
    self.tentative_active = false;
    self.tentative_recovery_eou_text_seen = false;
    self.tentative_recovery_vad_above_thr = false;
    // The tentative-entry path already emitted Finalizing; pass `already_pulsed`
    // so finish_turn doesn't fire it a second time.
    self.finish_turn(true)?;
    self.complete_turn_after_finish();
    Ok(())
  }

  fn tentative_window_elapsed_ms(&self) -> u32 {
    samples_to_ms_at_target_sr(
      self.turn_audio.len().saturating_sub(self.tentative_latched_at_audio_samples),
    )
  }

  /// Compose the draft text used for both the in-flight `Finalizing` pulse and
  /// the eventual `FinalLine` emission. Refined partial text owns the display
  /// once available; streaming-only turns use the stability tracker and then
  /// fall back to raw EOU text when no stable draft exists yet.
  fn current_finalize_draft(&self) -> String {
    match plan_live_draft_render_after_previous(
      &self.live_display.last_live_display_text,
      &self.live_display.live_refined_text,
      &self.eou_draft,
    ) {
      Some(LiveDraftRenderPlan::ReplacementDisplay(display)) => {
        if !live_display_can_replace(&self.live_display.last_live_display_text, &display)
          && !self.live_display.last_live_display_text.trim().is_empty()
        {
          return self.live_display.last_live_display_text.trim().to_string();
        }
        return display;
      }
      Some(LiveDraftRenderPlan::StreamingHypothesis(display))
        if !self.live_display.live_refined_text.trim().is_empty() =>
      {
        return display;
      }
      _ => {}
    }

    let mut draft = self.tracker.full_text().trim().to_string();
    if draft.is_empty() {
      draft = self.eou_draft.split_whitespace().collect::<Vec<_>>().join(" ");
    }
    draft
  }

  /// Emit `RenderEvent::Finalizing` for the current turn. Used before backend
  /// finalization so the overlay can show the in-flight state. Skipped when
  /// finalization UI is disabled, there is no audio, or there is no draft text.
  fn emit_finalizing_pulse(&self) {
    let draft = self.current_finalize_draft();
    let plan = finalizing_pulse_plan(
      self.cfg.finalizing_pulse_enabled,
      !self.turn_audio.is_empty(),
      !draft.is_empty(),
    );
    let skip_reason = match plan {
      FinalizingPulsePlan::Emit => None,
      FinalizingPulsePlan::SkipDisabled => Some("pulse_disabled"),
      FinalizingPulsePlan::SkipAudioEmpty => Some("audio_empty"),
      FinalizingPulsePlan::SkipDraftEmpty => Some("empty_draft"),
    };
    if let Some(reason) = skip_reason {
      if self.debug_stats_enabled() {
        eprintln!(
          "TOON_FINALIZE_PULSE turn_id={} action=skip reason={} \
           pulse_enabled={} audio_empty={} draft_empty={} tracker_len={} eou_draft_len={}",
          self.turn_id,
          reason,
          self.cfg.finalizing_pulse_enabled,
          self.turn_audio.is_empty(),
          draft.is_empty(),
          self.tracker.full_text().len(),
          self.eou_draft.len(),
        );
      }
      return;
    }
    if self.debug_stats_enabled() {
      eprintln!(
        "TOON_FINALIZE_PULSE turn_id={} action=emit audio_samples={} draft_chars={}",
        self.turn_id,
        self.turn_audio.len(),
        draft.chars().count(),
      );
    }
    self.renderer.emit(RenderEvent::Finalizing { id: self.turn_id, text: draft });
  }

  fn finish_turn(&mut self, already_pulsed: bool) -> Result<()> {
    let draft = self.current_finalize_draft();
    if !already_pulsed {
      self.emit_finalizing_pulse();
    }
    let audio_snapshot: Vec<f32> = std::mem::take(&mut self.turn_audio);
    self.finish_turn_dual_stream(draft, audio_snapshot)?;
    self.tracker.reset();
    self.eou_draft.clear();
    self.turn_started_by_vad = false;
    Ok(())
  }

  /// Dual-stream finalize: emit the live draft immediately, then flush the continuously-fed
  /// refined session (cheap — no whole-turn re-decode) and replace with the higher-quality
  /// refined text. No stitching, no coverage-gap bailout.
  fn finish_turn_dual_stream(&mut self, draft: String, audio_snapshot: Vec<f32>) -> Result<()> {
    if !draft.is_empty() {
      self
        .renderer
        .emit(RenderEvent::FinalLine { id: self.turn_id, text: draft.clone() });
    }
    self.drain_async_results();
    let _ = self.final_tx.send(FinalJob {
      turn_id: self.turn_id,
      audio: Vec::new(),
      kind: FinalJobKind::RefineFlush,
    });
    let finalize_started_at = Instant::now();
    // Drain the refined worker's backlog fully: RefineFlush is FIFO-queued behind every
    // RefineChunk for this turn, so RefinedFinal only arrives once the worker has consumed all
    // of the turn's audio. Break as soon as it lands (near-instant when the worker kept up);
    // the cap only bounds a pathologically backlogged worker.
    let deadline = finalize_started_at + Duration::from_secs(20);
    loop {
      let now = Instant::now();
      if now >= deadline {
        break;
      }
      match self.async_rx.recv_timeout(deadline.saturating_duration_since(now)) {
        Ok(FinalResult::RefinedDelta { turn_id, delta }) => {
          if turn_id == self.turn_id {
            self.apply_refined_delta(&delta);
          }
        }
        Ok(FinalResult::RefinedFinal { turn_id, text }) => {
          if turn_id == self.turn_id {
            self.apply_refined_delta(&text);
            break;
          }
        }
        Err(_) => break,
      }
    }
    let refined = self.live_display.live_refined_text.trim().to_string();
    let finalize_elapsed_ms = elapsed_ms_since(finalize_started_at);
    if self.debug_stats_enabled() {
      eprintln!(
        "TOON_DUAL_STREAM_FINAL turn_id={} elapsed_ms={} draft_chars={} refined_chars={}",
        self.turn_id,
        finalize_elapsed_ms,
        draft.chars().count(),
        refined.chars().count(),
      );
    }
    let final_text = if refined.is_empty() { draft.clone() } else { refined };
    if !final_text.is_empty() {
      self
        .renderer
        .emit(RenderEvent::ReplaceLine { id: self.turn_id, text: final_text.clone() });
    }
    // Debug-only: reproduce the on-device evidence the legacy audit worker used to write (wav +
    // sidecar) and emit the draft->refined divergence. Off the hot path — the sidecar write runs
    // on the recorder thread, and the divergence is a cheap token edit distance (no model).
    if self.debug_stats_enabled() {
      self.record_dual_final(draft, final_text, finalize_elapsed_ms, audio_snapshot);
    }
    Ok(())
  }

  /// Emit the draft->refined-final divergence (G3 correction magnitude) and enqueue the turn's
  /// wav + sidecar on the recorder thread. Debug-gated; never touches a finalization model.
  fn record_dual_final(
    &mut self,
    draft: String,
    final_text: String,
    finalize_elapsed_ms: u64,
    audio: Vec<f32>,
  ) {
    let event = log_partial_audit_result(
      self.turn_id,
      AuditEmittedKind::DualFinal,
      &[],
      &draft,
      &final_text,
      None,
    );
    self.renderer.emit(RenderEvent::DebugStats(event));

    let eou_emissions = self.live_display.eou_emissions.clone();
    let live_display_events = self.live_display.live_display_events.clone();
    let _ = self.debug_recording_tx.send(DebugRecordingJob {
      turn_id: self.turn_id,
      audio,
      emitted_kind: AuditEmittedKind::DualFinal,
      emitted_text: final_text,
      draft_text: draft,
      finalize_elapsed_ms: Some(finalize_elapsed_ms),
      eou_emissions,
      live_display_events,
    });
  }

  fn abort_turn(&mut self) {
    self.in_speech = false;
    self.start_run = 0;
    self.silence_samples = 0;
    self.pre_roll.clear();
    self.turn_audio.clear();
    self.prev_silence_ms = 0;
    self.seen_eou_since_speech = false;
    self.tentative_active = false;
    self.tentative_recovery_eou_text_seen = false;
    self.tentative_recovery_vad_above_thr = false;
    self.turn_started_by_vad = false;
    self.pending_active_clear_chunks = 0;
    self.tracker.reset();
    self.eou_draft.clear();
    let _ = self.streaming_asr.reset_turn();
    self.live_display.reset(Instant::now());

    self.renderer.emit(RenderEvent::Active {
      id: self.turn_id,
      committed: String::new(),
      live: String::new(),
    });
    self.renderer.emit(RenderEvent::Status(StatusView {
      state: EngineState::Idle,
      detail: "idle".to_string(),
    }));
  }

  fn should_timeout_empty_vad_turn(&self) -> bool {
    if !self.turn_started_by_vad {
      return false;
    }

    let manual_hold_active =
      self.controls.as_ref().map(|c| c.manual_hold_active()).unwrap_or(false);
    if manual_hold_active {
      return false;
    }

    let has_text = !self.tracker.full_text().trim().is_empty() || !self.eou_draft.trim().is_empty();
    if has_text {
      return false;
    }

    let timeout_ms = u64::from(self.cfg.eou_max_silence_ms.max(1)).saturating_mul(3);
    self.turn_started_at.elapsed().as_millis() >= u128::from(timeout_ms)
  }

  fn complete_turn_after_finish(&mut self) {
    if self.stop_after_turn {
      self.session_complete = true;
      self.in_speech = false;
      self.silence_samples = 0;
      self.pre_roll.clear();
      return;
    }

    self.in_speech = false;
    self.silence_samples = 0;
    self.pre_roll.clear();

    // Delay clearing "Active" so short-turn drafts don't get instantly erased before the TUI
    // has a chance to render them.
    self.pending_active_clear_chunks = 1;
    self.renderer.emit(RenderEvent::Status(StatusView {
      state: EngineState::Idle,
      detail: "idle".to_string(),
    }));
  }

  fn drain_async_results(&mut self) {
    while let Ok(result) = self.async_rx.try_recv() {
      match result {
        FinalResult::RefinedDelta { turn_id, delta } => {
          if turn_id == self.turn_id {
            self.apply_refined_delta(&delta);
          }
        }
        FinalResult::RefinedFinal { .. } => {
          // Only consumed synchronously by `finish_turn_dual_stream`; ignore stragglers.
        }
      }
    }
  }

  /// Append a refined streaming delta to the refined text and re-render the composed caption.
  /// The refined stream is append-only (transducer), so this is pure concatenation — the
  /// existing display stabilizer decides whether/how the refined text replaces the live one.
  fn apply_refined_delta(&mut self, delta: &str) {
    if delta.is_empty() {
      return;
    }
    let cleaned = delta.replace('▁', " ");
    let cleaned = normalize_chunk_case(&self.live_display.live_refined_text, cleaned);
    self.live_display.live_refined_text.push_str(&cleaned);
    self.live_display.has_refined_text = true;
    // Do NOT re-render the live caption here: dual-stream keeps the caption a pure streaming
    // hypothesis during speech (goal #1). The accumulated refined text is applied at finalize.
  }
}

fn spawn_final_worker(
  cfg: MlxNemotronConfig,
  renderer: Arc<dyn Renderer>,
) -> (
  crossbeam_channel::Sender<FinalJob>,
  crossbeam_channel::Receiver<FinalResult>,
  std::thread::JoinHandle<()>,
) {
  let (tx, rx) = crossbeam_channel::unbounded::<FinalJob>();
  let (async_tx, async_rx) = crossbeam_channel::unbounded::<FinalResult>();
  let handle = std::thread::spawn(move || {
    // Final-slice latency is user-visible at turn end, so prioritize this worker.
    thread_qos::user_initiated();

    let mut finalizer = match MlxNemotronAsr::load(&cfg) {
      Ok(model) => Some(model),
      Err(e) => {
        renderer.emit(RenderEvent::Error {
          message: format!("failed to load MLX finalization helper: {e}"),
        });
        None
      }
    };

    while let Ok(job) = rx.recv() {
      let Some(ref mut finalizer) = finalizer else {
        renderer.emit(RenderEvent::Error {
          message: "MLX finalization helper unavailable (failed to load)".to_string(),
        });
        continue;
      };

      match job.kind {
        FinalJobKind::RefineChunk => match finalizer.transcribe_chunk(&job.audio) {
          Ok(delta) => {
            if !delta.is_empty() {
              let _ = async_tx.send(FinalResult::RefinedDelta { turn_id: job.turn_id, delta });
            }
          }
          Err(e) => {
            renderer.emit(RenderEvent::Error { message: format!("MLX refined chunk failed: {e}") });
          }
        },
        FinalJobKind::RefineFlush => {
          let text = finalizer.stream_finish().ok().flatten().unwrap_or_default();
          let _ = async_tx.send(FinalResult::RefinedFinal { turn_id: job.turn_id, text });
        }
        FinalJobKind::RefineReset => {
          let _ = finalizer.reset_turn();
        }
      }
    }
  });

  (tx, async_rx, handle)
}

/// Slim debug recorder for dual-stream turns. Runs no finalization model: the draft->refined
/// divergence is emitted on the finalize thread, so this thread only persists the turn's wav +
/// sidecar (draft, refined final, live-display + EOU events) off the hot path. Only reached when
/// debug stats are enabled.
fn spawn_debug_recording_worker()
-> (crossbeam_channel::Sender<DebugRecordingJob>, std::thread::JoinHandle<()>) {
  let (tx, rx) = crossbeam_channel::unbounded::<DebugRecordingJob>();
  let handle = std::thread::spawn(move || {
    // Recording should remain timely for recent rows in debug views without competing with
    // user-interactive work.
    thread_qos::utility();

    while let Ok(job) = rx.recv() {
      if let Err(e) = save_debug_recording(
        job.turn_id,
        &job.audio,
        job.emitted_kind,
        &job.emitted_text,
        "",
        &[],
        &job.eou_emissions,
        &job.live_display_events,
        None,
        &job.draft_text,
        job.finalize_elapsed_ms,
      ) {
        eprintln!("Azad: failed to save dual-final debug recording for turn {}: {e}", job.turn_id);
      }
    }
  });

  (tx, handle)
}

/// Rolling capacity for `save_debug_recording` — non-bailout pairs (wav + json sidecar) kept on
/// disk. Raised from 10 to 40 when the sidecar became the primary on-device evidence stream for the
/// dual-stream rework: the live-proof gate needs ≥20 turns retained at once. Debug-gated; ~160 MB
/// worst case.
const DEBUG_RECORDING_MAX_FILES: usize = 40;

/// Rolling capacity for `save_debug_recording` — bailout pairs (turns whose
/// filename ends in `-bailout`) kept on disk. Larger than the normal cap
/// because bailouts are rare (~3% of turns) and high-value-to-keep for
/// post-hoc investigation; the wav for the failing window must survive
/// the surrounding noise of normal turns landing on top.
const DEBUG_RECORDING_BAILOUT_MAX_FILES: usize = 20;

/// Wall-clock Unix timestamp in milliseconds. Used as a `ts_ms=…` prefix on
/// state-transition log lines so post-hoc analysis can correlate events whose
/// adjacency in the log file does NOT imply adjacency in time (the engine emits
/// no diagnostic output during long idle windows, so two adjacent
/// `AZAD_CAPTURE` lines could be hours apart).
fn now_ms() -> u64 {
  std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .map(|d| d.as_millis() as u64)
    .unwrap_or(0)
}

/// Where `save_debug_recording` writes. `None` when `$HOME` isn't set (headless tests etc.).
fn debug_recordings_dir() -> Option<PathBuf> {
  let home = std::env::var("HOME").ok()?;
  Some(
    PathBuf::from(home)
      .join("Library")
      .join("Application Support")
      .join("Azad")
      .join("debug-recordings"),
  )
}

/// Persist a turn's raw audio + metadata to `~/Library/Application Support/Azad/debug-recordings/`
/// so a replay tool can feed the exact same samples back through the pipeline during validation.
/// Rolling buffer of the most recent [`DEBUG_RECORDING_MAX_FILES`] turns; older pairs are pruned.
/// Bailout jobs (`bailout_reason.is_some()`) get a `-bailout` filename suffix that puts them in
/// a separate, longer-retention pruning tier (`DEBUG_RECORDING_BAILOUT_MAX_FILES`) so the rare
/// failing turns aren't overwritten by the much more common successful ones.
///
/// Called from the audit worker, which only runs when debug-stats is enabled — so no extra
/// opt-in check is needed here. The audio is mono float32 @ 16 kHz, matching the pipeline's
/// internal format so a replay can skip resampling.
#[allow(clippy::too_many_arguments)]
fn save_debug_recording(
  turn_id: u64,
  audio: &[f32],
  emitted_kind: AuditEmittedKind,
  emitted_text: &str,
  full_text: &str,
  partial_segments: &[IncrementalPartialSegment],
  eou_emissions: &[EouEmission],
  live_display_events: &[LiveDisplayEvent],
  bailout_reason: Option<&str>,
  draft_text: &str,
  finalize_elapsed_ms: Option<u64>,
) -> std::io::Result<()> {
  let Some(dir) = debug_recordings_dir() else {
    return Ok(());
  };
  std::fs::create_dir_all(&dir)?;

  let ts_ms = std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .map(|d| d.as_millis() as u64)
    .unwrap_or(0);
  let base = debug_recording_stem(ts_ms, turn_id, bailout_reason.is_some());
  let wav_path = dir.join(format!("{base}.wav"));
  let json_path = dir.join(format!("{base}.json"));

  let spec = hound::WavSpec {
    channels: 1,
    sample_rate: TARGET_SR,
    bits_per_sample: 32,
    sample_format: hound::SampleFormat::Float,
  };
  let mut writer =
    hound::WavWriter::create(&wav_path, spec).map_err(|e| std::io::Error::other(e.to_string()))?;
  for &sample in audio {
    writer.write_sample(sample).map_err(|e| std::io::Error::other(e.to_string()))?;
  }
  writer.finalize().map_err(|e| std::io::Error::other(e.to_string()))?;

  let payload = debug_recording_payload(
    turn_id,
    ts_ms,
    audio.len(),
    emitted_kind,
    emitted_text,
    full_text,
    draft_text,
    finalize_elapsed_ms,
    partial_segments,
    eou_emissions,
    live_display_events,
    bailout_reason,
  );
  std::fs::write(&json_path, serde_json::to_string_pretty(&payload)?)?;

  prune_debug_recordings(&dir);
  Ok(())
}

/// Build the sidecar JSON for a debug recording. Pure (no I/O) so the payload shape is unit-tested
/// without touching disk or the real recordings directory. Additive schema: `pipeline`,
/// `draft_text`, and `finalize_elapsed_ms` were added for the dual-stream recorder; legacy captures
/// leave the latter two empty/null and analyzers key on `pipeline`.
#[allow(clippy::too_many_arguments)]
fn debug_recording_payload(
  turn_id: u64,
  ts_ms: u64,
  num_samples: usize,
  emitted_kind: AuditEmittedKind,
  emitted_text: &str,
  full_text: &str,
  draft_text: &str,
  finalize_elapsed_ms: Option<u64>,
  partial_segments: &[IncrementalPartialSegment],
  eou_emissions: &[EouEmission],
  live_display_events: &[LiveDisplayEvent],
  bailout_reason: Option<&str>,
) -> serde_json::Value {
  let partials_json: Vec<serde_json::Value> = partial_segments
    .iter()
    .map(|p| {
      serde_json::json!({
        "segment_id": p.segment_id,
        "start_sample": p.start_sample,
        "end_sample": p.end_sample,
        "is_tail": p.is_tail,
        "text": p.text,
      })
    })
    .collect();
  let eou_json: Vec<serde_json::Value> = eou_emissions
    .iter()
    .map(|e| {
      serde_json::json!({
        "audio_samples": e.audio_samples,
        "delta_chars": e.delta_chars,
        "text": e.text,
      })
    })
    .collect();
  let live_display_json: Vec<serde_json::Value> = live_display_events
    .iter()
    .map(|e| {
      serde_json::json!({
        "audio_samples": e.audio_samples,
        "source": e.source,
        "action": e.action,
        "text": e.text,
        "candidate_text": e.candidate_text,
      })
    })
    .collect();
  serde_json::json!({
    "turn_id": turn_id,
    "ts_ms": ts_ms,
    "sample_rate": TARGET_SR,
    "num_samples": num_samples,
    "pipeline": pipeline_label(emitted_kind),
    "emitted_kind": audit_kind_label(emitted_kind),
    "emitted_text": emitted_text,
    "draft_text": draft_text,
    "full_text": full_text,
    "finalize_elapsed_ms": finalize_elapsed_ms,
    "partials": partials_json,
    "eou_emissions": eou_json,
    "live_display_events": live_display_json,
    "bailout_reason": bailout_reason,
  })
}

/// Filename stem for the wav + json sidecar pair. The zero-padded `ts_ms`
/// prefix makes lexicographic sort match chronological order, which the
/// pruner depends on for cheap "newest-N" selection. Bailout turns get an
/// extra `-bailout` suffix so the pruner can partition the two tiers.
fn debug_recording_stem(ts_ms: u64, turn_id: u64, is_bailout: bool) -> String {
  if is_bailout {
    format!("{ts_ms:013}-turn-{turn_id:06}-bailout")
  } else {
    format!("{ts_ms:013}-turn-{turn_id:06}")
  }
}

/// Trim `dir` down to the most recent [`DEBUG_RECORDING_MAX_FILES`] regular `.wav` files
/// and the most recent [`DEBUG_RECORDING_BAILOUT_MAX_FILES`] bailout `.wav` files (and their
/// matching `.json` sidecars). The two tiers are pruned independently so a busy stretch of
/// successful turns can't evict a rare bailout's wav before we have a chance to inspect it.
/// Errors are swallowed intentionally — a failed prune leaves stale files behind but doesn't
/// block the next turn's capture.
fn prune_debug_recordings(dir: &Path) {
  let Ok(entries) = std::fs::read_dir(dir) else {
    return;
  };
  let (mut bailout, mut regular): (Vec<String>, Vec<String>) = entries
    .filter_map(|e| e.ok())
    .filter_map(|e| {
      let path = e.path();
      if path.extension().and_then(|s| s.to_str()) != Some("wav") {
        return None;
      }
      path.file_stem().and_then(|s| s.to_str()).map(ToOwned::to_owned)
    })
    .partition(|stem| stem.ends_with("-bailout"));
  bailout.sort();
  regular.sort();

  let regular_excess = regular.len().saturating_sub(DEBUG_RECORDING_MAX_FILES);
  for stale in &regular[..regular_excess] {
    let _ = std::fs::remove_file(dir.join(format!("{stale}.wav")));
    let _ = std::fs::remove_file(dir.join(format!("{stale}.json")));
  }
  let bailout_excess = bailout.len().saturating_sub(DEBUG_RECORDING_BAILOUT_MAX_FILES);
  for stale in &bailout[..bailout_excess] {
    let _ = std::fs::remove_file(dir.join(format!("{stale}.wav")));
    let _ = std::fs::remove_file(dir.join(format!("{stale}.json")));
  }
}

struct FinalJob {
  turn_id: u64,
  audio: Vec<f32>,
  kind: FinalJobKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AuditEmittedKind {
  /// Legacy-era sidecar kinds. The dual-stream recorder never emits these, but the sidecar
  /// schema (and its analyzers) must keep round-tripping banked legacy recordings, so the
  /// variants and their `pipeline_label`/`audit_kind_label` mappings survive and are exercised
  /// by the schema-compat tests.
  #[allow(dead_code)]
  Assembled,
  #[allow(dead_code)]
  DraftEmit,
  /// Dual-stream finalize: the refined replace that was pasted. Recorded with no model
  /// re-decode — the sidecar's `draft_text` vs `emitted_text` already carry the correction
  /// magnitude, so the audit worker never loads a finalization model for these.
  DualFinal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum FinalJobKind {
  /// Dual-stream: feed `audio` into the worker's persistent refined streaming session.
  RefineChunk,
  /// Dual-stream: cheap streaming flush of the refined session; emits `RefinedFinal`.
  RefineFlush,
  /// Dual-stream: reset the refined session for a new turn.
  RefineReset,
}

struct DebugRecordingJob {
  turn_id: u64,
  audio: Vec<f32>,
  emitted_kind: AuditEmittedKind,
  emitted_text: String,
  /// The live draft shown at finalize, before the refined replace. Paired with `emitted_text` (the
  /// refined final) so the sidecar captures the correction magnitude without a model re-decode.
  draft_text: String,
  /// Wall-time to drain + flush the refined stream at finalize (G2 latency).
  finalize_elapsed_ms: Option<u64>,
  eou_emissions: Vec<EouEmission>,
  live_display_events: Vec<LiveDisplayEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct IncrementalPartialSegment {
  segment_id: u64,
  start_sample: usize,
  end_sample: usize,
  is_tail: bool,
  text: String,
}

/// One non-empty streaming-model emission within the current turn. Stored verbatim
/// so the debug-recording sidecar can record *what* streaming heard for each span,
/// not just *how much* (the count-only form was insufficient when investigating
/// partial-empty bailouts).
#[derive(Debug, Clone)]
struct EouEmission {
  audio_samples: usize,
  delta_chars: usize,
  text: String,
}

#[derive(Debug, Clone)]
struct LiveDisplayEvent {
  audio_samples: usize,
  source: &'static str,
  action: &'static str,
  text: String,
  candidate_text: Option<String>,
}

/// Per-turn live-display + refined-stream state. Dual-stream keeps the live caption a pure
/// streaming hypothesis and folds the continuously-fed refined stream into it via the
/// stabilizer; the recorded `eou_emissions` / `live_display_events` feed the debug sidecar.
struct LiveDisplayState {
  has_refined_text: bool,
  /// Per-emission streaming history within the current turn. The `text` field is written to the
  /// debug-recording sidecar.
  eou_emissions: Vec<EouEmission>,
  last_live_output_audio_samples: Option<usize>,
  live_refined_text: String,
  last_live_display_text: String,
  live_display_events: Vec<LiveDisplayEvent>,
}

impl LiveDisplayState {
  fn new(_now: Instant) -> Self {
    Self {
      has_refined_text: false,
      eou_emissions: Vec::new(),
      last_live_output_audio_samples: None,
      live_refined_text: String::new(),
      last_live_display_text: String::new(),
      live_display_events: Vec::new(),
    }
  }

  fn reset(&mut self, _now: Instant) {
    self.has_refined_text = false;
    self.eou_emissions.clear();
    self.last_live_output_audio_samples = None;
    self.live_refined_text.clear();
    self.last_live_display_text.clear();
    self.live_display_events.clear();
  }
}

enum FinalResult {
  /// Dual-stream: an appended delta from the refined streaming session.
  RefinedDelta { turn_id: u64, delta: String },
  /// Dual-stream: the refined session's flushed final text for the turn.
  RefinedFinal { turn_id: u64, text: String },
}

fn elapsed_ms_since(instant: Instant) -> u64 {
  u64::try_from(instant.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn partials_debug_env_enabled() -> bool {
  static ENABLED: OnceLock<bool> = OnceLock::new();
  *ENABLED.get_or_init(|| {
    std::env::var("TOON_SHOW_PARTIALS")
      .map(|v| v == "1" || v.eq_ignore_ascii_case("true") || v.eq_ignore_ascii_case("yes"))
      .unwrap_or(false)
  })
}

fn audit_kind_label(kind: AuditEmittedKind) -> &'static str {
  match kind {
    AuditEmittedKind::Assembled => "assembled",
    AuditEmittedKind::DraftEmit => "draft_emit",
    AuditEmittedKind::DualFinal => "dual_final",
  }
}

/// Which pipeline produced a debug recording — lets analyzers separate the retiring legacy
/// captures from dual-stream ones. Derived from `emitted_kind`, so no extra job field is needed.
fn pipeline_label(kind: AuditEmittedKind) -> &'static str {
  match kind {
    AuditEmittedKind::DualFinal => "dual_stream",
    AuditEmittedKind::Assembled | AuditEmittedKind::DraftEmit => "legacy_stitch",
  }
}

fn audit_tokens(text: &str) -> Vec<String> {
  text
    .split_whitespace()
    .map(normalize_stitch_token)
    .filter(|t| !t.is_empty())
    .collect()
}

fn token_edit_distance(left: &[String], right: &[String]) -> usize {
  if left.is_empty() {
    return right.len();
  }
  if right.is_empty() {
    return left.len();
  }

  let mut prev = (0..=right.len()).collect::<Vec<_>>();
  let mut curr = vec![0usize; right.len() + 1];

  for (i, ltok) in left.iter().enumerate() {
    curr[0] = i + 1;
    for (j, rtok) in right.iter().enumerate() {
      let cost = if ltok == rtok { 0 } else { 1 };
      let deletion = prev[j + 1] + 1;
      let insertion = curr[j] + 1;
      let substitution = prev[j] + cost;
      curr[j + 1] = deletion.min(insertion).min(substitution);
    }
    std::mem::swap(&mut prev, &mut curr);
  }

  prev[right.len()]
}

fn longest_common_prefix_tokens(left: &[String], right: &[String]) -> usize {
  left.iter().zip(right.iter()).take_while(|(l, r)| l == r).count()
}

fn truncate_for_log(text: &str, max_chars: usize) -> String {
  let mut out = String::new();
  for ch in text.chars().take(max_chars) {
    out.push(ch);
  }
  if text.chars().count() > max_chars {
    out.push_str("...");
  }
  out
}

fn log_partial_audit_texts(
  turn_id: u64,
  emitted_kind: AuditEmittedKind,
  partial_segments: &[IncrementalPartialSegment],
  stitched_text: &str,
  full_text: &str,
) {
  eprintln!(
    "TOON_PARTIAL_AUDIT_PARTIALS turn_id={} emitted_kind={} partial_count={}",
    turn_id,
    audit_kind_label(emitted_kind),
    partial_segments.len()
  );

  for (partial_idx, partial) in partial_segments.iter().enumerate() {
    eprintln!(
      "TOON_PARTIAL_AUDIT_PART turn_id={} emitted_kind={} partial_idx={} segment_id={} is_tail={} range=[{}, {}) text={:?}",
      turn_id,
      audit_kind_label(emitted_kind),
      partial_idx + 1,
      partial.segment_id,
      partial.is_tail,
      partial.start_sample,
      partial.end_sample,
      partial.text.trim()
    );
  }

  eprintln!(
    "TOON_PARTIAL_AUDIT_COMPARE turn_id={} emitted_kind={} stitched_text={:?} actual_text={:?}",
    turn_id,
    audit_kind_label(emitted_kind),
    stitched_text.trim(),
    full_text.trim()
  );
}

fn log_partial_audit_result(
  turn_id: u64,
  emitted_kind: AuditEmittedKind,
  partial_segments: &[IncrementalPartialSegment],
  emitted_text: &str,
  full_text: &str,
  error: Option<&str>,
) -> DebugStatsEvent {
  log_partial_audit_texts(turn_id, emitted_kind, partial_segments, emitted_text, full_text);

  if let Some(message) = error {
    eprintln!(
      "TOON_PARTIAL_AUDIT turn_id={} emitted_kind={} status=error partial_count={} message={:?}",
      turn_id,
      audit_kind_label(emitted_kind),
      partial_segments.len(),
      message
    );
    return DebugStatsEvent::PartialAuditError {
      turn_id,
      emitted_kind: audit_kind_label(emitted_kind).to_string(),
      partial_count: partial_segments.len(),
      message: message.to_string(),
    };
  }

  let emitted = emitted_text.trim();
  let full = full_text.trim();
  let emitted_tokens = audit_tokens(emitted);
  let full_tokens = audit_tokens(full);
  let edit_distance = token_edit_distance(&emitted_tokens, &full_tokens);
  let full_len = full_tokens.len();
  let emitted_len = emitted_tokens.len();
  let denom = full_len.max(1) as f64;
  let wer_like = (edit_distance as f64) / denom;
  let lcp_tokens = longest_common_prefix_tokens(&emitted_tokens, &full_tokens);
  let lcp_pct = (lcp_tokens as f64 * 100.0) / denom;
  let exact = emitted == full;

  eprintln!(
    "TOON_PARTIAL_AUDIT turn_id={} emitted_kind={} status=ok exact={} partial_count={} emitted_tokens={} full_tokens={} edit_distance={} wer_like={:.3} lcp_tokens={} lcp_pct={:.1} emitted_text={:?} full_text={:?}",
    turn_id,
    audit_kind_label(emitted_kind),
    exact,
    partial_segments.len(),
    emitted_len,
    full_len,
    edit_distance,
    wer_like,
    lcp_tokens,
    lcp_pct,
    truncate_for_log(emitted, 220),
    truncate_for_log(full, 220)
  );

  DebugStatsEvent::PartialAuditResult {
    turn_id,
    emitted_kind: audit_kind_label(emitted_kind).to_string(),
    exact,
    partial_count: partial_segments.len(),
    emitted_tokens: emitted_len,
    full_tokens: full_len,
    edit_distance,
    wer_like,
    lcp_tokens,
    lcp_pct,
  }
}

fn samples_to_ms_at_target_sr(samples: usize) -> u32 {
  ((samples as u64) * 1000 / (TARGET_SR as u64)) as u32
}

#[cfg(test)]
mod tests {
  use std::time::Instant;

  use super::stitch::{
    is_one_char_audio_cutoff_truncation, tokenize_for_stitch, tokens_differ_only_in_non_alpha,
    tokens_equivalent, tokens_match_substantive_boundary, try_consume_number_run,
  };
  use super::{
    CaptureEnableTransition, DEBUG_RECORDING_BAILOUT_MAX_FILES, DEBUG_RECORDING_MAX_FILES,
    EouEmission, FinalizingPulsePlan, INCREMENTAL_STITCH_MIN_OVERLAP_TOKENS,
    INCREMENTAL_STITCH_TAIL_WINDOW_TOKENS, LIVE_DISPLAY_MUTABLE_TAIL_TOKENS, LiveDraftRenderPlan,
    PipelineControls, activation_level_blocks_start, capture_enable_transition,
    compose_live_display_text, debug_recording_stem, finalizing_pulse_plan,
    live_display_can_replace, live_stream_output_gap, normalize_chunk_case, plan_live_draft_render,
    plan_live_draft_render_after_previous, prune_debug_recordings, samples_to_ms_at_target_sr,
    stabilize_live_display_replacement, stitch_incremental_text,
  };

  /// The 25 captured incremental partials from turn 41 (debug-recording
  /// 1783487560687-turn-000041-bailout), in received order: (start_sample,
  /// end_sample, is_tail, text). Real 103 s dictation; the speaker repeated
  /// "add another instance to this cluster ... beefier".
  const TURN_41_PARTIALS: &[(usize, usize, bool, &str)] = &[
    (0, 81920, false, "The likeliest case here is that we will launch on a"),
    (0, 102400, false, "The likeliest case here is that we will launch on a"),
    (0, 122880, false, "The likeliest case here is that we will launch on a"),
    (15360, 143360, false, "Case here is that we will launch on a reasonably"),
    (
      112640,
      240640,
      false,
      "Reasonably inexpensive instance that gives us at least enough compute to handle.",
    ),
    (
      133120,
      261120,
      false,
      "Reasonably inexpensive instance that gives us at least enough compute to handle.",
    ),
    (171520, 299520, false, "That gives us at least enough compute to handle the"),
    (
      268800,
      396800,
      false,
      "The initial volume and I really have no idea on what this initial volume is.",
    ),
    (
      366080,
      494080,
      false,
      "On what this initial volume is going to be, it's likely not going to be very much at \
       all, but if it is",
    ),
    (
      386560,
      514560,
      false,
      "Show volume is going to be, it's likely not going to be very much at all, but if it is um.",
    ),
    (483840, 611840, false, "Then we will add another instance to this cluster that is beefier."),
    (
      581120,
      709120,
      false,
      "Is beefier, and if we get more traffic, then we're going to add another instance to \
       this cluster.",
    ),
    (
      678400,
      806400,
      false,
      "Another instance to this cluster that is beefier still and we may shut down the \
       original node so like.",
    ),
    (
      775680,
      903680,
      false,
      "original node so like that's generally how I'm probably going to launch this and so \
       this V one this G.",
    ),
    (844800, 972800, false, "And so this V one, this GA that we must ship must be able to uh."),
    (872960, 1000960, false, "V one, this GA that we must ship must be able to uh standard."),
    (926720, 1054720, false, "Must be able to uh stand itself up, run as a single node it."),
    (
      985600,
      1113600,
      false,
      "Stand itself up, run as a single node it must be able to like uh, you know",
    ),
    (1082880, 1210880, false, "Uh you know see that another node is trying to connect to it."),
    (1103360, 1231360, false, "You know see that another node is trying to connect to it and"),
    (1200640, 1328640, false, "To it and uh it you know like uh will have the Amazon."),
    (1297920, 1425920, false, "Will have the Amazon you know uh NLB do routing and like"),
    (
      1395200,
      1523200,
      false,
      "And like you know they should both sort of get equal traffic uh there's a question \
       about how do.",
    ),
    (
      1487360,
      1615360,
      false,
      "A question about how do we make sure that uh connections stay pinned to the instance \
       they start in, and that's a",
    ),
    (
      1530880,
      1658880,
      true,
      "Sure that connections stay pinned to the instance they start in, and that's a whole \
       thing I don't know.",
    ),
  ];

  /// Converts an audio-sample overlap into the stitcher's `max_right_start` cap: at most a few
  /// tokens past `right[0]`. Kept as a test helper — the dual live-display path always passes
  /// `None` for the cap, but the stitch regression fixtures exercise the capped branch.
  fn stitch_right_start_cap_from_overlap(overlap_samples: usize) -> usize {
    overlap_samples.saturating_div(4_000).saturating_add(2)
  }

  /// Replays the captured partials through the windowed stitch-accumulation loop and returns the
  /// final assembled text — the fixture the repeated-phrase stitcher regression is anchored on.
  fn assemble_turn_41() -> String {
    let mut assembled = String::new();
    let mut prev_refined_end = 0usize;
    for &(start, end, _is_tail, text) in TURN_41_PARTIALS {
      let audio_overlap = prev_refined_end.saturating_sub(start);
      let max_right_start = stitch_right_start_cap_from_overlap(audio_overlap);
      let stitched = stitch_incremental_text(
        &assembled,
        text,
        INCREMENTAL_STITCH_TAIL_WINDOW_TOKENS,
        INCREMENTAL_STITCH_MIN_OVERLAP_TOKENS,
        Some(max_right_start),
        audio_overlap,
      );
      eprintln!("--- after seg @{start}: {stitched}");
      assembled = stitched;
      prev_refined_end = prev_refined_end.max(end);
    }
    assembled
  }

  #[test]
  fn turn_41_repeated_phrase_keeps_the_middle_clause() {
    // The speaker said "...add another instance to this cluster that is beefier, AND IF WE
    // GET MORE TRAFFIC, then we're going to add another instance...". The repeated
    // "add another instance to this cluster" false-anchored the stitcher, dropping the
    // middle clause — which tripped `partial_core_coverage_gap` on seg 12 live and forced a
    // full-turn pass on 103 s of audio.
    let assembled = assemble_turn_41();
    assert!(
      assembled.to_lowercase().contains("if we get more traffic"),
      "stitcher dropped the repeated-phrase middle clause:\n{assembled}"
    );
  }

  #[test]
  fn activation_level_blocks_only_idle_start_below_rms_gate() {
    assert!(activation_level_blocks_start(false, true, -45.0, -40.0));
    assert!(!activation_level_blocks_start(false, true, -35.0, -40.0));
    assert!(!activation_level_blocks_start(false, false, -45.0, -40.0));
    assert!(!activation_level_blocks_start(true, true, -45.0, -40.0));
  }

  #[test]
  fn capture_enable_transition_marks_listen_boundaries() {
    let first = Instant::now();
    let second = first + std::time::Duration::from_millis(1);

    assert_eq!(capture_enable_transition(None, None), CaptureEnableTransition::Unchanged);
    assert_eq!(
      capture_enable_transition(None, Some(first)),
      CaptureEnableTransition::Enabled(first)
    );
    assert_eq!(
      capture_enable_transition(Some(first), Some(first)),
      CaptureEnableTransition::Unchanged
    );
    assert_eq!(
      capture_enable_transition(Some(first), Some(second)),
      CaptureEnableTransition::Enabled(second)
    );
    assert_eq!(capture_enable_transition(Some(first), None), CaptureEnableTransition::Disabled);
  }

  #[test]
  fn samples_to_ms_at_16k_zero_is_zero() {
    assert_eq!(samples_to_ms_at_target_sr(0), 0);
  }

  #[test]
  fn samples_to_ms_at_16k_one_chunk_is_10ms() {
    // 160 samples at 16 kHz = 10 ms (one CHUNK_SAMPLES).
    assert_eq!(samples_to_ms_at_target_sr(160), 10);
  }

  #[test]
  fn samples_to_ms_at_16k_recovery_window_default_500ms() {
    // 8000 samples at 16 kHz = 500 ms = the default `recovery_window_ms`.
    assert_eq!(samples_to_ms_at_target_sr(8_000), 500);
  }

  #[test]
  fn samples_to_ms_at_16k_truncates_sub_millisecond() {
    // 5 samples / 16 = 0.3125 ms → truncates to 0.
    assert_eq!(samples_to_ms_at_target_sr(5), 0);
    // 16 samples = exactly 1 ms.
    assert_eq!(samples_to_ms_at_target_sr(16), 1);
    // 31 samples = 1.9375 ms → truncates to 1.
    assert_eq!(samples_to_ms_at_target_sr(31), 1);
  }

  #[test]
  fn live_stream_output_gap_ignores_short_gaps() {
    assert_eq!(live_stream_output_gap(10_000, 10_000 + 31_999), None);
  }

  #[test]
  fn live_stream_output_gap_reports_two_second_gaps() {
    assert_eq!(live_stream_output_gap(10_000, 42_000), Some((32_000, 2_000)));
  }

  #[test]
  fn normalize_chunk_case_empty_prior_keeps_capital() {
    assert_eq!(normalize_chunk_case("", "That simplifies".to_string()), "That simplifies");
  }

  #[test]
  fn normalize_chunk_case_mid_sentence_lowers_capital() {
    assert_eq!(
      normalize_chunk_case("we schedule the worker itself", " That simplifies".to_string()),
      " that simplifies",
    );
  }

  #[test]
  fn normalize_chunk_case_after_terminal_punct_keeps_capital() {
    assert_eq!(normalize_chunk_case("done.", " That is next".to_string()), " That is next",);
    assert_eq!(normalize_chunk_case("really?", " Yes".to_string()), " Yes");
    assert_eq!(normalize_chunk_case("stop!", " Go".to_string()), " Go");
  }

  #[test]
  fn normalize_chunk_case_after_comma_lowers_capital() {
    assert_eq!(normalize_chunk_case("first clause,", " Then second".to_string()), " then second",);
  }

  #[test]
  fn normalize_chunk_case_lowercase_leading_is_noop() {
    assert_eq!(
      normalize_chunk_case("mid sentence", " and continues".to_string()),
      " and continues",
    );
  }

  #[test]
  fn normalize_chunk_case_non_ascii_upper_untouched() {
    assert_eq!(normalize_chunk_case("hola", " Árbol".to_string()), " Árbol",);
  }

  #[test]
  fn normalize_chunk_case_no_alpha_chunk_unchanged() {
    assert_eq!(normalize_chunk_case("prior", " , ".to_string()), " , ");
  }

  #[test]
  fn normalize_chunk_case_preserves_all_caps_word() {
    assert_eq!(
      normalize_chunk_case("prior text", " SOMETHING loud".to_string()),
      " SOMETHING loud",
    );
  }

  #[test]
  fn normalize_chunk_case_preserves_cpu_acronym_mid_sentence() {
    assert_eq!(normalize_chunk_case("on the", " CPU for a while".to_string()), " CPU for a while",);
  }

  #[test]
  fn normalize_chunk_case_preserves_short_acronyms() {
    assert_eq!(normalize_chunk_case("the", " CFS scheduler".to_string()), " CFS scheduler");
    assert_eq!(normalize_chunk_case("in the", " USA today".to_string()), " USA today");
    assert_eq!(normalize_chunk_case("met", " NASA yesterday".to_string()), " NASA yesterday");
  }

  #[test]
  fn normalize_chunk_case_preserves_single_letter_pronoun_i() {
    assert_eq!(normalize_chunk_case("said", " I think".to_string()), " I think");
    assert_eq!(normalize_chunk_case("then", " I'm done".to_string()), " I'm done");
  }

  #[test]
  fn normalize_chunk_case_lowers_normal_word_after_acronym_test() {
    // Ensure the acronym check doesn't accidentally save `That` when next letter is lowercase.
    assert_eq!(
      normalize_chunk_case("the worker itself", " That simplifies".to_string()),
      " that simplifies",
    );
  }

  #[test]
  fn normalize_chunk_case_lowers_every_mid_chunk_word_start() {
    assert_eq!(
      normalize_chunk_case(
        "engines",
        " Which Kinda means we pre-warm At a Lower priority".to_string(),
      ),
      " which kinda means we pre-warm at a lower priority",
    );
  }

  #[test]
  fn normalize_chunk_case_mixed_period_inside_chunk() {
    assert_eq!(
      normalize_chunk_case("a sentence", " ending here. Next sentence And after".to_string()),
      " ending here. Next sentence and after",
    );
  }

  #[test]
  fn normalize_chunk_case_chunk_starts_with_period_then_capital() {
    assert_eq!(normalize_chunk_case("some text", ". Then more".to_string()), ". Then more",);
  }

  #[test]
  fn normalize_chunk_case_empty_prior_then_mid_chunk_capital() {
    assert_eq!(
      normalize_chunk_case("", "Start of turn And more".to_string()),
      "Start of turn and more",
    );
  }

  #[test]
  fn normalize_chunk_case_final_text_regression() {
    let final_output = "The ideal is that we use excess capacity to use as the s compute for \
      pre-warming engines. Which Kinda means that we pre-warm At a Lower priority than \
      Executors would have Is the right way to do this would be to spin up a separate thread \
      that would listen on a queue for For requests from Groups to pre warm workers and And \
      just reduce the priority given to that thread.";
    let normalized = normalize_chunk_case("", final_output.to_string());
    let expected = "The ideal is that we use excess capacity to use as the s compute for \
      pre-warming engines. Which kinda means that we pre-warm at a lower priority than \
      executors would have is the right way to do this would be to spin up a separate thread \
      that would listen on a queue for for requests from groups to pre warm workers and and \
      just reduce the priority given to that thread.";
    assert_eq!(normalized, expected);
  }

  #[test]
  fn normalize_chunk_case_hyphenated_compound_stays_untouched() {
    // pre-warm has hyphen; alphabetic `w` follows non-alpha `-`, so `w` is a word start.
    // If lowercase already, nothing happens.
    assert_eq!(normalize_chunk_case("we", " pre-warm".to_string()), " pre-warm",);
  }

  #[test]
  fn stitch_exact_overlap() {
    let left = "we are going to test overlap now";
    let right = "overlap now with one more clause";
    let stitched = stitch_incremental_text(left, right, 64, 2, None, 0);
    assert_eq!(stitched, "we are going to test overlap now with one more clause");
  }

  #[test]
  fn stitch_case_and_punctuation_overlap() {
    let left = "This is the boundary. Next segment starts";
    let right = "boundary, next segment starts right here";
    let stitched = stitch_incremental_text(left, right, 64, 2, None, 0);
    assert_eq!(stitched, "This is the boundary, next segment starts right here");
  }

  #[test]
  fn stitch_no_overlap_appends_raw() {
    let left = "first part ends here";
    let right = "completely different opening text";
    let stitched = stitch_incremental_text(left, right, 64, 2, None, 0);
    assert_eq!(stitched, "first part ends here completely different opening text");
  }

  /// Production turn 62 shape (2026-05-08 stderr.log analysis):
  /// partial K ends with `"swoop"`, partial K+1 starts with `"swoop"`.
  /// The multi-token anchor search fails (no audio overlap in this test
  /// shape), so control reaches the no-anchor append path. The new seam-dedup
  /// drops the leading duplicate.
  #[test]
  fn stitch_seam_dedup_drops_repeated_word_when_no_anchor() {
    let left = "for the most part I think we can make all of these changes in one big swoop";
    let right = "swoop and then we can run benchmarks";
    let stitched = stitch_incremental_text(left, right, 64, 2, None, 0);
    assert_eq!(
      stitched,
      "for the most part I think we can make all of these changes in one big swoop and then \
       we can run benchmarks",
    );
  }

  /// Trailing punctuation on the assembled tail is a hard break — sentence
  /// boundaries, parenthetical groups, and explicit comma pauses must not
  /// be collapsed even when the alpha keys match.
  #[test]
  fn stitch_seam_dedup_respects_punctuation_barrier() {
    let left = "the cat sat on the mat.";
    let right = "the dog watched";
    let stitched = stitch_incremental_text(left, right, 64, 2, None, 0);
    assert_eq!(stitched, "the cat sat on the mat. the dog watched");
  }

  /// Single-letter spellings ("S, P, E, N, C, E, R" and the like) must
  /// survive seam dedup even if a letter happens to repeat at the boundary.
  /// The len-≥-2 alpha-key rule catches this case.
  #[test]
  fn stitch_seam_dedup_skips_single_letter_seam() {
    let left = "spelling out my name S P E N C E R";
    let right = "R is the last letter";
    let stitched = stitch_incremental_text(left, right, 64, 2, None, 0);
    assert_eq!(stitched, "spelling out my name S P E N C E R R is the last letter");
  }

  /// Numeric codes ("2288 2288" as a phone number being read aloud) must
  /// survive seam dedup. The `is_alpha_word_seam` predicate rejects
  /// digit-only tokens.
  #[test]
  fn stitch_seam_dedup_skips_digit_seam() {
    let left = "the access code is 2288";
    let right = "2288 again for verification";
    let stitched = stitch_incremental_text(left, right, 64, 2, None, 0);
    assert_eq!(stitched, "the access code is 2288 2288 again for verification");
  }

  /// When the multi-token anchor search succeeds (normal happy path), the
  /// seam-dedup branch is unreachable — control returns from the anchored
  /// merge before reaching the no-anchor append path. This pins that
  /// orthogonality: a normal-overlap shape with a duplicate seam token
  /// must still produce the anchor-driven merge, not the seam-dedup path.
  #[test]
  fn stitch_seam_dedup_does_not_disturb_anchored_merge() {
    let left = "we are going to test overlap now";
    let right = "overlap now with one more clause";
    let stitched = stitch_incremental_text(left, right, 64, 2, None, 0);
    assert_eq!(stitched, "we are going to test overlap now with one more clause");
  }

  #[test]
  fn stitch_drops_repeated_prefix_when_overlap_is_not_at_start() {
    let left = "one two three four five six";
    let right = "zero one two three four five six seven eight";
    let stitched = stitch_incremental_text(left, right, 64, 2, None, 0);
    assert_eq!(stitched, "one two three four five six seven eight");
  }

  #[test]
  fn stitch_tolerates_minor_spelling_drift() {
    let left = "instruction blades were originally caused";
    let right = "blades were originally cause using one on a mind";
    let stitched = stitch_incremental_text(left, right, 64, 2, None, 0);
    assert_eq!(stitched, "instruction blades were originally caused using one on a mind");
  }

  #[test]
  fn stitch_can_replace_unstable_tail_word_without_duplication() {
    let left = "the instruction blades were originally COS";
    let right = "the instruction blades were originally cause using one";
    let stitched = stitch_incremental_text(left, right, 64, 2, None, 0);
    assert_eq!(stitched, "the instruction blades were originally cause using one");
  }

  #[test]
  fn stitch_updates_punctuation_even_without_new_tail_tokens() {
    let left = "we should not stop. here";
    let right = "we should not stop, here";
    let stitched = stitch_incremental_text(left, right, 64, 2, None, 0);
    assert_eq!(stitched, "we should not stop, here");
  }

  /// Production turn 28 (2026-04-28). Partial 1 emitted `"let's"`; partial 2,
  /// covering the same audio span plus more right-context, emitted `"lets"`.
  /// The stitcher's strict-equality merge falls through to "left wins" on
  /// fuzzy diffs — wrong for this case because the only diff is an apostrophe
  /// and right is the higher-context decode. Pin right's `"lets"` so the
  /// punctuation-only fallback keeps firing here.
  #[test]
  fn stitch_regression_turn28_prefers_right_on_apostrophe_diff() {
    let left = "What fundamentally changed that let's that that makes us think that the results";
    let right = "What fundamentally changed that lets that that makes us think that the results \
                 are going to be different?";
    let stitched = stitch_incremental_text(left, right, 64, 2, Some(27), 102_400);
    assert!(!stitched.contains("let's"), "kept the apostrophe form: {stitched}");
    assert!(
      stitched.contains(" lets "),
      "expected punctuation-only merge to pick right's `lets`: {stitched}"
    );
    assert!(stitched.contains("are going to be different"), "tail merge dropped: {stitched}");
  }

  #[test]
  fn live_display_uses_partial_refinement_to_recover_streaming_gap() {
    let streaming = "All right, I think it's time that we create our own connector and by that I \
      mean that we recognize when the user is speaking to us this application. Specific commands";
    let refined = "All right, I think it's time that we create our Own connector and by that I \
      mean that We recognize when the user is speaking to us, This application and be able to \
      respond to specific commands.";

    let display = compose_live_display_text(refined, streaming);

    assert!(
      display.contains("application and be able to respond to specific commands"),
      "partial refinement did not repair the skipped middle phrase: {display}"
    );
    assert!(
      !display.contains("Specific commands All right"),
      "unsafe full-stream duplication leaked into live display: {display}"
    );
  }

  #[test]
  fn live_display_stitches_safe_new_stream_tail_onto_refinement() {
    let refined = "we recognize when the user is speaking to us, this application and be able to \
      respond to specific commands";
    let streaming = "the user is speaking to us this application. specific commands now";

    let display = compose_live_display_text(refined, streaming);

    assert_eq!(
      display,
      "we recognize when the user is speaking to us, this application and be able to respond to \
       specific commands now"
    );
  }

  #[test]
  fn live_display_does_not_concatenate_unrelated_refined_and_streaming_text() {
    let refined = "this is a short refined phrase";
    let streaming = "a completely different streaming hypothesis that has already moved far ahead \
      with enough newer words to own the display";

    let display = compose_live_display_text(refined, streaming);

    assert_eq!(display, streaming);
  }

  #[test]
  fn live_draft_render_streaming_only_uses_stability_hypothesis() {
    let plan = plan_live_draft_render("", "hello from the streaming decoder").unwrap();

    assert_eq!(
      plan,
      LiveDraftRenderPlan::StreamingHypothesis("hello from the streaming decoder".to_string())
    );
  }

  #[test]
  fn live_draft_render_refined_partial_replaces_display() {
    let streaming = "So how well does this work if I uh say something weird like half does it \
      figure out";
    let refined = "So how well does this work if I uh say something weird like half does it";

    let plan = plan_live_draft_render(refined, streaming).unwrap();

    assert_eq!(
      plan,
      LiveDraftRenderPlan::ReplacementDisplay(
        "So how well does this work if I uh say something weird like half does it figure out"
          .to_string()
      )
    );
  }

  #[test]
  fn live_draft_render_refined_with_stream_tail_still_replaces_display() {
    let refined = "we recognize when the user is speaking to us, this application and be able to \
      respond to specific commands";
    let streaming = "the user is speaking to us this application. specific commands now";

    let plan = plan_live_draft_render(refined, streaming).unwrap();

    assert_eq!(
      plan,
      LiveDraftRenderPlan::ReplacementDisplay(
        "we recognize when the user is speaking to us, this application and be able to respond to \
         specific commands now"
          .to_string()
      )
    );
  }

  #[test]
  fn live_draft_render_falls_back_to_streaming_when_refinement_stalls() {
    let previous = "I changed my mind on this these cannot be translated into the numbers because \
      they're just used in common speech so often, and having them";
    let refined = "I changed my mind on this these cannot be translated into the numbers because \
      they're just used in common speech so often and having the";
    let streaming = "I changed my mind on this these cannot be translated into the numbers because \
      they're just used in common speech so often and having them like get converted is just weird";

    let plan = plan_live_draft_render_after_previous(previous, refined, streaming).unwrap();

    assert_eq!(plan, LiveDraftRenderPlan::ReplacementDisplay(streaming.to_string()));
  }

  #[test]
  fn live_draft_render_keeps_refined_candidate_when_streaming_has_not_advanced() {
    let previous = "the text got pasted and then it stated open";
    let refined = "the text got pasted and then it stayed open";
    let streaming = "the text got pasted and then it stayed";

    let plan = plan_live_draft_render_after_previous(previous, refined, streaming).unwrap();

    assert_eq!(
      plan,
      LiveDraftRenderPlan::ReplacementDisplay("the text got pasted and then it stayed open".into())
    );
  }

  #[test]
  fn live_display_rejects_refined_candidate_that_rolls_back_visible_text() {
    let previous = "we should have some way to be able to rerun the recording through our system";
    let candidate = "we should have some way to be able to";

    assert!(!live_display_can_replace(previous, candidate));
  }

  #[test]
  fn live_display_accepts_refined_candidate_that_corrects_without_rolling_back() {
    let previous = "the text got pasted and then it stated open";
    let candidate = "the text got pasted and then it stayed open";

    assert!(live_display_can_replace(previous, candidate));
  }

  #[test]
  fn live_display_allows_small_token_shape_corrections() {
    let previous = "we are removing text and then it gets added back";
    let candidate = "we're removing text and then it gets added back";

    assert!(live_display_can_replace(previous, candidate));
  }

  #[test]
  fn dual_stream_tight_tail_freezes_settled_prefix_words() {
    // A lagging refined stream tries to flip a filler word ("uh") deep in a long, settled
    // caption. With dual-stream's tight mutable tail that word is frozen, so the swap is
    // rejected — no flip-flop of already-read text (goal #1).
    let previous = "so we are routing uh audio through it and it is our job to ensure that we \
      correctly map the audio volume that the device set to the system audio level right now";
    let candidate = "so we are routing audio through it and it is our job to ensure that we \
      correctly map the audio volume that the device set to the system audio level right now";

    let display =
      stabilize_live_display_replacement(previous, candidate, LIVE_DISPLAY_MUTABLE_TAIL_TOKENS);

    assert!(
      display.contains("routing uh audio"),
      "tight tail should freeze the settled filler decision: {display}"
    );
  }

  #[test]
  fn dual_stream_tight_tail_still_applies_a_recent_word_completion() {
    // The valuable refined correction: completing a truncated word in the live tail region.
    let previous = "the headset volume is not being correctly synchronized with the computer volume so we \
       need to make sure that we correctly map the audio vol";
    let candidate = "the headset volume is not being correctly synchronized with the computer volume so we \
       need to make sure that we correctly map the audio volume";

    let display =
      stabilize_live_display_replacement(previous, candidate, LIVE_DISPLAY_MUTABLE_TAIL_TOKENS);

    assert!(
      display.ends_with("map the audio volume"),
      "recent tail correction should apply: {display}"
    );
  }

  #[test]
  fn tight_tail_freezes_a_word_behind_the_mutable_window() {
    // A word ~30 tokens back sits behind the tight 12-token mutable tail, so the lagging 560ms
    // stream cannot churn it — the knob that keeps settled text from flip-flopping.
    let words: Vec<String> = (0..50).map(|i| format!("w{i}")).collect();
    let previous = words.join(" ");
    let mut edited = words.clone();
    edited[20] = "CHANGED".to_string();
    let candidate = edited.join(" ");

    let display =
      stabilize_live_display_replacement(&previous, &candidate, LIVE_DISPLAY_MUTABLE_TAIL_TOKENS);

    assert!(!display.contains("CHANGED"), "tight tail should freeze token 20: {display}");
  }

  #[test]
  fn live_display_preserves_safe_prefix_when_late_partial_rewrites_opening_text() {
    let previous = "I'm still getting some thrashing from the partials updating this streaming \
      text that I'm being shown, and for the most part it's good. Like, I can see that like it's \
      you know consolidating things that didn't think it was a sentence originally and then it's \
      it's doing the right thing but like I feel like I get this like";
    let candidate = "I'm still getting some thrashing from the partials updating the streaming \
      text that I'm being shown and for the most part it's good like I can see that like it's you \
      know consolidating things that didn't think it was A sentence originally, and then um it's \
      it's doing the right thing but like I feel like I get this like thrashing where previous \
      parts are being updated when they should have already resolved";

    let display =
      stabilize_live_display_replacement(previous, candidate, LIVE_DISPLAY_MUTABLE_TAIL_TOKENS);

    assert!(
      display.contains("updating this streaming text"),
      "safe prefix should keep prior wording: {display}"
    );
    assert!(
      display.contains("it's good. Like") && !display.contains("it's good like"),
      "safe prefix should keep prior sentence boundary: {display}"
    );
    assert!(
      !display.contains("updating the streaming text"),
      "late partial rewrote stable prefix: {display}"
    );
    assert!(
      display.contains("thrashing where previous parts are being updated"),
      "candidate tail should still advance: {display}"
    );
  }

  #[test]
  fn live_display_allows_short_turns_to_keep_refining_before_safe_prefix() {
    let previous = "the text got pasted and then it stated open";
    let candidate = "the text got pasted and then it stayed open";

    let display =
      stabilize_live_display_replacement(previous, candidate, LIVE_DISPLAY_MUTABLE_TAIL_TOKENS);

    assert_eq!(display, candidate);
  }

  #[test]
  fn live_display_holds_previous_text_when_stable_boundary_cannot_be_matched() {
    let previous = "one two three four five six seven eight nine ten eleven twelve thirteen \
      fourteen fifteen sixteen seventeen eighteen nineteen twenty twenty one twenty two twenty \
      three twenty four twenty five twenty six twenty seven twenty eight twenty nine thirty thirty \
      one thirty two thirty three thirty four thirty five thirty six thirty seven thirty eight";
    let candidate = "a completely different candidate without the stable boundary but with enough \
      extra words to otherwise look like forward progress for the overlay display";

    let display =
      stabilize_live_display_replacement(previous, candidate, LIVE_DISPLAY_MUTABLE_TAIL_TOKENS);

    assert_eq!(display, previous);
  }

  #[test]
  fn tokens_differ_only_in_non_alpha_handles_each_shape() {
    assert!(tokens_differ_only_in_non_alpha("let's", "lets"));
    assert!(tokens_differ_only_in_non_alpha("don't", "dont"));
    assert!(tokens_differ_only_in_non_alpha("well-defined", "welldefined"));
    assert!(tokens_differ_only_in_non_alpha("mr.", "mr"));
    assert!(tokens_differ_only_in_non_alpha("co-op", "coop"));

    // Alphabetic-letter diffs reject — alphabetic-edit-distance branch handles them.
    assert!(!tokens_differ_only_in_non_alpha("caused", "cause"));
    assert!(!tokens_differ_only_in_non_alpha("ur", "url"));
    assert!(!tokens_differ_only_in_non_alpha("the", "their"));

    // Strict equality is the keys-equal upstream branch's job, not this one.
    assert!(!tokens_differ_only_in_non_alpha("lets", "lets"));

    // No-alphabetic-content tokens reject (would otherwise spuriously merge punctuation runs).
    assert!(!tokens_differ_only_in_non_alpha("''", "'"));
    assert!(!tokens_differ_only_in_non_alpha("--", "-"));
  }

  /// The new punctuation-only branch must not steal slots from the alphabetic-
  /// drift default. Construct an overlap with one apostrophe-only diff (`let's`
  /// vs `lets`) AND one alphabetic-letter diff (`caused` vs `cause`) in the
  /// same merge: the punctuation slot picks right, the alphabetic slot picks
  /// left, so the merged output has `lets` (right) AND `caused` (left).
  #[test]
  fn stitch_punctuation_only_merge_does_not_disturb_alphabetic_drift() {
    let left = "we said let's go and that caused";
    let right = "let's go and that cause some stir";
    // No audio-overlap context: this is the same shape as
    // `stitch_tolerates_minor_spelling_drift` — pure assembled-vs-segment.
    let stitched = stitch_incremental_text(left, right, 64, 2, None, 0);
    assert!(stitched.contains("caused"), "alphabetic drift stole left's `caused`: {stitched}");
    assert!(!stitched.contains(" cause "), "right's shorter `cause` leaked through: {stitched}");
    assert!(stitched.contains("some stir"), "tail merge dropped: {stitched}");
  }

  #[test]
  fn finalizing_pulse_emits_for_backend_finalization() {
    let plan = finalizing_pulse_plan(true, true, true);
    assert_eq!(plan, FinalizingPulsePlan::Emit);
  }

  #[test]
  fn finalizing_pulse_can_be_disabled_independently() {
    let plan = finalizing_pulse_plan(false, true, true);
    assert_eq!(plan, FinalizingPulsePlan::SkipDisabled);
  }

  #[test]
  fn finalizing_pulse_skips_empty_audio() {
    let plan = finalizing_pulse_plan(true, false, true);
    assert_eq!(plan, FinalizingPulsePlan::SkipAudioEmpty);
  }

  #[test]
  fn finalizing_pulse_skips_empty_draft() {
    let plan = finalizing_pulse_plan(true, true, false);
    assert_eq!(plan, FinalizingPulsePlan::SkipDraftEmpty);
  }

  #[test]
  fn pipeline_controls_notify_audio_wakes_waiter_before_backstop() {
    let controls = std::sync::Arc::new(PipelineControls::default());
    let waiter_controls = controls.clone();
    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let waiter_done = done.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    let handle = std::thread::spawn(move || {
      let start = std::time::Instant::now();
      waiter_controls.wait_for_wake(std::time::Duration::from_secs(5));
      tx.send(start.elapsed()).unwrap();
      waiter_done.store(true, std::sync::atomic::Ordering::Release);
    });

    wake_until_waiter_returns(&controls, &done, |controls| controls.notify_audio());
    let elapsed = rx.recv_timeout(std::time::Duration::from_secs(6)).unwrap();
    handle.join().unwrap();

    assert!(elapsed < std::time::Duration::from_secs(3));
  }

  #[test]
  fn pipeline_controls_capture_flip_wakes_waiter_before_backstop() {
    let controls = std::sync::Arc::new(PipelineControls::default());
    let waiter_controls = controls.clone();
    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let waiter_done = done.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    let handle = std::thread::spawn(move || {
      let start = std::time::Instant::now();
      waiter_controls.wait_for_wake(std::time::Duration::from_secs(5));
      tx.send(start.elapsed()).unwrap();
      waiter_done.store(true, std::sync::atomic::Ordering::Release);
    });

    let next_capture_enabled = std::sync::atomic::AtomicBool::new(false);
    wake_until_waiter_returns(&controls, &done, |controls| {
      let next = next_capture_enabled.fetch_xor(true, std::sync::atomic::Ordering::Relaxed);
      controls.set_capture_enabled(next);
    });
    let elapsed = rx.recv_timeout(std::time::Duration::from_secs(6)).unwrap();
    handle.join().unwrap();

    assert!(elapsed < std::time::Duration::from_secs(3));
  }

  #[test]
  fn pipeline_controls_capture_enable_wakes_paused_waiter_before_backstop() {
    let controls = std::sync::Arc::new(PipelineControls::default());
    controls.set_capture_enabled(false);
    let waiter_controls = controls.clone();
    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let waiter_done = done.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    let handle = std::thread::spawn(move || {
      let start = std::time::Instant::now();
      waiter_controls.wait_for_capture_enable_or_wake(std::time::Duration::from_secs(5));
      tx.send(start.elapsed()).unwrap();
      waiter_done.store(true, std::sync::atomic::Ordering::Release);
    });

    wake_until_waiter_returns(&controls, &done, |controls| controls.set_capture_enabled(true));
    let elapsed = rx.recv_timeout(std::time::Duration::from_secs(6)).unwrap();
    handle.join().unwrap();

    assert!(elapsed < std::time::Duration::from_secs(3));
  }

  #[test]
  fn pipeline_controls_control_wake_wakes_paused_waiter_before_backstop() {
    let controls = std::sync::Arc::new(PipelineControls::default());
    controls.set_capture_enabled(false);
    let waiter_controls = controls.clone();
    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let waiter_done = done.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    let handle = std::thread::spawn(move || {
      let start = std::time::Instant::now();
      waiter_controls.wait_for_capture_enable_or_wake(std::time::Duration::from_secs(5));
      tx.send(start.elapsed()).unwrap();
      waiter_done.store(true, std::sync::atomic::Ordering::Release);
    });

    wake_until_waiter_returns(&controls, &done, |controls| controls.notify_control_wake());
    let elapsed = rx.recv_timeout(std::time::Duration::from_secs(6)).unwrap();
    handle.join().unwrap();

    assert!(elapsed < std::time::Duration::from_secs(3));
  }

  fn wake_until_waiter_returns<F>(
    controls: &PipelineControls,
    done: &std::sync::atomic::AtomicBool,
    wake: F,
  ) where
    F: Fn(&PipelineControls),
  {
    for _ in 0..200 {
      if done.load(std::sync::atomic::Ordering::Acquire) {
        return;
      }
      wake(controls);
      std::thread::sleep(std::time::Duration::from_millis(10));
    }
  }

  #[test]
  fn stitch_regression_turn62_sequence_preserves_early_context() {
    let segments = [
      "Instead of the existing work view when creating",
      "when creating a workblock. I'd like to remove the",
      "Move the hour minute display and add to the right of the",
      "add to the right of the time the few different bases",
      "the few different b presets for a countdown.",
    ];

    let mut stitched = String::new();
    for segment in segments {
      stitched = stitch_incremental_text(&stitched, segment, 64, 2, None, 0);
    }

    assert!(stitched.contains("workblock"));
    assert!(stitched.contains("countdown"));
  }

  #[test]
  fn stitch_rejects_destructive_shrink_on_weak_overlap() {
    let left = "Instead of the existing work view when creating a workblock I'd like to remove the";
    let right = "Move the hour minute display and add to the right of the";
    let stitched = stitch_incremental_text(left, right, 64, 2, None, 0);

    assert!(stitched.split_whitespace().count() >= 10);
    assert!(stitched.contains("workblock"));
    assert!(stitched.contains("display"));
  }

  #[test]
  fn stitch_regression_turn16_preserves_middle_via_audio_range_cap() {
    // Real data from a truncation incident: `[if, we, have]` recurs at right[10..13] and
    // previously outscored the true overlap at right[0..2] (`[fast, work]`), causing 17 tokens
    // of speech to vanish between "executors that handle" and "fast work, then we can all put
    // them together. And if we have".
    //
    // (As of the turn 667 fix, the uncapped path also rejects this via the pseudo-suffix-
    // stretched check — `tail_drop=7 + match_start=10 = 17 > overlap=3` — so the cap is no
    // longer the only line of defense. Test still verifies the with-cap path produces the
    // correct output for the real incident inputs.)
    let left = "My hunch is that over time we will want to distribute work uh into schedulers \
      that have similarly shaped work so that if we have you know executors that handle fast \
      work";
    let right = "fast work, then we can all put them together. And if we have executors that \
      are all slow";

    // Partial 3 ended at sample 302080; partial 4 started at 271360.
    // overlap = 30720 samples → cap = 30720/4000 + 2 = 9 tokens. match_start=10 gets rejected,
    // forcing the stitcher to find the correct 2-token overlap at match_start=0.
    let overlap_samples = 302_080usize - 271_360usize;
    let cap = stitch_right_start_cap_from_overlap(overlap_samples);
    let stitched = stitch_incremental_text(left, right, 64, 2, Some(cap), overlap_samples);

    assert!(
      stitched.contains("then we can all put them together"),
      "middle content must survive the stitch, got: {stitched:?}",
    );
    assert!(stitched.contains("executors that handle fast work"));
    assert!(stitched.ends_with("executors that are all slow"));
  }

  #[test]
  fn tokens_equivalent_rejects_fuzzy_match_on_short_tokens() {
    // Exact equality always wins, regardless of length.
    assert!(tokens_equivalent("i", "i"));
    assert!(tokens_equivalent("a", "a"));

    // Single-char pairs that differ are NOT equivalent — they're distinct words, not typos.
    assert!(!tokens_equivalent("i", "s"));
    assert!(!tokens_equivalent("a", "i"));
    assert!(!tokens_equivalent("s", "a"));

    // Two-char pairs that differ are not equivalent either.
    assert!(!tokens_equivalent("at", "it"));
    assert!(!tokens_equivalent("of", "if"));
    assert!(!tokens_equivalent("is", "it"));

    // 3+ char pairs with one edit distance ARE equivalent — these are plausible typos.
    assert!(tokens_equivalent("cause", "caused"));
    assert!(tokens_equivalent("what", "that"));
    assert!(tokens_equivalent("worker", "workers"));

    // 3+ char pairs with >1 edit distance are still rejected.
    assert!(!tokens_equivalent("cos", "cause"));
    assert!(!tokens_equivalent("foo", "bar"));
  }

  fn unique_tmp_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
      "azad-{tag}-{}",
      std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
  }

  fn write_pair(dir: &std::path::Path, stem: &str) {
    std::fs::write(dir.join(format!("{stem}.wav")), b"RIFF").unwrap();
    std::fs::write(dir.join(format!("{stem}.json")), b"{}").unwrap();
  }

  #[test]
  fn prune_debug_recordings_keeps_newest_regular_pairs_and_deletes_older() {
    let tmp = unique_tmp_dir("prune-test");

    let extras = 3;
    let total = DEBUG_RECORDING_MAX_FILES + extras;
    let stems: Vec<String> = (0..total)
      .map(|i| format!("{:013}-turn-{:06}", 1_700_000_000_000u64 + i as u64, i))
      .collect();
    for stem in &stems {
      write_pair(&tmp, stem);
    }

    prune_debug_recordings(&tmp);

    for stem in &stems[..extras] {
      assert!(!tmp.join(format!("{stem}.wav")).exists(), "stale wav was not pruned: {stem}");
      assert!(!tmp.join(format!("{stem}.json")).exists(), "stale json was not pruned: {stem}");
    }
    for stem in &stems[extras..] {
      assert!(tmp.join(format!("{stem}.wav")).exists(), "newer wav was incorrectly pruned: {stem}");
      assert!(
        tmp.join(format!("{stem}.json")).exists(),
        "newer json was incorrectly pruned: {stem}"
      );
    }

    prune_debug_recordings(&tmp);
    let remaining = std::fs::read_dir(&tmp).unwrap().count();
    assert_eq!(remaining, DEBUG_RECORDING_MAX_FILES * 2);

    let _ = std::fs::remove_dir_all(&tmp);
  }

  #[test]
  fn prune_debug_recordings_partitions_bailout_and_regular_tiers() {
    let tmp = unique_tmp_dir("prune-tiers");

    let regular_extras = 4;
    let bailout_extras = 5;
    let regular_total = DEBUG_RECORDING_MAX_FILES + regular_extras;
    let bailout_total = DEBUG_RECORDING_BAILOUT_MAX_FILES + bailout_extras;

    let regular_stems: Vec<String> = (0..regular_total)
      .map(|i| format!("{:013}-turn-{:06}", 1_700_000_000_000u64 + i as u64, i))
      .collect();
    let bailout_stems: Vec<String> = (0..bailout_total)
      .map(|i| format!("{:013}-turn-{:06}-bailout", 1_700_000_500_000u64 + i as u64, 1000 + i))
      .collect();
    for stem in regular_stems.iter().chain(bailout_stems.iter()) {
      write_pair(&tmp, stem);
    }

    prune_debug_recordings(&tmp);

    for stem in &regular_stems[..regular_extras] {
      assert!(!tmp.join(format!("{stem}.wav")).exists(), "regular wav not pruned: {stem}");
      assert!(!tmp.join(format!("{stem}.json")).exists(), "regular json not pruned: {stem}");
    }
    for stem in &regular_stems[regular_extras..] {
      assert!(tmp.join(format!("{stem}.wav")).exists(), "regular wav over-pruned: {stem}");
      assert!(tmp.join(format!("{stem}.json")).exists(), "regular json over-pruned: {stem}");
    }
    for stem in &bailout_stems[..bailout_extras] {
      assert!(!tmp.join(format!("{stem}.wav")).exists(), "bailout wav not pruned: {stem}");
      assert!(!tmp.join(format!("{stem}.json")).exists(), "bailout json not pruned: {stem}");
    }
    for stem in &bailout_stems[bailout_extras..] {
      assert!(tmp.join(format!("{stem}.wav")).exists(), "bailout wav over-pruned: {stem}");
      assert!(tmp.join(format!("{stem}.json")).exists(), "bailout json over-pruned: {stem}");
    }

    let _ = std::fs::remove_dir_all(&tmp);
  }

  #[test]
  fn prune_debug_recordings_keeps_all_when_under_caps() {
    let tmp = unique_tmp_dir("prune-under-cap");

    for i in 0..3 {
      write_pair(&tmp, &format!("{:013}-turn-{:06}", 1_700_000_000_000u64 + i as u64, i));
    }
    for i in 0..5 {
      write_pair(&tmp, &format!("{:013}-turn-{:06}-bailout", 1_700_000_500_000u64 + i as u64, i));
    }

    prune_debug_recordings(&tmp);

    let remaining = std::fs::read_dir(&tmp).unwrap().count();
    // (3 regular + 5 bailout) pairs × 2 files each.
    assert_eq!(remaining, (3 + 5) * 2);

    let _ = std::fs::remove_dir_all(&tmp);
  }

  #[test]
  fn debug_recording_stem_handles_bailout_suffix() {
    assert_eq!(debug_recording_stem(1_700_000_000_000, 42, false), "1700000000000-turn-000042");
    assert_eq!(
      debug_recording_stem(1_700_000_000_000, 42, true),
      "1700000000000-turn-000042-bailout"
    );
  }

  #[test]
  fn prune_debug_recordings_ignores_non_wav_and_orphan_json() {
    let tmp = std::env::temp_dir().join(format!(
      "azad-prune-orphans-{}",
      std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
    ));
    std::fs::create_dir_all(&tmp).unwrap();

    // Populate one full pair plus some noise the pruner must not touch.
    std::fs::write(tmp.join("1700000000000-turn-000001.wav"), b"RIFF").unwrap();
    std::fs::write(tmp.join("1700000000000-turn-000001.json"), b"{}").unwrap();
    std::fs::write(tmp.join("README.md"), b"notes").unwrap();
    // Orphan json with no matching wav — pruner only acts off .wav listing, so this stays.
    std::fs::write(tmp.join("orphan.json"), b"{}").unwrap();

    prune_debug_recordings(&tmp);

    assert!(tmp.join("1700000000000-turn-000001.wav").exists());
    assert!(tmp.join("1700000000000-turn-000001.json").exists());
    assert!(tmp.join("README.md").exists());
    assert!(tmp.join("orphan.json").exists());

    let _ = std::fs::remove_dir_all(&tmp);
  }

  #[test]
  fn eou_emission_text_is_preserved() {
    let emissions = vec![
      EouEmission { audio_samples: 1_000, delta_chars: 3, text: " on".into() },
      EouEmission { audio_samples: 2_000, delta_chars: 9, text: " AWS Nitro".into() },
      EouEmission { audio_samples: 3_000, delta_chars: 5, text: ", uh".into() },
    ];
    let in_range: Vec<&str> = emissions
      .iter()
      .filter(|e| e.audio_samples >= 1_000 && e.audio_samples < 3_000)
      .map(|e| e.text.as_str())
      .collect();
    assert_eq!(in_range, vec![" on", " AWS Nitro"]);
  }

  #[test]
  fn stitch_regression_turn667_rejects_anchored_pseudo_suffix() {
    // Real data from turn 667 (66.7% token-count accuracy, 10 tokens dropped from middle).
    // The user said "So it seems like…" twice — first "for the stress micro-tasks scenario
    // that", then restarted "it's running fine and that we don't need to address it…".
    //
    // partial 1 audio [0, 110080):  "So it seems like for the stress micro-tasks scenario that"
    // partial 2 audio [79360, 207360): "scenario that uh it seems like it's running fine and
    //                                   that we don't need to address it."
    //
    // Genuine overlap: [scenario, that] at left[8..10] vs right[0..2] (overlap=2,
    // tail_drop=0, match_start=0). Total slack=0.
    //
    // False overlap: [it, seems, like] at left[1..4] vs right[3..6] (overlap=3,
    // tail_drop=6, match_start=3). Total slack=9 — exactly hits the audio-overlap cap of
    // 9 derived from the 30720-sample range overlap, so the prior `tail_drop + match_start
    // <= cap` check let it through. But `slack=9 > overlap=3` means the anchor is being
    // stretched ~3× wider than its own matched length, and the "suffix" it's matching
    // against is actually left's *prefix* — pseudo-suffix territory even though
    // `overlap_start=1` (so the existing overlap_start==0 guard doesn't catch it).
    //
    // overlap=3 outranked the genuine overlap=2 at scoring time → wrong anchor wins →
    // ~10 tokens of real speech ("for the stress micro-tasks scenario that uh") dropped
    // from the middle.
    let left = "So it seems like for the stress micro-tasks scenario that";
    let right = "scenario that uh it seems like it's running fine and that we don't need to \
                 address it.";
    // Audio overlap: prev_end=110080, partial_2_start=79360 → 30720 samples → cap=9.
    let cap = stitch_right_start_cap_from_overlap(30_720);
    let stitched = stitch_incremental_text(left, right, 256, 2, Some(cap), 30_720);

    assert!(
      stitched.contains("for the stress micro-tasks scenario"),
      "lost middle content: {stitched:?}",
    );
    assert!(
      stitched.contains("running fine and that we don't need to address"),
      "lost right content: {stitched:?}",
    );
  }

  #[test]
  fn stitch_regression_turn8_rejects_combined_slack_false_overlap() {
    // Real data from turn 8 (48.1% token-count accuracy). The user said "It's not clear to me
    // how interwoven the…" *twice* in the same sentence ("…the scheduler is with the harness,
    // and it's not clear to me how interwoven the control…"). Stitching partial 2 into the
    // assembled partial-1 text, the stitcher found a 4-token *exact* match `[it's, not, clear,
    // to]` at (tail_drop=6, match_start=7) which outranks the correct 3-token
    // `[the, scheduler, is]` match at (tail_drop=0, match_start=0). High tail_drop pushed the
    // "suffix" all the way back to left's *beginning*, using left's prefix as a pseudo-suffix.
    //
    // The earlier `max_right_start` cap bounds match_start alone (9 for this 30720-sample
    // audio overlap); match_start=7 passes that. The fix: cap `tail_drop + match_start <=
    // max_right_start` — genuine overlaps can't have both slack dimensions non-trivial at
    // once.
    let left = "It's not clear to me how interwoven the scheduler is.";
    let right =
      "the scheduler is with the harness, and it's not clear to me how interwoven the control.";
    let cap = stitch_right_start_cap_from_overlap(30_720); // partial 1 end 110080, partial 2 start 79360
    let stitched = stitch_incremental_text(left, right, 256, 2, Some(cap), 30_720);

    assert!(
      stitched.contains("the scheduler is with the harness"),
      "lost middle content: {stitched:?}",
    );
    assert!(
      stitched.contains("and it's not clear to me how interwoven the control"),
      "lost right content: {stitched:?}",
    );
  }

  #[test]
  fn stitch_regression_turn237_rejects_short_token_fuzzy_false_overlap() {
    // Real data from a 63.9%-accurate transcription. Without the short-token guard in
    // `tokens_equivalent`, the stitcher's fuzzy-match path lets `[what, s, should]` from
    // left's tail look "equivalent" to `[that, i, should]` at right[3..6] — `what`≈`that`
    // and `s`≈`i` both pass the 1-edit test. That 3-token false overlap outscores the real
    // 2-token `[is, the]` overlap at right[0..2] and drops `"I tell the other agents? What
    // is the text that"` from the middle of the turn.
    let left = "What s should I tell the other agents? What is the";
    let right = "is the text that I should include along with this uh zip file and";

    // Audio-overlap cap for this partial pair: prev_end=110080, cur_start=79360 → 30720
    // samples → cap=9 tokens. Far more permissive than needed to let the real start=0 match
    // through, so the fix has to live at the token-equivalence layer, not the cap layer.
    let overlap_samples = 110_080usize - 79_360usize;
    let cap = stitch_right_start_cap_from_overlap(overlap_samples);

    let stitched = stitch_incremental_text(left, right, 256, 2, Some(cap), overlap_samples);

    // Middle content must survive the stitch.
    assert!(stitched.contains("I tell the other agents"), "lost middle content: {stitched:?}",);
    assert!(stitched.contains("the text that"), "lost right-prefix content: {stitched:?}",);
    assert!(stitched.contains("zip file and"), "lost right-suffix content: {stitched:?}");

    // Full expected shape: true 2-token overlap `[is, the]` joins left and right cleanly.
    assert_eq!(
      stitched,
      "What s should I tell the other agents? What is the text that I should include along \
       with this uh zip file and",
    );
  }

  #[test]
  fn stitch_regression_turn11_recovers_truncated_tail_at_partial_boundary() {
    // Turn 11 (2026-04-28). Partial 1's audio chunk ended at sample 110_080 mid-utterance,
    // so the model emitted `"...the UR"` instead of `"...the URL"`. Partial 2 covered samples
    // 79_360..207_360 — its audio extends past the cutoff, so it emitted `"the URL ..."`
    // cleanly. Without the boundary-recovery branch the stitcher couldn't anchor (the k=2
    // slice `["the","ur"]` vs `["the","url"]` fails because `tokens_equivalent` rejects
    // edit-distance-1 fuzzy on tokens <3 chars) and the result was the buggy
    // `"...the UR the URL..."`. With the fix, anchored at (tail_drop=0, match_start=0,
    // overlap=2) and the merge picks right's `"URL"`.
    let left = "Isn't the normal way that this works is that the UR";
    let right = "the URL that we would provide to share get redirected to a";
    // Live call passed audio_overlap_samples=30_720 → max_right_start=9.
    let stitched = stitch_incremental_text(left, right, 64, 2, Some(9), 30_720);
    assert!(!stitched.contains("UR the URL"), "stitcher kept the truncated tail: {stitched}");
    assert!(stitched.contains("the URL"), "lost the URL after merge: {stitched}");
    assert!(stitched.starts_with("Isn't the normal way"), "dropped prefix: {stitched}");
    // Stitch partial 3 onto the result. Should remain clean.
    let final_text = stitch_incremental_text(
      &stitched,
      "get redirected to a registered iOS redirect path.",
      64,
      2,
      Some(10),
      33_280,
    );
    assert!(
      final_text.contains("registered iOS redirect path"),
      "lost the tail after second stitch: {final_text}",
    );
    assert!(!final_text.contains("UR the URL"), "regressed: {final_text}");
  }

  #[test]
  fn boundary_recovery_does_not_fire_without_audio_overlap() {
    // Same shape as the turn-11 case, but with audio_overlap_samples=0 — i.e. no
    // structural evidence that left's tail was clipped mid-word. The recovery branch
    // must NOT fire; the stitcher falls back to its existing "no anchor, append right"
    // behaviour. Pins the gate so a future refactor doesn't widen it accidentally.
    let left = "Isn't the normal way that this works is that the UR";
    let right = "the URL that we would provide";
    let stitched = stitch_incremental_text(left, right, 64, 2, None, 0);
    // Without recovery the stitcher appends right whole.
    assert!(stitched.contains("UR the URL"), "recovery widened past gate: {stitched}");
  }

  #[test]
  fn boundary_recovery_does_not_fire_at_non_terminal_position() {
    // Recovery is allowed only at the LAST position of the overlap slice (i.e. the actual
    // end of the prior partial). A 1-char-shorter token earlier in the overlap window
    // should NOT match — that's a typo or a different word, not an audio-chunk-boundary
    // truncation. left ends with "the foo", right starts with "fool the foo bar baz" so
    // the genuine 3-token overlap "the foo bar" is found at right[1..4] by the regular
    // path; the leading-position prefix-extension "foo" → "fool" never kicks in.
    let left = "alpha beta gamma the foo";
    let right = "fool the foo bar baz";
    let stitched = stitch_incremental_text(left, right, 64, 2, Some(8), 16_000);
    assert!(!stitched.contains("fool the foo"), "recovery fired mid-slice: {stitched}");
    assert!(stitched.contains("the foo bar baz"), "lost legitimate overlap: {stitched}");
  }

  #[test]
  fn one_char_audio_cutoff_truncation_helper_is_strict() {
    assert!(is_one_char_audio_cutoff_truncation("ur", "url"));
    assert!(is_one_char_audio_cutoff_truncation("thi", "this"));
    assert!(is_one_char_audio_cutoff_truncation("ban", "band"));
    // Single-char left rejected (avoids `"i"` → `"in"`).
    assert!(!is_one_char_audio_cutoff_truncation("i", "in"));
    // Multi-char extension rejected (avoids `"to"` → `"tomato"`).
    assert!(!is_one_char_audio_cutoff_truncation("to", "tomato"));
    assert!(!is_one_char_audio_cutoff_truncation("th", "this"));
    // Not a strict prefix.
    assert!(!is_one_char_audio_cutoff_truncation("top", "stop"));
    // Right shorter or equal — directional only.
    assert!(!is_one_char_audio_cutoff_truncation("url", "ur"));
    assert!(!is_one_char_audio_cutoff_truncation("the", "the"));
  }

  #[test]
  fn stitch_regression_turn41_anchors_across_spelled_vs_digit_number_form() {
    // Turn 41 (2026-04-28). Partial 1 ended `"...the three eighteen from"` (3 tokens for
    // the number span); partial 2 covered the same audio and emitted `"318 from..."` (1
    // digit token). Without span-grouping the stitcher couldn't anchor (cardinality
    // mismatch in slice_tokens_match) and emitted
    // `"...three eighteen from 318 from Air Canada..."`. With grouping, both sides
    // produce a single `match_key="318"` token and anchor cleanly.
    let left = "He said that he was going to be here at three. Is the three eighteen from";
    let right = "318 from Air Canada the only flight that he could have taken if that were true?";
    // Audio overlap: 107_520 - 69_120 = 38_400 samples → cap = 11 tokens.
    let stitched = stitch_incremental_text(left, right, 64, 2, Some(11), 38_400);
    assert!(!stitched.contains("from 318 from"), "duplicated number form: {stitched}");
    assert!(!stitched.contains("eighteen from 318"), "duplicated number form: {stitched}",);
    assert!(stitched.contains("Air Canada"), "lost right content: {stitched}");
    assert!(stitched.contains("if that were true"), "lost right tail: {stitched}");
    assert!(stitched.starts_with("He said that he was going to be here at three."));
  }

  #[test]
  fn stitch_regression_turn23_anchors_on_single_word_seam() {
    // Production turn 23 (2026-04-30, 91.7% accuracy). Partial 1 ended at sample
    // 107_520 with `"...outcome."`; partial 2 started at sample 76_800 with
    // `"outcome uh and determine ..."`. Audio overlap was 30_720 samples (~1.92s)
    // — both decoders heard the same `"outcome"` at the seam. The standard k>=2
    // search found no anchor (`"scenario's outcome"` didn't equal
    // `"outcome uh"`), so the stitcher fell through to `join_with_single_space`
    // and emitted `"outcome. outcome uh ..."` — a duplicated word.
    //
    // With the new k=1 boundary anchor (substantive 7-char `"outcome"` keys
    // matching exactly at left's last + right's first), the stitcher anchors at
    // (tail_drop=0, match_start=0, overlap=1) and the merge picks right's
    // `"outcome"` (no trailing period — latest evidence wins per the existing
    // punctuation/casing rule).
    let left = "I want you to evaluate each scenario's outcome.";
    let right = "outcome uh and determine what would be the point at which this";
    // Audio overlap 30_720 samples → cap = 9 tokens.
    let stitched = stitch_incremental_text(left, right, 64, 2, Some(9), 30_720);
    assert!(!stitched.contains("outcome. outcome"), "duplicate at seam: {stitched}",);
    assert!(!stitched.contains("outcome outcome"), "duplicate at seam: {stitched}",);
    assert!(stitched.contains("outcome uh and determine"), "lost right content: {stitched}",);
    assert!(
      stitched.starts_with("I want you to evaluate each scenario's"),
      "dropped prefix: {stitched}",
    );
  }

  #[test]
  fn stitch_regression_turn15_preserves_clause_before_repeated_that_we() {
    // Turn 15 (2026-07-05). The stitcher found a weak 2-token overlap (`that we`) only
    // after dropping the nine-token left tail `will need ... sure that`, then appended
    // enough right-side tokens that the net length shrink looked harmless. That dropped
    // a real clause and forced the final partial-core coverage bailout.
    let left = "What are the requirements for server to serter connectivity That we will need \
      to be able to support for making sure that.";
    let right = "That we have you know the highest uh throughput, lowest latency.";
    let overlap_samples = 1_873_920usize - 1_843_200usize;
    let cap = stitch_right_start_cap_from_overlap(overlap_samples);

    let stitched = stitch_incremental_text(left, right, 64, 2, Some(cap), overlap_samples);
    let normalized = stitched.to_lowercase();

    assert!(
      normalized.contains(
        "connectivity that we will need to be able to support for making sure that we have"
      ),
      "dropped the middle clause: {stitched}"
    );
    assert!(
      !normalized.contains("connectivity that we have"),
      "accepted the false repeated-prefix overlap: {stitched}"
    );
  }

  #[test]
  fn boundary_recovery_does_not_anchor_on_short_common_word_seam_dedup_cleans() {
    // 2-char `"of"` fails the boundary-recovery `tokens_match_substantive_boundary`
    // len-≥-3 filter — control falls through to the no-anchor append path where
    // the new seam-dedup (len-≥-2 alpha key, no trailing-punct on prev) catches
    // the duplicated `"of"` and drops it. Result: clean stitch, no `"of of"`
    // artifact, no false anchor consuming downstream content. The earlier
    // pinned trade-off ("of of is acceptable to avoid false anchoring") no
    // longer applies — both halves of the boundary are now well-handled.
    let left = "this is a kind of";
    let right = "of course we can";
    let stitched = stitch_incremental_text(left, right, 64, 2, Some(9), 30_720);
    assert_eq!(stitched, "this is a kind of course we can");
  }

  #[test]
  fn boundary_recovery_k1_does_not_anchor_without_audio_overlap() {
    // Same shape as turn 23 but with `audio_overlap_samples == 0`. Without the
    // structural evidence of overlapping audio, the new branch must NOT fire —
    // the seam-share could be coincidence rather than the same-audio reading.
    let left = "I want you to evaluate each scenario's outcome.";
    let right = "outcome uh and determine what would be";
    let stitched = stitch_incremental_text(left, right, 64, 2, None, 0);
    assert!(stitched.contains("outcome. outcome"), "anchored without audio overlap: {stitched}",);
  }

  #[test]
  fn boundary_recovery_k1_anchors_on_three_char_word() {
    // Smallest accepted token length. `"the"` at the seam with audio overlap
    // anchors the merge cleanly.
    let left = "I went to the";
    let right = "the store today";
    let stitched = stitch_incremental_text(left, right, 64, 2, Some(9), 30_720);
    assert_eq!(stitched, "I went to the store today");
  }

  #[test]
  fn boundary_recovery_k1_loses_to_higher_k_match() {
    // When a k>=2 match exists, it wins over the k=1 fallback — the new branch
    // only fires when the standard search returned None.
    let left = "alpha beta the cat";
    let right = "the cat sat down";
    let stitched = stitch_incremental_text(left, right, 64, 2, Some(9), 30_720);
    // k=2 anchor on ["the","cat"] wins; no double anchor.
    assert_eq!(stitched, "alpha beta the cat sat down");
  }

  #[test]
  fn tokens_match_substantive_boundary_filters_short_tokens() {
    // Substantive 3+ char keys anchor.
    assert!(tokens_match_substantive_boundary("outcome", "outcome"));
    assert!(tokens_match_substantive_boundary("the", "the"));
    assert!(tokens_match_substantive_boundary("318", "318"));
    // 1-2 char tokens reject — particles like `"of"`/`"is"`/`"i"` carry too
    // much false-match risk to anchor on.
    assert!(!tokens_match_substantive_boundary("of", "of"));
    assert!(!tokens_match_substantive_boundary("is", "is"));
    assert!(!tokens_match_substantive_boundary("i", "i"));
    assert!(!tokens_match_substantive_boundary("a", "a"));
    // Inequality always rejects — this never widens to fuzzy match.
    assert!(!tokens_match_substantive_boundary("outcome", "outcomes"));
    assert!(!tokens_match_substantive_boundary("the", "thee"));
    assert!(!tokens_match_substantive_boundary("318", "319"));
  }

  #[test]
  fn stitch_anchors_on_single_cardinal_to_digit() {
    // Turn 50 single-token shape: each cardinal-to-digit pair is matched via the
    // tens-rule single-token path, so no concat-rule needed.
    let left = "more than like 30, 40 minutes maybe an hour";
    let right = "thirty, forty minutes maybe an hour or longer";
    let stitched = stitch_incremental_text(left, right, 64, 2, Some(9), 30_720);
    assert!(stitched.contains("or longer"), "lost right tail: {stitched}");
    // No duplicated number form.
    assert!(!stitched.contains("hour thirty"), "duplicated: {stitched}");
    assert!(!stitched.contains("an hour an hour"), "duplicated: {stitched}");
  }

  #[test]
  fn stitch_anchors_on_tens_rule_spell_out() {
    // 2-token overlap: the number group + the trailing word "Maple".
    let left = "see you at twenty three Maple";
    let right = "23 Maple Street";
    let stitched = stitch_incremental_text(left, right, 64, 2, Some(6), 12_000);
    assert!(stitched.contains("Street"), "lost right tail: {stitched}");
    assert!(!stitched.contains("twenty three 23"), "duplicated: {stitched}");
    assert!(!stitched.contains("23 23"), "duplicated: {stitched}");
  }

  #[test]
  fn stitch_anchors_on_hundreds_with_and() {
    // 2-token overlap: the number group + the trailing word "to".
    let left = "flight one hundred and three to";
    let right = "103 to Boston";
    let stitched = stitch_incremental_text(left, right, 64, 2, Some(6), 12_000);
    assert!(stitched.contains("Boston"), "lost right tail: {stitched}");
    assert!(!stitched.contains("hundred and three 103"), "duplicated: {stitched}");
    assert!(!stitched.contains("103 103"), "duplicated: {stitched}");
  }

  #[test]
  fn stitch_period_breaks_number_run() {
    // The period after "three" must end the run there — not join with "Eighteen"
    // into key "318". If it joined, the assembled would phantom-anchor.
    let left = "give me three. Eighteen of them";
    let right = "Eighteen of them are red";
    let stitched = stitch_incremental_text(left, right, 64, 2, Some(6), 12_000);
    assert!(stitched.contains("are red"), "lost right tail: {stitched}");
    assert!(!stitched.contains("Eighteen of them Eighteen"), "duplicated: {stitched}",);
  }

  #[test]
  fn stitch_does_not_anchor_on_different_numbers() {
    let left = "the 318 flight";
    let right = "320 flight from Air Canada";
    let stitched = stitch_incremental_text(left, right, 64, 2, Some(6), 12_000);
    assert!(stitched.contains("318"), "lost left number: {stitched}");
    assert!(stitched.contains("320"), "lost right number: {stitched}");
  }

  #[test]
  fn tokenize_for_stitch_groups_number_runs() {
    let toks = tokenize_for_stitch("the three eighteen from");
    let pairs: Vec<(String, String)> =
      toks.into_iter().map(|t| (t.original, t.match_key)).collect();
    assert_eq!(
      pairs,
      vec![
        ("the".to_string(), "the".to_string()),
        ("three eighteen".to_string(), "318".to_string()),
        ("from".to_string(), "from".to_string()),
      ],
    );
  }

  #[test]
  fn tokenize_for_stitch_period_ends_run() {
    let toks = tokenize_for_stitch("give me three. Eighteen of them");
    let keys: Vec<String> = toks.iter().map(|t| t.match_key.clone()).collect();
    // Period after "three" terminates the run — no phantom "318" group.
    assert!(keys.contains(&"3".to_string()), "missing `3`: {keys:?}");
    assert!(keys.contains(&"18".to_string()), "missing `18`: {keys:?}");
    assert!(!keys.contains(&"318".to_string()), "phantom `318` formed: {keys:?}");
  }

  #[test]
  fn tokenize_for_stitch_does_not_overgroup_pure_ones_chain() {
    // Existing test data: "one two three four five six" was a sequence of arbitrary
    // ordered labels in the stitcher's regression suite. The number-run grouper must
    // emit each as its own digit-keyed token, NOT a single concat group "123456".
    let toks = tokenize_for_stitch("one two three four five six");
    let keys: Vec<String> = toks.iter().map(|t| t.match_key.clone()).collect();
    assert_eq!(keys, vec!["1", "2", "3", "4", "5", "6"]);
    assert!(!keys.contains(&"123456".to_string()), "over-grouped chain: {keys:?}");
  }

  #[test]
  fn try_consume_number_run_handles_grammar() {
    // Ones, teens, tens — single tokens via tens-rule.
    assert_eq!(try_consume_number_run(&["five"], 0), Some((1, "5".into())));
    assert_eq!(try_consume_number_run(&["eighteen"], 0), Some((1, "18".into())));
    assert_eq!(try_consume_number_run(&["ninety"], 0), Some((1, "90".into())));
    // Tens-rule: twenty + three = 23.
    assert_eq!(try_consume_number_run(&["twenty", "three"], 0), Some((2, "23".into())));
    // Concat-rule: ones + teen = 318 (no tens-rule combination, has Teen).
    assert_eq!(try_consume_number_run(&["three", "eighteen"], 0), Some((2, "318".into())));
    // Hundreds.
    assert_eq!(try_consume_number_run(&["one", "hundred"], 0), Some((2, "100".into())));
    assert_eq!(
      try_consume_number_run(&["one", "hundred", "and", "three"], 0),
      Some((4, "103".into())),
    );
    assert_eq!(try_consume_number_run(&["one", "hundred", "three"], 0), Some((3, "103".into())),);
    // Ordinals reject.
    assert_eq!(try_consume_number_run(&["third"], 0), None);
    assert_eq!(try_consume_number_run(&["eighteenth"], 0), None);
    // Non-number rejects.
    assert_eq!(try_consume_number_run(&["apple"], 0), None);
    // Bare digit string passes through.
    assert_eq!(try_consume_number_run(&["318"], 0), Some((1, "318".into())));
    // Mixed digit + cardinal under concat-rule (digit "18" is multi-digit).
    assert_eq!(try_consume_number_run(&["three", "18"], 0), Some((2, "318".into())));
    // Pure-Ones chain longer than 1 is NOT concat-grouped (no Teen/Tens/multi-digit).
    // Resolves to single token "1" and the caller advances.
    assert_eq!(try_consume_number_run(&["one", "two", "three"], 0), Some((1, "1".into())),);
    // Dangling `and` retracts.
    assert_eq!(
      try_consume_number_run(&["one", "hundred", "and", "apples"], 0),
      Some((2, "100".into())),
    );
  }

  #[test]
  fn dual_final_sidecar_carries_additive_dual_stream_keys() {
    let live = vec![
      super::LiveDisplayEvent {
        audio_samples: 8_000,
        source: "streaming",
        action: "emit",
        text: "forty two".to_string(),
        candidate_text: None,
      },
      super::LiveDisplayEvent {
        audio_samples: 16_000,
        source: "refined",
        action: "emit",
        text: "forty-two percent".to_string(),
        candidate_text: Some("forty two per".to_string()),
      },
    ];
    let eou =
      vec![EouEmission { audio_samples: 8_000, delta_chars: 9, text: "forty two".to_string() }];
    let payload = super::debug_recording_payload(
      7,
      1_700_000_000_000,
      16_000,
      super::AuditEmittedKind::DualFinal,
      "forty-two percent", // emitted_text = refined final (what was pasted)
      "",                  // full_text: dual has no whole-turn model reference
      "forty two per",     // draft_text = live caption at finalize
      Some(123),
      &[],
      &eou,
      &live,
      None,
    );
    assert_eq!(payload["pipeline"], "dual_stream");
    assert_eq!(payload["emitted_kind"], "dual_final");
    assert_eq!(payload["draft_text"], "forty two per");
    assert_eq!(payload["emitted_text"], "forty-two percent");
    // No model re-decode in dual: the empty full_text is the observable signature of "no model".
    assert_eq!(payload["full_text"], "");
    assert_eq!(payload["finalize_elapsed_ms"], 123);
    assert_eq!(payload["partials"].as_array().unwrap().len(), 0);
    assert_eq!(payload["live_display_events"].as_array().unwrap().len(), 2);
    assert_eq!(payload["eou_emissions"].as_array().unwrap().len(), 1);
  }

  #[test]
  fn legacy_sidecar_keeps_empty_dual_keys_and_legacy_pipeline() {
    let payload = super::debug_recording_payload(
      3,
      1_700_000_000_000,
      4_000,
      super::AuditEmittedKind::Assembled,
      "hello world",
      "hello world model",
      "",   // legacy jobs carry no draft_text
      None, // and no finalize_elapsed_ms
      &[],
      &[],
      &[],
      Some("no_incremental_or_draft_text"),
    );
    assert_eq!(payload["pipeline"], "legacy_stitch");
    assert_eq!(payload["draft_text"], "");
    assert!(payload["finalize_elapsed_ms"].is_null());
    assert_eq!(payload["bailout_reason"], "no_incremental_or_draft_text");
    assert_eq!(payload["full_text"], "hello world model");
  }

  #[test]
  fn dual_final_divergence_measures_draft_to_refined_edits() {
    // draft -> refined final: two single-token sharpenings (per->percent, allot->allotment).
    let event = super::log_partial_audit_result(
      11,
      super::AuditEmittedKind::DualFinal,
      &[],
      "raise it by ten per and allot more",
      "raise it by ten percent and allotment more",
      None,
    );
    match event {
      super::DebugStatsEvent::PartialAuditResult { emitted_kind, edit_distance, exact, .. } => {
        assert_eq!(emitted_kind, "dual_final");
        assert_eq!(edit_distance, 2);
        assert!(!exact);
      }
      other => panic!("expected PartialAuditResult, got {other:?}"),
    }
  }

  #[test]
  fn dual_final_divergence_is_zero_when_refined_equals_draft() {
    // When the refined stream produced nothing, final_text == draft: zero corrections.
    let event = super::log_partial_audit_result(
      12,
      super::AuditEmittedKind::DualFinal,
      &[],
      "the plan is unchanged",
      "the plan is unchanged",
      None,
    );
    match event {
      super::DebugStatsEvent::PartialAuditResult { edit_distance, exact, .. } => {
        assert_eq!(edit_distance, 0);
        assert!(exact);
      }
      other => panic!("expected PartialAuditResult, got {other:?}"),
    }
  }

  #[test]
  fn pipeline_label_separates_dual_from_legacy() {
    assert_eq!(super::pipeline_label(super::AuditEmittedKind::DualFinal), "dual_stream");
    assert_eq!(super::pipeline_label(super::AuditEmittedKind::Assembled), "legacy_stitch");
    assert_eq!(super::pipeline_label(super::AuditEmittedKind::DraftEmit), "legacy_stitch");
    assert_eq!(super::audit_kind_label(super::AuditEmittedKind::DualFinal), "dual_final");
  }
}
